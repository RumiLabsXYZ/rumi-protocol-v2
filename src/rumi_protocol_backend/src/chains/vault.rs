//! Chain-agnostic foreign-chain vault record + state machine.
//!
//! Hoisted out of `chains::monad::chain_vault` (Task F3) so a second chain
//! (Solana, Task 6) can reuse the SAME open/verify/withdraw/close logic instead
//! of forking it. The types and the two already-generalized helpers
//! (`collateral_ratio_e4`, `verify_deposit_and_enqueue_mint_in_state`) moved here
//! VERBATIM. The three address/price-aware helpers
//! (`open_chain_vault_in_state`, `withdraw_collateral_in_state`,
//! `close_chain_vault_in_state`) moved here PARAMETERIZED on two seams that were
//! previously hardcoded to Monad:
//!
//! - `address_validator: fn(&str) -> bool` - the per-chain recipient/destination
//!   address well-formedness check (was `monad::tecdsa::is_valid_evm_address`).
//! - `price_symbol: &str` - the manual-price key for the chain's native gas asset
//!   (was the literal `"MON"`; Solana uses `"SOL"`).
//!
//! `chains::monad::chain_vault` re-exports the types + un-parameterized helper
//! and wraps these three with the Monad specifics baked in
//! (`is_valid_evm_address`, `"MON"`), so EVERY existing Monad caller and test
//! resolves and behaves byte-identically. Task 6 calls these generalized helpers
//! directly with `(is_valid_solana_address, "SOL")`.
//!
//! Lives in `MultiChainStateV2.chain_vaults`, keyed by the globally-unique
//! u64 vault_id. The core ICP-native `Vault` struct is untouched in Phase 1b;
//! unifying the two models is a deliberate Phase 2 task.
//!
//! Design B (confirmed-supply): `debt_e8s` is the CONFIRMED debt. While a mint
//! is in flight, the intended amount lives in `pending_mint_e8s` and does NOT
//! count toward `total_debt` or `chain_supplies` until the on-chain mint is
//! observed at finality (settlement worker, Task 10).

use crate::chains::config::ChainId;
use crate::chains::multi_chain_state::MultiChainStateV4;
use crate::chains::settlement_queue::{SettlementOp, SettlementOpKind};
use candid::{CandidType, Deserialize, Principal};
use serde::Serialize;

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub enum ChainVaultStatus {
    /// Vault opened; awaiting the on-chain collateral deposit. No mint enqueued
    /// yet (open-then-verify). deposit-watch flips this to MintPending once the
    /// custody-address balance covers the declared collateral at finality.
    AwaitingDeposit,
    MintPending,
    Open,
    Closing,
    Closed,
}

#[derive(CandidType, Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
pub struct ChainVaultV1 {
    pub vault_id: u64,
    pub owner: Principal,
    pub collateral_chain: ChainId,
    /// Unvalidated 0x hex string. The deposit-watch task (Task 9) validates
    /// on-chain before crediting any collateral.
    pub custody_address: String,
    /// Collateral in the chain's NATIVE base units (wei for 18-decimal MON,
    /// lamports for 9-decimal SOL). `#[serde(rename)]` keeps the on-wire and
    /// candid field name as the legacy `collateral_amount_e18`, so existing state
    /// snapshots and the candid interface stay byte-compatible (no migration);
    /// only the Rust-visible name is corrected. Pair with
    /// `ChainConfig.chain_native_decimals` for any USD/CR math.
    #[serde(rename = "collateral_amount_e18")]
    pub collateral_amount_native: u128,
    pub debt_e8s: u128,
    /// Unvalidated 0x hex string. The settlement task (Task 10) validates
    /// before submitting the on-chain mint transaction.
    pub mint_recipient: String,
    pub pending_mint_e8s: u128,
    pub status: ChainVaultStatus,
    pub opened_at_ns: u64,
    /// EVM owner address (lowercase `0x`) for vaults opened via the M2 self-serve
    /// `_evm` path; `None` for developer-opened / Monad / Solana vaults. Used to
    /// authorize borrow/withdraw/close by re-recovering the EIP-712 signer.
    /// `#[serde(default)]` keeps pre-M2 ciborium snapshots decoding cleanly
    /// (State is ciborium-encoded — see storage.rs).
    #[serde(default)]
    pub owner_evm: Option<String>,
}

