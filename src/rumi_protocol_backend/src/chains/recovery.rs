//! On-chain-verified recovery for wedged settlement ops and stuck chain vaults
//! (audit M-08 / RECOV-01 and M-09 / RECOV-02).
//!
//! Both recovery endpoints (`resolve_stuck_settlement_op`,
//! `recover_stuck_chain_vault`) reverse or release protocol accounting on what
//! used to be PURE operator assertion. That is dangerous: reversing a Mint op
//! whose tx actually landed leaves icUSD minted with no recorded debt (an
//! unbacked mint), and flipping a `MintPending` vault to `Open` releases its
//! collateral — so if the mint really landed the user keeps both the on-chain
//! icUSD AND the collateral.
//!
//! This module re-reads the relevant on-chain state through the SAME
//! multi-provider EVM-RPC quorum the observer/settlement use (so a single
//! lagging/lying provider cannot spoof the check) and REFUSES the reversal /
//! release when on-chain state shows the op landed (or cannot be shown NOT to
//! have landed). The `#[update]` handlers in `main.rs` are thin async wrappers
//! over these helpers (the wrappers keep the developer-only auth check + the
//! event/log emission); all the verification + state-machine logic lives here so
//! it is unit-testable without PocketIC.
//!
//! Borrow discipline mirrors the settlement worker: snapshot under `read_state`,
//! `.await` the RPC, then commit under `mutate_state`; no state borrow is held
//! across an `.await`.

use crate::chains::config::ChainId;
use crate::chains::monad::chain_vault::ChainVaultStatus;
use crate::chains::settlement_queue::{SettlementOpKind, SettlementOpStatus};
use crate::state::{mutate_state, read_state};

/// Why a verified recovery was refused. The `#[update]` wrappers map this to a
/// `ProtocolError::ChainAdmin(String)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryError {
    /// The chain id is not registered.
    UnknownChain(ChainId),
    /// The op id does not exist on the chain's settlement queue.
    UnknownOp(u64),
    /// The op is not `Inflight` (nothing to resolve).
    NotInflight(String),
    /// The vault id is unknown.
    UnknownVault(u64),
    /// The vault is not on the supplied chain.
    WrongChain { vault_id: u64, chain: ChainId },
    /// The vault is not a recoverable stuck mint (wrong status / nonzero pending).
    NotRecoverable(String),
    /// A live (Queued/Inflight) mint op still exists for the vault.
    LiveMintOp(u64),
    /// An EVM-RPC quorum read failed; recovery is refused (fail-closed) because
    /// we could not confirm the op did NOT land.
    VerificationUnavailable(String),
    /// On-chain state shows the op DID land — reversing/releasing would create an
    /// unbacked mint or double-release collateral. Refused.
    OnChainLanded(String),
}

impl std::fmt::Display for RecoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self)
    }
}

// ─── M-08: resolve a stuck settlement op, on-chain-verified ───────────────────

/// Read the op, confirm it is `Inflight`, and capture its `last_tx_hash` (the
/// only datum the async re-verification needs; the per-kind reversal re-reads the
/// kind fresh from state under the commit borrow). Returns the tx hash, or
/// `Ok(None)` if the op never broadcast a tx (so it cannot have landed).
fn snapshot_inflight_op_tx(chain: ChainId, op_id: u64) -> Result<Option<String>, RecoveryError> {
    read_state(|s| {
        let q = s
            .multi_chain
            .settlement_queues
            .get(&chain)
            .ok_or(RecoveryError::UnknownChain(chain))?;
        let op = q.pending.get(&op_id).ok_or(RecoveryError::UnknownOp(op_id))?;
        if !matches!(op.status, SettlementOpStatus::Inflight { .. }) {
            return Err(RecoveryError::NotInflight(format!(
                "op {op_id} status {:?}",
                op.status
            )));
        }
        Ok(op.last_tx_hash.clone())
    })
}

