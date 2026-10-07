// ICRC-1 and ICRC-2 token standard implementation for the 3USD LP token.
//
// The LP token balances are tracked internally in ThreePoolState::lp_balances.
// This module exposes them as a proper ICRC-1/ICRC-2 compliant token.
//
// Token: 3USD | Decimals: 8 | Fee: 0
// Subaccounts: balances are tracked by owner principal only — subaccounts are
// accepted on all fields (from, to, spender) but effectively ignored for
// balance lookups. This allows DEX canisters that use per-pool subaccounts
// (e.g. the Rumi AMM) to hold and transfer 3USD without issues.

use candid::{Nat, Principal};
use icrc_ledger_types::icrc::generic_metadata_value::MetadataValue;
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc1::transfer::{TransferArg, TransferError};
use icrc_ledger_types::icrc2::allowance::{Allowance, AllowanceArgs};
use icrc_ledger_types::icrc2::approve::{ApproveArgs, ApproveError};
use icrc_ledger_types::icrc2::transfer_from::{TransferFromArgs, TransferFromError};
use crate::state::{mutate_state, read_state};
use crate::storage::{
    lp_transfer_dedup_cutover, LpTransferDedupEntry, LpTransferExpiryKey, StorableHash, Unit,
    LP_TRANSFER_DEDUP, LP_TRANSFER_DEDUP_EXPIRY,
};
use crate::types::{LpAllowance, Icrc3Transaction};

// ─── Transaction deduplication (audit 2026-06-09, ICRC-001) ───
//
// ICRC-1 standard dedup: when a caller supplies `created_at_time`, the ledger
// must reject transactions outside the dedup window (TooOld / CreatedInFuture)
// and reject an identical (caller, args) resubmission within the window with
// `Duplicate { duplicate_of }`. Window constants match the reference ICRC-1
// ledger used by icUSD/ICP (TRANSACTION_WINDOW = 24h, PERMITTED_DRIFT = 60s).
//
// The exact transfer hash, original timestamp, and original block are stored
// in stable memory. A reply-lost retry must stay a Duplicate even if the pool
// upgrades within the 24h window. Old heap-only identities cannot be
// reconstructed from ICRC-3 blocks (which omit memo and created_at_time), so
// pre-cutover timestamps are held as TooOld until their retry window expires.
// A stable expiry index prunes expired rows in bounded batches on insertion.

pub const TRANSACTION_WINDOW_NS: u64 = 24 * 60 * 60 * 1_000_000_000;
pub const PERMITTED_DRIFT_NS: u64 = 60 * 1_000_000_000;
const MAX_EXPIRED_DEDUP_ENTRIES_PER_TRANSFER: usize = 512;

#[derive(Debug, PartialEq, Eq)]
pub enum DedupReject {
    TooOld,
    CreatedInFuture { ledger_time: u64 },
    Duplicate { duplicate_of: u64 },
}

fn tx_expired(created_at_time: u64, now: u64) -> bool {
    created_at_time
        .saturating_add(TRANSACTION_WINDOW_NS)
        .saturating_add(PERMITTED_DRIFT_NS)
        < now
}

/// Validate `created_at_time` against the dedup window and the seen-tx map.
/// `None` keeps the legacy no-dedup behavior (per ICRC-1, dedup only applies
/// when the caller supplies `created_at_time`).
fn dedup_check(
    now: u64,
    created_at_time: Option<u64>,
    tx_hash: &[u8; 32],
) -> Result<(), DedupReject> {
    dedup_check_with_cutover(now, created_at_time, tx_hash, lp_transfer_dedup_cutover())
}

fn dedup_check_with_cutover(
    now: u64,
    created_at_time: Option<u64>,
    tx_hash: &[u8; 32],
    cutover: Option<u64>,
) -> Result<(), DedupReject> {
    let Some(cat) = created_at_time else {
        return Ok(());
    };
    if tx_expired(cat, now) {
        return Err(DedupReject::TooOld);
    }
    if cat > now.saturating_add(PERMITTED_DRIFT_NS) {
        return Err(DedupReject::CreatedInFuture { ledger_time: now });
    }
    if let Some(entry) = LP_TRANSFER_DEDUP.with(|m| m.borrow().get(&StorableHash(*tx_hash))) {
        return Err(DedupReject::Duplicate {
            duplicate_of: entry.block_index,
        });
    }
    // Before stable dedup was introduced, a still-live retry may have no
    // identity in this map: transfer identities lived only in heap, while
    // approval dedup did not exist. Historical ICRC-3 blocks omit memo and
    // created_at_time, so those transactions cannot be reconstructed. The
    // old ledger also accepted created_at_time up to PERMITTED_DRIFT_NS beyond
    // current time. Hold the whole possible range as TooOld until its normal
    // window expires. Check the stable map first so transactions recorded by
    // this version still return their original block across the first upgrade.
    if cutover.is_some_and(|first_upgrade| {
        cat <= first_upgrade.saturating_add(PERMITTED_DRIFT_NS)
    }) {
        return Err(DedupReject::TooOld);
    }
    Ok(())
}

