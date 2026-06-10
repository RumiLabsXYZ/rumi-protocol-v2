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
use std::cell::RefCell;
use std::collections::BTreeMap;

use crate::state::{mutate_state, read_state};
use crate::types::{LpAllowance, Icrc3Transaction};

// ─── Transaction deduplication (audit 2026-06-09, ICRC-001) ───
//
// ICRC-1 standard dedup: when a caller supplies `created_at_time`, the ledger
// must reject transactions outside the dedup window (TooOld / CreatedInFuture)
// and reject an identical (caller, args) resubmission within the window with
// `Duplicate { duplicate_of }`. Window constants match the reference ICRC-1
// ledger used by icUSD/ICP (TRANSACTION_WINDOW = 24h, PERMITTED_DRIFT = 60s).
//
// The seen-transaction map is heap-only ON PURPOSE: an upgrade clears the
// dedup window. This is an accepted tradeoff per ICRC-1 (dedup is a
// best-effort retry guard, and several production ledgers behave the same
// way); a stable structure would consume a fresh MemoryId and add upgrade
// surface for a strictly best-effort guarantee, and the ICRC-3 block log
// remains the complete audit record either way. The map is BOUNDED: expired
// entries are pruned on every insert, so it holds at most the deduplicated
// transfers of the trailing 24h window.

pub const TRANSACTION_WINDOW_NS: u64 = 24 * 60 * 60 * 1_000_000_000;
pub const PERMITTED_DRIFT_NS: u64 = 60 * 1_000_000_000;

#[derive(Debug, PartialEq, Eq)]
pub enum DedupReject {
    TooOld,
    CreatedInFuture { ledger_time: u64 },
    Duplicate { duplicate_of: u64 },
}

thread_local! {
    /// tx hash -> (created_at_time, block index of the original transaction).
    static SEEN_TXS: RefCell<BTreeMap<[u8; 32], (u64, u64)>> = RefCell::new(BTreeMap::new());
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
    let Some(cat) = created_at_time else {
        return Ok(());
    };
    if tx_expired(cat, now) {
        return Err(DedupReject::TooOld);
    }
    if cat > now.saturating_add(PERMITTED_DRIFT_NS) {
        return Err(DedupReject::CreatedInFuture { ledger_time: now });
    }
    if let Some((_, block)) = SEEN_TXS.with(|m| m.borrow().get(tx_hash).copied()) {
        return Err(DedupReject::Duplicate { duplicate_of: block });
    }
    Ok(())
}

/// Record an executed deduplicated transaction. Prunes expired entries so the
/// map stays bounded to the trailing window. No-op when `created_at_time` is
/// `None` (such transactions are never deduplicated).
fn dedup_record(now: u64, created_at_time: Option<u64>, tx_hash: [u8; 32], block_index: u64) {
    let Some(cat) = created_at_time else {
        return;
    };
    SEEN_TXS.with(|m| {
        let mut m = m.borrow_mut();
        m.retain(|_, (seen_cat, _)| !tx_expired(*seen_cat, now));
        m.insert(tx_hash, (cat, block_index));
    });
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
        let now = ic_cdk::api::time();
        if expires_at < now {
            return Err(ApproveError::Expired { ledger_time: now });
        }
    }

    mutate_state(|s| {
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
        });
        Ok(Nat::from(id))
    })
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
        // Check and deduct allowance (unless self-transfer)
        if caller != from_principal {
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

            // Deduct allowance
            let mut entry = existing.unwrap();
            entry.amount = entry.amount.saturating_sub(amount);
            if entry.amount == 0 {
                crate::storage::allowance_remove(&from_principal, &caller);
            } else {
                crate::storage::allowance_set(from_principal, caller, entry);
            }
        }

        // Check balance
        let from_balance = crate::storage::lp_balance_get(&from_principal);
        if from_balance < amount {
            return Err(TransferFromError::InsufficientFunds {
                balance: Nat::from(from_balance),
            });
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
    SEEN_TXS.with(|m| m.borrow().len())
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
}