/// Pure reversal applied AFTER on-chain verification clears the op as not-landed.
/// Mirrors the confirm-reverted path exactly, guarded on the op still being
/// `Inflight` (compare-and-swap) so a concurrent confirm cannot double-apply.
/// Returns `true` iff this call performed the reversal (the op was still
/// Inflight).
pub fn apply_resolve_reversal_in_state(
    state: &mut crate::chains::multi_chain_state::MultiChainStateV5,
    chain: ChainId,
    op_id: u64,
    now: u64,
) -> bool {
    let still_inflight = state
        .settlement_queues
        .get(&chain)
        .and_then(|q| q.pending.get(&op_id))
        .map(|o| matches!(o.status, SettlementOpStatus::Inflight { .. }))
        .unwrap_or(false);
    if !still_inflight {
        return false;
    }
    // Clone the kind out so we can mutate vaults and the op without overlapping
    // borrows.
    let kind = state
        .settlement_queues
        .get(&chain)
        .and_then(|q| q.pending.get(&op_id))
        .map(|o| o.kind.clone());
    if let Some(kind) = kind {
        match &kind {
            SettlementOpKind::Mint { vault_id, .. } => {
                if let Some(v) = state.chain_vaults.get_mut(vault_id) {
                    // Design B: no debt counted; clear pending, leave MintPending.
                    v.pending_mint_e8s = 0;
                }
            }
            SettlementOpKind::NativeWithdrawal { vault_id, amount_e18, .. } => {
                if let Some(v) = state.chain_vaults.get_mut(vault_id) {
                    v.collateral_amount_native =
                        v.collateral_amount_native.saturating_add(*amount_e18);
                    if v.status == ChainVaultStatus::Closing {
                        v.status = ChainVaultStatus::Open;
                    }
                }
            }
            SettlementOpKind::Burn { .. } => {}
        }
    }
    if let Some(o) = state
        .settlement_queues
        .get_mut(&chain)
        .and_then(|q| q.pending.get_mut(&op_id))
    {
        o.mark_failed("manually resolved (stuck Inflight, on-chain-verified not landed)".to_string(), now);
    }
    true
}

/// M-08 (RECOV-01): resolve a stuck `Inflight` settlement op, but ONLY after
/// re-reading its on-chain tx receipt through the EVM-RPC quorum and confirming
/// the tx did NOT land.
///
/// Policy (fail-closed):
///  - Receipt shows the tx SUCCEEDED → the op LANDED → REFUSE
///    (`OnChainLanded`). Reversing a landed Mint = unbacked mint; reversing a
///    landed NativeWithdrawal = double-credited collateral.
///  - Receipt shows the tx REVERTED → safe to reverse (the on-chain effect did
///    not happen) → apply the reversal.
///  - Receipt is PENDING/UNKNOWN (`None`) → the tx is not mined, so its effect
///    has NOT landed; reversing only marks the op Failed + clears a Mint's
///    pending (it does NOT release collateral — the vault stays MintPending, and
///    M-09's separate verified check gates the eventual collateral release).
///    Allowed, but logged.
///  - No `last_tx_hash` recorded → the op never broadcast a tx (it cannot have
///    landed) → safe to reverse.
///  - The quorum read itself ERRORS → REFUSE (`VerificationUnavailable`): we
///    must not reverse on an unverifiable state.
///
/// Returns `Ok(true)` if the reversal was applied, `Ok(false)` if the op was no
/// longer Inflight by commit time (a concurrent confirm won the CAS).
pub async fn resolve_stuck_settlement_op_verified(
    chain: ChainId,
    op_id: u64,
) -> Result<bool, RecoveryError> {
    let last_tx_hash = snapshot_inflight_op_tx(chain, op_id)?;

    // Re-verify on-chain unless there is simply no tx to check.
    if let Some(tx_hash) = &last_tx_hash {
        match crate::chains::monad::evm_rpc::get_transaction_receipt(chain, tx_hash).await {
            Ok(Some((true, block))) => {
                return Err(RecoveryError::OnChainLanded(format!(
                    "op {op_id} tx {tx_hash} succeeded on-chain at block {block}; refusing to reverse (would create an unbacked mint / double-release collateral)"
                )));
            }
            Ok(Some((false, _block))) => {
                // Reverted on-chain — the effect did not happen; safe to reverse.
            }
            Ok(None) => {
                // Pending / unknown: not mined, so not landed. Allowed (clears
                // accounting only; collateral release is gated separately by M-09).
            }
            Err(e) => {
                return Err(RecoveryError::VerificationUnavailable(format!(
                    "could not read receipt for op {op_id} tx {tx_hash} via quorum: {e}"
                )));
            }
        }
    }

    let now = ic_cdk::api::time();
    let did_reverse =
        mutate_state(|s| apply_resolve_reversal_in_state(&mut s.multi_chain, chain, op_id, now));
    Ok(did_reverse)
}