/// Record an executed deduplicated transaction. Prunes a bounded batch of
/// expired entries from the ordered expiry index. No-op when
/// `created_at_time` is `None` (such transactions are never deduplicated).
fn dedup_record(now: u64, created_at_time: Option<u64>, tx_hash: [u8; 32], block_index: u64) {
    let Some(cat) = created_at_time else {
        return;
    };
    // Walk only the expiry-ordered prefix and bound work per transfer. Any
    // expired rows left behind are rejected by `dedup_check`'s timestamp
    // validation, so deferring their physical removal cannot enable replay.
    let expired_hashes = LP_TRANSFER_DEDUP_EXPIRY.with(|m| {
        let mut m = m.borrow_mut();
        let expired: Vec<_> = m
            .iter()
            .take_while(|(key, _)| key.expires_at < now)
            .take(MAX_EXPIRED_DEDUP_ENTRIES_PER_TRANSFER)
            .map(|(key, _)| key)
            .collect();
        for key in &expired {
            m.remove(key);
        }
        expired.into_iter().map(|key| key.hash).collect::<Vec<_>>()
    });
    LP_TRANSFER_DEDUP.with(|m| {
        let mut m = m.borrow_mut();
        for hash in expired_hashes {
            m.remove(&hash);
        }

        let hash = StorableHash(tx_hash);
        if let Some(previous) = m.get(&hash) {
            LP_TRANSFER_DEDUP_EXPIRY.with(|expiry| {
                expiry.borrow_mut().remove(&LpTransferExpiryKey {
                    expires_at: dedup_expires_at(previous.created_at_time),
                    hash,
                });
            });
        }
        m.insert(
            hash,
            LpTransferDedupEntry {
                created_at_time: cat,
                block_index,
            },
        );
        LP_TRANSFER_DEDUP_EXPIRY.with(|expiry| {
            expiry.borrow_mut().insert(
                LpTransferExpiryKey {
                    expires_at: dedup_expires_at(cat),
                    hash,
                },
                Unit,
            );
        });
    });
}

fn dedup_expires_at(created_at_time: u64) -> u64 {
    created_at_time
        .saturating_add(TRANSACTION_WINDOW_NS)
        .saturating_add(PERMITTED_DRIFT_NS)
}

/// Feed an optional length-prefixed field into the hasher. The presence byte
/// plus length prefix make the serialization unambiguous (no field-boundary
/// collisions between adjacent variable-length fields).
fn hash_part(h: &mut sha2::Sha256, part: Option<&[u8]>) {
    use sha2::Digest;
    match part {
        Some(b) => {
            h.update([1u8]);
            h.update((b.len() as u64).to_be_bytes());
            h.update(b);
        }
        None => h.update([0u8]),
    }
}

/// Hash the full (caller, args) identity of an icrc1_transfer for dedup.
fn hash_icrc1_transfer(caller: &Principal, args: &TransferArg) -> [u8; 32] {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    h.update(b"3usd.icrc1_transfer");
    hash_part(&mut h, Some(caller.as_slice()));
    hash_part(&mut h, args.from_subaccount.as_ref().map(|s| s.as_slice()));
    hash_part(&mut h, Some(args.to.owner.as_slice()));
    hash_part(&mut h, args.to.subaccount.as_ref().map(|s| s.as_slice()));
    let amount_bytes = args.amount.0.to_bytes_be();
    hash_part(&mut h, Some(&amount_bytes));
    let fee_bytes = args.fee.as_ref().map(|f| f.0.to_bytes_be());
    hash_part(&mut h, fee_bytes.as_deref());
    hash_part(&mut h, args.memo.as_ref().map(|m| m.0.as_slice()));
    let cat_bytes = args.created_at_time.map(|t| t.to_be_bytes());
    hash_part(&mut h, cat_bytes.as_ref().map(|b| &b[..]));
    h.finalize().into()
}

/// Hash the full (caller, args) identity of an icrc2_transfer_from for dedup.
fn hash_icrc2_transfer_from(caller: &Principal, args: &TransferFromArgs) -> [u8; 32] {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    h.update(b"3usd.icrc2_transfer_from");
    hash_part(&mut h, Some(caller.as_slice()));
    hash_part(&mut h, args.spender_subaccount.as_ref().map(|s| s.as_slice()));
    hash_part(&mut h, Some(args.from.owner.as_slice()));
    hash_part(&mut h, args.from.subaccount.as_ref().map(|s| s.as_slice()));
    hash_part(&mut h, Some(args.to.owner.as_slice()));
    hash_part(&mut h, args.to.subaccount.as_ref().map(|s| s.as_slice()));
    let amount_bytes = args.amount.0.to_bytes_be();
    hash_part(&mut h, Some(&amount_bytes));
    let fee_bytes = args.fee.as_ref().map(|f| f.0.to_bytes_be());
    hash_part(&mut h, fee_bytes.as_deref());
    hash_part(&mut h, args.memo.as_ref().map(|m| m.0.as_slice()));
    let cat_bytes = args.created_at_time.map(|t| t.to_be_bytes());
    hash_part(&mut h, cat_bytes.as_ref().map(|b| &b[..]));
    h.finalize().into()
}

