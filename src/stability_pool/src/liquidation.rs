use candid::{Nat, Principal};
use ic_canister_log::log;
use ic_cdk::call;
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc1::transfer::{Memo, TransferArg, TransferError};
use icrc_ledger_types::icrc2::approve::{ApproveArgs, ApproveError};
use num_traits::ToPrimitive;
use rumi_protocol_backend::chains::config::ChainId;
use std::collections::BTreeMap;

use crate::logs::INFO;
use crate::state::{mutate_state, read_state, StabilityPoolState};
use crate::types::*;

/// Conservative fallback for a collateral ledger's transfer fee, used only when
/// the live `icrc1_fee` query fails (SP-104). Set to the common ICRC fee
/// (10_000 e8s, as on ICP/ckBTC-class ledgers). Over-estimating the fee
/// under-credits depositors slightly (solvency-safe) rather than over-crediting
/// them as a fee=0 fallback would. The next successful liquidation reconciles.
/// Shared with `claim_collateral`'s fee lookup (ICRC-004 / SP-203).
pub(crate) const FALLBACK_COLLATERAL_FEE_E8S: u64 = 10_000;

pub(crate) const CHAIN_WRITEDOWN_MEMO_PREFIX: &[u8] = b"RUMI-LIQ-004:";

pub fn encode_chain_writedown_memo(vault_id: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(CHAIN_WRITEDOWN_MEMO_PREFIX.len() + 8);
    out.extend_from_slice(CHAIN_WRITEDOWN_MEMO_PREFIX);
    out.extend_from_slice(&vault_id.to_be_bytes());
    out
}

pub fn build_icusd_burn_transfer_arg(
    minting_account: Account,
    amount_e8s: u64,
    vault_id: u64,
    created_at_time: u64,
) -> TransferArg {
    TransferArg {
        from_subaccount: None,
        to: minting_account,
        fee: None,
        created_at_time: Some(created_at_time),
        memo: Some(Memo::from(encode_chain_writedown_memo(vault_id))),
        amount: Nat::from(amount_e8s),
    }
}

pub fn build_icusd_burn_proof(
    block_index: u64,
    vault_id: u64,
) -> rumi_protocol_backend::icrc3_proof::SpWritedownProof {
    rumi_protocol_backend::icrc3_proof::SpWritedownProof {
        block_index,
        ledger_kind: rumi_protocol_backend::icrc3_proof::SpProofLedger::IcusdBurn,
        vault_id_memo: vault_id,
    }
}

pub async fn fetch_icusd_minting_account(
    icusd_ledger: Principal,
) -> Result<Account, StabilityPoolError> {
    match call::<(), (Option<Account>,)>(icusd_ledger, "icrc1_minting_account", ()).await {
        Ok((Some(account),)) => Ok(account),
        Ok((None,)) => Err(StabilityPoolError::LedgerTransferFailed {
            reason: "icUSD ledger has no minting account; cannot burn".to_string(),
        }),
        Err(_) => Err(StabilityPoolError::InterCanisterCallFailed {
            target: format!("{}", icusd_ledger),
            method: "icrc1_minting_account".to_string(),
        }),
    }
}

pub async fn burn_icusd_for_chain_writedown_with_account(
    icusd_ledger: Principal,
    minting_account: Account,
    amount_e8s: u64,
    vault_id: u64,
    created_at_time: u64,
) -> Result<rumi_protocol_backend::icrc3_proof::SpWritedownProof, StabilityPoolError> {
    if amount_e8s == 0 {
        return Err(StabilityPoolError::AmountTooLow { minimum_e8s: 1 });
    }
    let transfer_arg =
        build_icusd_burn_transfer_arg(minting_account, amount_e8s, vault_id, created_at_time);

    let result: Result<(Result<Nat, TransferError>,), _> =
        call(icusd_ledger, "icrc1_transfer", (transfer_arg,)).await;

    let block_index = match result {
        Ok((Ok(block_index),)) => nat_block_index_to_u64(block_index)?,
        Ok((Err(TransferError::Duplicate { duplicate_of }),)) => {
            nat_block_index_to_u64(duplicate_of)?
        }
        Ok((Err(error),)) => {
            return Err(StabilityPoolError::LedgerTransferFailed {
                reason: format!("{:?}", error),
            });
        }
        Err(_) => {
            return Err(StabilityPoolError::InterCanisterCallFailed {
                target: format!("{}", icusd_ledger),
                method: "icrc1_transfer".to_string(),
            });
        }
    };

    Ok(build_icusd_burn_proof(block_index, vault_id))
}

pub async fn burn_icusd_for_chain_writedown(
    icusd_ledger: Principal,
    amount_e8s: u64,
    vault_id: u64,
) -> Result<rumi_protocol_backend::icrc3_proof::SpWritedownProof, StabilityPoolError> {
    let minting_account = fetch_icusd_minting_account(icusd_ledger).await?;
    burn_icusd_for_chain_writedown_with_account(
        icusd_ledger,
        minting_account,
        amount_e8s,
        vault_id,
        ic_cdk::api::time(),
    )
    .await
}