// ─── M-09: recover a stuck chain vault, on-chain-verified ─────────────────────

/// Pure pre-checks for `recover_stuck_chain_vault` (no on-chain read, operates on
/// a borrowed state so it is unit-testable). On success returns the tx hashes of
/// any TERMINAL Mint ops for this vault (so the async path can re-verify each did
/// not actually succeed on-chain). Mirrors the pre-fix endpoint's guards: vault
/// exists, is on `chain`, is `MintPending` with `pending_mint_e8s == 0`, and has
/// NO live (Queued/Inflight) Mint op.
pub fn precheck_recover_vault_in_state(
    state: &crate::chains::multi_chain_state::MultiChainStateV5,
    chain: ChainId,
    vault_id: u64,
) -> Result<Vec<String>, RecoveryError> {
    let v = state
        .chain_vaults
        .get(&vault_id)
        .ok_or(RecoveryError::UnknownVault(vault_id))?;
    if v.collateral_chain != chain {
        return Err(RecoveryError::WrongChain { vault_id, chain });
    }
    if v.status != ChainVaultStatus::MintPending || v.pending_mint_e8s != 0 {
        return Err(RecoveryError::NotRecoverable(format!(
            "vault {vault_id} status {:?}, pending_mint_e8s {}",
            v.status, v.pending_mint_e8s
        )));
    }
    let queue = state.settlement_queues.get(&chain);
    let has_live_mint = queue
        .map(|q| {
            q.pending.values().any(|op| {
                matches!(&op.kind, SettlementOpKind::Mint { vault_id: vid, .. } if *vid == vault_id)
                    && matches!(
                        op.status,
                        SettlementOpStatus::Queued | SettlementOpStatus::Inflight { .. }
                    )
            })
        })
        .unwrap_or(false);
    if has_live_mint {
        return Err(RecoveryError::LiveMintOp(vault_id));
    }
    // Collect tx hashes of TERMINAL Mint ops for this vault, so the async path can
    // re-verify none of them actually succeeded on-chain (a Failed/Succeeded op
    // may carry a `last_tx_hash` that did land).
    let tx_hashes: Vec<String> = queue
        .map(|q| {
            q.pending
                .values()
                .filter_map(|op| match &op.kind {
                    SettlementOpKind::Mint { vault_id: vid, .. } if *vid == vault_id => {
                        op.last_tx_hash.clone()
                    }
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(tx_hashes)
}

/// `read_state` wrapper over `precheck_recover_vault_in_state` for the async path.
pub fn precheck_recover_vault(
    chain: ChainId,
    vault_id: u64,
) -> Result<Vec<String>, RecoveryError> {
    read_state(|s| precheck_recover_vault_in_state(&s.multi_chain, chain, vault_id))
}

/// Apply the MintPending->Open transition AFTER on-chain verification. Re-checks
/// the guards inside the same `mutate_state` (defense against a state change
/// between the precheck snapshot and commit). Returns `Ok(())` on success.
pub fn apply_recover_vault_in_state(
    state: &mut crate::chains::multi_chain_state::MultiChainStateV5,
    chain: ChainId,
    vault_id: u64,
) -> Result<(), RecoveryError> {
    // Re-check live mint op (could have been enqueued between precheck and now).
    let has_live_mint = state
        .settlement_queues
        .get(&chain)
        .map(|q| {
            q.pending.values().any(|op| {
                matches!(&op.kind, SettlementOpKind::Mint { vault_id: vid, .. } if *vid == vault_id)
                    && matches!(
                        op.status,
                        SettlementOpStatus::Queued | SettlementOpStatus::Inflight { .. }
                    )
            })
        })
        .unwrap_or(false);
    if has_live_mint {
        return Err(RecoveryError::LiveMintOp(vault_id));
    }
    let v = state
        .chain_vaults
        .get_mut(&vault_id)
        .ok_or(RecoveryError::UnknownVault(vault_id))?;
    if v.collateral_chain != chain {
        return Err(RecoveryError::WrongChain { vault_id, chain });
    }
    if v.status != ChainVaultStatus::MintPending || v.pending_mint_e8s != 0 {
        return Err(RecoveryError::NotRecoverable(format!(
            "vault {vault_id} status {:?}, pending_mint_e8s {} (changed since precheck)",
            v.status, v.pending_mint_e8s
        )));
    }
    v.status = ChainVaultStatus::Open;
    Ok(())
}

/// M-09 (RECOV-02): recover a `MintPending` vault to `Open` (releasing its
/// collateral via the existing close/withdraw paths) ONLY after re-verifying
/// on-chain that the mint did NOT actually land.
///
/// `recover_stuck_chain_vault` previously trusted `pending_mint_e8s == 0`, a
/// value the (then-unverified) `resolve_stuck_settlement_op` could set even when
/// the mint had landed. After M-08 hardens that path, this adds an INDEPENDENT
/// on-chain re-check so a vault whose mint really landed can never be flipped to
/// Open:
///
///  1. For every TERMINAL Mint op recorded for this vault, re-read its tx
///     receipt through the quorum. Any one that SUCCEEDED → REFUSE
///     (`OnChainLanded`): the mint landed, releasing collateral would let the
///     user keep both the on-chain icUSD and the collateral.
///  2. Defensive supply re-check: read the icUSD `totalSupply()` on this chain
///     at the finalized cursor (consensus-safe) and compare to the recorded
///     `chain_supplies` plus any in-flight mints. If on-chain supply EXCEEDS
///     recorded + in-flight (the unbacked-mint signature), REFUSE — even if no
///     terminal op tx hash pointed at it, a mint may have landed.
///  3. Only when both checks pass: flip MintPending->Open.
///
/// Any quorum read error is fail-closed (`VerificationUnavailable`).
pub async fn recover_stuck_chain_vault_verified(
    chain: ChainId,
    vault_id: u64,
) -> Result<(), RecoveryError> {
    let mint_tx_hashes = precheck_recover_vault(chain, vault_id)?;

    // (1) Re-verify each terminal Mint op's tx did not succeed on-chain.
    for tx_hash in &mint_tx_hashes {
        match crate::chains::monad::evm_rpc::get_transaction_receipt(chain, tx_hash).await {
            Ok(Some((true, block))) => {
                return Err(RecoveryError::OnChainLanded(format!(
                    "vault {vault_id} mint tx {tx_hash} succeeded on-chain at block {block}; refusing to release collateral"
                )));
            }
            Ok(Some((false, _))) | Ok(None) => { /* reverted or not mined — fine */ }
            Err(e) => {
                return Err(RecoveryError::VerificationUnavailable(format!(
                    "could not read mint receipt for vault {vault_id} tx {tx_hash} via quorum: {e}"
                )));
            }
        }
    }

    // (2) Defensive on-chain supply re-check (catches a landed mint with no
    // recorded tx hash). Skipped only when there is no contract or no finalized
    // cursor yet (then there can be no confirmed on-chain mint to find).
    let (contract, cursor, recorded_supply, in_flight_mint) = read_state(|s| {
        let contract = s.multi_chain.chain_contracts.get(&chain).cloned();
        let cursor = s.multi_chain.last_observed_block.get(&chain).copied().unwrap_or(0);
        let recorded_supply = s.multi_chain.chain_supplies.get(&chain).copied().unwrap_or(0);
        let in_flight_mint: u128 = s
            .multi_chain
            .chain_vaults
            .values()
            .filter(|v| v.collateral_chain == chain)
            .map(|v| v.pending_mint_e8s)
            .sum();
        (contract, cursor, recorded_supply, in_flight_mint)
    });
    if let (Some(contract), true) = (contract, cursor > 0) {
        match crate::chains::monad::evm_rpc::erc20_total_supply_at(chain, &contract, cursor).await {
            Ok(onchain_supply) => {
                if onchain_supply > recorded_supply.saturating_add(in_flight_mint) {
                    return Err(RecoveryError::OnChainLanded(format!(
                        "vault {vault_id}: on-chain icUSD supply {} exceeds recorded {} + in-flight {} at block {}; a mint may have landed — refusing to release collateral",
                        onchain_supply, recorded_supply, in_flight_mint, cursor
                    )));
                }
            }
            Err(e) => {
                return Err(RecoveryError::VerificationUnavailable(format!(
                    "could not read on-chain totalSupply for vault {vault_id} via quorum: {e}"
                )));
            }
        }
    }

    // (3) Verified clear — flip MintPending->Open under a fresh borrow.
    mutate_state(|s| apply_recover_vault_in_state(&mut s.multi_chain, chain, vault_id))
}