/// Reasons `open_chain_vault_in_state` / `verify_deposit_and_enqueue_mint_in_state`
/// can reject. Kept distinct from `ChainAdminError` so the open path can report
/// CR-specific failures the caller can surface.
#[derive(Debug, PartialEq, Eq)]
pub enum OpenVaultError {
    /// The collateral chain is not registered in `chain_configs`.
    UnknownChain,
    /// No manual native-asset price is set for the chain
    /// (`manual_prices[(chain, price_symbol)]`).
    NoPrice,
    /// Declared collateral ratio is below the minimum.
    BelowMinCr { cr_e4: u64, min_e4: u64 },
    /// Declared debt is zero. A zero-debt vault has nothing to mint; allowing it
    /// would enqueue a wasted zero-value on-chain mint once "deposited".
    ZeroDebt,
    /// Enqueuing the Mint op failed (e.g. duplicate idempotency key).
    QueueError(String),
    /// `verify_deposit_and_enqueue_mint_in_state` could not find the vault.
    UnknownVault,
    /// The developer-supplied `mint_recipient` is not a well-formed address for
    /// the chain (per the injected `address_validator`; for Monad: `0x` + 40
    /// hex). Rejected at the boundary so it can never reach the tx-building
    /// helpers (e.g. `tx::abi_word_address`), which panic on malformed
    /// hex/length deep on the settlement worker path. Carries the bad address.
    InvalidAddress(String),
}

/// Reasons `withdraw_collateral_in_state` / `close_chain_vault_in_state` can
/// reject. No mutation occurs on any error path.
#[derive(Debug, PartialEq, Eq)]
pub enum WithdrawError {
    /// No such vault.
    UnknownVault,
    /// No manual native-asset price is set for the chain
    /// (`manual_prices[(chain, price_symbol)]`).
    NoPrice,
    /// Requested amount exceeds the vault's `collateral_amount_native`.
    InsufficientCollateral,
    /// The post-withdrawal collateral ratio would fall below `min_cr_e4`.
    BelowMinCr { cr_e4: u64, min_e4: u64 },
    /// Enqueuing the `NativeWithdrawal` op failed (e.g. duplicate idempotency key).
    QueueError(String),
    /// `close_chain_vault_in_state` was called on a vault with non-zero debt.
    HasDebt,
    /// The developer-supplied `dest_address` is not a well-formed address for the
    /// chain (per the injected `address_validator`; for Monad: `0x` + 40 hex).
    /// Rejected at the boundary so it can never reach the tx-building helpers
    /// (e.g. `tx::parse_address`), which panic on malformed hex/length deep on
    /// the settlement worker path. Carries the bad address.
    InvalidAddress(String),
    /// Withdraw was attempted on a vault whose status is not `Open`. Only an
    /// `Open` vault has confirmed, on-chain-deposited collateral and confirmed
    /// debt; an `AwaitingDeposit` vault holds DECLARED-but-never-deposited
    /// collateral (paying it out would drain the settlement hot wallet for
    /// phantom collateral), and a `MintPending` vault has a mint in flight
    /// against its collateral. Carries the offending status.
    WrongStatus { status: ChainVaultStatus },
    /// A borrow mint is in flight for this Open vault (`pending_mint_e8s != 0`).
    /// The CR check would run against the STALE confirmed `debt_e8s` (which does
    /// not yet include the pending borrow), so releasing collateral now could
    /// leave the vault below `min_cr_e4` once the borrow mint confirms. Reject
    /// until the mint settles (mirrors `BorrowError::MintInFlight`). M2 review-A.
    MintInFlight,
}