/// Hash the full (caller, args) identity of an icrc2_approve for dedup.
///
/// `expected_allowance` is part of the transaction tuple: it controls whether
/// the approval may commit, so retries with a different CAS precondition must
/// not alias an already committed approval.
fn hash_icrc2_approve(caller: &Principal, args: &ApproveArgs) -> [u8; 32] {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    h.update(b"3usd.icrc2_approve");
    hash_part(&mut h, Some(caller.as_slice()));
    hash_part(&mut h, args.from_subaccount.as_ref().map(|s| s.as_slice()));
    hash_part(&mut h, Some(args.spender.owner.as_slice()));
    hash_part(&mut h, args.spender.subaccount.as_ref().map(|s| s.as_slice()));
    let amount_bytes = args.amount.0.to_bytes_be();
    hash_part(&mut h, Some(&amount_bytes));
    let expected_bytes = args.expected_allowance.as_ref().map(|n| n.0.to_bytes_be());
    hash_part(&mut h, expected_bytes.as_deref());
    let expires_bytes = args.expires_at.map(|t| t.to_be_bytes());
    hash_part(&mut h, expires_bytes.as_ref().map(|b| &b[..]));
    let fee_bytes = args.fee.as_ref().map(|f| f.0.to_bytes_be());
    hash_part(&mut h, fee_bytes.as_deref());
    hash_part(&mut h, args.memo.as_ref().map(|m| m.0.as_slice()));
    let cat_bytes = args.created_at_time.map(|t| t.to_be_bytes());
    hash_part(&mut h, cat_bytes.as_ref().map(|b| &b[..]));
    h.finalize().into()
}

// ─── Helpers ───

fn nat_to_u128(n: &Nat) -> Result<u128, ()> {
    use num_traits::cast::ToPrimitive;
    n.0.to_u128().ok_or(())
}

fn effective_allowance(a: &LpAllowance) -> u128 {
    if let Some(exp) = a.expires_at {
        if exp < ic_cdk::api::time() {
            return 0;
        }
    }
    a.amount
}

fn logo_data_uri() -> String {
    let svg = include_str!("../../vault_frontend/static/3pool-logo-v5.svg");
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(svg.as_bytes());
    format!("data:image/svg+xml;base64,{}", encoded)
}

// ─── ICRC-1 Queries ───

pub fn icrc1_name() -> String {
    "3USD".to_string()
}

pub fn icrc1_symbol() -> String {
    "3USD".to_string()
}

pub fn icrc1_decimals() -> u8 {
    8
}

pub fn icrc1_fee() -> Nat {
    Nat::from(0u64)
}

pub fn icrc1_total_supply() -> Nat {
    Nat::from(read_state(|s| s.lp_total_supply))
}

pub fn icrc1_minting_account() -> Option<Account> {
    None
}

pub fn icrc1_balance_of(account: Account) -> Nat {
    let p = account.owner;
    Nat::from(crate::storage::lp_balance_get(&p))
}

pub fn icrc1_metadata() -> Vec<(String, MetadataValue)> {
    vec![
        ("icrc1:name".to_string(), MetadataValue::Text("3USD".to_string())),
        ("icrc1:symbol".to_string(), MetadataValue::Text("3USD".to_string())),
        ("icrc1:decimals".to_string(), MetadataValue::Nat(Nat::from(8u64))),
        ("icrc1:fee".to_string(), MetadataValue::Nat(Nat::from(0u64))),
        ("icrc1:logo".to_string(), MetadataValue::Text(logo_data_uri())),
    ]
}

// ─── ICRC-1 Transfer ───