fn nat_block_index_to_u64(block_index: Nat) -> Result<u64, StabilityPoolError> {
    block_index
        .0
        .to_u64()
        .ok_or_else(|| StabilityPoolError::LedgerTransferFailed {
            reason: format!("ledger block index {} does not fit in u64", block_index),
        })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ChainAbsorbPlan {
    pub vault_id: u64,
    pub chain_id: ChainId,
    pub chain_sentinel: Principal,
    pub icusd_ledger: Principal,
    pub icusd_to_burn_e8s: u64,
    pub stables_consumed: BTreeMap<Principal, u64>,
}

fn chain_absorb_result_from_backend(
    plan: &ChainAbsorbPlan,
    result: ChainStabilityPoolLiquidationResult,
) -> Result<ChainSpAbsorbResult, StabilityPoolError> {
    if !result.success {
        return Err(StabilityPoolError::LiquidationFailed {
            vault_id: plan.vault_id,
            reason: "backend reported unsuccessful chain absorb".to_string(),
        });
    }
    if result.vault_id != plan.vault_id || result.chain_id != plan.chain_id {
        return Err(StabilityPoolError::LiquidationFailed {
            vault_id: plan.vault_id,
            reason: "backend chain absorb result does not match requested vault".to_string(),
        });
    }
    if result.liquidated_debt_e8s != plan.icusd_to_burn_e8s as u128 {
        return Err(StabilityPoolError::LiquidationFailed {
            vault_id: plan.vault_id,
            reason: "backend liquidated debt does not match SP burn".to_string(),
        });
    }
    if result.collateral_received_native == 0 {
        return Err(StabilityPoolError::LiquidationFailed {
            vault_id: plan.vault_id,
            reason: "backend returned zero chain collateral".to_string(),
        });
    }

    Ok(ChainSpAbsorbResult {
        success: true,
        vault_id: result.vault_id,
        chain_id: result.chain_id,
        icusd_burned_e8s: plan.icusd_to_burn_e8s,
        liquidated_debt_e8s: result.liquidated_debt_e8s,
        collateral_received_native: result.collateral_received_native,
        claim_id: result.claim_id,
        custody_address: result.custody_address,
        block_index: result.block_index,
        collateral_price_e8s: result.collateral_price_e8s,
    })
}

fn intent_matches_plan(
    intent: &ChainSpAbsorbIntent,
    plan: &ChainAbsorbPlan,
    minting_account: Account,
) -> bool {
    intent.vault_id == plan.vault_id
        && intent.chain_id == plan.chain_id
        && intent.chain_sentinel == plan.chain_sentinel
        && intent.icusd_ledger == plan.icusd_ledger
        && intent.icusd_minting_account == minting_account
        && intent.icusd_to_burn_e8s == plan.icusd_to_burn_e8s
        && intent.stables_consumed == plan.stables_consumed
}

fn chain_absorb_plan_from_intent(intent: &ChainSpAbsorbIntent) -> ChainAbsorbPlan {
    ChainAbsorbPlan {
        vault_id: intent.vault_id,
        chain_id: intent.chain_id,
        chain_sentinel: intent.chain_sentinel,
        icusd_ledger: intent.icusd_ledger,
        icusd_to_burn_e8s: intent.icusd_to_burn_e8s,
        stables_consumed: intent.stables_consumed.clone(),
    }
}

fn burned_chain_absorb_replay_plan(
    intent: &ChainSpAbsorbIntent,
) -> Option<(
    ChainAbsorbPlan,
    rumi_protocol_backend::icrc3_proof::SpWritedownProof,
)> {
    intent
        .burn_proof
        .clone()
        .map(|proof| (chain_absorb_plan_from_intent(intent), proof))
}

fn ensure_no_other_pending_chain_absorb(
    state: &StabilityPoolState,
    vault_id: u64,
) -> Result<(), StabilityPoolError> {
    if state.has_pending_chain_absorbs() && state.get_pending_chain_absorb(vault_id).is_none() {
        return Err(StabilityPoolError::SystemBusy);
    }
    Ok(())
}

pub(crate) fn prepare_or_reuse_chain_absorb_intent_in_state(
    state: &mut StabilityPoolState,
    plan: &ChainAbsorbPlan,
    minting_account: Account,
    now_ns: u64,
) -> Result<ChainSpAbsorbIntent, StabilityPoolError> {
    if let Some(completion) = state.completed_chain_absorb(plan.vault_id) {
        return Err(StabilityPoolError::LiquidationFailed {
            vault_id: plan.vault_id,
            reason: format!(
                "chain absorb already completed at block {}",
                completion.result.block_index
            ),
        });
    }

    if let Some(existing) = state.get_pending_chain_absorb(plan.vault_id) {
        if intent_matches_plan(&existing, plan, minting_account) {
            return Ok(existing);
        }
        return Err(StabilityPoolError::LiquidationFailed {
            vault_id: plan.vault_id,
            reason: "pending chain absorb intent conflicts with current preflight".to_string(),
        });
    }

    let intent = ChainSpAbsorbIntent {
        vault_id: plan.vault_id,
        chain_id: plan.chain_id,
        chain_sentinel: plan.chain_sentinel,
        icusd_ledger: plan.icusd_ledger,
        icusd_minting_account: minting_account,
        icusd_to_burn_e8s: plan.icusd_to_burn_e8s,
        stables_consumed: plan.stables_consumed.clone(),
        burn_created_at_time_ns: now_ns,
        status: ChainSpAbsorbIntentStatus::Prepared,
        burn_proof: None,
        backend_result: None,
        last_error: None,
        created_at_ns: now_ns,
        updated_at_ns: now_ns,
    };
    state.put_pending_chain_absorb(intent.clone())?;
    Ok(intent)
}

pub(crate) fn mark_chain_absorb_burned_in_state(
    state: &mut StabilityPoolState,
    vault_id: u64,
    proof: rumi_protocol_backend::icrc3_proof::SpWritedownProof,
    now_ns: u64,
) -> Result<ChainSpAbsorbIntent, StabilityPoolError> {
    let mut intent = state.get_pending_chain_absorb(vault_id).ok_or_else(|| {
        StabilityPoolError::LiquidationFailed {
            vault_id,
            reason: "missing pending chain absorb intent".to_string(),
        }
    })?;
    if let Some(existing) = &intent.burn_proof {
        if existing != &proof {
            return Err(StabilityPoolError::LiquidationFailed {
                vault_id,
                reason: "pending chain absorb burn proof conflicts with retry proof".to_string(),
            });
        }
    }
    intent.burn_proof = Some(proof);
    intent.status = ChainSpAbsorbIntentStatus::Burned;
    intent.last_error = None;
    intent.updated_at_ns = now_ns;
    state.put_pending_chain_absorb(intent.clone())?;
    Ok(intent)
}

pub(crate) fn mark_chain_absorb_backend_result_in_state(
    state: &mut StabilityPoolState,
    vault_id: u64,
    result: ChainStabilityPoolLiquidationResult,
    now_ns: u64,
) -> Result<ChainSpAbsorbIntent, StabilityPoolError> {
    let mut intent = state.get_pending_chain_absorb(vault_id).ok_or_else(|| {
        StabilityPoolError::LiquidationFailed {
            vault_id,
            reason: "missing pending chain absorb intent".to_string(),
        }
    })?;
    if let Some(existing) = &intent.backend_result {
        if existing != &result {
            return Err(StabilityPoolError::LiquidationFailed {
                vault_id,
                reason: "pending chain absorb backend result conflicts with retry result"
                    .to_string(),
            });
        }
    }
    intent.backend_result = Some(result);
    intent.status = ChainSpAbsorbIntentStatus::BackendAccepted;
    intent.last_error = None;
    intent.updated_at_ns = now_ns;
    state.put_pending_chain_absorb(intent.clone())?;
    Ok(intent)
}

pub(crate) fn mark_chain_absorb_error_in_state(
    state: &mut StabilityPoolState,
    vault_id: u64,
    status: ChainSpAbsorbIntentStatus,
    reason: String,
    now_ns: u64,
) {
    if let Some(mut intent) = state.get_pending_chain_absorb(vault_id) {
        intent.status = status;
        intent.last_error = Some(reason);
        intent.updated_at_ns = now_ns;
        let _ = state.put_pending_chain_absorb(intent);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CfxClaimPayoutPlan {
    pub claimant: Principal,
    pub chain_sentinel: Principal,
    pub claim_id: u64,
    pub amount_wei: u128,
    pub dest_evm: String,
}

fn chain_id_from_sentinel(sentinel: &Principal) -> Option<ChainId> {
    let bytes = sentinel.as_slice();
    let prefix = b"rumi-chain-collateral";
    if bytes.len() != 29 || !bytes.starts_with(prefix) || bytes[28] != 0x7f {
        return None;
    }
    if bytes[prefix.len()..24].iter().any(|b| *b != 0) {
        return None;
    }
    let mut chain_bytes = [0u8; 4];
    chain_bytes.copy_from_slice(&bytes[24..28]);
    Some(ChainId(u32::from_le_bytes(chain_bytes)))
}

pub(crate) fn registered_chain_ids_from_sentinels(state: &StabilityPoolState) -> Vec<ChainId> {
    let mut chains: Vec<ChainId> = state
        .chain_collateral_sentinels
        .as_ref()
        .into_iter()
        .flat_map(|sentinels| sentinels.iter())
        .filter_map(chain_id_from_sentinel)
        .collect();
    chains.sort();
    chains.dedup();
    chains
}

pub(crate) fn prepare_chain_absorb_plan_in_state(
    state: &StabilityPoolState,
    vault: &ChainLiquidatableVaultInfo,
) -> Result<ChainAbsorbPlan, StabilityPoolError> {
    if !vault.sp_attempted {
        return Err(StabilityPoolError::LiquidationFailed {
            vault_id: vault.vault_id,
            reason: "chain vault has not been escalated to the stability pool".to_string(),
        });
    }
    if !state.is_chain_collateral_sentinel(&vault.chain_collateral_sentinel) {
        return Err(StabilityPoolError::CollateralNotFound {
            ledger: vault.chain_collateral_sentinel,
        });
    }
    if chain_id_from_sentinel(&vault.chain_collateral_sentinel) != Some(vault.chain_id) {
        return Err(StabilityPoolError::LiquidationFailed {
            vault_id: vault.vault_id,
            reason: "chain collateral sentinel does not match chain id".to_string(),
        });
    }
    let debt_e8s =
        u64::try_from(vault.debt_e8s).map_err(|_| StabilityPoolError::LiquidationFailed {
            vault_id: vault.vault_id,
            reason: format!(
                "chain vault debt {} exceeds SP u64 burn amount",
                vault.debt_e8s
            ),
        })?;
    if debt_e8s == 0 {
        return Err(StabilityPoolError::LiquidationFailed {
            vault_id: vault.vault_id,
            reason: "chain vault has no debt to absorb".to_string(),
        });
    }

    let available_icusd =
        state.effective_icusd_pool_for_collateral(&vault.chain_collateral_sentinel);
    if available_icusd < debt_e8s {
        return Err(StabilityPoolError::InsufficientPoolBalance);
    }
    let stables_consumed =
        state.compute_icusd_chain_draw(debt_e8s, &vault.chain_collateral_sentinel);
    let icusd_ledger = state
        .icusd_ledger()
        .ok_or(StabilityPoolError::TokenNotAccepted {
            ledger: Principal::anonymous(),
        })?;
    if stables_consumed.get(&icusd_ledger).copied().unwrap_or(0) != debt_e8s {
        return Err(StabilityPoolError::InsufficientPoolBalance);
    }

    Ok(ChainAbsorbPlan {
        vault_id: vault.vault_id,
        chain_id: vault.chain_id,
        chain_sentinel: vault.chain_collateral_sentinel,
        icusd_ledger,
        icusd_to_burn_e8s: debt_e8s,
        stables_consumed,
    })
}

pub(crate) fn apply_chain_absorb_success_in_state(
    state: &mut StabilityPoolState,
    plan: &ChainAbsorbPlan,
    result: ChainStabilityPoolLiquidationResult,
) -> Result<ChainSpAbsorbResult, StabilityPoolError> {
    apply_chain_absorb_success_in_state_at(state, plan, result, ic_cdk::api::time())
}

pub(crate) fn apply_chain_absorb_success_in_state_at(
    state: &mut StabilityPoolState,
    plan: &ChainAbsorbPlan,
    result: ChainStabilityPoolLiquidationResult,
    timestamp: u64,
) -> Result<ChainSpAbsorbResult, StabilityPoolError> {
    let absorbed = chain_absorb_result_from_backend(plan, result)?;
    if let Some(completion) = state.completed_chain_absorb(plan.vault_id) {
        if completion.result == absorbed {
            return Ok(completion.result);
        }
        return Err(StabilityPoolError::LiquidationFailed {
            vault_id: plan.vault_id,
            reason: "completed chain absorb conflicts with backend result".to_string(),
        });
    }

    state.record_chain_claim_source(
        plan.chain_sentinel,
        absorbed.claim_id,
        absorbed.collateral_received_native,
    );
    state.process_chain_liquidation_gains_at(
        plan.vault_id,
        plan.chain_sentinel,
        &plan.stables_consumed,
        absorbed.collateral_received_native,
        absorbed.collateral_price_e8s,
        timestamp,
    );
    state.take_pending_chain_absorb(plan.vault_id);
    state.record_completed_chain_absorb(ChainSpAbsorbCompletion {
        vault_id: plan.vault_id,
        result: absorbed.clone(),
        completed_at_ns: timestamp,
    });

    Ok(absorbed)
}

pub(crate) fn prepare_cfx_claim_payout_in_state(
    state: &mut StabilityPoolState,
    claimant: Principal,
    chain_sentinel: Principal,
    dest_evm: String,
    address_validator: impl Fn(&str) -> bool,
) -> Result<Option<CfxClaimPayoutPlan>, StabilityPoolError> {
    if !address_validator(&dest_evm) {
        return Err(StabilityPoolError::LiquidationFailed {
            vault_id: 0,
            reason: "invalid EVM address".to_string(),
        });
    }
    if !state.is_chain_collateral_sentinel(&chain_sentinel) {
        return Err(StabilityPoolError::CollateralNotFound {
            ledger: chain_sentinel,
        });
    }
    let owed = state
        .deposits
        .get(&claimant)
        .and_then(|pos| pos.cfx_claims.as_ref())
        .and_then(|claims| claims.get(&chain_sentinel).copied())
        .unwrap_or(0);
    if owed == 0 {
        return Ok(None);
    }

    let (claim_id, amount_wei) = {
        let sources = state
            .chain_claim_sources
            .as_mut()
            .and_then(|m| m.get_mut(&chain_sentinel))
            .ok_or_else(|| StabilityPoolError::LiquidationFailed {
                vault_id: 0,
                reason: "no backend chain claim source available".to_string(),
            })?;
        let source_index = sources
            .iter()
            .position(|source| source.remaining_native > 0)
            .ok_or_else(|| StabilityPoolError::LiquidationFailed {
                vault_id: 0,
                reason: "no funded backend chain claim source available".to_string(),
            })?;
        let source = &mut sources[source_index];
        let amount_wei = owed.min(source.remaining_native);
        let claim_id = source.claim_id;
        source.remaining_native = source.remaining_native.saturating_sub(amount_wei);
        if source.remaining_native == 0 {
            sources.remove(source_index);
        }
        (claim_id, amount_wei)
    };

    if state
        .chain_claim_sources
        .as_ref()
        .and_then(|m| m.get(&chain_sentinel))
        .map(|sources| sources.is_empty())
        .unwrap_or(false)
    {
        if let Some(sources) = state.chain_claim_sources.as_mut() {
            sources.remove(&chain_sentinel);
        }
    }

    state.mark_cfx_claimed(&claimant, &chain_sentinel, amount_wei);

    Ok(Some(CfxClaimPayoutPlan {
        claimant,
        chain_sentinel,
        claim_id,
        amount_wei,
        dest_evm,
    }))
}

pub(crate) fn rollback_cfx_claim_payout_in_state(
    state: &mut StabilityPoolState,
    plan: &CfxClaimPayoutPlan,
) {
    state.record_chain_claim_source(plan.chain_sentinel, plan.claim_id, plan.amount_wei);
    let position = state
        .deposits
        .entry(plan.claimant)
        .or_insert_with(|| DepositPosition::new(0));
    let claims = position.cfx_claims.get_or_insert_with(BTreeMap::new);
    let entry = claims.entry(plan.chain_sentinel).or_insert(0);
    *entry = entry.saturating_add(plan.amount_wei);
}

pub(crate) fn is_duplicate_chain_claim_error(error: &rumi_protocol_backend::ProtocolError) -> bool {
    match error {
        rumi_protocol_backend::ProtocolError::ChainAdmin(msg)
        | rumi_protocol_backend::ProtocolError::GenericError(msg) => {
            msg.contains("Duplicate chain collateral claim payout idempotency key")
        }
        _ => false,
    }
}

/// Called by the backend when it detects liquidatable vaults (push model).
/// Processes each vault sequentially, consuming stablecoins and distributing collateral.
pub async fn notify_liquidatable_vaults(
    vaults: Vec<LiquidatableVaultInfo>,
) -> Vec<LiquidationResult> {
    if read_state(|s| s.configuration.emergency_pause) {
        log!(
            INFO,
            "Pool is paused — ignoring {} liquidatable vaults",
            vaults.len()
        );
        return vec![];
    }

    // SP-102: hold the per-pool liquidation lock across the whole batch so
    // deposit/withdraw/claim cannot land between a vault's snapshot and its
    // burn apportionment (which would let a withdrawer escape their share).
    // If another liquidation is already running, skip this batch (no retry —
    // the backend re-notifies on its next tick).
    let _liq_guard = match crate::pool_guard::SpLiquidationGuard::new() {
        Ok(g) => g,
        Err(_) => {
            log!(INFO, "notify_liquidatable_vaults: a liquidation is already in flight; skipping this batch");
            return vec![];
        }
    };

    log!(
        INFO,
        "Received push notification: {} liquidatable vaults",
        vaults.len()
    );

    let max_batch = read_state(|s| s.configuration.max_liquidations_per_batch) as usize;

    let mut results = Vec::new();
    for vault_info in vaults.into_iter().take(max_batch) {
        // Skip if already in-flight
        if read_state(|s| s.in_flight_liquidations.contains(&vault_info.vault_id)) {
            log!(
                INFO,
                "Vault {} already in-flight, skipping",
                vault_info.vault_id
            );
            continue;
        }

        // Check effective pool coverage for this collateral type
        let effective_pool =
            read_state(|s| s.effective_pool_for_collateral(&vault_info.collateral_type));
        if effective_pool < vault_info.debt_amount {
            log!(
                INFO,
                "Insufficient pool coverage for vault {}: need {} e8s, have {} e8s",
                vault_info.vault_id,
                vault_info.debt_amount,
                effective_pool
            );
            continue;
        }

        // Mark as in-flight
        mutate_state(|s| {
            s.in_flight_liquidations.insert(vault_info.vault_id);
        });

        let result = execute_single_liquidation(&vault_info).await;

        // Clear in-flight
        mutate_state(|s| {
            s.in_flight_liquidations.remove(&vault_info.vault_id);
        });

        if result.success {
            log!(
                INFO,
                "Liquidated vault {}: gained {} collateral",
                vault_info.vault_id,
                result.collateral_gained
            );
        } else {
            log!(
                INFO,
                "Liquidation failed for vault {}: {}",
                vault_info.vault_id,
                result.error_message.as_deref().unwrap_or("unknown")
            );
        }

        results.push(result);
    }

    results
}

/// Public fallback: anyone (except the anonymous principal) can call this to
/// trigger a liquidation for a specific vault.
///
/// SP-111 (audit 2026-06-05): the previous comment claimed a per-caller guard
/// was enforced at the lib.rs level — there was none. Concurrency is now
/// serialized by the per-pool `SpLiquidationGuard` acquired below (SP-102), and
/// the anonymous principal is rejected here to keep the permissionless trigger
/// from being driven by unauthenticated cycle-griefing callers.
pub async fn execute_liquidation(vault_id: u64) -> Result<LiquidationResult, StabilityPoolError> {
    if ic_cdk::api::caller() == Principal::anonymous() {
        return Err(StabilityPoolError::Unauthorized);
    }

    if read_state(|s| s.configuration.emergency_pause) {
        return Err(StabilityPoolError::EmergencyPaused);
    }

    if read_state(|s| s.in_flight_liquidations.contains(&vault_id)) {
        return Err(StabilityPoolError::SystemBusy);
    }

    // SP-102: hold the per-pool liquidation lock across snapshot -> await ->
    // apportion so deposit/withdraw/claim cannot race the apportionment.
    let _liq_guard = crate::pool_guard::SpLiquidationGuard::new()?;

    // Fetch vault info from backend
    let protocol_id = read_state(|s| s.protocol_canister_id);

    let (vaults,): (Vec<rumi_protocol_backend::vault::CandidVault>,) =
        call(protocol_id, "get_liquidatable_vaults", ())
            .await
            .map_err(|_e| StabilityPoolError::InterCanisterCallFailed {
                target: "Protocol".to_string(),
                method: "get_liquidatable_vaults".to_string(),
            })?;
    let target_vault = vaults.into_iter().find(|v| v.vault_id == vault_id);

    let vault = match target_vault {
        Some(v) => v,
        None => {
            return Err(StabilityPoolError::LiquidationFailed {
                vault_id,
                reason: "Vault not found in liquidatable list".to_string(),
            })
        }
    };

    let vault_info = LiquidatableVaultInfo {
        vault_id: vault.vault_id,
        collateral_type: vault.collateral_type,
        debt_amount: vault.borrowed_icusd_amount,
        collateral_amount: vault.icp_margin_amount,
        recommended_liquidation_amount: 0,
        collateral_price_e8s: 0,
    };

    // Check pool coverage
    let effective_pool =
        read_state(|s| s.effective_pool_for_collateral(&vault_info.collateral_type));
    if effective_pool < vault_info.debt_amount {
        return Err(StabilityPoolError::InsufficientPoolBalance);
    }

    mutate_state(|s| {
        s.in_flight_liquidations.insert(vault_id);
    });
    let result = execute_single_liquidation(&vault_info).await;
    mutate_state(|s| {
        s.in_flight_liquidations.remove(&vault_id);
    });

    Ok(result)
}

pub async fn scan_chain_absorb_candidates(
    max_per_chain: Option<u64>,
) -> Result<Vec<ChainSpAbsorbCandidate>, StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    if caller == Principal::anonymous() {
        return Err(StabilityPoolError::Unauthorized);
    }
    if !read_state(|s| s.is_admin(&caller)) {
        return Err(StabilityPoolError::Unauthorized);
    }

    let (protocol_id, chains) = read_state(|s| {
        (
            s.protocol_canister_id,
            registered_chain_ids_from_sentinels(s),
        )
    });
    let per_chain = max_per_chain.unwrap_or(100).min(500) as usize;
    let mut candidates = Vec::new();
    for chain in chains {
        let call_result: Result<(Vec<ChainLiquidatableVaultInfo>,), _> =
            call(protocol_id, "get_chain_liquidatable_vaults", (chain,)).await;
        let (vaults,) = call_result.map_err(|_| StabilityPoolError::InterCanisterCallFailed {
            target: format!("{}", protocol_id),
            method: "get_chain_liquidatable_vaults".to_string(),
        })?;

        for vault in vaults
            .into_iter()
            .filter(|v| v.sp_attempted)
            .take(per_chain)
        {
            if let Ok(plan) = read_state(|s| prepare_chain_absorb_plan_in_state(s, &vault)) {
                let pending_status = read_state(|s| s.pending_chain_absorb_status(vault.vault_id));
                candidates.push(ChainSpAbsorbCandidate {
                    vault,
                    icusd_to_burn_e8s: plan.icusd_to_burn_e8s,
                    pending_status,
                });
            }
        }
    }

    Ok(candidates)
}

async fn submit_chain_absorb_to_backend(
    protocol_id: Principal,
    vault_id: u64,
    plan: &ChainAbsorbPlan,
    proof: rumi_protocol_backend::icrc3_proof::SpWritedownProof,
) -> Result<ChainStabilityPoolLiquidationResult, StabilityPoolError> {
    let backend_result: Result<
        (Result<ChainStabilityPoolLiquidationResult, rumi_protocol_backend::ProtocolError>,),
        _,
    > = call(
        protocol_id,
        "stability_pool_liquidate_chain_vault",
        (vault_id, plan.icusd_to_burn_e8s, proof),
    )
    .await;

    match backend_result {
        Ok((Ok(result),)) => {
            mutate_state(|s| {
                mark_chain_absorb_backend_result_in_state(
                    s,
                    vault_id,
                    result.clone(),
                    ic_cdk::api::time(),
                )
            })?;
            Ok(result)
        }
        Ok((Err(error),)) => {
            let reason = format!("backend rejected chain absorb after burn: {:?}", error);
            mutate_state(|s| {
                mark_chain_absorb_error_in_state(
                    s,
                    vault_id,
                    ChainSpAbsorbIntentStatus::BackendRejected,
                    reason.clone(),
                    ic_cdk::api::time(),
                );
            });
            Err(StabilityPoolError::LiquidationFailed { vault_id, reason })
        }
        Err(_) => {
            mutate_state(|s| {
                mark_chain_absorb_error_in_state(
                    s,
                    vault_id,
                    ChainSpAbsorbIntentStatus::Burned,
                    "backend call failed after icUSD burn".to_string(),
                    ic_cdk::api::time(),
                );
            });
            Err(StabilityPoolError::InterCanisterCallFailed {
                target: format!("{}", protocol_id),
                method: "stability_pool_liquidate_chain_vault".to_string(),
            })
        }
    }
}

pub async fn sp_absorb_chain_vault(
    vault_id: u64,
) -> Result<ChainSpAbsorbResult, StabilityPoolError> {
    let caller = ic_cdk::api::caller();
    if caller == Principal::anonymous() {
        return Err(StabilityPoolError::Unauthorized);
    }
    if !read_state(|s| s.is_admin(&caller)) {
        return Err(StabilityPoolError::Unauthorized);
    }
    if read_state(|s| s.configuration.emergency_pause) {
        return Err(StabilityPoolError::EmergencyPaused);
    }
    if let Some(completion) = read_state(|s| s.completed_chain_absorb(vault_id)) {
        return Ok(completion.result);
    }

    crate::ensure_no_pool_balance_async_in_flight()?;
    let _liq_guard = crate::pool_guard::SpLiquidationGuard::new()?;
    read_state(|s| ensure_no_other_pending_chain_absorb(s, vault_id))?;
    if let Some(intent) = read_state(|s| s.get_pending_chain_absorb(vault_id)) {
        let plan = chain_absorb_plan_from_intent(&intent);
        if let Some(result) = intent.backend_result.clone() {
            return mutate_state(|s| apply_chain_absorb_success_in_state(s, &plan, result));
        }
        if let Some((plan, proof)) = burned_chain_absorb_replay_plan(&intent) {
            let protocol_id = read_state(|s| s.protocol_canister_id);
            let result =
                submit_chain_absorb_to_backend(protocol_id, vault_id, &plan, proof).await?;
            return mutate_state(|s| apply_chain_absorb_success_in_state(s, &plan, result));
        }
    }

    let (protocol_id, chains) = read_state(|s| {
        (
            s.protocol_canister_id,
            registered_chain_ids_from_sentinels(s),
        )
    });
    if chains.is_empty() {
        return Err(StabilityPoolError::LiquidationFailed {
            vault_id,
            reason: "no registered chain collateral sentinels".to_string(),
        });
    }

    let mut candidate: Option<ChainLiquidatableVaultInfo> = None;
    for chain in chains {
        let call_result: Result<(Vec<ChainLiquidatableVaultInfo>,), _> =
            call(protocol_id, "get_chain_liquidatable_vaults", (chain,)).await;
        let (vaults,) = call_result.map_err(|_| StabilityPoolError::InterCanisterCallFailed {
            target: format!("{}", protocol_id),
            method: "get_chain_liquidatable_vaults".to_string(),
        })?;
        if let Some(vault) = vaults.into_iter().find(|v| v.vault_id == vault_id) {
            candidate = Some(vault);
            break;
        }
    }

    let candidate = candidate.ok_or_else(|| StabilityPoolError::LiquidationFailed {
        vault_id,
        reason: "chain vault not found in liquidatable discovery".to_string(),
    })?;
    let plan = read_state(|s| prepare_chain_absorb_plan_in_state(s, &candidate))?;

    let minting_account = match read_state(|s| s.get_pending_chain_absorb(vault_id)) {
        Some(intent) => intent.icusd_minting_account,
        None => fetch_icusd_minting_account(plan.icusd_ledger).await?,
    };
    let now = ic_cdk::api::time();
    let mut intent = mutate_state(|s| {
        prepare_or_reuse_chain_absorb_intent_in_state(s, &plan, minting_account, now)
    })?;

    let proof = if let Some(proof) = intent.burn_proof.clone() {
        proof
    } else {
        match burn_icusd_for_chain_writedown_with_account(
            intent.icusd_ledger,
            intent.icusd_minting_account,
            intent.icusd_to_burn_e8s,
            intent.vault_id,
            intent.burn_created_at_time_ns,
        )
        .await
        {
            Ok(proof) => {
                intent = mutate_state(|s| {
                    mark_chain_absorb_burned_in_state(
                        s,
                        vault_id,
                        proof.clone(),
                        ic_cdk::api::time(),
                    )
                })?;
                proof
            }
            Err(error) => {
                mutate_state(|s| {
                    mark_chain_absorb_error_in_state(
                        s,
                        vault_id,
                        ChainSpAbsorbIntentStatus::Prepared,
                        format!("icUSD burn failed: {:?}", error),
                        ic_cdk::api::time(),
                    );
                });
                return Err(error);
            }
        }
    };

    let result = if let Some(result) = intent.backend_result.clone() {
        result
    } else {
        submit_chain_absorb_to_backend(protocol_id, vault_id, &plan, proof).await?
    };

    mutate_state(|s| apply_chain_absorb_success_in_state(s, &plan, result))
}

pub async fn claim_cfx(
    chain_sentinel: Principal,
    dest_evm: String,
) -> Result<u128, StabilityPoolError> {
    if crate::pool_balance_mutation_blocked() {
        return Err(StabilityPoolError::SystemBusy);
    }
    let caller = ic_cdk::api::caller();
    if caller == Principal::anonymous() {
        return Err(StabilityPoolError::Unauthorized);
    }
    if read_state(|s| s.configuration.emergency_pause) {
        return Err(StabilityPoolError::EmergencyPaused);
    }

    let plan = mutate_state(|s| {
        prepare_cfx_claim_payout_in_state(
            s,
            caller,
            chain_sentinel,
            dest_evm,
            rumi_protocol_backend::chains::evm::tecdsa::is_valid_evm_address,
        )
    })?;
    let Some(plan) = plan else {
        return Ok(0);
    };

    let protocol_id = read_state(|s| s.protocol_canister_id);
    let backend_result: Result<(Result<u64, rumi_protocol_backend::ProtocolError>,), _> = call(
        protocol_id,
        "claim_chain_collateral",
        (
            plan.claim_id,
            plan.claimant,
            plan.amount_wei,
            plan.dest_evm.clone(),
        ),
    )
    .await;

    match backend_result {
        Ok((Ok(_op_id),)) => Ok(plan.amount_wei),
        Ok((Err(error),)) if is_duplicate_chain_claim_error(&error) => Ok(plan.amount_wei),
        Ok((Err(error),)) => {
            mutate_state(|s| rollback_cfx_claim_payout_in_state(s, &plan));
            Err(StabilityPoolError::LiquidationFailed {
                vault_id: plan.claim_id,
                reason: format!("backend rejected CFX claim: {:?}", error),
            })
        }
        Err(_) => {
            mutate_state(|s| rollback_cfx_claim_payout_in_state(s, &plan));
            Err(StabilityPoolError::InterCanisterCallFailed {
                target: format!("{}", protocol_id),
                method: "claim_chain_collateral".to_string(),
            })
        }
    }
}

/// Core liquidation logic for a single vault.
///
/// Strategy:
/// 1. Non-LP stablecoins (icUSD, ckUSDC, ckUSDT): approve backend → call liquidate_vault_partial
/// 2. LP tokens (3USD): burn on 3pool via authorized_redeem_and_burn → call backend
///    stability_pool_liquidate_debt_burned to write down debt and release collateral
///
/// No circuit breaker / suspension mechanism — if a token fails, we skip it and try the
/// next one. If they all fail, the liquidation simply doesn't happen this round.
async fn execute_single_liquidation(vault_info: &LiquidatableVaultInfo) -> LiquidationResult {
    let protocol_id = read_state(|s| s.protocol_canister_id);

    // Step 1: Compute token draw
    // Use recommended_liquidation_amount (partial cap) if available, otherwise full debt
    let draw_amount = if vault_info.recommended_liquidation_amount > 0 {
        vault_info.recommended_liquidation_amount
    } else {
        vault_info.debt_amount
    };
    let token_draw = read_state(|s| s.compute_token_draw(draw_amount, &vault_info.collateral_type));

    if token_draw.is_empty() {
        return LiquidationResult {
            vault_id: vault_info.vault_id,
            stables_consumed: BTreeMap::new(),
            collateral_gained: 0,
            collateral_type: vault_info.collateral_type,
            success: false,
            error_message: Some("No stablecoins available for liquidation".to_string()),
        };
    }

    log!(
        INFO,
        "Token draw for vault {}: {:?}",
        vault_info.vault_id,
        token_draw
    );

    // Step 2: Process each token in the draw
    let mut total_collateral_gained: u64 = 0;
    let mut actual_consumed: BTreeMap<Principal, u64> = BTreeMap::new();

    let stablecoin_configs: BTreeMap<Principal, StablecoinConfig> =
        read_state(|s| s.stablecoin_registry.clone());
    let icusd_ledger = stablecoin_configs
        .iter()
        .find(|(_, c)| c.symbol == "icUSD")
        .map(|(id, _)| *id);

    // --- Non-LP tokens: approve + liquidate_vault_partial ---
    for (token_ledger, amount) in &token_draw {
        // Skip LP tokens — handled separately below
        if stablecoin_configs
            .get(token_ledger)
            .map(|c| c.is_lp_token.unwrap_or(false))
            .unwrap_or(false)
        {
            continue;
        }

        let is_icusd = icusd_ledger.map(|id| id == *token_ledger).unwrap_or(false);
        let token_decimals = stablecoin_configs
            .get(token_ledger)
            .map(|c| c.decimals)
            .unwrap_or(8);

        // Pre-check: backend minimum is 10_000_000 e8s (0.1 icUSD)
        let amount_e8s_check = if is_icusd {
            *amount
        } else {
            crate::types::normalize_to_e8s(*amount, token_decimals)
        };
        if amount_e8s_check < 10_000_000 {
            log!(
                INFO,
                "Skipping token {}: amount {} e8s below backend minimum (0.1)",
                token_ledger,
                amount_e8s_check
            );
            continue;
        }

        // Approve backend to spend this token
        let approve_args = ApproveArgs {
            from_subaccount: None,
            spender: Account {
                owner: protocol_id,
                subaccount: None,
            },
            amount: candid::Nat::from(*amount as u128 * 2), // 2x buffer for fees
            expected_allowance: None,
            expires_at: Some(ic_cdk::api::time() + 300_000_000_000), // 5 min
            fee: None,
            memo: None,
            created_at_time: Some(ic_cdk::api::time()),
        };

        let approve_result: Result<(Result<candid::Nat, ApproveError>,), _> =
            call(*token_ledger, "icrc2_approve", (approve_args,)).await;

        match approve_result {
            Ok((Ok(_),)) => {
                // Deduct the approve fee from tracked balances
                if let Some(fee) = stablecoin_configs
                    .get(token_ledger)
                    .and_then(|c| c.transfer_fee)
                {
                    if fee > 0 {
                        mutate_state(|s| s.deduct_fee_from_pool(*token_ledger, fee));
                    }
                }
            }
            Ok((Err(e),)) => {
                log!(INFO, "Approve failed for {}: {:?}", token_ledger, e);
                continue;
            }
            Err(e) => {
                log!(INFO, "Approve call failed for {}: {:?}", token_ledger, e);
                continue;
            }
        }

        // No pre-deduct of depositor balances: `process_liquidation_gains` is the
        // single point of truth for stablecoin bookkeeping on a successful
        // liquidation (SP-001 regression fix, audit 2026-04-22-28e9896). Calling
        // `deduct_burned_lp_from_balances` here previously caused depositor balances
        // and the aggregate total to be decremented twice per liquidation — once
        // pre-call, once inside `process_liquidation_gains_at` — leaving phantom
        // tokens in the pool account per liquidation.

        // Call the appropriate backend endpoint
        let liq_result = if is_icusd {
            let call_result: Result<
                (
                    Result<
                        rumi_protocol_backend::SuccessWithFee,
                        rumi_protocol_backend::ProtocolError,
                    >,
                ),
                _,
            > = call(
                protocol_id,
                "liquidate_vault_partial",
                (rumi_protocol_backend::vault::VaultArg {
                    vault_id: vault_info.vault_id,
                    amount: *amount,
                },),
            )
            .await;
            call_result.map(|(r,)| r)
        } else {
            let token_type = determine_stable_token_type(*token_ledger, &stablecoin_configs);
            match token_type {
                Some(tt) => {
                    let amount_e8s = crate::types::normalize_to_e8s(*amount, token_decimals);
                    let call_result: Result<
                        (
                            Result<
                                rumi_protocol_backend::SuccessWithFee,
                                rumi_protocol_backend::ProtocolError,
                            >,
                        ),
                        _,
                    > = call(
                        protocol_id,
                        "liquidate_vault_partial_with_stable",
                        (rumi_protocol_backend::VaultArgWithToken {
                            vault_id: vault_info.vault_id,
                            amount: amount_e8s,
                            token_type: tt,
                        },),
                    )
                    .await;
                    call_result.map(|(r,)| r)
                }
                None => {
                    // Backend was never called; no bookkeeping to roll back.
                    log!(
                        INFO,
                        "Unknown stable token type for {}, skipping",
                        token_ledger
                    );
                    continue;
                }
            }
        };

        match liq_result {
            Ok(Ok(success)) => {
                let collateral = success
                    .collateral_amount_received
                    .unwrap_or(success.fee_amount_paid);
                log!(
                    INFO,
                    "Liquidation succeeded for vault {} with token {}: collateral={}, fee={}",
                    vault_info.vault_id,
                    token_ledger,
                    collateral,
                    success.fee_amount_paid
                );
                // SP-101 / SP-110: debit by what the backend ACTUALLY pulled from
                // the pool, not the amount we requested, so the tracked aggregate
                // never drifts from the real ledger balance. `process_liquidation_gains`
                // debits depositor balances exactly once, after this loop.
                //   - icUSD path: the backend pulled exactly the realized debt
                //     (`debt_liquidated_e8s`), no surcharge.
                //   - ckStable path: the backend pulled `base + repay-fee surcharge`
                //     (`stable_pulled_e6s`). Using only the base-debt conversion
                //     left the surcharge un-debited and the aggregate above the
                //     ledger (SP-110). Prefer the exact `stable_pulled_e6s`; fall
                //     back to the base conversion for an older backend wasm.
                let realized_consumed = match (success.debt_liquidated_e8s, is_icusd) {
                    (Some(debt_e8), true) => debt_e8,
                    (Some(debt_e8), false) => success.stable_pulled_e6s.unwrap_or_else(|| {
                        crate::types::denormalize_from_e8s(debt_e8, token_decimals)
                    }),
                    (None, _) => *amount,
                };
                actual_consumed.insert(*token_ledger, realized_consumed);
                total_collateral_gained += collateral;
                // Bug 7: one token per vault per round — vault state changed, remaining draws are stale
                break;
            }
            Ok(Err(protocol_error)) => {
                // Backend explicitly rejected; nothing was pre-deducted, so no rollback needed.
                log!(
                    INFO,
                    "Protocol rejected liquidation for vault {} with token {}: {:?}",
                    vault_info.vault_id,
                    token_ledger,
                    protocol_error
                );
            }
            Err(call_error) => {
                // Inter-canister call failed; outcome is unknown. We do NOT mutate
                // depositor bookkeeping here — the previous "conservative deduct" path
                // (SP-005) caused permanent depositor loss when the backend was in
                // fact a no-op. If the backend rolled forward (took the tokens via
                // transfer_from but failed to reply), the next liquidation or a manual
                // `correct_balance` reconciliation against `icrc1_balance_of(pool)`
                // will reconcile the divergence. Log loudly so operators notice.
                log!(
                    INFO,
                    "Liquidation call failed for vault {} with token {}: {:?}. \
                      No bookkeeping change; ledger balance should be reconciled if \
                      tokens moved silently.",
                    vault_info.vault_id,
                    token_ledger,
                    call_error
                );
            }
        }
    }

    // --- LP tokens (3USD): approve + backend pull (atomic) ---
    for (token_ledger, amount) in &token_draw {
        let config = match stablecoin_configs.get(token_ledger) {
            Some(c) if c.is_lp_token.unwrap_or(false) => c,
            _ => continue,
        };

        // Calculate icUSD equivalent using cached virtual price
        let vp = read_state(|s| {
            s.virtual_prices()
                .get(token_ledger)
                .copied()
                .unwrap_or(1_000_000_000_000_000_000)
        });
        let icusd_equiv_e8s = lp_to_usd_e8s(*amount, vp);

        if icusd_equiv_e8s < 10_000_000 {
            log!(
                INFO,
                "Skipping LP token {}: icUSD equivalent {} e8s below backend minimum",
                token_ledger,
                icusd_equiv_e8s
            );
            continue;
        }

        // Step A: Approve backend to pull 3USD (same pattern as non-LP tokens)
        let approve_args = ApproveArgs {
            from_subaccount: None,
            spender: Account {
                owner: protocol_id,
                subaccount: None,
            },
            amount: candid::Nat::from(*amount as u128 * 2), // 2x buffer for fees
            expected_allowance: None,
            expires_at: Some(ic_cdk::api::time() + 300_000_000_000), // 5 min
            fee: None,
            memo: None,
            created_at_time: Some(ic_cdk::api::time()),
        };

        let approve_result: Result<(Result<candid::Nat, ApproveError>,), _> =
            call(*token_ledger, "icrc2_approve", (approve_args,)).await;

        match approve_result {
            Ok((Ok(_),)) => {
                // Deduct the approve fee from tracked balances
                if let Some(fee) = config.transfer_fee {
                    if fee > 0 {
                        mutate_state(|s| s.deduct_fee_from_pool(*token_ledger, fee));
                    }
                }
            }
            Ok((Err(e),)) => {
                log!(
                    INFO,
                    "3USD approve failed for vault {}: {:?}",
                    vault_info.vault_id,
                    e
                );
                continue;
            }
            Err(e) => {
                log!(
                    INFO,
                    "3USD approve call failed for vault {}: {:?}",
                    vault_info.vault_id,
                    e
                );
                continue;
            }
        }

        // Step B: Ask backend to pull 3USD + write down debt atomically.
        // `process_liquidation_gains` runs once after this loop and is the single
        // point of truth for bookkeeping — no pre-deduct (SP-001 regression fix,
        // audit 2026-04-22-28e9896).

        let liq_result: Result<
            (Result<StabilityPoolLiquidationResult, rumi_protocol_backend::ProtocolError>,),
            _,
        > = call(
            protocol_id,
            "stability_pool_liquidate_with_reserves",
            (vault_info.vault_id, icusd_equiv_e8s, *amount, *token_ledger),
        )
        .await;

        match liq_result {
            Ok((Ok(success),)) => {
                // VER-002 (audit 2026-06-05): the backend caps the writedown to
                // the vault's current debt and refunds the proportional excess
                // 3USD (see stability_pool_liquidate_with_reserves). Record only
                // the REALIZED 3USD using the SAME floor formula the backend
                // refund uses, so the SP's tracked aggregate and its ledger
                // balance both net to exactly the realized amount (no drift).
                // `icusd_equiv_e8s` here equals the `icusd_debt_covered_e8s` the
                // backend received, so the two formulas are identical.
                let realized_3usd =
                    if icusd_equiv_e8s > 0 && success.liquidated_debt < icusd_equiv_e8s {
                        ((*amount as u128).saturating_mul(success.liquidated_debt as u128)
                            / icusd_equiv_e8s as u128) as u64
                    } else {
                        *amount
                    };
                actual_consumed.insert(*token_ledger, realized_3usd);
                total_collateral_gained += success.collateral_received;
                log!(INFO, "3USD reserves liquidation succeeded for vault {}: {} collateral, {} 3USD consumed (requested {})",
                    vault_info.vault_id, success.collateral_received, realized_3usd, amount);
                break; // one token per vault per round
            }
            Ok((Err(e),)) => {
                // Backend explicitly rejected; approval expires harmlessly and nothing
                // was pre-deducted, so there is no bookkeeping to roll back.
                log!(
                    INFO,
                    "Backend rejected 3USD reserves liquidation for vault {}: {:?}",
                    vault_info.vault_id,
                    e
                );
            }
            Err(e) => {
                // Inter-canister call failed; outcome unknown. We do NOT mutate
                // depositor bookkeeping (SP-005 regression fix). If the backend
                // pulled the 3USD silently, operator reconciliation against
                // `icrc1_balance_of(pool)` will reconcile.
                log!(
                    INFO,
                    "3USD reserves liquidation call failed for vault {}: {:?}. \
                      No bookkeeping change; ledger balance should be reconciled if \
                      tokens moved silently.",
                    vault_info.vault_id,
                    e
                );
            }
        }
    }

    // Record liquidation event
    let stables_consumed_e8s: u64 = actual_consumed.values().sum();
    let liq_success = !actual_consumed.is_empty() && total_collateral_gained > 0;
    mutate_state(|s| {
        s.push_event(
            s.protocol_canister_id,
            PoolEventType::LiquidationExecuted {
                vault_id: vault_info.vault_id,
                stables_consumed_e8s,
                collateral_gained: total_collateral_gained,
                collateral_type: vault_info.collateral_type,
                success: liq_success,
            },
        );
    });

    // Step 3: If any liquidation calls succeeded, process gains
    if !actual_consumed.is_empty() && total_collateral_gained > 0 {
        // Deduct the collateral ledger's transfer fee from gains — the backend reports
        // gross collateral but the transfer to the SP deducts one fee.
        let collateral_fee: u64 = match call::<(), (candid::Nat,)>(
            vault_info.collateral_type,
            "icrc1_fee",
            (),
        )
        .await
        {
            Ok((fee_nat,)) => {
                let fee: u128 = fee_nat.0.try_into().unwrap_or(0);
                fee as u64
            }
            Err(e) => {
                // SP-104 (audit 2026-06-05): do NOT fall back to fee=0. The actual
                // payout transfer deducts the real ledger fee, so crediting the full
                // gross over-credits depositors and leaves the pool short by one fee.
                // Use a conservative fallback so we under- rather than over-credit
                // (solvency-safe); the next successful interaction reconciles.
                log!(INFO, "icrc1_fee query failed for collateral {}: {:?}; using conservative fallback {} e8s",
                    vault_info.collateral_type, e, FALLBACK_COLLATERAL_FEE_E8S);
                FALLBACK_COLLATERAL_FEE_E8S
            }
        };
        let net_collateral = total_collateral_gained.saturating_sub(collateral_fee);

        mutate_state(|s| {
            s.process_liquidation_gains(
                vault_info.vault_id,
                vault_info.collateral_type,
                &actual_consumed,
                net_collateral,
                vault_info.collateral_price_e8s,
            );
        });

        LiquidationResult {
            vault_id: vault_info.vault_id,
            stables_consumed: actual_consumed,
            collateral_gained: net_collateral,
            collateral_type: vault_info.collateral_type,
            success: true,
            error_message: None,
        }
    } else {
        LiquidationResult {
            vault_id: vault_info.vault_id,
            stables_consumed: BTreeMap::new(),
            collateral_gained: 0,
            collateral_type: vault_info.collateral_type,
            success: false,
            error_message: Some("All liquidation calls failed".to_string()),
        }
    }
}

/// Thin translation layer: map a ledger principal to the backend's StableTokenType enum.
fn determine_stable_token_type(
    ledger: Principal,
    configs: &BTreeMap<Principal, StablecoinConfig>,
) -> Option<rumi_protocol_backend::StableTokenType> {
    let config = configs.get(&ledger)?;
    match config.symbol.as_str() {
        "ckUSDT" => Some(rumi_protocol_backend::StableTokenType::CKUSDT),
        "ckUSDC" => Some(rumi_protocol_backend::StableTokenType::CKUSDC),
        _ => None,
    }
}

/// Backend result type for debt-already-burned liquidations.
#[derive(candid::CandidType, candid::Deserialize, Debug)]
struct StabilityPoolLiquidationResult {
    pub success: bool,
    pub vault_id: u64,
    pub liquidated_debt: u64,
    pub collateral_received: u64,
    pub collateral_type: String,
    pub block_index: u64,
    pub fee: u64,
    pub collateral_price_e8s: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{chain_collateral_sentinel, StabilityPoolState};
    use candid::Nat;
    use icrc_ledger_types::icrc1::transfer::Memo;

    fn principal(byte: u8) -> Principal {
        Principal::from_slice(&[byte])
    }

    fn icusd_ledger() -> Principal {
        Principal::from_slice(&[10])
    }

    fn ckusdc_ledger() -> Principal {
        Principal::from_slice(&[11])
    }

    fn user_a() -> Principal {
        Principal::from_slice(&[1])
    }

    fn user_b() -> Principal {
        Principal::from_slice(&[2])
    }

    fn test_state() -> StabilityPoolState {
        let mut state = StabilityPoolState::default();
        state.register_stablecoin(StablecoinConfig {
            ledger_id: icusd_ledger(),
            symbol: "icUSD".to_string(),
            decimals: 8,
            priority: 1,
            is_active: true,
            transfer_fee: Some(100_000),
            is_lp_token: None,
            underlying_pool: None,
        });
        state.register_stablecoin(StablecoinConfig {
            ledger_id: ckusdc_ledger(),
            symbol: "ckUSDC".to_string(),
            decimals: 6,
            priority: 2,
            is_active: true,
            transfer_fee: Some(10),
            is_lp_token: None,
            underlying_pool: None,
        });
        state
    }

    fn add_deposit_direct(
        state: &mut StabilityPoolState,
        user: Principal,
        token: Principal,
        amount: u64,
    ) {
        let position = state
            .deposits
            .entry(user)
            .or_insert_with(|| DepositPosition::new(0));
        *position.stablecoin_balances.entry(token).or_insert(0) += amount;
        *state.total_stablecoin_balances.entry(token).or_insert(0) += amount;
    }

    fn chain_vault(debt_e8s: u128, sp_attempted: bool) -> ChainLiquidatableVaultInfo {
        chain_vault_with_id(77, debt_e8s, sp_attempted)
    }

    fn chain_vault_with_id(
        vault_id: u64,
        debt_e8s: u128,
        sp_attempted: bool,
    ) -> ChainLiquidatableVaultInfo {
        ChainLiquidatableVaultInfo {
            vault_id,
            chain_id: rumi_protocol_backend::chains::config::ChainId(1030),
            chain_collateral_sentinel: chain_collateral_sentinel(1030),
            sp_attempted,
            debt_e8s,
            effective_debt_e8s: debt_e8s,
            collateral_native: 1_000_000_000_000_000_000_000,
            cr_e4: 12_000,
            liquidation_threshold_e4: 13_500,
            sized_repay_e8s: debt_e8s,
        }
    }

    fn minting_account() -> Account {
        Account {
            owner: principal(90),
            subaccount: None,
        }
    }

    fn backend_chain_result() -> ChainStabilityPoolLiquidationResult {
        ChainStabilityPoolLiquidationResult {
            success: true,
            vault_id: 77,
            chain_id: rumi_protocol_backend::chains::config::ChainId(1030),
            liquidated_debt_e8s: 100_00000000,
            collateral_received_native: 10_000_000_000_000_000_000u128,
            claim_id: 77,
            custody_address: "0xcustody".to_string(),
            block_index: 44,
            collateral_price_e8s: 5_000_000,
        }
    }

    #[test]
    fn chain_writedown_memo_matches_backend_liq_004_shape() {
        let vault_id: u64 = 0x0102_0304_0506_0708;
        let memo = encode_chain_writedown_memo(vault_id);

        assert_eq!(&memo[..13], b"RUMI-LIQ-004:");
        assert_eq!(&memo[13..], &vault_id.to_be_bytes());
        assert_eq!(
            rumi_protocol_backend::icrc3_proof::decode_writedown_memo(&memo),
            Ok(vault_id),
            "SP burn memo must be accepted by backend proof verifier",
        );
    }

    #[test]
    fn icusd_burn_request_targets_minting_account_and_builds_proof() {
        let minting_account = Account {
            owner: principal(90),
            subaccount: None,
        };
        let amount_e8s = 12_345_00000000;
        let vault_id = 77;
        let created_at_time = 123_456_789;
        let block_index = 999;

        let transfer =
            build_icusd_burn_transfer_arg(minting_account, amount_e8s, vault_id, created_at_time);

        assert_eq!(transfer.to, minting_account);
        assert_eq!(transfer.amount, Nat::from(amount_e8s));
        assert_eq!(
            transfer.fee, None,
            "ICRC-1 burns to the minting account have zero fee"
        );
        assert_eq!(transfer.from_subaccount, None);
        assert_eq!(transfer.created_at_time, Some(created_at_time));
        assert_eq!(
            transfer.memo,
            Some(Memo::from(encode_chain_writedown_memo(vault_id))),
        );

        let proof = build_icusd_burn_proof(block_index, vault_id);
        assert_eq!(proof.block_index, block_index);
        assert_eq!(
            proof.ledger_kind,
            rumi_protocol_backend::icrc3_proof::SpProofLedger::IcusdBurn
        );
        assert_eq!(proof.vault_id_memo, vault_id);
    }

    #[test]
    fn registered_chain_ids_decode_from_registered_sentinels() {
        let mut state = test_state();
        state
            .register_chain_collateral(1030, "CFX".to_string(), 18)
            .unwrap();

        assert_eq!(
            registered_chain_ids_from_sentinels(&state),
            vec![rumi_protocol_backend::chains::config::ChainId(1030)],
        );
    }

    #[test]
    fn chain_absorb_preflight_requires_escalation_and_icusd_coverage() {
        let mut state = test_state();
        state
            .register_chain_collateral(1030, "CFX".to_string(), 18)
            .unwrap();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 40_00000000);
        add_deposit_direct(&mut state, user_a(), ckusdc_ledger(), 5_000_000_000);
        add_deposit_direct(&mut state, user_b(), icusd_ledger(), 100_00000000);

        let not_escalated =
            prepare_chain_absorb_plan_in_state(&state, &chain_vault(70_00000000, false))
                .unwrap_err();
        assert!(matches!(
            not_escalated,
            StabilityPoolError::LiquidationFailed { .. }
        ));

        let no_opt_in = prepare_chain_absorb_plan_in_state(&state, &chain_vault(70_00000000, true))
            .unwrap_err();
        assert!(matches!(
            no_opt_in,
            StabilityPoolError::InsufficientPoolBalance
        ));

        state
            .opt_in_cfx(&user_a(), chain_collateral_sentinel(1030))
            .unwrap();
        let undercovered =
            prepare_chain_absorb_plan_in_state(&state, &chain_vault(70_00000000, true))
                .unwrap_err();
        assert!(
            matches!(undercovered, StabilityPoolError::InsufficientPoolBalance),
            "ckUSDC must not count toward chain absorb coverage",
        );

        state
            .opt_in_cfx(&user_b(), chain_collateral_sentinel(1030))
            .unwrap();
        let plan = prepare_chain_absorb_plan_in_state(&state, &chain_vault(70_00000000, true))
            .expect("covered by opted-in icUSD");
        assert_eq!(plan.icusd_to_burn_e8s, 70_00000000);
        assert_eq!(
            plan.stables_consumed.get(&icusd_ledger()).copied(),
            Some(70_00000000)
        );
        assert_eq!(plan.stables_consumed.len(), 1);
    }

    #[test]
    fn chain_absorb_intent_reuses_timestamp_and_rejects_conflicting_recompute() {
        let mut state = test_state();
        state
            .register_chain_collateral(1030, "CFX".to_string(), 18)
            .unwrap();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_00000000);
        state
            .opt_in_cfx(&user_a(), chain_collateral_sentinel(1030))
            .unwrap();

        let plan = prepare_chain_absorb_plan_in_state(&state, &chain_vault(100_00000000, true))
            .expect("covered");
        let intent = prepare_or_reuse_chain_absorb_intent_in_state(
            &mut state,
            &plan,
            minting_account(),
            123,
        )
        .expect("intent is recorded");

        assert_eq!(intent.burn_created_at_time_ns, 123);
        assert!(state.has_pending_chain_absorbs());
        assert_eq!(state.pending_chain_absorb_count(), 1);

        let reused = prepare_or_reuse_chain_absorb_intent_in_state(
            &mut state,
            &plan,
            minting_account(),
            999,
        )
        .expect("same plan reuses existing intent");
        assert_eq!(
            reused.burn_created_at_time_ns, 123,
            "retry must not recompute ledger dedupe timestamp",
        );

        let mut conflicting = plan.clone();
        conflicting.icusd_to_burn_e8s -= 1;
        let err = prepare_or_reuse_chain_absorb_intent_in_state(
            &mut state,
            &conflicting,
            minting_account(),
            1_000,
        )
        .unwrap_err();
        assert!(matches!(err, StabilityPoolError::LiquidationFailed { .. }));
    }

    #[test]
    fn chain_absorb_intent_records_single_burn_proof() {
        let mut state = test_state();
        state
            .register_chain_collateral(1030, "CFX".to_string(), 18)
            .unwrap();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_00000000);
        state
            .opt_in_cfx(&user_a(), chain_collateral_sentinel(1030))
            .unwrap();
        let plan = prepare_chain_absorb_plan_in_state(&state, &chain_vault(100_00000000, true))
            .expect("covered");
        prepare_or_reuse_chain_absorb_intent_in_state(&mut state, &plan, minting_account(), 123)
            .expect("intent is recorded");

        let proof = build_icusd_burn_proof(44, 77);
        let burned = mark_chain_absorb_burned_in_state(&mut state, 77, proof.clone(), 456)
            .expect("proof recorded");
        assert_eq!(burned.status, ChainSpAbsorbIntentStatus::Burned);
        assert_eq!(burned.burn_proof, Some(proof.clone()));

        mark_chain_absorb_burned_in_state(&mut state, 77, proof, 789)
            .expect("same proof is idempotent");
        let conflict =
            mark_chain_absorb_burned_in_state(&mut state, 77, build_icusd_burn_proof(45, 77), 790)
                .unwrap_err();
        assert!(matches!(
            conflict,
            StabilityPoolError::LiquidationFailed { .. }
        ));
    }

    #[test]
    fn burned_chain_absorb_intent_replays_backend_without_discovery() {
        let mut state = test_state();
        state
            .register_chain_collateral(1030, "CFX".to_string(), 18)
            .unwrap();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_00000000);
        state
            .opt_in_cfx(&user_a(), chain_collateral_sentinel(1030))
            .unwrap();
        let plan = prepare_chain_absorb_plan_in_state(&state, &chain_vault(100_00000000, true))
            .expect("covered");
        prepare_or_reuse_chain_absorb_intent_in_state(&mut state, &plan, minting_account(), 123)
            .expect("intent is recorded");
        let proof = build_icusd_burn_proof(44, 77);
        let burned = mark_chain_absorb_burned_in_state(&mut state, 77, proof.clone(), 456)
            .expect("proof recorded");

        let (replay_plan, replay_proof) =
            burned_chain_absorb_replay_plan(&burned).expect("burned intent replays by proof");
        assert_eq!(replay_plan, plan);
        assert_eq!(replay_proof, proof);
        assert!(
            burned.backend_result.is_none(),
            "this covers backend-accepted/lost-reply state before local result journaling",
        );
    }

    #[test]
    fn other_pending_chain_absorb_blocks_new_burn_plan() {
        let mut state = test_state();
        state
            .register_chain_collateral(1030, "CFX".to_string(), 18)
            .unwrap();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_00000000);
        state
            .opt_in_cfx(&user_a(), chain_collateral_sentinel(1030))
            .unwrap();

        let plan_a =
            prepare_chain_absorb_plan_in_state(&state, &chain_vault_with_id(77, 100_00000000, true))
                .expect("vault A covered");
        prepare_or_reuse_chain_absorb_intent_in_state(&mut state, &plan_a, minting_account(), 123)
            .expect("intent A is recorded");
        mark_chain_absorb_burned_in_state(&mut state, 77, build_icusd_burn_proof(44, 77), 456)
            .expect("intent A burn proof recorded");

        assert!(
            ensure_no_other_pending_chain_absorb(&state, 77).is_ok(),
            "the original vault remains retryable",
        );
        assert!(
            matches!(
                ensure_no_other_pending_chain_absorb(&state, 78),
                Err(StabilityPoolError::SystemBusy)
            ),
            "a burned pending intent reserves its icUSD until local finalization",
        );
    }

    #[test]
    fn chain_absorb_backend_result_intent_can_finalize_without_discovery() {
        let mut state = test_state();
        state
            .register_chain_collateral(1030, "CFX".to_string(), 18)
            .unwrap();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_00000000);
        state
            .opt_in_cfx(&user_a(), chain_collateral_sentinel(1030))
            .unwrap();
        let plan = prepare_chain_absorb_plan_in_state(&state, &chain_vault(100_00000000, true))
            .expect("covered");
        prepare_or_reuse_chain_absorb_intent_in_state(&mut state, &plan, minting_account(), 123)
            .expect("intent is recorded");
        let backend_result = backend_chain_result();
        let intent = mark_chain_absorb_backend_result_in_state(&mut state, 77, backend_result, 456)
            .expect("backend result is stored");

        let resumed_plan = chain_absorb_plan_from_intent(&intent);
        let absorbed = apply_chain_absorb_success_in_state_at(
            &mut state,
            &resumed_plan,
            intent.backend_result.expect("stored"),
            789,
        )
        .expect("stored backend result finalizes locally");

        assert_eq!(absorbed.icusd_burned_e8s, 100_00000000);
        assert!(
            state.get_pending_chain_absorb(77).is_none(),
            "local finalization clears pending journal entry",
        );
        assert!(state.completed_chain_absorb(77).is_some());
    }

    #[test]
    fn chain_absorb_rejects_partial_backend_result_without_deducting_pool() {
        let mut state = test_state();
        state
            .register_chain_collateral(1030, "CFX".to_string(), 18)
            .unwrap();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 100_00000000);
        state
            .opt_in_cfx(&user_a(), chain_collateral_sentinel(1030))
            .unwrap();

        let plan = prepare_chain_absorb_plan_in_state(&state, &chain_vault(100_00000000, true))
            .expect("covered");
        let mut partial = backend_chain_result();
        partial.liquidated_debt_e8s = 80_00000000;

        let err =
            apply_chain_absorb_success_in_state_at(&mut state, &plan, partial, 123).unwrap_err();

        assert!(
            matches!(err, StabilityPoolError::LiquidationFailed { .. }),
            "partial backend result must be rejected before pool accounting"
        );
        assert_eq!(
            state
                .total_stablecoin_balances
                .get(&icusd_ledger())
                .copied(),
            Some(100_00000000),
            "pool icUSD balance is unchanged",
        );
        assert!(
            state
                .deposits
                .get(&user_a())
                .unwrap()
                .cfx_claims
                .clone()
                .unwrap_or_default()
                .is_empty(),
            "no CFX claim is credited on a partial backend result",
        );
        assert!(state.completed_chain_absorb(77).is_none());
    }

    #[test]
    fn chain_absorb_success_credits_cfx_claims_and_deducts_burned_icusd() {
        let mut state = test_state();
        state
            .register_chain_collateral(1030, "CFX".to_string(), 18)
            .unwrap();
        add_deposit_direct(&mut state, user_a(), icusd_ledger(), 40_00000000);
        add_deposit_direct(&mut state, user_b(), icusd_ledger(), 60_00000000);
        state
            .opt_in_cfx(&user_a(), chain_collateral_sentinel(1030))
            .unwrap();
        state
            .opt_in_cfx(&user_b(), chain_collateral_sentinel(1030))
            .unwrap();

        let plan = prepare_chain_absorb_plan_in_state(&state, &chain_vault(100_00000000, true))
            .expect("covered");
        let result = backend_chain_result();

        let absorbed =
            apply_chain_absorb_success_in_state_at(&mut state, &plan, result.clone(), 123)
                .expect("success finalizes");

        assert_eq!(absorbed.icusd_burned_e8s, 100_00000000);
        assert_eq!(
            state
                .total_stablecoin_balances
                .get(&icusd_ledger())
                .copied(),
            Some(0),
            "SP aggregate tracks the burned icUSD",
        );
        let claim_a = state
            .deposits
            .get(&user_a())
            .unwrap()
            .cfx_claims
            .as_ref()
            .unwrap()
            .get(&chain_collateral_sentinel(1030))
            .copied()
            .unwrap_or(0);
        let claim_b = state
            .deposits
            .get(&user_b())
            .unwrap()
            .cfx_claims
            .as_ref()
            .unwrap()
            .get(&chain_collateral_sentinel(1030))
            .copied()
            .unwrap_or(0);
        assert_eq!(claim_a, 4_000_000_000_000_000_000u128);
        assert_eq!(claim_b, 6_000_000_000_000_000_000u128);

        let repeated = apply_chain_absorb_success_in_state_at(&mut state, &plan, result, 124)
            .expect("same result is idempotent");
        assert_eq!(repeated, absorbed);
        assert_eq!(
            state
                .total_stablecoin_balances
                .get(&icusd_ledger())
                .copied(),
            Some(0),
            "replay must not double-deduct icUSD",
        );
        assert_eq!(
            state
                .deposits
                .get(&user_a())
                .unwrap()
                .cfx_claims
                .as_ref()
                .unwrap()
                .get(&chain_collateral_sentinel(1030))
                .copied()
                .unwrap_or(0),
            claim_a,
            "replay must not double-credit user A CFX",
        );
        assert_eq!(
            state
                .deposits
                .get(&user_b())
                .unwrap()
                .cfx_claims
                .as_ref()
                .unwrap()
                .get(&chain_collateral_sentinel(1030))
                .copied()
                .unwrap_or(0),
            claim_b,
            "replay must not double-credit user B CFX",
        );
        assert_eq!(state.completed_chain_absorbs(10).len(), 1);
    }

    #[test]
    fn cfx_claim_payout_deducts_from_user_and_claim_source() {
        let mut state = test_state();
        let sentinel = state
            .register_chain_collateral(1030, "CFX".to_string(), 18)
            .unwrap();
        let mut pos = DepositPosition::new(0);
        pos.cfx_claims
            .get_or_insert_with(BTreeMap::new)
            .insert(sentinel, 12);
        state.deposits.insert(user_a(), pos);
        state.record_chain_claim_source(sentinel, 77, 10);

        let invalid = prepare_cfx_claim_payout_in_state(
            &mut state,
            user_a(),
            sentinel,
            "not-evm".to_string(),
            |_| false,
        )
        .unwrap_err();
        assert!(matches!(
            invalid,
            StabilityPoolError::LiquidationFailed { .. }
        ));
        assert_eq!(
            state.deposits[&user_a()].cfx_claims.as_ref().unwrap()[&sentinel],
            12,
            "destination validation happens before mutation",
        );
        assert_eq!(
            state.chain_claim_sources.as_ref().unwrap()[&sentinel][0].remaining_native,
            10
        );

        let plan = prepare_cfx_claim_payout_in_state(
            &mut state,
            user_a(),
            sentinel,
            "0x000000000000000000000000000000000000c0de".to_string(),
            rumi_protocol_backend::chains::evm::tecdsa::is_valid_evm_address,
        )
        .expect("claim plan")
        .expect("nonzero claim");
        assert_eq!(plan.claim_id, 77);
        assert_eq!(plan.amount_wei, 10);
        assert_eq!(
            state.deposits[&user_a()].cfx_claims.as_ref().unwrap()[&sentinel],
            2,
            "only the covered source amount is deducted",
        );
        assert!(
            state
                .chain_claim_sources
                .as_ref()
                .unwrap()
                .get(&sentinel)
                .is_none(),
            "depleted source is pruned before await",
        );

        rollback_cfx_claim_payout_in_state(&mut state, &plan);
        assert_eq!(
            state.deposits[&user_a()].cfx_claims.as_ref().unwrap()[&sentinel],
            12,
            "rollback restores user claim",
        );
        assert_eq!(
            state.chain_claim_sources.as_ref().unwrap()[&sentinel][0].remaining_native,
            10,
            "rollback restores backend claim source",
        );
    }

    #[test]
    fn duplicate_chain_claim_error_is_not_rolled_back() {
        let duplicate = rumi_protocol_backend::ProtocolError::ChainAdmin(
            "Duplicate chain collateral claim payout idempotency key chain-collateral-claim-77"
                .to_string(),
        );
        let ordinary = rumi_protocol_backend::ProtocolError::ChainAdmin(
            "chain collateral claim: unknown claim 77".to_string(),
        );

        assert!(is_duplicate_chain_claim_error(&duplicate));
        assert!(!is_duplicate_chain_claim_error(&ordinary));
    }
}