/// Compute the collateral ratio (e4: 25000 == 250.00%) for a foreign-chain vault.
///
/// `collateral_native` is the collateral amount in the chain's NATIVE base units
/// (wei for an 18-decimal EVM gas asset, lamports for 9-decimal SOL).
/// `native_decimals` is that asset's decimal count (18 for MON/ETH, 9 for SOL);
/// the amount is divided by `10^native_decimals` to recover a whole-unit value.
/// `price_e8` is the asset's USD price as e8 (e.g. $2.00 == 2_0000_0000).
/// `debt_e8s` is the icUSD debt as e8s.
///
/// Returns `u64::MAX` when `debt_e8s == 0` (an unbounded ratio; a debt-free
/// vault is trivially over-collateralized). All arithmetic saturates so an
/// adversarial (or merely huge) input can never panic.
pub fn collateral_ratio_e4(
    collateral_native: u128,
    native_decimals: u8,
    price_e8: u64,
    debt_e8s: u128,
) -> u64 {
    if debt_e8s == 0 {
        return u64::MAX;
    }
    // collateral_usd_e8 = collateral_native * price_e8 / 10^native_decimals.
    // Dividing by the native scale drops the base-unit scale and leaves a USD
    // value in e8. Saturating so a colossal collateral input cannot overflow.
    let native_scale = 10u128.saturating_pow(native_decimals as u32);
    let collateral_usd_e8 = collateral_native
        .saturating_mul(price_e8 as u128)
        / native_scale;
    // cr_e4 = collateral_usd_e8 / debt_e8s * 10_000. Multiply first (saturating)
    // then divide so we keep e4 precision; both are e8 so the e8 scales cancel.
    let cr = collateral_usd_e8.saturating_mul(10_000) / debt_e8s;
    cr.min(u64::MAX as u128) as u64
}

/// Open a foreign-chain vault in the `AwaitingDeposit` state (open-then-verify).
///
/// CR-checks the DECLARED collateral against `min_cr_e4`. On success, inserts a
/// `ChainVaultV1` with:
/// - `status = AwaitingDeposit`
/// - `collateral_amount_native = collateral_e18` (the declared amount)
/// - `debt_e8s = 0` (no confirmed debt until the mint is observed at finality)
/// - `pending_mint_e8s = debt_e8s` (the INTENDED mint amount, surfaced for
///   deposit-watch to enqueue once the on-chain deposit is verified)
///
/// **Enqueues NOTHING.** icUSD is only minted against a verified on-chain
/// deposit; the mint-enqueue lives in `verify_deposit_and_enqueue_mint_in_state`
/// (driven by deposit-watch), NOT here.
///
/// ## Chain-agnostic seams
///
/// - `address_validator` validates `mint_recipient` well-formedness for the
///   target chain (Monad: `is_valid_evm_address`; Solana: `is_valid_solana_address`).
/// - `price_symbol` is the `manual_prices` key for the chain's native gas asset
///   (Monad: `"MON"`; Solana: `"SOL"`).
///
/// Rejections (no mutation on any error path):
/// - chain not in `chain_configs` -> `UnknownChain`
/// - no `manual_prices[(chain, price_symbol)]` -> `NoPrice`
/// - declared CR `< min_cr_e4` -> `BelowMinCr`
#[allow(clippy::too_many_arguments)]
pub fn open_chain_vault_in_state(
    state: &mut MultiChainStateV4,
    chain: ChainId,
    owner: Principal,
    custody_address: String,
    collateral_e18: u128,
    debt_e8s: u128,
    mint_recipient: String,
    address_validator: fn(&str) -> bool,
    price_symbol: &str,
    min_cr_e4: u64,
    now_ns: u64,
    vault_id: u64,
) -> Result<(), OpenVaultError> {
    // Reject an unregistered chain before reading anything else.
    if !state.chain_configs.contains_key(&chain) {
        return Err(OpenVaultError::UnknownChain);
    }
    // A zero-debt vault has nothing to mint; with debt 0 the CR check is a
    // no-op (u64::MAX) and deposit-watch would later enqueue a wasted
    // zero-value on-chain mint. Reject up front. (A zero-collateral vault with
    // positive debt is already rejected below by the CR check.)
    if debt_e8s == 0 {
        return Err(OpenVaultError::ZeroDebt);
    }
    // Reject a malformed developer-supplied mint recipient BEFORE it enters
    // state. An unvalidated address would later panic the tx-building helpers
    // (`tx::abi_word_address`) deep on the settlement worker path, after the
    // re-entrancy guard + awaits, permanently blocking the chain's worker.
    // (`custody_address` is threshold-key-derived and always valid - do NOT validate it.)
    if !address_validator(&mint_recipient) {
        return Err(OpenVaultError::InvalidAddress(mint_recipient));
    }
    // Native-asset price (USD e8) for the declared-collateral CR check.
    let price_e8 = *state
        .manual_prices
        .get(&(chain, price_symbol.to_string()))
        .ok_or(OpenVaultError::NoPrice)?;
    // Native-asset decimals for the chain (18 for MON; falls back to 18 if the
    // config is somehow absent, though `chain` was verified registered above).
    let native_decimals = state
        .chain_configs
        .get(&chain)
        .map(|c| c.chain_native_decimals)
        .unwrap_or(18);

    let cr_e4 = collateral_ratio_e4(collateral_e18, native_decimals, price_e8, debt_e8s);
    if cr_e4 < min_cr_e4 {
        return Err(OpenVaultError::BelowMinCr { cr_e4, min_e4: min_cr_e4 });
    }

    state.chain_vaults.insert(
        vault_id,
        ChainVaultV1 {
            vault_id,
            owner,
            collateral_chain: chain,
            custody_address,
            collateral_amount_native: collateral_e18,
            // Design B: no confirmed debt until the on-chain mint is observed.
            debt_e8s: 0,
            mint_recipient,
            // The INTENDED mint amount. deposit-watch enqueues exactly this once
            // the custody-address balance covers the declared collateral.
            pending_mint_e8s: debt_e8s,
            status: ChainVaultStatus::AwaitingDeposit,
            opened_at_ns: now_ns, owner_evm: None,
        },
    );
    // No mint enqueued - that happens in verify_deposit_and_enqueue_mint_in_state.
    Ok(())
}