pub fn icrc1_transfer(caller: Principal, args: TransferArg) -> Result<Nat, TransferError> {
    // Validate fee
    if let Some(ref fee) = args.fee {
        if *fee != Nat::from(0u64) {
            return Err(TransferError::BadFee {
                expected_fee: Nat::from(0u64),
            });
        }
    }

    // ICRC-001: standard dedup when the caller supplies created_at_time.
    let now = ic_cdk::api::time();
    let tx_hash = args
        .created_at_time
        .map(|_| hash_icrc1_transfer(&caller, &args));
    if let Some(h) = &tx_hash {
        if let Err(e) = dedup_check(now, args.created_at_time, h) {
            return Err(match e {
                DedupReject::TooOld => TransferError::TooOld,
                DedupReject::CreatedInFuture { ledger_time } => {
                    TransferError::CreatedInFuture { ledger_time }
                }
                DedupReject::Duplicate { duplicate_of } => TransferError::Duplicate {
                    duplicate_of: Nat::from(duplicate_of),
                },
            });
        }
    }

    // Both from_subaccount and to accept any subaccount — balances are keyed
    // by owner principal only, so subaccounts are effectively ignored for
    // *balance* lookups. The subaccounts ARE preserved into the ICRC-3 block
    // log so external consumers (e.g. the protocol_backend's SP writedown
    // proof verifier) see the actual destination Account the caller chose.
    let to_principal = args.to.owner;
    let from_subaccount = args.from_subaccount.map(|s| s.to_vec());
    let to_subaccount = args.to.subaccount.map(|s| s.to_vec());

    let amount = nat_to_u128(&args.amount).map_err(|_| TransferError::GenericError {
        error_code: Nat::from(2u64),
        message: "amount overflow".to_string(),
    })?;

    if amount == 0 {
        return Err(TransferError::GenericError {
            error_code: Nat::from(3u64),
            message: "transfer amount must be positive".to_string(),
        });
    }

    // NOTE (audit 2026-06-05, SAT-007): a previous over-broad guard rejected
    // every transfer where `caller == to.owner`, which broke legitimate wallet
    // flows (notably the "3USD send with Internet Identity" bug) whenever a
    // wallet routed a send to the same owning principal under a different
    // subaccount. Because balances are keyed by owner principal only, a
    // same-owner transfer is a self-cancelling no-op on the balance (debit then
    // credit the same key, net zero), so allowing it is safe and matches the
    // ICP/ICRC-1 ledger convention of permitting self-transfers.

    let result = mutate_state(|s| {
        let from_balance = crate::storage::lp_balance_get(&caller);
        if from_balance < amount {
            return Err(TransferError::InsufficientFunds {
                balance: Nat::from(from_balance),
            });
        }

        // Debit (set-to-0 removes the entry from stable storage)
        crate::storage::lp_balance_set(caller, from_balance - amount);

        // Credit
        let to_balance = crate::storage::lp_balance_get(&to_principal);
        crate::storage::lp_balance_set(to_principal, to_balance + amount);

        let id = s.log_block(Icrc3Transaction::Transfer {
            from: caller,
            to: to_principal,
            amount,
            spender: None,
            from_subaccount,
            to_subaccount,
            spender_subaccount: None,
            memo: args.memo.as_ref().map(|memo| memo.0.to_vec()),
            created_at_time: args.created_at_time,
            transaction_fee: Some(0),
        });
        Ok(id)
    });

    let id = result?;
    if let Some(h) = tx_hash {
        dedup_record(now, args.created_at_time, h, id);
    }
    Ok(Nat::from(id))
}

// ─── ICRC-2 Approve ───

pub fn icrc2_approve(caller: Principal, args: ApproveArgs) -> Result<Nat, ApproveError> {
    // Validate fee
    if let Some(ref fee) = args.fee {
        if *fee != Nat::from(0u64) {
            return Err(ApproveError::BadFee {
                expected_fee: Nat::from(0u64),
            });
        }
    }

    // ICRC-2 approval retries must resolve before checking expiry or the
    // allowance CAS. The first execution may already have changed the
    // allowance (and the retry may arrive just after expires_at), but an
    // identical retry still returns the original block rather than applying
    // the state transition a second time or producing a misleading error.
    let now = ic_cdk::api::time();
    let tx_hash = args
        .created_at_time
        .map(|_| hash_icrc2_approve(&caller, &args));
    if let Some(h) = &tx_hash {
        if let Err(e) = dedup_check(now, args.created_at_time, h) {
            return Err(match e {
                DedupReject::TooOld => ApproveError::TooOld,
                DedupReject::CreatedInFuture { ledger_time } => {
                    ApproveError::CreatedInFuture { ledger_time }
                }
                DedupReject::Duplicate { duplicate_of } => ApproveError::Duplicate {
                    duplicate_of: Nat::from(duplicate_of),
                },
            });
        }
    }

    // Subaccounts accepted but ignored for balance/allowance keying — the
    // 3pool tracks balances per principal only. Block log preserves the
    // subaccounts the caller chose for ICRC-3 consumers.
    let spender_principal = args.spender.owner;
    let from_subaccount = args.from_subaccount.map(|s| s.to_vec());
    let spender_subaccount = args.spender.subaccount.map(|s| s.to_vec());

    let amount = nat_to_u128(&args.amount).map_err(|_| ApproveError::GenericError {
        error_code: Nat::from(2u64),
        message: "amount overflow".to_string(),
    })?;

    // Check expires_at is in the future
    if let Some(expires_at) = args.expires_at {
        if expires_at < now {
            return Err(ApproveError::Expired { ledger_time: now });
        }
    }

    let result = mutate_state(|s| {
        // CAS: check expected_allowance
        if let Some(ref expected) = args.expected_allowance {
            let current = crate::storage::allowance_get(&caller, &spender_principal)
                .map(|a| effective_allowance(&a))
                .unwrap_or(0);
            let expected_u128 = nat_to_u128(expected).unwrap_or(u128::MAX);
            if current != expected_u128 {
                return Err(ApproveError::AllowanceChanged {
                    current_allowance: Nat::from(current),
                });
            }
        }

        // Set allowance
        crate::storage::allowance_set(
            caller,
            spender_principal,
            LpAllowance {
                amount,
                expires_at: args.expires_at,
            },
        );

        let id = s.log_block(Icrc3Transaction::Approve {
            from: caller,
            spender: spender_principal,
            amount,
            expires_at: args.expires_at,
            from_subaccount,
            spender_subaccount,
            memo: args.memo.as_ref().map(|memo| memo.0.to_vec()),
            created_at_time: args.created_at_time,
            transaction_fee: args.fee.as_ref().map(|_| 0),
        });
        Ok(id)
    })?;
    if let Some(h) = tx_hash {
        dedup_record(now, args.created_at_time, h, result);
    }
    Ok(Nat::from(result))
}

