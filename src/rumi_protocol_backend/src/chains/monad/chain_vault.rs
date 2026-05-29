//! Monad (and future foreign-chain) vault record.
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
use crate::chains::multi_chain_state::MultiChainStateV2;
use crate::chains::settlement_queue::{SettlementOp, SettlementOpKind};
use candid::{CandidType, Deserialize, Principal};
use serde::Serialize;

/// Minimum collateral ratio (e4: 13000 == 130.00%) required to open a Monad
/// chain vault. Checked against DECLARED collateral at open time. Per-collateral
/// configurability is a later refinement (Phase 2 unifies the foreign-chain and
/// ICP-native CDP parameter models).
pub const MONAD_MIN_CR_E4: u64 = 13_000;

/// 1e18 — the wei scale of EVM-native (MON) collateral amounts.
const E18: u128 = 1_000_000_000_000_000_000;

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
    pub collateral_amount_e18: u128,
    pub debt_e8s: u128,
    /// Unvalidated 0x hex string. The settlement task (Task 10) validates
    /// before submitting the on-chain mint transaction.
    pub mint_recipient: String,
    pub pending_mint_e8s: u128,
    pub status: ChainVaultStatus,
    pub opened_at_ns: u64,
}

/// Reasons `open_chain_vault_in_state` / `verify_deposit_and_enqueue_mint_in_state`
/// can reject. Kept distinct from `ChainAdminError` so the open path can report
/// CR-specific failures the caller can surface.
#[derive(Debug, PartialEq, Eq)]
pub enum OpenVaultError {
    /// The collateral chain is not registered in `chain_configs`.
    UnknownChain,
    /// No manual MON price is set for the chain (`manual_prices[(chain,"MON")]`).
    NoPrice,
    /// Declared collateral ratio is below the minimum.
    BelowMinCr { cr_e4: u64, min_e4: u64 },
    /// Enqueuing the Mint op failed (e.g. duplicate idempotency key).
    QueueError(String),
    /// `verify_deposit_and_enqueue_mint_in_state` could not find the vault.
    UnknownVault,
}

/// Compute the collateral ratio (e4: 25000 == 250.00%) for a foreign-chain vault.
///
/// `collateral_e18` is the native-gas-asset amount in wei (1e18 scale).
/// `price_e8` is the asset's USD price as e8 (e.g. $2.00 == 2_0000_0000).
/// `debt_e8s` is the icUSD debt as e8s.
///
/// Returns `u64::MAX` when `debt_e8s == 0` (an unbounded ratio; a debt-free
/// vault is trivially over-collateralized). All arithmetic saturates so an
/// adversarial (or merely huge) input can never panic.
pub fn collateral_ratio_e4(collateral_e18: u128, price_e8: u64, debt_e8s: u128) -> u64 {
    if debt_e8s == 0 {
        return u64::MAX;
    }
    // collateral_usd_e8 = collateral_e18 * price_e8 / 1e18. Both operands are
    // already in their respective fixed-point scales; dividing by 1e18 drops the
    // wei scale and leaves a USD value in e8. Saturating so a colossal collateral
    // input cannot overflow.
    let collateral_usd_e8 = collateral_e18
        .saturating_mul(price_e8 as u128)
        / E18;
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
/// - `collateral_amount_e18 = collateral_e18` (the declared amount)
/// - `debt_e8s = 0` (no confirmed debt until the mint is observed at finality)
/// - `pending_mint_e8s = debt_e8s` (the INTENDED mint amount, surfaced for
///   deposit-watch to enqueue once the on-chain deposit is verified)
///
/// **Enqueues NOTHING.** icUSD is only minted against a verified on-chain
/// deposit; the mint-enqueue lives in `verify_deposit_and_enqueue_mint_in_state`
/// (driven by deposit-watch), NOT here.
///
/// Rejections (no mutation on any error path):
/// - chain not in `chain_configs` -> `UnknownChain`
/// - no `manual_prices[(chain,"MON")]` -> `NoPrice`
/// - declared CR `< min_cr_e4` -> `BelowMinCr`
#[allow(clippy::too_many_arguments)]
pub fn open_chain_vault_in_state(
    state: &mut MultiChainStateV2,
    chain: ChainId,
    owner: Principal,
    custody_address: String,
    collateral_e18: u128,
    debt_e8s: u128,
    mint_recipient: String,
    min_cr_e4: u64,
    now_ns: u64,
    vault_id: u64,
) -> Result<(), OpenVaultError> {
    // Reject an unregistered chain before reading anything else.
    if !state.chain_configs.contains_key(&chain) {
        return Err(OpenVaultError::UnknownChain);
    }
    // MON price (USD e8) for the declared-collateral CR check.
    let price_e8 = *state
        .manual_prices
        .get(&(chain, "MON".to_string()))
        .ok_or(OpenVaultError::NoPrice)?;

    let cr_e4 = collateral_ratio_e4(collateral_e18, price_e8, debt_e8s);
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
            collateral_amount_e18: collateral_e18,
            // Design B: no confirmed debt until the on-chain mint is observed.
            debt_e8s: 0,
            mint_recipient,
            // The INTENDED mint amount. deposit-watch enqueues exactly this once
            // the custody-address balance covers the declared collateral.
            pending_mint_e8s: debt_e8s,
            status: ChainVaultStatus::AwaitingDeposit,
            opened_at_ns: now_ns,
        },
    );
    // No mint enqueued — that happens in verify_deposit_and_enqueue_mint_in_state.
    Ok(())
}

/// Verify an observed custody-address balance and (if it covers the declared
/// collateral) flip an `AwaitingDeposit` vault to `MintPending` and enqueue its
/// `Mint` op. Driven by the deposit-watch loop.
///
/// Returns:
/// - `Ok(true)`  — transitioned + enqueued exactly one Mint.
/// - `Ok(false)` — no-op: either the vault is not `AwaitingDeposit` (already
///   processed; idempotent) OR the observed balance does not yet cover the
///   declared collateral.
/// - `Err(UnknownVault)` — no such vault.
/// - `Err(QueueError)`   — enqueue rejected (e.g. duplicate idempotency key).
///
/// ## Mutation ordering (no-mutation-on-rejection guarantee)
///
/// The enqueue runs FIRST (it can fail on a duplicate idempotency key). Only
/// after a successful enqueue does the status flip to `MintPending`. So a
/// rejected enqueue leaves the vault `AwaitingDeposit` and the queue unchanged,
/// and the next deposit-watch tick retries cleanly.
pub fn verify_deposit_and_enqueue_mint_in_state(
    state: &mut MultiChainStateV2,
    vault_id: u64,
    observed_balance_e18: u128,
    now_ns: u64,
) -> Result<bool, OpenVaultError> {
    // Read-only validation first — no mutation on any rejection / no-op path.
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
            v.collateral_amount_e18,
        )
    };

    // Not enough on-chain collateral yet — no mutation, retry next tick.
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