/// Verify an observed custody-address balance and (if it covers the declared
/// collateral) flip an `AwaitingDeposit` vault to `MintPending` and enqueue its
/// `Mint` op. Driven by the deposit-watch loop.
///
/// Returns:
/// - `Ok(true)` - transitioned + enqueued exactly one Mint.
/// - `Ok(false)` - no-op: either the vault is not `AwaitingDeposit` (already
///   processed; idempotent) OR the observed balance does not yet cover the
///   declared collateral.
/// - `Err(UnknownVault)` - no such vault.
/// - `Err(QueueError)`  - enqueue rejected (e.g. duplicate idempotency key).
///
/// ## Mutation ordering (no-mutation-on-rejection guarantee)
///
/// The enqueue runs FIRST (it can fail on a duplicate idempotency key). Only
/// after a successful enqueue does the status flip to `MintPending`. So a
/// rejected enqueue leaves the vault `AwaitingDeposit` and the queue unchanged,
/// and the next deposit-watch tick retries cleanly.
pub fn verify_deposit_and_enqueue_mint_in_state(
    state: &mut MultiChainStateV4,
    vault_id: u64,
    observed_balance_e18: u128,
    now_ns: u64,
) -> Result<bool, OpenVaultError> {
    // Read-only validation first - no mutation on any rejection / no-op path.
    let (chain, recipient, amount_e8s, declared_e18) = {
        let v = state
            .chain_vaults
            .get(&vault_id)
            .ok_or(OpenVaultError::UnknownVault)?;
        // Idempotent: anything other than AwaitingDeposit was already processed.
        if v.status != ChainVaultStatus::AwaitingDeposit {
            return Ok(false);
        }
        (
            v.collateral_chain,
            v.mint_recipient.clone(),
            v.pending_mint_e8s,
            v.collateral_amount_native,
        )
    };

    // Not enough on-chain collateral yet - no mutation, retry next tick.
    if observed_balance_e18 < declared_e18 {
        return Ok(false);
    }

    // Enqueue FIRST (it can fail on a duplicate idempotency key). The key is
    // per (chain, vault) so a retried tick cannot double-enqueue.
    let op = SettlementOp::new(
        SettlementOpKind::Mint { recipient, amount_e8s, vault_id },
        format!("mint-{}-{}", chain.0, vault_id),
        now_ns,
    );
    state
        .settlement_queues
        .entry(chain)
        .or_default()
        .enqueue(op)
        .map_err(|e| OpenVaultError::QueueError(format!("{e:?}")))?;

    // Only after a successful enqueue: flip the vault to MintPending.
    let v = state
        .chain_vaults
        .get_mut(&vault_id)
        .expect("vault present: checked above");
    v.status = ChainVaultStatus::MintPending;
    Ok(true)
}