// ─── ICRC-2 Allowance Query ───

pub fn icrc2_allowance(args: AllowanceArgs) -> Allowance {
    let owner = args.account.owner;
    let spender = args.spender.owner;

    match crate::storage::allowance_get(&owner, &spender) {
        Some(a) => {
            let eff = effective_allowance(&a);
            Allowance {
                allowance: Nat::from(eff),
                expires_at: if eff > 0 { a.expires_at } else { None },
            }
        }
        None => Allowance {
            allowance: Nat::from(0u64),
            expires_at: None,
        },
    }
}

// ─── ICRC-2 Transfer From ───

pub fn icrc2_transfer_from(
    caller: Principal,
    args: TransferFromArgs,
) -> Result<Nat, TransferFromError> {
    // Validate fee
    if let Some(ref fee) = args.fee {
        if *fee != Nat::from(0u64) {
            return Err(TransferFromError::BadFee {
                expected_fee: Nat::from(0u64),
            });
        }
    }

    // ICRC-001: standard dedup when the caller supplies created_at_time.
    let now = ic_cdk::api::time();
    let tx_hash = args
        .created_at_time
        .map(|_| hash_icrc2_transfer_from(&caller, &args));
    if let Some(h) = &tx_hash {
        if let Err(e) = dedup_check(now, args.created_at_time, h) {
            return Err(match e {
                DedupReject::TooOld => TransferFromError::TooOld,
                DedupReject::CreatedInFuture { ledger_time } => {
                    TransferFromError::CreatedInFuture { ledger_time }
                }
                DedupReject::Duplicate { duplicate_of } => TransferFromError::Duplicate {
                    duplicate_of: Nat::from(duplicate_of),
                },
            });
        }
    }

    // Subaccounts accepted but ignored for balance keying — block log
    // preserves them for ICRC-3 consumers (see icrc1_transfer comment).
    let from_principal = args.from.owner;
    let to_principal = args.to.owner;
    let from_subaccount = args.from.subaccount.map(|s| s.to_vec());
    let to_subaccount = args.to.subaccount.map(|s| s.to_vec());
    let spender_subaccount = args.spender_subaccount.map(|s| s.to_vec());

    let amount = nat_to_u128(&args.amount).map_err(|_| TransferFromError::GenericError {
        error_code: Nat::from(2u64),
        message: "amount overflow".to_string(),
    })?;

    if amount == 0 {
        return Err(TransferFromError::GenericError {
            error_code: Nat::from(3u64),
            message: "transfer amount must be positive".to_string(),
        });
    }

    let result = mutate_state(|s| {
        // Preflight the allowance (unless self-transfer); do not mutate it
        // until the sender's balance has also passed validation.
        let allowance_after = if caller != from_principal {
            let existing = crate::storage::allowance_get(&from_principal, &caller);
            let current_allowance = existing
                .as_ref()
                .map(|a| effective_allowance(a))
                .unwrap_or(0);
            if current_allowance < amount {
                return Err(TransferFromError::InsufficientAllowance {
                    allowance: Nat::from(current_allowance),
                });
            }
            let mut entry = existing.expect("sufficient allowance must have an entry");
            entry.amount = entry.amount.checked_sub(amount).ok_or(
                TransferFromError::InsufficientAllowance {
                    allowance: Nat::from(current_allowance),
                },
            )?;
            Some(entry)
        } else {
            None
        };

        // Check balance
        let from_balance = crate::storage::lp_balance_get(&from_principal);
        if from_balance < amount {
            return Err(TransferFromError::InsufficientFunds {
                balance: Nat::from(from_balance),
            });
        }

        // All rejecting preconditions have now passed. Mutate allowance only
        // alongside a transfer that can commit, so InsufficientFunds cannot
        // consume an approved spender's allowance.
        if let Some(entry) = allowance_after {
            if entry.amount == 0 {
                crate::storage::allowance_remove(&from_principal, &caller);
            } else {
                crate::storage::allowance_set(from_principal, caller, entry);
            }
        }

        // Debit (set-to-0 removes the entry from stable storage)
        crate::storage::lp_balance_set(from_principal, from_balance - amount);

        // Credit
        let to_balance = crate::storage::lp_balance_get(&to_principal);
        crate::storage::lp_balance_set(to_principal, to_balance + amount);

        let id = s.log_block(Icrc3Transaction::Transfer {
            from: from_principal,
            to: to_principal,
            amount,
            spender: Some(caller),
            from_subaccount,
            to_subaccount,
            spender_subaccount,
            memo: args.memo.as_ref().map(|memo| memo.0.to_vec()),
            created_at_time: args.created_at_time,
            transaction_fee: args.fee.as_ref().map(|_| 0),
        });
        Ok(id)
    });

    let id = result?;
    if let Some(h) = tx_hash {
        dedup_record(now, args.created_at_time, h, id);
    }
    Ok(Nat::from(id))
}