/// Withdraw foreign-chain collateral, enqueuing a native gas-asset transfer-out.
///
/// ## Repay is observer-driven, NOT here
///
/// There is no separate repay path: the user burns icUSD on the foreign chain
/// (`IcUSD.burn`) and the Task-9 burn-watch (`deposit_watch::run_observer` ->
/// `apply_burn_to_state`) decrements `debt_e8s` + chain supply at finality.
/// This helper only moves COLLATERAL out; it never touches `debt_e8s`,
/// `chain_supplies`, or `pending_mint_e8s`.
///
/// ## Chain-agnostic seams
///
/// - `address_validator` validates `dest_address` well-formedness for the target
///   chain (Monad: `is_valid_evm_address`; Solana: `is_valid_solana_address`).
/// - `price_symbol` is the `manual_prices` key for the chain's native gas asset
///   (Monad: `"MON"`; Solana: `"SOL"`).
///
/// ## Semantics
///
/// - `UnknownVault` if the vault is absent.
/// - `InsufficientCollateral` if `amount_e18 > collateral_amount_native`.
/// - `NoPrice` if no `manual_prices[(chain, price_symbol)]` is set.
/// - When `debt_e8s > 0`, the REMAINING collateral
///   (`collateral_amount_native - amount_e18`) is CR-checked against `min_cr_e4`;
///   below it -> `BelowMinCr`. A debt-free vault skips the CR check (any
///   remainder is trivially over-collateralized).
///
/// On success: enqueues a `NativeWithdrawal { recipient: dest_address,
/// amount_e18, vault_id }` op (idempotency key
/// `withdraw-{chain}-{vault_id}-{now_ns}`), RESERVES the collateral by
/// decrementing `collateral_amount_native` to the remainder, and flips the vault
/// to `Closing` iff `remaining == 0 && debt_e8s == 0`. The settlement worker
/// flips `Closing -> Closed` once the on-chain transfer confirms (and ADDS the
/// reserved collateral back if the transfer reverts).
///
/// ## Reserve-at-enqueue
///
/// Decrementing `collateral_amount_native` at enqueue time is the standard CDP
/// reserve-on-request pattern: a second withdraw cannot double-spend the same
/// collateral while the first is still in flight. A reverted transfer restores
/// the reserve in `settlement::confirm_op`.
///
/// ## Mutation ordering (no-mutation-on-rejection guarantee)
///
/// 1. Validate (vault lookup, balance, price, CR) - no mutation on any reject.
/// 2. Enqueue FIRST (can fail on a duplicate idempotency key -> `QueueError`,
///    no collateral mutation).
/// 3. Only after a successful enqueue: decrement collateral + set `Closing`.
///
/// So a rejected enqueue leaves the vault and its collateral untouched.
#[allow(clippy::too_many_arguments)]
pub fn withdraw_collateral_in_state(
    state: &mut MultiChainStateV4,
    vault_id: u64,
    amount_e18: u128,
    dest_address: String,
    address_validator: fn(&str) -> bool,
    price_symbol: &str,
    min_cr_e4: u64,
    now_ns: u64,
) -> Result<(), WithdrawError> {
    // Step 1: read-only validation. No mutation on any rejection path.
    let (chain, remaining, debt_e8s) = {
        let v = state
            .chain_vaults
            .get(&vault_id)
            .ok_or(WithdrawError::UnknownVault)?;
        // CRITICAL: `Open` is the ONLY state a withdraw can proceed from - the
        // only state with confirmed, on-chain-deposited collateral AND confirmed
        // debt. In `AwaitingDeposit` the collateral is DECLARED but never
        // deposited (debt 0 would skip the CR check below), so paying it out from
        // the settlement hot wallet would drain protocol funds for phantom
        // collateral. In `MintPending` a mint is in flight against the collateral.
        // `Closing`/`Closed` are terminal. Reject before reading balance/price or
        // mutating anything. (close_chain_vault_in_state delegates here, so it
        // inherits this gate after its own debt==0 check.)
        if v.status != ChainVaultStatus::Open {
            return Err(WithdrawError::WrongStatus { status: v.status.clone() });
        }
        // A borrow mint in flight means the CR check below would run against the
        // stale confirmed debt (pending borrow not yet folded in); releasing
        // collateral now could undercollateralize the vault once the mint
        // confirms. Reject until it settles (the settlement queue is sequential,
        // so this clears quickly). M2 review finding A.
        if v.pending_mint_e8s != 0 {
            return Err(WithdrawError::MintInFlight);
        }
        if amount_e18 > v.collateral_amount_native {
            return Err(WithdrawError::InsufficientCollateral);
        }
        let remaining = v.collateral_amount_native - amount_e18;
        (v.collateral_chain, remaining, v.debt_e8s)
    };

    // Price is needed for the post-withdrawal CR check (only when debt remains).
    let price_e8 = *state
        .manual_prices
        .get(&(chain, price_symbol.to_string()))
        .ok_or(WithdrawError::NoPrice)?;
    let native_decimals = state
        .chain_configs
        .get(&chain)
        .map(|c| c.chain_native_decimals)
        .unwrap_or(18);

    // A debt-free vault is trivially over-collateralized; skip the CR check.
    if debt_e8s > 0 {
        let cr_e4 = collateral_ratio_e4(remaining, native_decimals, price_e8, debt_e8s);
        if cr_e4 < min_cr_e4 {
            return Err(WithdrawError::BelowMinCr { cr_e4, min_e4: min_cr_e4 });
        }
    }

    // Reject a malformed developer-supplied destination BEFORE enqueuing. An
    // unvalidated address would later panic the tx-building helper
    // (`tx::parse_address`) deep on the settlement worker path, after the
    // re-entrancy guard + awaits, permanently blocking the chain's worker. This
    // is read-only (still no mutation on this rejection path).
    if !address_validator(&dest_address) {
        return Err(WithdrawError::InvalidAddress(dest_address));
    }

    // Step 2: enqueue FIRST (it can fail on a duplicate idempotency key). The
    // key is per (chain, vault, now_ns) so distinct withdraws never collide.
    let op = SettlementOp::new(
        SettlementOpKind::NativeWithdrawal {
            recipient: dest_address,
            amount_e18,
            vault_id,
        },
        format!("withdraw-{}-{}-{}", chain.0, vault_id, now_ns),
        now_ns,
    );
    state
        .settlement_queues
        .entry(chain)
        .or_default()
        .enqueue(op)
        .map_err(|e| WithdrawError::QueueError(format!("{e:?}")))?;

    // Step 3: only after a successful enqueue - reserve the collateral and flip
    // to Closing when the vault is now empty AND debt-free.
    let v = state
        .chain_vaults
        .get_mut(&vault_id)
        .expect("vault present: checked above");
    v.collateral_amount_native = remaining;
    if remaining == 0 && debt_e8s == 0 {
        v.status = ChainVaultStatus::Closing;
    }
    Ok(())
}

/// Close a debt-free vault by withdrawing the FULL remaining collateral.
///
/// Requires `debt_e8s == 0` (`HasDebt` otherwise) - debt must first be repaid
/// by burning icUSD on the foreign chain (observer-driven, see
/// `withdraw_collateral_in_state`). Then delegates to
/// `withdraw_collateral_in_state` for the full `collateral_amount_native`, which
/// inherits the `status == Open` gate (a non-Open vault rejects with
/// `WrongStatus`), reserves the collateral, and flips the vault to `Closing`
/// (the worker flips it to `Closed` on the transfer's confirmation).
///
/// `address_validator` and `price_symbol` are the same per-chain seams as
/// `withdraw_collateral_in_state`; they are threaded straight through to the
/// delegate.
///
/// ## Zero-collateral short-circuit
///
/// If the vault is `Open`, debt-free, AND already holds zero collateral, this
/// stamps it `Closed` directly and returns `Ok(())` WITHOUT enqueuing a
/// zero-value `NativeWithdrawal`. A 21k-gas native transfer of value 0 from the
/// settlement hot wallet is pure waste, and this protocol is gas/cycle-sensitive.
/// (A non-Open zero-collateral vault is NOT short-circuited - it falls through
/// to the delegate's gate and rejects with `WrongStatus`.)
pub fn close_chain_vault_in_state(
    state: &mut MultiChainStateV4,
    vault_id: u64,
    dest_address: String,
    address_validator: fn(&str) -> bool,
    price_symbol: &str,
    min_cr_e4: u64,
    now_ns: u64,
) -> Result<(), WithdrawError> {
    // Validate debt-free + capture the full remaining collateral and status (no
    // mutation on any rejection path).
    let (full, status) = {
        let v = state
            .chain_vaults
            .get(&vault_id)
            .ok_or(WithdrawError::UnknownVault)?;
        if v.debt_e8s != 0 {
            return Err(WithdrawError::HasDebt);
        }
        // A repaid (debt 0) but still-Open vault CAN have a borrow mint in flight
        // (pending_mint_e8s != 0). Closing it now would release all collateral,
        // then the borrow mint confirms and leaves debt with no backing. Reject
        // BEFORE the degenerate short-circuit below. M2 review finding A. (The
        // delegate's withdraw path also guards this, but the zero-collateral
        // short-circuit bypasses the delegate, so the check must live here too.)
        if v.pending_mint_e8s != 0 {
            return Err(WithdrawError::MintInFlight);
        }
        (v.collateral_amount_native, v.status.clone())
    };

    // Degenerate close: an Open, debt-free vault with no collateral to return.
    // Stamp Closed directly - enqueuing a zero-value transfer would burn 21k gas
    // for nothing. Restricted to `Open` so a non-Open (Closing/Closed/etc.)
    // zero-collateral vault still rejects via the delegate's WrongStatus gate.
    if full == 0 && status == ChainVaultStatus::Open {
        let v = state
            .chain_vaults
            .get_mut(&vault_id)
            .expect("vault present: checked above");
        v.status = ChainVaultStatus::Closed;
        return Ok(());
    }

    withdraw_collateral_in_state(
        state,
        vault_id,
        full,
        dest_address,
        address_validator,
        price_symbol,
        min_cr_e4,
        now_ns,
    )
}