// ─── Tests ───

#[cfg(test)]
fn seen_txs_len() -> usize {
    LP_TRANSFER_DEDUP.with(|m| m.borrow().len() as usize)
}

#[cfg(test)]
fn seen_txs_expiry_len() -> usize {
    LP_TRANSFER_DEDUP_EXPIRY.with(|m| m.borrow().len() as usize)
}

#[cfg(test)]
fn seen_tx_contains(hash: &[u8; 32]) -> bool {
    LP_TRANSFER_DEDUP.with(|m| m.borrow().contains_key(&StorableHash(*hash)))
}

#[cfg(test)]
mod icrc_001_dedup_tests {
    use super::*;

    const NOW: u64 = 1_700_000_000_000_000_000;

    fn sample_transfer_arg(memo_byte: u8, created_at_time: Option<u64>) -> TransferArg {
        TransferArg {
            from_subaccount: None,
            to: Account {
                owner: Principal::self_authenticating(&[9, 9, 9]),
                subaccount: None,
            },
            amount: Nat::from(1_000u64),
            fee: None,
            memo: Some(icrc_ledger_types::icrc1::transfer::Memo(
                serde_bytes::ByteBuf::from(vec![memo_byte]),
            )),
            created_at_time,
        }
    }

    fn sample_approve_args() -> ApproveArgs {
        ApproveArgs {
            from_subaccount: None,
            spender: Account {
                owner: Principal::self_authenticating(&[8, 8, 8]),
                subaccount: None,
            },
            amount: Nat::from(1_000u64),
            expected_allowance: Some(Nat::from(500u64)),
            expires_at: Some(NOW + 100),
            fee: Some(Nat::from(0u64)),
            memo: Some(icrc_ledger_types::icrc1::transfer::Memo(
                serde_bytes::ByteBuf::from(b"approval".to_vec()),
            )),
            created_at_time: Some(NOW),
        }
    }

    #[test]
    fn icrc_001_duplicate_within_window_returns_original_block() {
        let h = [1u8; 32];
        assert_eq!(dedup_check(NOW, Some(NOW), &h), Ok(()));
        dedup_record(NOW, Some(NOW), h, 42);
        assert_eq!(
            dedup_check(NOW + 1_000, Some(NOW), &h),
            Err(DedupReject::Duplicate { duplicate_of: 42 })
        );
        // Still a duplicate near the end of the window.
        assert_eq!(
            dedup_check(NOW + TRANSACTION_WINDOW_NS, Some(NOW), &h),
            Err(DedupReject::Duplicate { duplicate_of: 42 })
        );
    }

    #[test]
    fn icrc_001_too_old_rejected() {
        let h = [2u8; 32];
        let cat = NOW - TRANSACTION_WINDOW_NS - PERMITTED_DRIFT_NS - 1;
        assert_eq!(dedup_check(NOW, Some(cat), &h), Err(DedupReject::TooOld));
        // Exactly at the boundary is still accepted.
        let cat_edge = NOW - TRANSACTION_WINDOW_NS - PERMITTED_DRIFT_NS;
        assert_eq!(dedup_check(NOW, Some(cat_edge), &h), Ok(()));
    }