// ─── M2: borrow + anti-spam ───────────────────────────────────────────────────

/// Anti-spam: max non-terminal vaults a single synthetic owner may hold.
pub const MAX_VAULTS_PER_OWNER: usize = 25;

/// Reasons `borrow_chain_vault_in_state` can reject. No mutation on any error.
#[derive(Debug, PartialEq, Eq)]
pub enum BorrowError {
    UnknownVault,
    /// Vault is not `Open` (only an Open vault has confirmed collateral + debt).
    WrongStatus { status: ChainVaultStatus },
    /// A mint is already in flight for this vault (`pending_mint_e8s != 0`).
    MintInFlight,
    ZeroDebt,
    NoPrice,
    BelowMinCr { cr_e4: u64, min_e4: u64 },
    QueueError(String),
    InvalidAddress(String),
}

/// Borrow additional icUSD against an existing `Open` vault — a SECOND on-chain
/// mint, gated by the per-op `IcUSD` idempotency (the mint op carries the
/// settlement queue's unique-per-chain `op_id`). Sets `pending_mint_e8s =
/// additional_e8s` and enqueues a `Mint` op; the settlement confirm moves
/// pending→debt + chain supply at finality (Design B). The off-chain idempotency
/// key embeds `now_ns` so it never collides with the genesis open mint key
/// (`mint-{chain}-{vault}`).
///
/// `address_validator` / `price_symbol` are the same per-chain seams as the open
/// helper. Rejections (no mutation on any path): `additional == 0` → `ZeroDebt`;
/// malformed `recipient` → `InvalidAddress`; absent/non-Open vault →
/// `UnknownVault`/`WrongStatus`; an in-flight mint → `MintInFlight`; no price →
/// `NoPrice`; post-borrow CR `< min_cr_e4` → `BelowMinCr`.
#[allow(clippy::too_many_arguments)]
pub fn borrow_chain_vault_in_state(
    state: &mut MultiChainStateV4,
    vault_id: u64,
    additional_e8s: u128,
    recipient: String,
    address_validator: fn(&str) -> bool,
    price_symbol: &str,
    min_cr_e4: u64,
    now_ns: u64,
) -> Result<(), BorrowError> {
    if additional_e8s == 0 {
        return Err(BorrowError::ZeroDebt);
    }
    if !address_validator(&recipient) {
        return Err(BorrowError::InvalidAddress(recipient));
    }
    // Step 1: read-only validation — no mutation on any rejection path.
    let (chain, collateral, new_debt) = {
        let v = state.chain_vaults.get(&vault_id).ok_or(BorrowError::UnknownVault)?;
        // Only an Open vault has confirmed, on-chain-deposited collateral AND
        // confirmed debt to borrow against.
        if v.status != ChainVaultStatus::Open {
            return Err(BorrowError::WrongStatus { status: v.status.clone() });
        }
        // No stacked borrows: a non-zero pending means a mint is already in flight
        // for this vault, and the settlement queue is sequential per chain.
        if v.pending_mint_e8s != 0 {
            return Err(BorrowError::MintInFlight);
        }
        (
            v.collateral_chain,
            v.collateral_amount_native,
            v.debt_e8s.saturating_add(additional_e8s),
        )
    };
    let price_e8 = *state
        .manual_prices
        .get(&(chain, price_symbol.to_string()))
        .ok_or(BorrowError::NoPrice)?;
    let native_decimals = state
        .chain_configs
        .get(&chain)
        .map(|c| c.chain_native_decimals)
        .unwrap_or(18);
    let cr_e4 = collateral_ratio_e4(collateral, native_decimals, price_e8, new_debt);
    if cr_e4 < min_cr_e4 {
        return Err(BorrowError::BelowMinCr { cr_e4, min_e4: min_cr_e4 });
    }
    // Step 2: enqueue FIRST (can fail on a duplicate key). The `now_ns` suffix
    // keeps this key distinct from the genesis open mint (`mint-{chain}-{vault}`).
    let op = SettlementOp::new(
        SettlementOpKind::Mint { recipient, amount_e8s: additional_e8s, vault_id },
        format!("mint-{}-{}-{}", chain.0, vault_id, now_ns),
        now_ns,
    );
    state
        .settlement_queues
        .entry(chain)
        .or_default()
        .enqueue(op)
        .map_err(|e| BorrowError::QueueError(format!("{e:?}")))?;
    // Step 3: only after a successful enqueue — reserve the borrow as pending.
    let v = state.chain_vaults.get_mut(&vault_id).expect("vault present: checked above");
    v.pending_mint_e8s = additional_e8s;
    Ok(())
}