    #[test]
    fn icrc_001_created_in_future_rejected() {
        let h = [3u8; 32];
        let cat = NOW + PERMITTED_DRIFT_NS + 1;
        assert_eq!(
            dedup_check(NOW, Some(cat), &h),
            Err(DedupReject::CreatedInFuture { ledger_time: NOW })
        );
        // Within the permitted drift is accepted.
        assert_eq!(dedup_check(NOW, Some(NOW + PERMITTED_DRIFT_NS), &h), Ok(()));
    }

    #[test]
    fn first_upgrade_holds_legacy_window_and_allows_new_transfers() {
        let cutover = NOW;
        let legacy_retry = [0x31u8; 32];
        // The old ledger could accept a timestamp exactly 60 seconds ahead
        // just before upgrade. It remains inside the ordinary retry window,
        // but its identity could have lived only in the old heap map.
        let old_ledger_time = cutover - 1;
        let predecessor_future_cat = old_ledger_time + PERMITTED_DRIFT_NS;
        assert_eq!(
            dedup_check_with_cutover(
                old_ledger_time,
                Some(predecessor_future_cat),
                &legacy_retry,
                None
            ),
            Ok(())
        );
        assert_eq!(
            dedup_check_with_cutover(
                cutover,
                Some(predecessor_future_cat),
                &legacy_retry,
                Some(cutover)
            ),
            Err(DedupReject::TooOld)
        );

        // The inclusive upper endpoint is held as well; it is valid under
        // ordinary future-drift validation at cutover.
        let upper_boundary = cutover + PERMITTED_DRIFT_NS;
        assert_eq!(
            dedup_check_with_cutover(cutover, Some(upper_boundary), &legacy_retry, Some(cutover)),
            Err(DedupReject::TooOld)
        );

        // Once the legacy timestamp range has passed, current transfers
        // follow ordinary dedup and retain their block index on retries.
        let post_upgrade_transfer = [0x32u8; 32];
        let after_hold = cutover + PERMITTED_DRIFT_NS + 1;
        assert_eq!(
            dedup_check_with_cutover(
                after_hold,
                Some(after_hold),
                &post_upgrade_transfer,
                Some(cutover)
            ),
            Ok(())
        );
        dedup_record(after_hold, Some(after_hold), post_upgrade_transfer, 91);
        assert_eq!(
            dedup_check_with_cutover(
                after_hold + 1,
                Some(after_hold),
                &post_upgrade_transfer,
                Some(cutover)
            ),
            Err(DedupReject::Duplicate { duplicate_of: 91 })
        );

        // A transaction already recorded in stable memory under the new
        // version must take precedence over the legacy hold on first upgrade.
        let stable_before_first_upgrade = [0x33u8; 32];
        dedup_record(cutover - 1, Some(cutover - 1), stable_before_first_upgrade, 92);
        assert_eq!(
            dedup_check_with_cutover(
                cutover,
                Some(cutover - 1),
                &stable_before_first_upgrade,
                Some(cutover)
            ),
            Err(DedupReject::Duplicate { duplicate_of: 92 })
        );
    }

    #[test]
    fn icrc_001_none_created_at_time_skips_dedup() {
        let h = [4u8; 32];
        assert_eq!(dedup_check(NOW, None, &h), Ok(()));
        // Recording with None is a no-op; the same hash stays fresh forever.
        dedup_record(NOW, None, h, 7);
        assert_eq!(dedup_check(NOW, None, &h), Ok(()));
        assert_eq!(dedup_check(NOW, Some(NOW), &h), Ok(()));
    }

    #[test]
    fn icrc_001_pruning_keeps_map_bounded() {
        let h_old = [5u8; 32];
        let h_new = [6u8; 32];
        dedup_record(NOW, Some(NOW), h_old, 1);
        let before = seen_txs_len();
        // Inserting after the old entry expired must prune it.
        let later = NOW + TRANSACTION_WINDOW_NS + PERMITTED_DRIFT_NS + 1;
        dedup_record(later, Some(later), h_new, 2);
        assert!(seen_txs_len() <= before, "expired entries must be pruned on insert");
        assert_eq!(
            dedup_check(later, Some(later), &h_new),
            Err(DedupReject::Duplicate { duplicate_of: 2 })
        );
        // The pruned entry no longer matches as Duplicate (it is TooOld anyway).
        assert_eq!(dedup_check(later, Some(later - 1), &h_old), Ok(()));
    }