// ─── M2: stale AwaitingDeposit GC (anti-spam backstop) ────────────────────────

/// TTL for an unfunded vault before the GC reaps it (24h in ns).
pub const AWAITING_DEPOSIT_TTL_NS: u64 = 24 * 60 * 60 * 1_000_000_000;

/// Remove `AwaitingDeposit` vaults whose `opened_at_ns` is older than `ttl_ns`.
/// Returns the number pruned.
///
/// Safe: an `AwaitingDeposit` vault has NO confirmed debt, NO enqueued mint, and
/// contributes nothing to `chain_supplies`, so removing it cannot break the
/// supply invariant. Only unfunded vaults are reaped — the observer flips a
/// funded vault to `MintPending` within its tick (seconds–minutes) long before
/// the 24h TTL, so a real deposit is never stranded. This is the anti-spam
/// backstop: it bounds total unfunded state without the self-DoS of a hard cap.
///
/// M2 review finding F: a vault is reaped ONLY if its chain's observer is
/// currently running — `status == Registered` AND not `reorg_halted`. If the
/// observer is halted or the chain is disabled, a funded-but-not-yet-observed
/// deposit could otherwise be stranded by the GC. A vault on an
/// inactive-observer chain is left until the observer resumes (then it either
/// flips to `MintPending` if funded, or ages out on a later GC tick once active).
pub fn prune_stale_awaiting_deposit(
    state: &mut MultiChainStateV4,
    now_ns: u64,
    ttl_ns: u64,
) -> usize {
    use crate::chains::config::ChainStatus;
    let stale: Vec<u64> = state
        .chain_vaults
        .iter()
        .filter(|(_, v)| {
            let observer_active = matches!(
                state.chain_configs.get(&v.collateral_chain).map(|c| c.status),
                Some(ChainStatus::Registered)
            ) && !state.reorg_halted.get(&v.collateral_chain).copied().unwrap_or(false);
            observer_active
                && v.status == ChainVaultStatus::AwaitingDeposit
                && now_ns.saturating_sub(v.opened_at_ns) > ttl_ns
        })
        .map(|(&id, _)| id)
        .collect();
    for id in &stale {
        state.chain_vaults.remove(id);
    }
    stale.len()
}