    #[test]
    fn icrc_001_pruning_is_batched_and_resumes_from_expiry_index() {
        let initial_count = MAX_EXPIRED_DEDUP_ENTRIES_PER_TRANSFER + 3;
        let mut hashes = Vec::with_capacity(initial_count);
        for i in 0..initial_count {
            let mut hash = [0xa5u8; 32];
            hash[1..9].copy_from_slice(&(i as u64).to_be_bytes());
            hashes.push(hash);
            dedup_record(NOW, Some(NOW), hash, i as u64);
        }
        assert_eq!(seen_txs_len(), initial_count);
        assert_eq!(seen_txs_expiry_len(), initial_count);

        let after_expiry = NOW + TRANSACTION_WINDOW_NS + PERMITTED_DRIFT_NS + 1;
        let fresh_a = [0xfau8; 32];
        dedup_record(after_expiry, Some(after_expiry), fresh_a, 10_000);
        assert_eq!(
            seen_txs_len(),
            initial_count - MAX_EXPIRED_DEDUP_ENTRIES_PER_TRANSFER + 1
        );
        assert_eq!(seen_txs_len(), seen_txs_expiry_len());

        let fresh_b = [0xfbu8; 32];
        dedup_record(after_expiry, Some(after_expiry), fresh_b, 10_001);
        assert_eq!(seen_txs_len(), 2);
        assert_eq!(seen_txs_len(), seen_txs_expiry_len());
        assert!(hashes.iter().all(|hash| !seen_tx_contains(hash)));
    }

    #[test]
    fn icrc_001_tx_hash_covers_caller_and_args() {
        let caller_a = Principal::self_authenticating(&[1]);
        let caller_b = Principal::self_authenticating(&[2]);
        let args = sample_transfer_arg(1, Some(NOW));

        // Identical (caller, args) hash identically.
        assert_eq!(
            hash_icrc1_transfer(&caller_a, &args),
            hash_icrc1_transfer(&caller_a, &args)
        );
        // Different caller, memo, or created_at_time produce different hashes.
        assert_ne!(
            hash_icrc1_transfer(&caller_a, &args),
            hash_icrc1_transfer(&caller_b, &args)
        );
        assert_ne!(
            hash_icrc1_transfer(&caller_a, &args),
            hash_icrc1_transfer(&caller_a, &sample_transfer_arg(2, Some(NOW)))
        );
        assert_ne!(
            hash_icrc1_transfer(&caller_a, &args),
            hash_icrc1_transfer(&caller_a, &sample_transfer_arg(1, Some(NOW + 1)))
        );
    }

    #[test]
    fn icrc2_approve_hash_covers_domain_caller_and_complete_args() {
        let caller = Principal::self_authenticating(&[1]);
        let other_caller = Principal::self_authenticating(&[2]);
        let args = sample_approve_args();
        let base = hash_icrc2_approve(&caller, &args);

        assert_eq!(base, hash_icrc2_approve(&caller, &args));
        assert_ne!(base, hash_icrc2_approve(&other_caller, &args));
        assert_ne!(
            base,
            hash_icrc1_transfer(&caller, &sample_transfer_arg(1, Some(NOW))),
            "approval and transfer hash domains must remain separate"
        );

        let mut changed = args.clone();
        changed.from_subaccount = Some([1; 32]);
        assert_ne!(base, hash_icrc2_approve(&caller, &changed));
        let mut changed = args.clone();
        changed.spender.owner = other_caller;
        assert_ne!(base, hash_icrc2_approve(&caller, &changed));
        let mut changed = args.clone();
        changed.spender.subaccount = Some([2; 32]);
        assert_ne!(base, hash_icrc2_approve(&caller, &changed));
        let mut changed = args.clone();
        changed.amount = Nat::from(1_001u64);
        assert_ne!(base, hash_icrc2_approve(&caller, &changed));
        let mut changed = args.clone();
        changed.expected_allowance = Some(Nat::from(501u64));
        assert_ne!(base, hash_icrc2_approve(&caller, &changed));
        let mut changed = args.clone();
        changed.expires_at = Some(NOW + 101);
        assert_ne!(base, hash_icrc2_approve(&caller, &changed));
        let mut changed = args.clone();
        changed.fee = None;
        assert_ne!(base, hash_icrc2_approve(&caller, &changed));
        let mut changed = args.clone();
        changed.memo = Some(icrc_ledger_types::icrc1::transfer::Memo(
            serde_bytes::ByteBuf::from(b"changed".to_vec()),
        ));
        assert_ne!(base, hash_icrc2_approve(&caller, &changed));
        let mut changed = args;
        changed.created_at_time = Some(NOW + 1);
        assert_ne!(base, hash_icrc2_approve(&caller, &changed));
    }

    #[test]
    fn pre_cutover_approval_identity_is_held_when_it_cannot_be_recovered() {
        let caller = Principal::self_authenticating(&[0x71]);
        let args = sample_approve_args();
        let hash = hash_icrc2_approve(&caller, &args);

        // Historical ICRC-3 blocks do not carry the complete approval tuple,
        // and older code did not store approval dedup identities. Do not turn
        // a still-live historical retry into a fresh approval after upgrade.
        assert_eq!(
            dedup_check_with_cutover(NOW + 1, Some(NOW), &hash, Some(NOW)),
            Err(DedupReject::TooOld)
        );
    }
}
