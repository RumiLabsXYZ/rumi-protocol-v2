use crate::event::{
    record_add_margin_to_vault, record_borrow_from_vault, record_open_vault,
    record_repayed_to_vault,
};
use crate::guard::{GuardPrincipal, VaultLiquidationGuard};
use crate::logs::INFO;
use crate::management;
use crate::management::{
    transfer_collateral, transfer_collateral_from, transfer_icusd_from, transfer_stable_from,
};
use crate::numeric::{Ratio, UsdIcp, ICP, ICUSD};
use crate::state::{compute_redemption_fee_with_rate, Mode, RedemptionSimulationPlan};
use crate::GuardError;
use crate::PendingMarginTransfer;
use crate::DEBUG;
use crate::{
    mutate_state, read_state, PreparedRedemptionOffer, ProtocolError, RedeemQuotedRequest,
    RedemptionError, RedemptionOfferRefreshError, RedemptionPayoutStatus, RedemptionPreview,
    RedemptionQueue, RedemptionQueueEntry, RedemptionQuote, RedemptionResult,
    StabilityPoolLiquidationResult, StableTokenType, SuccessWithFee, VaultArgWithToken,
    DUST_THRESHOLD,
};
use candid::{CandidType, Deserialize, Principal};
use ic_canister_log::log;
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc2::transfer_from::TransferFromError;
use rust_decimal::prelude::{FromPrimitive, ToPrimitive};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::panic::{catch_unwind, AssertUnwindSafe};

/// One ten-minute maximum-age contract shared by quote display, ranking refresh,
/// and pre/post-pull verification. This matches XRC's existing hard ceiling.
const REDEMPTION_PRICE_MAX_AGE_NS: u64 = 10 * 60 * 1_000_000_000;
pub(crate) const MAX_REDEMPTION_PRICE_CANDIDATES: usize = 64;
const MAX_REDEMPTION_OFFER_REFRESH_PASSES: usize = 2;
const MAX_LST_EXTERNAL_CALLS_PER_REFRESH: usize = 2;
const MAX_REDEMPTION_OFFER_REFRESH_TARGETS_PER_PASS: usize = MAX_REDEMPTION_PRICE_CANDIDATES + 1; // one off-set ICP source dependency
const REDEMPTION_OFFER_REFRESH_COOLDOWN_NS: u64 = 300 * 1_000_000_000;
const REDEMPTION_OFFER_REFRESH_LEASE_NS: u64 = 300 * 1_000_000_000;
// Deliberately loose upper bound: two passes over at most 64 candidates plus
// one ICP dependency target, at most two source calls per direct refresh
// (LstWrapped's initial call plus one catch-up), plus two ICP XRC calls and two
// possible ICP-coupled LST waves. At most one ICP target is included per pass
// even when it is both a candidate and an LST dependency. `fetch_icp_rate`
// couples at most 64 LSTs, each with the same two-call ceiling. This is 518
// calls maximum.
const MAX_REDEMPTION_OFFER_EXTERNAL_CALLS: usize = (MAX_REDEMPTION_OFFER_REFRESH_PASSES
    * MAX_REDEMPTION_OFFER_REFRESH_TARGETS_PER_PASS
    * MAX_LST_EXTERNAL_CALLS_PER_REFRESH)
    + MAX_REDEMPTION_OFFER_REFRESH_PASSES
    + (MAX_REDEMPTION_OFFER_REFRESH_PASSES
        * MAX_REDEMPTION_PRICE_CANDIDATES
        * MAX_LST_EXTERNAL_CALLS_PER_REFRESH);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct RedemptionOfferRefreshGateState {
    next_token: u64,
    in_flight: Option<(u64, u64)>,
    cooldown_until_ns: u64,
}

impl RedemptionOfferRefreshGateState {
    fn try_acquire(&mut self, now_ns: u64) -> Result<u64, RedemptionOfferRefreshError> {
        if let Some((_, started_at_ns)) = self.in_flight {
            let elapsed = now_ns.saturating_sub(started_at_ns);
            if now_ns < started_at_ns || elapsed < REDEMPTION_OFFER_REFRESH_LEASE_NS {
                return Err(RedemptionOfferRefreshError::RefreshInProgress {
                    retry_after_ns: REDEMPTION_OFFER_REFRESH_LEASE_NS.saturating_sub(elapsed),
                });
            }
            // A suspended refresh may have lost its continuation. Expire its
            // lease; the token check in release prevents its late Drop from
            // clearing a newer owner's guard.
            self.in_flight = None;
        }
        if now_ns < self.cooldown_until_ns {
            return Err(RedemptionOfferRefreshError::RefreshCooldown {
                retry_after_ns: self.cooldown_until_ns - now_ns,
            });
        }

        self.next_token = self.next_token.wrapping_add(1).max(1);
        let token = self.next_token;
        self.in_flight = Some((token, now_ns));
        self.cooldown_until_ns = now_ns.saturating_add(REDEMPTION_OFFER_REFRESH_COOLDOWN_NS);
        Ok(token)
    }

    fn release(&mut self, token: u64) {
        if self.in_flight.map(|(current, _)| current) == Some(token) {
            self.in_flight = None;
        }
    }
}

thread_local! {
    static BORROW_MINT_DISPATCHES: RefCell<std::collections::BTreeSet<u128>> =
        RefCell::new(std::collections::BTreeSet::new());
    static SP_V2_PAYOUT_DISPATCHES: RefCell<std::collections::BTreeSet<(Principal, u64)>> =
        RefCell::new(std::collections::BTreeSet::new());
    /// The refresh lock/cooldown is transient because it controls public oracle
    /// work rather than financial state. A 300s owner-safe lease recovers a
    /// dropped continuation; refreshes are revalidated from current State.
    static REDEMPTION_OFFER_REFRESH_GATE: RefCell<RedemptionOfferRefreshGateState> =
        RefCell::new(RedemptionOfferRefreshGateState::default());
}

struct SpV2PayoutDispatchGuard((Principal, u64));

impl SpV2PayoutDispatchGuard {
    fn try_new(stability_pool: Principal, request_id: u64) -> Option<Self> {
        SP_V2_PAYOUT_DISPATCHES.with(|active| {
            let mut active = active.borrow_mut();
            let key = (stability_pool, request_id);
            if active.insert(key) {
                Some(Self(key))
            } else {
                None
            }
        })
    }
}

impl Drop for SpV2PayoutDispatchGuard {
    fn drop(&mut self) {
        SP_V2_PAYOUT_DISPATCHES.with(|active| {
            active.borrow_mut().remove(&self.0);
        });
    }
}

struct BorrowMintDispatchGuard(u128);

impl BorrowMintDispatchGuard {
    fn try_new(op_nonce: u128) -> Option<Self> {
        BORROW_MINT_DISPATCHES.with(|active| {
            let mut active = active.borrow_mut();
            if active.insert(op_nonce) {
                Some(Self(op_nonce))
            } else {
                None
            }
        })
    }
}

impl Drop for BorrowMintDispatchGuard {
    fn drop(&mut self) {
        BORROW_MINT_DISPATCHES.with(|active| {
            active.borrow_mut().remove(&self.0);
        });
    }
}

struct RedemptionOfferRefreshGuard(u64);

impl RedemptionOfferRefreshGuard {
    fn try_acquire(now_ns: u64) -> Result<Self, RedemptionOfferRefreshError> {
        REDEMPTION_OFFER_REFRESH_GATE.with(|gate| gate.borrow_mut().try_acquire(now_ns).map(Self))
    }
}

impl Drop for RedemptionOfferRefreshGuard {
    fn drop(&mut self) {
        REDEMPTION_OFFER_REFRESH_GATE.with(|gate| gate.borrow_mut().release(self.0));
    }
}

fn redemption_offer_refresh_cooldown_remaining(now_ns: u64) -> u64 {
    REDEMPTION_OFFER_REFRESH_GATE
        .with(|gate| gate.borrow().cooldown_until_ns.saturating_sub(now_ns))
}

/// Fee inputs frozen for one read-only queue calculation. The elapsed-hour
/// decay and per-collateral debt total are invariant across capacity probes;
/// snapshotting them avoids repeating a full debt-index scan and decay loop.
#[derive(Clone, Copy)]
struct RedemptionFeeSnapshot {
    total_debt: ICUSD,
    decayed_base_rate: Ratio,
    fee_floor: Ratio,
    fee_ceiling: Ratio,
}

impl RedemptionFeeSnapshot {
    fn fee(self, amount: ICUSD) -> Ratio {
        compute_redemption_fee_with_rate(
            amount,
            self.total_debt,
            self.decayed_base_rate,
            self.fee_floor,
            self.fee_ceiling,
        )
    }
}

fn redemption_fee_snapshot(
    state: &crate::state::State,
    collateral_type: &Principal,
    now: u64,
) -> RedemptionFeeSnapshot {
    let (total_debt, current_base_rate, fee_floor, fee_ceiling, last_redemption_time) = state
        .get_collateral_config(collateral_type)
        .map(|config| {
            (
                state.total_debt_for_collateral(collateral_type),
                config.current_base_rate,
                config.redemption_fee_floor,
                config.redemption_fee_ceiling,
                config.last_redemption_time,
            )
        })
        .unwrap_or((
            state.total_borrowed_icusd_amount(),
            state.current_base_rate,
            state.redemption_fee_floor,
            state.redemption_fee_ceiling,
            state.last_redemption_time,
        ));
    let elapsed_hours = (now - last_redemption_time) / 1_000_000_000 / 3600;
    RedemptionFeeSnapshot {
        total_debt,
        decayed_base_rate: current_base_rate * Ratio::new(dec!(0.94)).pow(elapsed_hours),
        fee_floor,
        fee_ceiling,
    }
}

fn redemption_simulation_plan(
    state: &crate::state::State,
    run: &crate::state::RedemptionRun,
) -> RedemptionSimulationPlan {
    let price = Decimal::from_f64_retain(run.price_usd).unwrap_or(Decimal::ZERO);
    state.prepare_redemption_simulation_for_vault_ids(
        UsdIcp::from(price),
        &run.collateral_type,
        &run.vault_ids,
    )
}

pub(crate) fn redemption_candidate_types(state: &crate::state::State) -> Vec<Principal> {
    let mut types = std::collections::BTreeSet::new();
    for vault in state.vault_id_to_vaults.values() {
        if vault.borrowed_icusd_amount == 0
            || vault.bot_processing
            || crate::guard::is_vault_liquidating(vault.vault_id)
        {
            continue;
        }
        let collateral_type = if vault.collateral_type == Principal::anonymous() {
            state.icp_collateral_type()
        } else {
            vault.collateral_type
        };
        let Some(config) = state.get_collateral_config(&collateral_type) else {
            continue;
        };
        if config.status.allows_redemption() && !config.is_native_xrp() {
            types.insert(collateral_type);
        }
    }
    types.into_iter().collect()
}

fn redemption_candidate_prices_are_fresh(
    state: &crate::state::State,
    candidates: &[Principal],
    now: u64,
) -> bool {
    candidates.iter().all(|collateral_type| {
        redemption_candidate_price_is_valid(state, collateral_type, now, true)
    })
}

fn stale_redemption_candidate_types(
    state: &crate::state::State,
    candidates: &[Principal],
    now: u64,
) -> Vec<Principal> {
    candidates
        .iter()
        .copied()
        .filter(|ct| !redemption_candidate_price_is_valid(state, ct, now, true))
        .collect()
}

fn is_icp_wrapped_lst(state: &crate::state::State, collateral_type: &Principal) -> bool {
    state
        .get_collateral_config(collateral_type)
        .is_some_and(|config| {
            matches!(
                &config.price_source,
                crate::state::PriceSource::LstWrapped { base_asset, .. }
                    if base_asset == "ICP"
            )
        })
}

fn cached_icp_source_is_fresh(state: &crate::state::State, now: u64) -> bool {
    state
        .last_icp_rate
        .is_some_and(|rate| rate.0 > Decimal::ZERO)
        && state
            .last_icp_timestamp
            .is_some_and(|timestamp| redemption_price_timestamp_is_fresh(timestamp, now))
}

#[derive(Clone, Debug)]
struct RedemptionOfferPriceRefreshSnapshot {
    candidates: Vec<Principal>,
    refresh_targets: Vec<Principal>,
    ranking_fresh: bool,
}

fn redemption_offer_price_refresh_snapshot(
    state: &crate::state::State,
    now: u64,
) -> RedemptionOfferPriceRefreshSnapshot {
    let candidates = redemption_candidate_types(state);
    let stale = stale_redemption_candidate_types(state, &candidates, now);
    let stale_icp_lst = stale
        .iter()
        .any(|collateral_type| is_icp_wrapped_lst(state, collateral_type));
    let mut refresh_targets = stale;

    // An LstWrapped candidate's value is derived from the cached ICP source,
    // even when there is no ICP-denominated vault in the candidate set. Add
    // that dependency once and put it first so the LST refresh observes the
    // newly accepted source timestamp. A direct ICP candidate is deduplicated.
    if stale_icp_lst && !cached_icp_source_is_fresh(state, now) {
        let icp_collateral_type = state.icp_collateral_type();
        refresh_targets.retain(|candidate| *candidate != icp_collateral_type);
        refresh_targets.insert(0, icp_collateral_type);
    }

    let ranking_fresh = redemption_ranking_is_fresh(state, &state.redemption_runs(), now);
    RedemptionOfferPriceRefreshSnapshot {
        candidates,
        refresh_targets,
        ranking_fresh,
    }
}

fn redemption_candidate_price_is_valid(
    state: &crate::state::State,
    collateral_type: &Principal,
    now: u64,
    require_fresh: bool,
) -> bool {
    state
        .get_collateral_config(collateral_type)
        .and_then(|config| Some((config.last_price?, config.last_price_timestamp?)))
        .map(|(price, timestamp)| {
            price.is_finite()
                && price > 0.0
                && timestamp <= now
                && (!require_fresh || now - timestamp <= REDEMPTION_PRICE_MAX_AGE_NS)
        })
        .unwrap_or(false)
}

fn redemption_price_timestamp_is_fresh(timestamp: u64, now: u64) -> bool {
    timestamp <= now && now - timestamp <= REDEMPTION_PRICE_MAX_AGE_NS
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct RedemptionRunSnapshot {
    collateral_type: Principal,
    vault_ids: Vec<u64>,
    price_bits: u64,
    decimals: u8,
}

impl RedemptionRunSnapshot {
    fn capture(run: &crate::state::RedemptionRun) -> Self {
        Self {
            collateral_type: run.collateral_type,
            vault_ids: run.vault_ids.clone(),
            price_bits: run.price_usd.to_bits(),
            decimals: run.decimals,
        }
    }

    fn matches(&self, run: &crate::state::RedemptionRun) -> bool {
        self.collateral_type == run.collateral_type
            && self.vault_ids == run.vault_ids
            && self.price_bits == run.price_usd.to_bits()
            && self.decimals == run.decimals
    }
}

fn redemption_run_snapshot_error(
    expected: &RedemptionRunSnapshot,
    actual: &crate::state::RedemptionRun,
) -> Option<RedemptionError> {
    if expected.matches(actual) {
        None
    } else if expected.collateral_type != actual.collateral_type {
        Some(RedemptionError::RedemptionPriorityChanged {
            expected: expected.collateral_type,
            actual: actual.collateral_type,
        })
    } else {
        Some(RedemptionError::RedemptionQuoteUnavailable(
            "The selected collateral run or price changed during the icUSD pull.".to_string(),
        ))
    }
}

fn legacy_redemption_run_for_request(
    state: &crate::state::State,
    requested_collateral_type: Principal,
) -> Result<crate::state::RedemptionRun, ProtocolError> {
    let requested_collateral_type = if requested_collateral_type == Principal::anonymous() {
        state.icp_collateral_type()
    } else {
        requested_collateral_type
    };
    let run = state.redemption_runs().into_iter().next().ok_or_else(|| {
        ProtocolError::TemporarilyUnavailable(
            "No eligible collateral vaults are available for redemption.".to_string(),
        )
    })?;
    if run.collateral_type != requested_collateral_type {
        let requested_symbol = state
            .get_collateral_config(&requested_collateral_type)
            .and_then(|config| config.symbol.clone())
            .unwrap_or_else(|| requested_collateral_type.to_text());
        return Err(ProtocolError::GenericError(format!(
            "The next collateral is {}, but this request is for {}. Refresh the redemption page and review a new quote.",
            run.symbol, requested_symbol
        )));
    }
    Ok(run)
}

fn validate_legacy_reserve_spillover_asset(
    state: &crate::state::State,
    run: &crate::state::RedemptionRun,
) -> Result<(), ProtocolError> {
    let icp = state.icp_collateral_type();
    if run.collateral_type == icp {
        return Ok(());
    }
    Err(ProtocolError::GenericError(format!(
        "The next collateral is {}, but this reserve redemption can only spill over to ICP. Refresh the redemption page and review a new quote.",
        run.symbol
    )))
}

fn legacy_reserve_spillover_run(
    state: &crate::state::State,
    spillover_e8s: u64,
) -> Result<Option<crate::state::RedemptionRun>, ProtocolError> {
    if spillover_e8s == 0 {
        return Ok(None);
    }
    let run = state.redemption_runs().into_iter().next().ok_or_else(|| {
        ProtocolError::TemporarilyUnavailable(
            "No eligible collateral vaults are available for reserve spillover.".to_string(),
        )
    })?;
    validate_legacy_reserve_spillover_asset(state, &run)?;
    Ok(Some(run))
}

fn current_legacy_reserve_run_for_snapshot(
    state: &crate::state::State,
    expected: &RedemptionRunSnapshot,
) -> Option<crate::state::RedemptionRun> {
    let run = state.redemption_runs().into_iter().next()?;
    (expected.matches(&run) && validate_legacy_reserve_spillover_asset(state, &run).is_ok())
        .then_some(run)
}

fn current_fresh_legacy_reserve_run_for_snapshot(
    state: &crate::state::State,
    expected: &RedemptionRunSnapshot,
    now: u64,
) -> Option<crate::state::RedemptionRun> {
    let runs = state.redemption_runs();
    if !redemption_ranking_is_fresh(state, &runs, now) {
        return None;
    }
    let run = runs.into_iter().next()?;
    (expected.matches(&run) && validate_legacy_reserve_spillover_asset(state, &run).is_ok())
        .then_some(run)
}

fn reserve_spillover_raw_refund_budget(
    net_after_reserve_fee_e8s: u64,
    stable_paid_e6s: u64,
    rmr: Ratio,
) -> u64 {
    if rmr.0 <= Decimal::ZERO {
        return 0;
    }
    // Reserve payment is denominated in the already-RMR-scaled effective
    // amount. Convert it back with ceiling so a rounded raw refund cannot
    // overlap the raw icUSD budget attributable to the stable transfer.
    let stable_effective_e8s = Decimal::from(stable_paid_e6s) * Decimal::from(100u64);
    let stable_raw_e8s = (stable_effective_e8s / rmr.0)
        .ceil()
        .to_u64()
        .unwrap_or(net_after_reserve_fee_e8s);
    net_after_reserve_fee_e8s.saturating_sub(stable_raw_e8s)
}

fn redemption_tail_raw_refund(
    unconsumed_effective_e8s: Decimal,
    rmr: Ratio,
    raw_spillover_budget_e8s: u64,
) -> u64 {
    if unconsumed_effective_e8s <= Decimal::ZERO || rmr.0 <= Decimal::ZERO {
        return 0;
    }
    ((unconsumed_effective_e8s / rmr.0)
        .floor()
        .min(Decimal::from(raw_spillover_budget_e8s)))
    .to_u64()
    .unwrap_or(0)
}

fn redemption_raw_refund(
    raw_post_fee_budget_e8s: u64,
    consumed_effective_e8s: u64,
    rmr: Ratio,
    raw_refund_cap_e8s: u64,
) -> u64 {
    let unconsumed_effective = (Decimal::from(raw_post_fee_budget_e8s) * rmr.0
        - Decimal::from(consumed_effective_e8s))
    .max(Decimal::ZERO);
    redemption_tail_raw_refund(unconsumed_effective, rmr, raw_refund_cap_e8s)
}

fn reserve_spillover_snapshot_mismatch_refund(
    _spillover_e8s: u64,
    _rmr: Ratio,
    raw_spillover_budget_e8s: u64,
) -> u64 {
    // The stable leg has settled, but no native fee or debt consumption has
    // committed. Return the entire post-stable raw input budget.
    raw_spillover_budget_e8s
}

fn reserve_post_settlement_raw_refund(
    raw_post_reserve_fee_budget_e8s: u64,
    stable_paid_e6s: u64,
    committed_native_fee_e8s: u64,
    actual_native_consumed_e8s: u64,
    rmr: Ratio,
) -> u64 {
    if rmr.0 <= Decimal::ZERO {
        return 0;
    }

    // Convert all committed effective-value legs back to raw icUSD together.
    // A single ceiling avoids losing additional units to per-leg rounding.
    let effective_spent_e8s = Decimal::from(stable_paid_e6s) * Decimal::from(100u64)
        + Decimal::from(committed_native_fee_e8s)
        + Decimal::from(actual_native_consumed_e8s);
    let raw_spent = (effective_spent_e8s / rmr.0).ceil();
    let raw_spent = raw_spent.min(Decimal::from(raw_post_reserve_fee_budget_e8s));
    raw_post_reserve_fee_budget_e8s.saturating_sub(raw_spent.to_u64().unwrap_or(u64::MAX))
}

fn persist_rejected_redemption_refund(
    state: &mut crate::state::State,
    owner: Principal,
    burn_block_index: u64,
    amount_e8s: u64,
    op_nonce: u128,
) {
    state.pending_refunds.insert(
        burn_block_index,
        crate::state::PendingRefund {
            user: owner,
            amount_e8s,
            retry_count: 0,
            op_nonce,
        },
    );
}

pub(crate) fn redemption_ranking_is_fresh(
    state: &crate::state::State,
    runs: &[crate::state::RedemptionRun],
    now: u64,
) -> bool {
    let candidates = redemption_candidate_types(state);
    let ranked_types = runs
        .iter()
        .map(|run| run.collateral_type)
        .collect::<Vec<_>>();
    let all_prices_fresh = redemption_candidate_prices_are_fresh(state, &candidates, now);
    redemption_ranking_is_complete(
        &candidates,
        &ranked_types,
        all_prices_fresh,
        MAX_REDEMPTION_PRICE_CANDIDATES,
    )
}

fn redemption_ranking_has_complete_cached_prices(
    state: &crate::state::State,
    runs: &[crate::state::RedemptionRun],
    now: u64,
) -> bool {
    let candidates = redemption_candidate_types(state);
    let ranked_types = runs
        .iter()
        .map(|run| run.collateral_type)
        .collect::<Vec<_>>();
    let all_prices_valid = candidates.iter().all(|collateral_type| {
        redemption_candidate_price_is_valid(state, collateral_type, now, false)
    });
    redemption_ranking_is_complete(
        &candidates,
        &ranked_types,
        all_prices_valid,
        MAX_REDEMPTION_PRICE_CANDIDATES,
    )
}

fn redemption_ranking_is_complete(
    candidates: &[Principal],
    ranked_types: &[Principal],
    all_prices_fresh: bool,
    max_candidates: usize,
) -> bool {
    if candidates.is_empty() || candidates.len() > max_candidates || !all_prices_fresh {
        return false;
    }
    let candidate_set: std::collections::BTreeSet<_> = candidates.iter().copied().collect();
    let ranked_set: std::collections::BTreeSet<_> = ranked_types.iter().copied().collect();
    candidate_set == ranked_set
}

fn cached_redemption_offer_is_fresh(state: &crate::state::State, now: u64) -> bool {
    let runs = state.redemption_runs();
    redemption_ranking_is_fresh(state, &runs, now)
}

/// Refresh all assets that can influence the global sort, then verify the set
/// and timestamps after the XRC awaits. Two bounded passes handle candidate-set
/// changes during an await without creating an unbounded refresh loop.
async fn refresh_redemption_candidate_prices() -> Result<(), ProtocolError> {
    for _ in 0..2 {
        let candidates = read_state(redemption_candidate_types);
        if candidates.len() > MAX_REDEMPTION_PRICE_CANDIDATES {
            return Err(ProtocolError::TemporarilyUnavailable(format!(
                "Too many eligible collateral price sources to refresh safely ({} > {}).",
                candidates.len(),
                MAX_REDEMPTION_PRICE_CANDIDATES
            )));
        }
        for collateral_type in &candidates {
            crate::xrc::ensure_fresh_price_for(collateral_type).await?;
        }
        let now = ic_cdk::api::time();
        let current = read_state(redemption_candidate_types);
        if current == candidates
            && current.len() <= MAX_REDEMPTION_PRICE_CANDIDATES
            && read_state(|state| redemption_candidate_prices_are_fresh(state, &current, now))
        {
            return Ok(());
        }
    }
    Err(ProtocolError::TemporarilyUnavailable(
        "Eligible collateral prices changed or remained stale while refreshing redemption priority. Retry shortly.".to_string(),
    ))
}

/// Refresh only candidates whose cached price is missing, invalid, future-dated,
/// or older than the redemption's unchanged 600s bound. There are at most 64
/// candidates per pass and two passes. Each direct LstWrapped refresh can make
/// at most two rate-canister calls due to its existing single catch-up. Each
/// pass has at most one ICP candidate refresh, and that XRC path may couple at
/// most 64 LST refreshes (again at most two rate calls each). A second ICP
/// refresh/coupling wave is possible if time elapses across awaits, so the
/// deliberately loose ceiling is 518 external calls. LstWrapped refreshes use
/// cached ICP and do not recursively fetch ICP. Unlike
/// `refresh_redemption_candidate_prices`, this helper is only used by the
/// public no-funds offer endpoint.
async fn refresh_stale_redemption_candidates_for_offer() -> Result<(), RedemptionOfferRefreshError>
{
    refresh_stale_redemption_candidates_for_offer_with(
        || ic_cdk::api::time(),
        |now| read_state(|state| redemption_offer_price_refresh_snapshot(state, now)),
        |collateral_type| async move {
            crate::xrc::ensure_fresh_price_for(&collateral_type)
                .await
                .map_err(|error| RedemptionOfferRefreshError::RefreshUnavailable {
                    message: format!("Unable to refresh redemption collateral prices: {error:?}"),
                    retry_after_ns: redemption_offer_refresh_cooldown_remaining(
                        ic_cdk::api::time(),
                    ),
                })
        },
    )
    .await
}

async fn refresh_stale_redemption_candidates_for_offer_with<Now, Snapshot, Refresh, RefreshFuture>(
    mut now: Now,
    mut snapshot: Snapshot,
    mut refresh: Refresh,
) -> Result<(), RedemptionOfferRefreshError>
where
    Now: FnMut() -> u64,
    Snapshot: FnMut(u64) -> RedemptionOfferPriceRefreshSnapshot,
    Refresh: FnMut(Principal) -> RefreshFuture,
    RefreshFuture: std::future::Future<Output = Result<(), RedemptionOfferRefreshError>>,
{
    for _ in 0..MAX_REDEMPTION_OFFER_REFRESH_PASSES {
        let current = snapshot(now());
        if current.candidates.len() > MAX_REDEMPTION_PRICE_CANDIDATES {
            return Err(RedemptionOfferRefreshError::CandidateLimitExceeded {
                max_candidates: MAX_REDEMPTION_PRICE_CANDIDATES as u64,
            });
        }
        if current.refresh_targets.is_empty() {
            if current.ranking_fresh {
                return Ok(());
            }
            continue;
        }

        for collateral_type in current.refresh_targets {
            refresh(collateral_type).await?;
        }

        let verified = snapshot(now());
        if verified.candidates == current.candidates
            && verified.candidates.len() <= MAX_REDEMPTION_PRICE_CANDIDATES
            && verified.ranking_fresh
        {
            return Ok(());
        }
    }

    Err(RedemptionOfferRefreshError::RefreshUnavailable {
        message: "Eligible collateral prices or the candidate set changed during refresh; no live offer is available.".to_string(),
        retry_after_ns: redemption_offer_refresh_cooldown_remaining(ic_cdk::api::time()),
    })
}

fn build_redemption_queue_and_quote(
    state: &crate::state::State,
    now: u64,
    amount_e8s: u64,
    allow_stale_complete_ranking: bool,
) -> (RedemptionQueue, Result<RedemptionQuote, RedemptionError>) {
    let runs = state.redemption_runs();
    let ranking_fresh = redemption_ranking_is_fresh(state, &runs, now);
    let ranking_complete = ranking_fresh
        || (allow_stale_complete_ranking
            && redemption_ranking_has_complete_cached_prices(state, &runs, now));
    let mut fee_snapshots = std::collections::BTreeMap::new();
    for run in &runs {
        fee_snapshots
            .entry(run.collateral_type)
            .or_insert_with(|| redemption_fee_snapshot(state, &run.collateral_type, now));
    }

    let mut entries = Vec::with_capacity(runs.len());
    let mut quote_result = None;
    for (index, run) in runs.iter().enumerate() {
        let fee_snapshot = fee_snapshots
            .get(&run.collateral_type)
            .copied()
            .expect("fee snapshot prepared for every redemption run");
        let simulation = redemption_simulation_plan(state, run);
        let max_input = max_input_for_run_with(state, run, fee_snapshot, &simulation);
        let max_net =
            net_for_run_input(state, run, max_input, fee_snapshot, &simulation).unwrap_or(0);
        if index == 0 {
            quote_result = Some(if ranking_complete {
                quote_for_redemption_run(
                    state,
                    run,
                    &simulation,
                    fee_snapshot,
                    max_input,
                    amount_e8s,
                    now,
                    ranking_fresh,
                )
            } else {
                Err(RedemptionError::RedemptionQuoteUnavailable(
                    "Cached collateral prices are incomplete or invalid; no complete advisory estimate is available.".to_string(),
                ))
            });
        }
        entries.push(RedemptionQueueEntry {
            run_index: run.run_index,
            collateral_type: run.collateral_type,
            symbol: run.symbol.clone(),
            decimals: run.decimals,
            price_usd: run.price_usd,
            price_timestamp_ns: run.price_timestamp_ns,
            price_fresh: redemption_price_timestamp_is_fresh(run.price_timestamp_ns, now),
            min_cr: run.min_cr,
            liquidation_cr: run.liquidation_cr,
            weakest_vault_cr: run.weakest_vault_cr,
            health_headroom: run.health_headroom,
            vault_count: run.vault_ids.len() as u64,
            eligible_collateral_raw: run.eligible_collateral_raw,
            eligible_debt_e8s: run.eligible_debt_e8s,
            max_input_icusd_e8s: max_input,
            max_net_collateral_raw: max_net,
        });
    }

    let quote = quote_result.unwrap_or_else(|| {
        Err(RedemptionError::RedemptionQuoteUnavailable(
            "No eligible collateral vaults are available for redemption.".to_string(),
        ))
    });
    (
        RedemptionQueue {
            observed_at_ns: now,
            ranking_fresh,
            rmr: state.get_redemption_margin_ratio().to_f64(),
            price_freshness_window_ns: REDEMPTION_PRICE_MAX_AGE_NS,
            entries,
        },
        quote,
    )
}

fn quote_for_redemption_run(
    state: &crate::state::State,
    run: &crate::state::RedemptionRun,
    simulation: &RedemptionSimulationPlan,
    fee_snapshot: RedemptionFeeSnapshot,
    max_input: u64,
    amount_e8s: u64,
    now: u64,
    ranking_fresh: bool,
) -> Result<RedemptionQuote, RedemptionError> {
    if amount_e8s < state.min_icusd_amount.to_u64() {
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: state.min_icusd_amount.to_u64(),
        }
        .into());
    }
    if amount_e8s > max_input {
        return Err(RedemptionError::RedemptionCapacityExceeded {
            max_input_icusd_e8s: max_input,
        });
    }
    let amount = ICUSD::from(amount_e8s);
    let fee = amount * fee_snapshot.fee(amount);
    let rmr = state.get_redemption_margin_ratio();
    let effective = (amount - fee) * rmr;
    let simulated = simulation.try_simulate(effective).ok_or_else(|| {
        RedemptionError::RedemptionQuoteUnavailable(
            "A selected collateral share exceeds the supported raw-token range.".to_string(),
        )
    })?;
    let gross = simulated_collateral_total_raw(&simulated).ok_or_else(|| {
        RedemptionError::RedemptionQuoteUnavailable(
            "The selected collateral payout exceeds the supported raw-token range.".to_string(),
        )
    })?;
    let config = state
        .get_collateral_config(&run.collateral_type)
        .ok_or_else(|| {
            RedemptionError::RedemptionQuoteUnavailable(
                "Collateral configuration disappeared.".to_string(),
            )
        })?;
    let ledger_fee = config.ledger_fee;
    let net = gross.saturating_sub(ledger_fee);
    if net == 0 {
        return Err(RedemptionError::RedemptionQuoteUnavailable(
            "The selected collateral run cannot produce a positive net payout.".to_string(),
        ));
    }
    Ok(RedemptionQuote {
        quoted_at_ns: now,
        quote_validity_window_ns: 60 * 1_000_000_000,
        ranking_fresh,
        amount_e8s,
        run_index: run.run_index,
        collateral_type: run.collateral_type,
        symbol: run.symbol.clone(),
        decimals: run.decimals,
        price_usd: run.price_usd,
        price_timestamp_ns: run.price_timestamp_ns,
        price_fresh: redemption_price_timestamp_is_fresh(run.price_timestamp_ns, now),
        fee_e8s: fee.to_u64(),
        rmr: rmr.to_f64(),
        effective_icusd_e8s: effective.to_u64(),
        gross_collateral_raw: gross,
        ledger_fee_raw: ledger_fee,
        net_collateral_raw: net,
        max_input_icusd_e8s: max_input,
    })
}

fn prepared_offer_from_current_state(
    state: &crate::state::State,
    now: u64,
    amount_e8s: u64,
    retry_after_ns: u64,
) -> Result<PreparedRedemptionOffer, RedemptionOfferRefreshError> {
    let (queue, quote) = build_redemption_queue_and_quote(state, now, amount_e8s, false);
    if !queue.ranking_fresh {
        return Err(RedemptionOfferRefreshError::RefreshUnavailable {
            message: "The complete collateral ranking is no longer fresh; refresh the offer again."
                .to_string(),
            retry_after_ns,
        });
    }
    Ok(PreparedRedemptionOffer { queue, quote })
}

/// Return a snapshot of consecutive collateral runs in the global health order.
/// Each row's capacity is limited to that run's vault IDs; a later row requires
/// a separate redemption call and may move after every state change.
pub fn get_redemption_queue() -> RedemptionQueue {
    let now = ic_cdk::api::time();
    read_state(|s| {
        let runs = s.redemption_runs();
        let ranking_fresh = redemption_ranking_is_fresh(s, &runs, now);
        let mut fee_snapshots = std::collections::BTreeMap::new();
        for run in &runs {
            fee_snapshots
                .entry(run.collateral_type)
                .or_insert_with(|| redemption_fee_snapshot(s, &run.collateral_type, now));
        }
        let entries = runs
            .into_iter()
            .map(|run| {
                let fee_snapshot = fee_snapshots
                    .get(&run.collateral_type)
                    .copied()
                    .expect("fee snapshot prepared for every redemption run");
                let simulation = redemption_simulation_plan(s, &run);
                let max_input = max_input_for_run_with(s, &run, fee_snapshot, &simulation);
                let max_net =
                    net_for_run_input(s, &run, max_input, fee_snapshot, &simulation).unwrap_or(0);
                RedemptionQueueEntry {
                    run_index: run.run_index,
                    collateral_type: run.collateral_type,
                    symbol: run.symbol,
                    decimals: run.decimals,
                    price_usd: run.price_usd,
                    price_timestamp_ns: run.price_timestamp_ns,
                    price_fresh: redemption_price_timestamp_is_fresh(run.price_timestamp_ns, now),
                    min_cr: run.min_cr,
                    liquidation_cr: run.liquidation_cr,
                    weakest_vault_cr: run.weakest_vault_cr,
                    health_headroom: run.health_headroom,
                    vault_count: run.vault_ids.len() as u64,
                    eligible_collateral_raw: run.eligible_collateral_raw,
                    eligible_debt_e8s: run.eligible_debt_e8s,
                    max_input_icusd_e8s: max_input,
                    max_net_collateral_raw: max_net,
                }
            })
            .collect();
        RedemptionQueue {
            observed_at_ns: now,
            ranking_fresh,
            rmr: s.get_redemption_margin_ratio().to_f64(),
            price_freshness_window_ns: REDEMPTION_PRICE_MAX_AGE_NS,
            entries,
        }
    })
}

/// Build an authoritative snapshot quote for the first health-ranked run.
/// The quote reports cached price age; submit refreshes and revalidates it.
pub fn get_redemption_quote(amount_e8s: u64) -> Result<RedemptionQuote, RedemptionError> {
    let now = ic_cdk::api::time();
    read_state(|s| {
        let runs = s.redemption_runs();
        if !redemption_ranking_is_fresh(s, &runs, now) {
            return Err(RedemptionError::RedemptionQuoteUnavailable(
                "The global collateral ranking has a missing, invalid, or stale candidate price. Refresh and retry.".to_string(),
            ));
        }
        let run = runs.into_iter().next().ok_or_else(|| {
            RedemptionError::RedemptionQuoteUnavailable(
                "No eligible collateral vaults are available for redemption.".to_string(),
            )
        })?;
        if amount_e8s < s.min_icusd_amount.to_u64() {
            return Err(ProtocolError::AmountTooLow {
                minimum_amount: s.min_icusd_amount.to_u64(),
            }
            .into());
        }
        let fee_snapshot = redemption_fee_snapshot(s, &run.collateral_type, now);
        let simulation = redemption_simulation_plan(s, &run);
        let max_input = max_input_for_run_with(s, &run, fee_snapshot, &simulation);
        if amount_e8s > max_input {
            return Err(RedemptionError::RedemptionCapacityExceeded {
                max_input_icusd_e8s: max_input,
            });
        }
        let amount = ICUSD::from(amount_e8s);
        let fee_ratio = fee_snapshot.fee(amount);
        let fee = amount * fee_ratio;
        let rmr = s.get_redemption_margin_ratio();
        let effective = (amount - fee) * rmr;
        let simulated = simulation.try_simulate(effective).ok_or_else(|| {
            RedemptionError::RedemptionQuoteUnavailable(
                "A selected collateral share exceeds the supported raw-token range.".to_string(),
            )
        })?;
        let gross = simulated_collateral_total_raw(&simulated).ok_or_else(|| {
            RedemptionError::RedemptionQuoteUnavailable(
                "The selected collateral payout exceeds the supported raw-token range.".to_string(),
            )
        })?;
        let config = s
            .get_collateral_config(&run.collateral_type)
            .ok_or_else(|| {
                RedemptionError::RedemptionQuoteUnavailable(
                    "Collateral configuration disappeared.".to_string(),
                )
            })?;
        let ledger_fee = config.ledger_fee;
        let net = gross.saturating_sub(ledger_fee);
        if net == 0 {
            return Err(RedemptionError::RedemptionQuoteUnavailable(
                "The selected collateral run cannot produce a positive net payout.".to_string(),
            ));
        }
        Ok(RedemptionQuote {
            quoted_at_ns: now,
            quote_validity_window_ns: 60 * 1_000_000_000,
            ranking_fresh: true,
            amount_e8s,
            run_index: run.run_index,
            collateral_type: run.collateral_type,
            symbol: run.symbol,
            decimals: run.decimals,
            price_usd: run.price_usd,
            price_timestamp_ns: run.price_timestamp_ns,
            price_fresh: redemption_price_timestamp_is_fresh(run.price_timestamp_ns, now),
            fee_e8s: fee.to_u64(),
            rmr: rmr.to_f64(),
            effective_icusd_e8s: effective.to_u64(),
            gross_collateral_raw: gross,
            ledger_fee_raw: ledger_fee,
            net_collateral_raw: net,
            max_input_icusd_e8s: max_input,
        })
    })
}

/// Cached preview for display only. A stale but complete finite-positive price
/// snapshot can produce an estimate; callers must not use it as a submission
/// authorization. Missing, invalid, or future-dated candidate data remains an
/// explicit incomplete-ranking error.
pub fn get_redemption_preview(amount_e8s: u64) -> RedemptionPreview {
    let now = ic_cdk::api::time();
    read_state(|state| {
        let (queue, estimate) = build_redemption_queue_and_quote(state, now, amount_e8s, true);
        RedemptionPreview { queue, estimate }
    })
}

/// Prepare a fresh executable offer without approving or pulling icUSD and
/// without mutating vault, debt, redemption-event, or payout state. When cache
/// prices are already fresh this performs no oracle calls. Stale-price work is
/// guarded and globally rate-limited; submission still runs its independent
/// pre- and post-pull refresh/revalidation.
pub async fn prepare_redemption_offer(
    amount_e8s: u64,
) -> Result<PreparedRedemptionOffer, RedemptionOfferRefreshError> {
    let now = ic_cdk::api::time();
    let cached_fresh = read_state(|state| cached_redemption_offer_is_fresh(state, now));

    if !cached_fresh {
        let now = ic_cdk::api::time();
        let (candidate_count, has_stale_price) = read_state(|state| {
            let candidates = redemption_candidate_types(state);
            let stale = stale_redemption_candidate_types(state, &candidates, now);
            (candidates.len(), !stale.is_empty())
        });
        if candidate_count > MAX_REDEMPTION_PRICE_CANDIDATES {
            return Err(RedemptionOfferRefreshError::CandidateLimitExceeded {
                max_candidates: MAX_REDEMPTION_PRICE_CANDIDATES as u64,
            });
        }
        if !has_stale_price {
            return Err(RedemptionOfferRefreshError::RefreshUnavailable {
                message: "Cached candidate prices are fresh, but they do not form a complete eligible ranking.".to_string(),
                retry_after_ns: 0,
            });
        }
        let _refresh_guard = RedemptionOfferRefreshGuard::try_acquire(now)?;
        refresh_stale_redemption_candidates_for_offer().await?;
    }

    let snapshot_now = ic_cdk::api::time();
    let retry_after_ns = redemption_offer_refresh_cooldown_remaining(snapshot_now);
    read_state(|state| {
        prepared_offer_from_current_state(state, snapshot_now, amount_e8s, retry_after_ns)
    })
}

fn max_input_for_run(state: &crate::state::State, run: &crate::state::RedemptionRun) -> u64 {
    let now = ic_cdk::api::time();
    let fee_snapshot = redemption_fee_snapshot(state, &run.collateral_type, now);
    let simulation = redemption_simulation_plan(state, run);
    max_input_for_run_with(state, run, fee_snapshot, &simulation)
}

fn max_input_for_run_with(
    state: &crate::state::State,
    run: &crate::state::RedemptionRun,
    fee_snapshot: RedemptionFeeSnapshot,
    simulation: &RedemptionSimulationPlan,
) -> u64 {
    let debt = run.eligible_debt_e8s;
    let rmr = state.get_redemption_margin_ratio();
    if debt == 0 || rmr.0 <= Decimal::ZERO {
        return 0;
    }
    let Some(price) = Decimal::from_f64_retain(run.price_usd) else {
        return 0;
    };
    max_input_matching(u64::MAX, |raw| {
        let amount = ICUSD::from(raw);
        let fee = amount * fee_snapshot.fee(amount);
        let effective = (amount - fee) * rmr;
        if effective.to_u64() > debt {
            return false;
        }
        if !theoretical_collateral_target_fits_u64(effective, price, run.decimals) {
            return false;
        }
        let Some(simulated) = simulation.try_simulate(effective) else {
            return false;
        };
        simulated_collateral_total_raw(&simulated).is_some()
    })
}

fn theoretical_collateral_target_fits_u64(
    effective_icusd: ICUSD,
    price_usd: Decimal,
    decimals: u8,
) -> bool {
    if price_usd <= Decimal::ZERO {
        return false;
    }
    let Some(scale) = 10u64.checked_pow(decimals as u32) else {
        return false;
    };
    let target_raw = Decimal::from(effective_icusd.to_u64()) / dec!(100_000_000) / price_usd
        * Decimal::from(scale);
    target_raw.is_sign_positive() && target_raw <= Decimal::from(u64::MAX)
}

fn max_effective_icusd_for_u64_payout(price_usd: Decimal, decimals: u8) -> u64 {
    max_input_matching(u64::MAX, |effective_e8s| {
        theoretical_collateral_target_fits_u64(ICUSD::new(effective_e8s), price_usd, decimals)
    })
}

fn max_input_matching(upper_bound: u64, mut fits: impl FnMut(u64) -> bool) -> u64 {
    let mut low = 0u64;
    let mut high = upper_bound;
    while low < high {
        // Upper midpoint guarantees progress even when low == MAX - 1.
        let mid = low + (high - low) / 2 + 1;
        if fits(mid) {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    low
}

fn simulated_collateral_total_raw(redemptions: &[crate::event::VaultRedemption]) -> Option<u64> {
    let total = redemptions.iter().try_fold(0u128, |sum, redemption| {
        sum.checked_add(redemption.collateral_seized as u128)
    })?;
    u64::try_from(total).ok()
}

fn redemption_record_error_to_protocol(
    error: crate::event::RedemptionRecordError,
) -> RedemptionError {
    match error {
        crate::event::RedemptionRecordError::PayoutUnrepresentable => {
            RedemptionError::RedemptionQuoteUnavailable(
                "The selected collateral payout exceeds the supported raw-token range.".to_string(),
            )
        }
        crate::event::RedemptionRecordError::PayoutAlreadyPending => {
            RedemptionError::RedemptionQuoteUnavailable(
                "This icUSD burn already has a pending collateral payout.".to_string(),
            )
        }
        crate::event::RedemptionRecordError::VaultIngressPending => {
            RedemptionError::RedemptionQuoteUnavailable(
                "A selected vault has an unresolved caller-funded collateral transfer.".to_string(),
            )
        }
        crate::event::RedemptionRecordError::PayoutFeeConsumesGross => {
            RedemptionError::RedemptionQuoteUnavailable(
                "The selected collateral payout is no greater than its ledger fee.".to_string(),
            )
        }
        crate::event::RedemptionRecordError::PayoutConsumesDebtWithoutCollateral => {
            RedemptionError::RedemptionQuoteUnavailable(
                "The selected collateral run would consume debt without producing a transferable payout.".to_string(),
            )
        }
        crate::event::RedemptionRecordError::IdentityBackfillPending => {
            RedemptionError::RedemptionQuoteUnavailable(
                "Redemption is temporarily paused while historical burn identities are reconciled.".to_string(),
            )
        }
        crate::event::RedemptionRecordError::MinimumNotMet {
            minimum_net_raw,
            actual_net_raw,
        } => RedemptionError::RedemptionMinimumNotMet {
            minimum_net_raw,
            actual_net_raw,
        },
    }
}

/// Preserve the historical error ABI for legacy redemption entry points.
fn legacy_redemption_error(error: RedemptionError) -> ProtocolError {
    match error {
        RedemptionError::Protocol(error) => error,
        RedemptionError::RedemptionQuoteUnavailable(message) => {
            ProtocolError::TemporarilyUnavailable(message)
        }
        RedemptionError::RedemptionCapacityExceeded {
            max_input_icusd_e8s,
        } => ProtocolError::GenericError(format!(
            "The requested amount exceeds the current first collateral run capacity ({} icUSD e8s). Refresh and review the redemption order.",
            max_input_icusd_e8s
        )),
        RedemptionError::RedemptionPriorityChanged { expected, actual } => {
            ProtocolError::GenericError(format!(
                "The redemption priority changed from {} to {}. Refresh and review the redemption order.",
                expected, actual
            ))
        }
        RedemptionError::RedemptionMinimumNotMet {
            minimum_net_raw,
            actual_net_raw,
        } => ProtocolError::GenericError(format!(
            "The actual net payout ({actual_net_raw} raw units) is below the requested minimum ({minimum_net_raw} raw units). Refresh and review the redemption order."
        )),
    }
}

fn net_for_run_input(
    state: &crate::state::State,
    run: &crate::state::RedemptionRun,
    amount_e8s: u64,
    fee_snapshot: RedemptionFeeSnapshot,
    simulation: &RedemptionSimulationPlan,
) -> Option<u64> {
    if amount_e8s == 0 {
        return Some(0);
    }
    let amount = ICUSD::from(amount_e8s);
    let fee = amount * fee_snapshot.fee(amount);
    let effective = (amount - fee) * state.get_redemption_margin_ratio();
    let simulated = simulation.try_simulate(effective)?;
    let gross = simulated_collateral_total_raw(&simulated)?;
    let ledger_fee = state
        .get_collateral_config(&run.collateral_type)?
        .ledger_fee;
    Some(gross.saturating_sub(ledger_fee))
}

const THREE_USD_VIRTUAL_PRICE_SCALE: u128 = 1_000_000_000_000_000_000;

#[derive(CandidType, Deserialize)]
struct ThreePoolVirtualPriceView {
    virtual_price: candid::Nat,
}

async fn fetch_three_pool_virtual_price(pool: Principal) -> Result<u128, ProtocolError> {
    let (view,): (ThreePoolVirtualPriceView,) =
        ic_cdk::call(pool, "get_pool_state", ()).await.map_err(
            |(code, message): (ic_cdk::api::call::RejectionCode, String)| {
                ProtocolError::GenericError(format!(
                    "Unable to read configured 3pool virtual price ({code:?}): {message}"
                ))
            },
        )?;
    view.virtual_price.0.to_u128().ok_or_else(|| {
        ProtocolError::GenericError("3pool virtual price exceeds the supported range.".to_string())
    })
}

fn three_usd_covers_debt(three_usd_e8s: u64, debt_e8s: u64, virtual_price: u128) -> bool {
    (three_usd_e8s as u128)
        .checked_mul(virtual_price)
        .zip((debt_e8s as u128).checked_mul(THREE_USD_VIRTUAL_PRICE_SCALE))
        .is_some_and(|(provided, required)| provided >= required)
}

fn three_usd_live_value_matches_debt_quote(
    three_usd_e8s: u64,
    quoted_debt_e8s: u64,
    virtual_price: u128,
) -> bool {
    (three_usd_e8s as u128)
        .checked_mul(virtual_price)
        .and_then(|value| u64::try_from(value / THREE_USD_VIRTUAL_PRICE_SCALE).ok())
        == Some(quoted_debt_e8s)
}

fn realized_three_usd_for_applied_debt(
    gross_three_usd_e8s: u64,
    requested_debt_e8s: u64,
    applied_debt_e8s: u64,
) -> u64 {
    if requested_debt_e8s == 0 {
        return 0;
    }
    ((gross_three_usd_e8s as u128).saturating_mul(applied_debt_e8s as u128)
        / requested_debt_e8s as u128) as u64
}

fn preflight_three_usd_reserve_credit(
    existing_reserves_e8s: u64,
    gross_three_usd_e8s: Option<u64>,
    requested_debt_e8s: u64,
    applied_debt_e8s: u64,
) -> Result<Option<u64>, ProtocolError> {
    let credit = gross_three_usd_e8s.map(|gross| {
        realized_three_usd_for_applied_debt(gross, requested_debt_e8s, applied_debt_e8s)
    });
    if credit.is_some_and(|amount| existing_reserves_e8s.checked_add(amount).is_none()) {
        return Err(ProtocolError::GenericError(
            "3USD reserve accounting would overflow; ingress retained for exact refund".into(),
        ));
    }
    Ok(credit)
}

fn stored_three_usd_absorb_matches(
    stored: &crate::state::StoredThreeUsdReserveAbsorbResult,
    caller: Principal,
    vault_id: u64,
    debt_e8s: u64,
    three_usd_e8s: Option<u64>,
    ledger: Option<Principal>,
    proof: &crate::icrc3_proof::SpWritedownProof,
) -> bool {
    stored.caller == caller
        && stored.vault_id == vault_id
        && stored.icusd_debt_covered_e8s == debt_e8s
        && Some(stored.three_usd_amount_e8s) == three_usd_e8s
        && Some(stored.ledger) == ledger
        && &stored.proof == proof
}

fn replay_stored_three_usd_absorb_result(
    state: &crate::state::State,
    caller: Principal,
    vault_id: u64,
    debt_e8s: u64,
    three_usd_e8s: Option<u64>,
    proof: &crate::icrc3_proof::SpWritedownProof,
) -> Result<Option<StabilityPoolLiquidationResult>, ProtocolError> {
    if proof.ledger_kind != crate::icrc3_proof::SpProofLedger::ThreePoolTransferDefault {
        return Ok(None);
    }
    let proof_key = (proof.ledger_kind, proof.block_index);
    let Some(stored) = state
        .sp_three_usd_reserve_absorb_results_by_proof
        .get(&proof_key)
    else {
        if state.consumed_writedown_proofs.contains(&proof_key) {
            return Err(ProtocolError::GenericError(
                "V2 reserve proof was consumed without its replay result; hold for manual reconciliation".into(),
            ));
        }
        return Ok(None);
    };
    if stored_three_usd_absorb_matches(
        stored,
        caller,
        vault_id,
        debt_e8s,
        three_usd_e8s,
        state.three_pool_canister,
        proof,
    ) {
        return Ok(Some(stored.result.clone()));
    }
    Err(ProtocolError::GenericError(format!(
        "SP reserve proof replay rejected: ({:?}, block {}) is committed to a different request",
        proof.ledger_kind, proof.block_index
    )))
}

/// Read the exact committed V2 result before an ingress retry chooses any
/// refund or re-executes accounting. A consumed proof without its result is a
/// reconciliation hold, never evidence that a refund is safe.
pub fn replay_committed_three_usd_reserve_absorb_result(
    caller: Principal,
    vault_id: u64,
    debt_e8s: u64,
    three_usd_e8s: Option<u64>,
    proof: &crate::icrc3_proof::SpWritedownProof,
) -> Result<Option<StabilityPoolLiquidationResult>, ProtocolError> {
    read_state(|state| {
        replay_stored_three_usd_absorb_result(
            state,
            caller,
            vault_id,
            debt_e8s,
            three_usd_e8s,
            proof,
        )
    })
}

/// Verify and atomically retain the exact collateral payout receipt linked to
/// a committed 3USD V2 absorb. The ordinary margin-transfer path cannot clear
/// this row: it must first prove the pinned account, amount, fee, memo, time,
/// ledger adapter, parent identity, and queued obligation.
pub async fn verify_and_record_three_usd_reserve_payout(
    op_nonce: u128,
    block_index: u64,
) -> Result<Option<bool>, String> {
    use crate::state::{PayoutProofKind, ThreeUsdReserveIngressPayoutReceipt};
    use icrc_ledger_types::icrc1::account::Account;

    let snapshot = read_state(|state| -> Result<_, String> {
        let Some(key) = state
            .three_usd_reserve_payout_parents
            .get(&op_nonce)
            .cloned()
        else {
            return Ok(None);
        };
        let journal = state
            .three_usd_reserve_ingress_journals
            .get(&key)
            .ok_or_else(|| "linked V2 payout has no parent ingress journal".to_string())?;
        let payout = journal
            .payout
            .as_ref()
            .ok_or_else(|| "linked V2 payout parent has no exact pinned tuple".to_string())?;
        if payout.tuple.op_nonce != op_nonce {
            return Err("linked V2 payout nonce differs from its parent tuple".into());
        }
        if let Some(receipt) = payout.receipt.as_ref() {
            if receipt.block_index == block_index
                && receipt.tuple == payout.tuple
                && !state.pending_margin_transfers.contains_key(&op_nonce)
            {
                return Ok(Some((key, payout.tuple.clone(), None)));
            }
            return Err("linked V2 payout already has a different retained receipt".into());
        }
        let transfer = state
            .pending_margin_transfers
            .get(&op_nonce)
            .copied()
            .ok_or_else(|| {
                "linked V2 payout has neither a receipt nor a pending transfer".to_string()
            })?;
        if transfer.op_nonce != op_nonce
            || transfer.owner != key.stability_pool
            || transfer.collateral_type != payout.tuple.collateral_type
            || transfer.margin.to_u64() != payout.tuple.gross_amount_e8s
        {
            return Err("linked V2 payout parent no longer matches its exact queue row".into());
        }
        Ok(Some((key, payout.tuple.clone(), Some(transfer))))
    })?;
    let Some((key, tuple, transfer)) = snapshot else {
        return Ok(None);
    };
    let Some(transfer) = transfer else {
        return Ok(Some(false));
    };

    let canonical_icp =
        Principal::from_text("ryjl3-tyaaa-aaaaa-aaaba-cai").expect("canonical ICP principal");
    let source = Account {
        owner: ic_cdk::id(),
        subaccount: None,
    };
    let destination = Account {
        owner: key.stability_pool,
        subaccount: None,
    };
    let memo = management::nonce_to_memo(op_nonce);
    let created_at_time = management::nonce_to_created_at_time(op_nonce);
    if tuple.ledger == Principal::anonymous()
        || tuple.source != source
        || tuple.destination != destination
        || tuple.gross_amount_e8s == 0
        || tuple.net_amount_e8s == 0
        || tuple.net_amount_e8s.checked_add(tuple.fee_e8s) != Some(tuple.gross_amount_e8s)
        || tuple.memo.as_slice() != memo.0.as_slice()
        || tuple.created_at_time_ns != created_at_time
    {
        return Err("linked V2 payout contains an invalid pinned account or transfer tuple".into());
    }
    match (tuple.proof_kind, tuple.ledger == canonical_icp) {
        (PayoutProofKind::NativeIcp, true) => {
            crate::treasury::verify_native_icp_transfer_receipt(
                tuple.ledger,
                tuple.source.owner,
                tuple.destination.owner,
                tuple.net_amount_e8s,
                tuple.fee_e8s,
                tuple.memo.as_slice(),
                tuple.created_at_time_ns,
                block_index,
            )
            .await?;
        }
        (PayoutProofKind::Icrc3, false) => {
            let block = crate::icrc3_proof::fetch_icrc3_block(tuple.ledger, block_index).await?;
            crate::icrc3_proof::validate_icrc3_transfer_block_with_fee(
                &block,
                tuple.source,
                tuple.destination,
                tuple.net_amount_e8s,
                tuple.fee_e8s,
                tuple.memo.as_slice(),
                tuple.created_at_time_ns,
            )?;
        }
        _ => {
            return Err("linked V2 payout ledger does not match its pinned receipt adapter".into())
        }
    }

    let receipt = ThreeUsdReserveIngressPayoutReceipt { block_index, tuple };
    Ok(Some(mutate_state(|state| {
        let exact = state.pending_margin_transfers.get(&op_nonce) == Some(&transfer)
            && state.three_usd_reserve_payout_parents.get(&op_nonce) == Some(&key)
            && state
                .three_usd_reserve_ingress_journals
                .get(&key)
                .and_then(|journal| journal.payout.as_ref())
                .is_some_and(|payout| payout.tuple == receipt.tuple && payout.receipt.is_none());
        if !exact {
            return false;
        }
        if let Some(payout) = state
            .three_usd_reserve_ingress_journals
            .get_mut(&key)
            .and_then(|journal| journal.payout.as_mut())
        {
            payout.receipt = Some(receipt);
        } else {
            return false;
        }
        crate::event::record_pending_payout_settled(
            state,
            op_nonce,
            crate::event::PendingPayoutKind::Margin,
            key.vault_id,
            block_index,
        );
        true
    })))
}

/// Retry receipt-backed V2 collateral payouts before the legacy margin queue
/// runs. Every retry uses the tuple persisted with the debt commit.
fn linked_three_usd_payout_is_retryable(
    operation_id: u128,
    payout_op_nonce: u128,
    transfer: &PendingMarginTransfer,
) -> bool {
    operation_id != 0
        && payout_op_nonce == operation_id
        && transfer.op_nonce == operation_id
        && !transfer.held_for_manual_retry
        && !transfer.reconciliation_required
}

pub async fn process_pending_three_usd_reserve_payouts() {
    let payouts = read_state(|state| {
        state
            .three_usd_reserve_payout_parents
            .iter()
            .filter_map(|(nonce, key)| {
                let tuple = state
                    .three_usd_reserve_ingress_journals
                    .get(key)?
                    .payout
                    .as_ref()?
                    .tuple
                    .clone();
                let transfer = state.pending_margin_transfers.get(nonce)?;
                linked_three_usd_payout_is_retryable(*nonce, tuple.op_nonce, transfer)
                    .then_some((*nonce, tuple))
            })
            .collect::<Vec<_>>()
    });
    for (nonce, tuple) in payouts {
        match management::transfer_idempotent_exact(
            tuple.ledger,
            None,
            tuple.destination,
            u128::from(tuple.net_amount_e8s),
            tuple.fee_e8s,
            icrc_ledger_types::icrc1::transfer::Memo::from(tuple.memo.to_vec()),
            tuple.created_at_time_ns,
        ).await {
            Ok(block_index) => match verify_and_record_three_usd_reserve_payout(nonce, block_index).await {
                Ok(Some(true)) | Ok(Some(false)) | Ok(None) => {}
                Err(error) => log!(INFO,
                    "[three_usd_reserve_payout] nonce {} retained pending after receipt verification failure: {}", nonce, error),
            },
            Err(error) => log!(INFO,
                "[three_usd_reserve_payout] nonce {} retained pending after exact transfer attempt: {}", nonce, error),
        }
    }
}

fn v2_reserve_commit_is_paused(state: &crate::state::State) -> bool {
    state.frozen || state.liquidation_frozen || state.sp_writedown_disabled
}

#[cfg(test)]
mod three_usd_reserve_value_tests {
    use super::*;

    #[test]
    fn reserve_value_must_cover_retired_debt_with_checked_units() {
        let one = THREE_USD_VIRTUAL_PRICE_SCALE;
        assert!(three_usd_covers_debt(10_000_000_000, 10_000_000_000, one));
        assert!(!three_usd_covers_debt(9_999_999_999, 10_000_000_000, one));
        assert!(three_usd_covers_debt(
            10_000_000_000,
            10_000_000_000,
            one + 1
        ));
        assert!(!three_usd_covers_debt(u64::MAX, u64::MAX, u128::MAX));
    }

    #[test]
    fn live_value_must_match_the_exact_v2_debt_quote_after_pull() {
        let one = THREE_USD_VIRTUAL_PRICE_SCALE;
        assert!(three_usd_live_value_matches_debt_quote(
            12_500_000_000,
            12_500_000_000,
            one
        ));
        assert!(!three_usd_live_value_matches_debt_quote(
            12_500_000_000,
            12_499_999_999,
            one
        ));
        assert!(!three_usd_live_value_matches_debt_quote(
            12_500_000_001,
            12_500_000_000,
            one
        ));
        assert!(!three_usd_live_value_matches_debt_quote(
            u64::MAX,
            1,
            u128::MAX
        ));
    }

    #[test]
    fn applied_debt_records_only_its_proportional_reserve_value() {
        assert_eq!(realized_three_usd_for_applied_debt(100, 100, 50), 50);
        assert_eq!(realized_three_usd_for_applied_debt(100, 0, 50), 0);
    }

    #[test]
    fn reserve_overflow_is_rejected_before_proof_or_debt_commit() {
        assert!(preflight_three_usd_reserve_credit(u64::MAX - 4, Some(10), 10, 5).is_err());
        assert_eq!(
            preflight_three_usd_reserve_credit(u64::MAX - 5, Some(10), 10, 5).unwrap(),
            Some(5),
        );
        assert_eq!(
            preflight_three_usd_reserve_credit(u64::MAX, None, 10, 5).unwrap(),
            None,
        );
    }

    #[test]
    fn checked_liquidation_conversion_rejects_unrepresentable_collateral() {
        assert_eq!(
            crate::numeric::try_icusd_to_collateral_amount(ICUSD::new(100_000_000), dec!(1), 8,),
            Some(100_000_000),
        );
        assert_eq!(
            crate::numeric::try_icusd_to_collateral_amount(ICUSD::new(1), dec!(0), 8,),
            None,
        );
        assert_eq!(
            crate::numeric::try_icusd_to_collateral_amount(
                ICUSD::new(u64::MAX),
                Decimal::new(1, 28),
                u8::MAX,
            ),
            None,
        );
    }
}

use crate::compute_collateral_ratio;

/// INT-003 defense in depth: clamp a raw borrow fee so `amount - fee >= 1 e8s`.
/// The validation cap on borrowing-fee curve multipliers (see
/// `state::MAX_BORROWING_FEE_MULTIPLIER`) is the primary fence; this clamp
/// protects against any path that bypasses validation (legacy state, future
/// drift) by ensuring `borrow_from_vault_internal::mint_icusd(amount - fee)`
/// never underflows. `min_icusd_amount` (the protocol's borrow minimum) is
/// orders of magnitude above 1 e8s, so the clamp never reduces a legitimate
/// fee.
pub fn clamp_borrow_fee(amount: ICUSD, raw_fee: ICUSD) -> ICUSD {
    raw_fee.min(amount.saturating_sub(ICUSD::new(1)))
}

/// Checks that a partial repayment won't leave the vault with dust debt below `min_vault_debt`.
/// Returns Ok(()) if remaining debt is zero or >= min_vault_debt, Err otherwise.
fn check_min_vault_debt_after_repay(
    vault: &Vault,
    repay_amount: ICUSD,
) -> Result<(), ProtocolError> {
    let remaining_debt = vault.borrowed_icusd_amount - repay_amount;
    if remaining_debt > ICUSD::new(0) {
        let min_vault_debt = read_state(|s| {
            s.get_collateral_config(&vault.collateral_type)
                .map(|c| c.min_vault_debt)
                .unwrap_or(ICUSD::new(0))
        });
        if remaining_debt < min_vault_debt {
            return Err(ProtocolError::GenericError(format!(
                "Partial repayment would leave {} icUSD debt, below the minimum of {}. \
                 Repay the full amount or leave at least {} icUSD.",
                remaining_debt, min_vault_debt, min_vault_debt
            )));
        }
    }
    Ok(())
}

/// Compute the ckStable pull for a finalized icUSD debt reduction. Since
/// icUSD uses 8 decimals and ckStable uses 6, round the base pull upward so
/// the transferred principal always covers the debt that will be retired.
/// The configurable surcharge is then charged on that rounded base amount.
fn stable_repay_pull_e6s(amount: ICUSD, fee_rate: Ratio) -> Result<(u64, u64, u64), ProtocolError> {
    let amount_e8s = amount.0;
    let base_e6s = (amount_e8s / 100)
        .checked_add(u64::from(amount_e8s % 100 != 0))
        .ok_or_else(|| ProtocolError::GenericError("Stable repayment amount overflow.".into()))?;
    let fee_e6s = Decimal::from(base_e6s)
        .checked_mul(fee_rate.0)
        .and_then(|fee| fee.to_u64())
        .ok_or_else(|| ProtocolError::GenericError("Stable repayment fee overflow.".into()))?;
    let total_e6s = base_e6s
        .checked_add(fee_e6s)
        .ok_or_else(|| ProtocolError::GenericError("Stable repayment pull overflow.".into()))?;
    Ok((base_e6s, fee_e6s, total_e6s))
}

#[cfg(test)]
mod stable_repay_rounding_tests {
    use super::stable_repay_pull_e6s;
    use crate::numeric::{Ratio, ICUSD};
    use crate::ProtocolError;
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;

    #[test]
    fn rounded_base_covers_final_debt_after_near_full_repay_snap() {
        let requested_e8s = 10_000_000_000u64;
        let debt_e8s = 10_050_000_000u64;
        let snap_threshold = std::cmp::max(debt_e8s / 100, 1_000_000);
        assert!(debt_e8s - requested_e8s <= snap_threshold);

        // Current full-repay policy retires all of debt when the remainder is
        // inside the dust threshold. A base computed from the request alone
        // would cover only 10,000,000,000 e8s and underfund that retirement.
        assert!(u128::from(requested_e8s / 100) * 100 < u128::from(debt_e8s));
        let (base_e6s, fee_e6s, total_e6s) =
            stable_repay_pull_e6s(ICUSD::new(debt_e8s), Ratio::new(dec!(0.01))).unwrap();
        assert_eq!(base_e6s, 100_500_000);
        assert_eq!(fee_e6s, 1_005_000);
        assert_eq!(total_e6s, 101_505_000);
        assert!(u128::from(base_e6s) * 100 >= u128::from(debt_e8s));
        assert!(u128::from(base_e6s - 1) * 100 < u128::from(debt_e8s));
        assert_eq!(base_e6s.checked_add(fee_e6s), Some(total_e6s));
    }

    #[test]
    fn fractional_e8_debt_rounds_up_and_fee_is_not_counted_as_principal() {
        let finalized_debt_e8s = 10_000_000_001u64;
        let (base_e6s, fee_e6s, total_e6s) =
            stable_repay_pull_e6s(ICUSD::new(finalized_debt_e8s), Ratio::new(dec!(0.0005)))
                .unwrap();
        assert_eq!(base_e6s, 100_000_001);
        assert_eq!(fee_e6s, 50_000);
        assert_eq!(total_e6s, 100_050_001);
        assert!(u128::from(base_e6s) * 100 >= u128::from(finalized_debt_e8s));
        assert_eq!(
            u128::from(base_e6s) * 100 - u128::from(finalized_debt_e8s),
            99,
            "only decimal conversion rounding may exceed the retired debt",
        );
    }

    #[test]
    fn stable_repay_pull_rejects_fee_overflow() {
        assert!(matches!(
            stable_repay_pull_e6s(ICUSD::new(u64::MAX), Ratio::new(Decimal::MAX)),
            Err(ProtocolError::GenericError(_)),
        ));
    }

    #[test]
    fn stable_liquidation_dust_round_up_pulls_enough_for_full_nonmultiple_debt() {
        let debt_e8s = 10_000_000_001u64;
        let vault = super::Vault {
            owner: candid::Principal::anonymous(),
            borrowed_icusd_amount: ICUSD::new(debt_e8s),
            collateral_amount: 1,
            vault_id: 1,
            collateral_type: candid::Principal::anonymous(),
            last_accrual_time: 0,
            accrued_interest: ICUSD::new(0),
            bot_processing: false,
        };
        let retired =
            super::round_up_partial_liq_dust(&vault, ICUSD::new(debt_e8s - 1), ICUSD::new(100));
        assert_eq!(retired, ICUSD::new(debt_e8s));

        let (base_e6s, fee_e6s, total_e6s) =
            stable_repay_pull_e6s(retired, Ratio::new(dec!(0.0005))).unwrap();
        assert_eq!(base_e6s, 100_000_001);
        assert_eq!(fee_e6s, 50_000);
        assert_eq!(total_e6s, 100_050_001);
        assert!(u128::from(base_e6s) * 100 >= u128::from(retired.0));
        assert!(u128::from(retired.0 / 100) * 100 < u128::from(retired.0));
    }
}

/// LIQ-003: round a partial-liquidation amount up to the vault's full debt if
/// the residual would land in the open interval `(0, min_vault_debt)`. Mirrors
/// the dust-forgiveness pattern in `repay_to_vault`. The repay path enforces
/// `residual == 0 || residual >= min_vault_debt` via
/// `check_min_vault_debt_after_repay`; partial-liquidation endpoints must
/// enforce the same invariant on the residual after their cap math, otherwise
/// a liquidator could leave a vault with debt below `min_vault_debt` and bypass
/// the repay-side guarantee.
///
/// Returns the (possibly-rounded-up) amount the liquidator will actually
/// consume. The caller is responsible for pulling the corresponding icUSD
/// (or stable) from the liquidator and reducing vault debt by the returned
/// amount.
pub fn round_up_partial_liq_dust(
    vault: &Vault,
    proposed_amount: ICUSD,
    min_vault_debt: ICUSD,
) -> ICUSD {
    let residual = vault.borrowed_icusd_amount.saturating_sub(proposed_amount);
    if residual > ICUSD::new(0) && residual < min_vault_debt {
        vault.borrowed_icusd_amount
    } else {
        proposed_amount
    }
}

#[derive(CandidType, Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct OpenVaultSuccess {
    pub vault_id: u64,
    pub block_index: u64,
}

#[derive(CandidType, Deserialize)]
pub struct VaultArg {
    pub vault_id: u64,
    pub amount: u64,
}

/// Returns `Principal::anonymous()` as sentinel for old events missing `collateral_type`.
/// The replay handler replaces this with the actual ICP ledger principal.
pub(crate) fn default_collateral_type() -> Principal {
    Principal::anonymous()
}

/// Returns zero ICUSD for serde default of `accrued_interest` field.
fn default_zero_icusd() -> ICUSD {
    ICUSD::new(0)
}

#[derive(CandidType, Clone, Debug, PartialEq, Eq, Deserialize, Serialize, PartialOrd, Ord)]
pub struct Vault {
    pub owner: Principal,
    pub borrowed_icusd_amount: ICUSD,
    /// Raw collateral amount in token's native precision (e.g., e8s for ICP).
    /// Renamed from `icp_margin_amount`; serde alias handles old events.
    #[serde(alias = "icp_margin_amount")]
    pub collateral_amount: u64,
    pub vault_id: u64,
    /// Ledger canister ID identifying the collateral token.
    /// Old events lack this field; serde default → Principal::anonymous(),
    /// fixed up to ICP ledger principal during event replay.
    #[serde(default = "default_collateral_type")]
    pub collateral_type: Principal,
    /// Nanosecond timestamp of last interest accrual for this vault.
    /// Defaults to 0 for existing vaults (migration sets it in post_upgrade).
    #[serde(default)]
    pub last_accrual_time: u64,
    /// Accumulated interest on this vault's debt.
    /// Sub-component of `borrowed_icusd_amount` — tracks how much is interest vs principal.
    /// Defaults to 0 for existing vaults (backward compat).
    #[serde(default = "default_zero_icusd")]
    pub accrued_interest: ICUSD,
    /// True while the bot has claimed this vault for liquidation but hasn't
    /// confirmed or cancelled yet. Blocks ALL user operations on the vault.
    #[serde(default)]
    pub bot_processing: bool,
}

/// Current backend state stores collateral only in the canister's default
/// ledger account. Bot claim and cancellation paths pin that same source in
/// their exact transfer tuple; there is no per-vault ICRC source variant to
/// fall back from in this schema.
pub fn require_supported_icrc_collateral_source(
    _vault: &Vault,
) -> Result<(), crate::ProtocolError> {
    Ok(())
}

impl Vault {
    /// Compute the vault's health score: CR / liquidation_ratio.
    /// A score of 1.0 means the vault is at its liquidation threshold.
    /// Higher is healthier. Normalizes across collateral types so that
    /// vaults with different liquidation thresholds can be compared.
    ///
    /// `cr` — the vault's current collateral ratio (from compute_collateral_ratio)
    /// `liquidation_ratio` — the collateral type's liquidation threshold (e.g. 1.33)
    pub fn health_score(&self, cr: f64, liquidation_ratio: f64) -> f64 {
        if self.borrowed_icusd_amount == 0 {
            return f64::MAX;
        }
        if liquidation_ratio <= 0.0 {
            return f64::MAX; // defensive: avoid division by zero
        }
        cr / liquidation_ratio
    }
}

/// Returns an error if the vault is locked for bot processing.
pub fn require_vault_not_processing(vault: &Vault) -> Result<(), ProtocolError> {
    require_vault_not_processing_except(vault, None, None)
}

fn require_vault_not_processing_except(
    vault: &Vault,
    repayment_request: Option<(Principal, u128)>,
    push_sweep_request: Option<(Principal, Principal, u128)>,
) -> Result<(), ProtocolError> {
    if vault.bot_processing {
        Err(ProtocolError::GenericError(format!(
            "Vault #{} is locked — bot liquidation in progress",
            vault.vault_id
        )))
    } else if read_state(|s| {
        s.pending_collateral_withdrawals
            .contains_key(&vault.vault_id)
    }) {
        Err(ProtocolError::GenericError(format!(
            "Vault #{} is locked — collateral withdrawal is awaiting exact receipt reconciliation",
            vault.vault_id
        )))
    } else if read_state(|s| {
        s.pending_inbound_collateral.values().any(|row| {
            matches!(&row.operation, crate::state::InboundCollateralOperation::AddMargin { vault_id, .. }
                if *vault_id == vault.vault_id)
        }) || s.pending_push_deposit_sweeps.iter().any(|((owner, ledger), row)| {
            matches!(&row.operation, crate::state::PushDepositSweepOperation::AddMargin { vault_id, .. }
                if *vault_id == vault.vault_id)
                && !push_sweep_request.is_some_and(|(expected_owner, expected_ledger, expected_id)| {
                    *owner == expected_owner
                        && *ledger == expected_ledger
                        && row.request_id == expected_id
                        && row.owner == expected_owner
                })
        })
    }) {
        Err(ProtocolError::TemporarilyUnavailable(format!(
            "Vault #{} is locked — collateral ingress awaits exact receipt reconciliation",
            vault.vault_id
        )))
    } else if read_state(|s| {
        s.pending_borrow_mints
            .values()
            .any(|row| row.vault_id == vault.vault_id)
    }) {
        Err(ProtocolError::GenericError(format!(
            "Vault #{} is locked — borrow mint is awaiting exact receipt reconciliation",
            vault.vault_id
        )))
    } else if read_state(|state| {
        state
            .sp_liquidation_v2_journals
            .values()
            .any(|journal| journal.request.vault_id == vault.vault_id)
    }) {
        Err(ProtocolError::GenericError(format!(
            "Vault #{} is locked — SP liquidation V2 awaits exact settlement",
            vault.vault_id
        )))
    } else if read_state(|state| {
        state.repayment_v2_active.iter().any(|(owner, journal)| {
            journal.vault_id == vault.vault_id
                && Some((*owner, journal.request_id)) != repayment_request
        })
    }) {
        Err(ProtocolError::GenericError(format!(
            "Vault #{} is locked — repayment V2 awaits exact receipt or close recovery",
            vault.vault_id
        )))
    } else if read_state(|state| {
        state
            .stable_repayment_v2_active
            .values()
            .any(|journal| journal.vault_id == vault.vault_id)
    }) {
        Err(ProtocolError::TemporarilyUnavailable(format!(
            "Vault #{} is locked — stable repayment V2 awaits exact receipt recovery",
            vault.vault_id
        )))
    } else {
        Ok(())
    }
}

/// LIQ-101: vault-id wrapper around `require_vault_not_processing` for the
/// liquidation entry points (manual + SP), which only fetch the vault later in
/// their amount-computing read_state. If the liquidation bot has already
/// claimed this vault (`bot_processing` set, with the write-down deferred until
/// the bot's swap settles), a manual / stability-pool liquidation here would
/// seize the same collateral a second time. Mirrors the lock every user op
/// already honors. Absent vault => Ok (a later check surfaces "not found").
pub fn reject_if_bot_processing(vault_id: u64) -> Result<(), ProtocolError> {
    match read_state(|s| s.vault_id_to_vaults.get(&vault_id).cloned()) {
        Some(vault) => require_vault_not_processing(&vault),
        None => Ok(()),
    }
}

/// Final synchronous check immediately before a liquidation pulls stable
/// assets. A pending collateral transfer is a durable mutation fence.
fn reject_pending_collateral_withdrawal(vault_id: u64) -> Result<(), ProtocolError> {
    if read_state(|state| state.pending_collateral_withdrawals.contains_key(&vault_id)) {
        return Err(ProtocolError::GenericError(format!(
            "Vault #{vault_id} has an unresolved collateral withdrawal"
        )));
    }
    Ok(())
}

#[derive(CandidType, Serialize, Deserialize, Debug)]
pub struct CandidVault {
    pub owner: Principal,
    pub borrowed_icusd_amount: u64,
    /// Kept for frontend backward compatibility
    pub icp_margin_amount: u64,
    pub vault_id: u64,
    /// Raw collateral amount (same value as icp_margin_amount for ICP vaults)
    pub collateral_amount: u64,
    /// Ledger canister ID of the collateral token
    pub collateral_type: Principal,
    /// Accumulated interest portion of the vault's debt (in e8s)
    pub accrued_interest: u64,
}

impl From<Vault> for CandidVault {
    fn from(vault: Vault) -> Self {
        Self {
            owner: vault.owner,
            borrowed_icusd_amount: vault.borrowed_icusd_amount.to_u64(),
            icp_margin_amount: vault.collateral_amount,
            vault_id: vault.vault_id,
            collateral_amount: vault.collateral_amount,
            collateral_type: vault.collateral_type,
            accrued_interest: vault.accrued_interest.to_u64(),
        }
    }
}

/// Redeem icUSD for ckStable tokens from the protocol's reserves.
/// Two-tier system: reserves first (flat fee), then vault spillover (dynamic fee).
pub async fn redeem_reserves(
    icusd_amount_raw: u64,
    preferred_token: Option<Principal>,
) -> Result<crate::ReserveRedemptionResult, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let _guard_principal = GuardPrincipal::new(caller, "redeem_reserves")?;

    // RED-101 / RED-003: gate on protocol mode here too. The endpoint keeps its
    // own validate_mode() (defense in depth), but the spillover branch below
    // calls record_redemption_on_vault_run DIRECTLY (it does not route through
    // redeem_collateral), so the redeem_collateral gate does not cover it. Gate
    // at this entry point so the reserve path is covered by construction as well.
    if read_state(|s| s.mode) == Mode::ReadOnly {
        return Err(ProtocolError::read_only_mode());
    }

    let icusd_amount: ICUSD = icusd_amount_raw.into();

    if icusd_amount < read_state(|s| s.min_icusd_amount) {
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: read_state(|s| s.min_icusd_amount).to_u64(),
        });
    }

    // Check reserve redemptions are enabled
    let (enabled, reserve_fee_ratio, ckusdt_ledger, ckusdc_ledger, treasury) = read_state(|s| {
        (
            s.reserve_redemptions_enabled,
            s.reserve_redemption_fee,
            s.ckusdt_ledger_principal,
            s.ckusdc_ledger_principal,
            s.treasury_principal,
        )
    });

    if !enabled {
        return Err(ProtocolError::GenericError(
            "Reserve redemptions are currently disabled.".to_string(),
        ));
    }

    // Determine which ledger to use
    let stable_ledger = if let Some(pref) = preferred_token {
        // Validate it's one of our known stable ledgers
        if Some(pref) == ckusdt_ledger || Some(pref) == ckusdc_ledger {
            pref
        } else {
            return Err(ProtocolError::GenericError(
                "Preferred token is not a supported reserve token.".to_string(),
            ));
        }
    } else {
        // Default: try ckUSDT first, then ckUSDC
        ckusdt_ledger.or(ckusdc_ledger).ok_or_else(|| {
            ProtocolError::GenericError("No reserve token ledgers configured.".to_string())
        })?
    };

    // Calculate fee (flat rate)
    let fee_icusd = icusd_amount * reserve_fee_ratio;
    let net_icusd = icusd_amount - fee_icusd;

    // Apply dynamic Redemption Margin Ratio: redeemers get RMR × face value
    let rmr = read_state(|s| s.get_redemption_margin_ratio());
    let effective_icusd = net_icusd * rmr;

    // Convert e8s (icUSD) to e6s (ckStable): divide by 100
    let net_e6s = effective_icusd.to_u64() / 100;
    let fee_e6s = fee_icusd.to_u64() / 100;

    if net_e6s == 0 {
        return Err(ProtocolError::GenericError(
            "Redemption amount too small after fee.".to_string(),
        ));
    }

    // Check reserve balance before pulling icUSD
    let reserve_balance = management::get_token_balance(stable_ledger)
        .await
        .map_err(|e| {
            ProtocolError::TemporarilyUnavailable(format!("Cannot query reserve balance: {}", e))
        })?;

    // Determine how much can come from reserves vs vault spillover.
    // Each ICRC-1 transfer also costs a ledger fee (deducted from sender balance).
    // Query the actual fee from the ledger rather than hardcoding.
    let ledger_fee = management::get_ledger_fee(stable_ledger)
        .await
        .unwrap_or(10_000); // fallback to 10_000 e6s (0.01 USD) if query fails
    let fee_budget = if fee_e6s > 0 {
        ledger_fee * 2
    } else {
        ledger_fee
    };
    let total_needed_e6s = net_e6s + fee_e6s + fee_budget;
    let available_for_user = if reserve_balance >= total_needed_e6s {
        net_e6s
    } else if reserve_balance > fee_e6s + fee_budget {
        // Partial: reserve can cover some but not all
        reserve_balance - fee_e6s - fee_budget
    } else {
        0
    };

    let spillover_e6s = net_e6s - available_for_user;
    let spillover_e8s = spillover_e6s * 100; // convert back to icUSD e8s
    let raw_spillover_refund_budget_e8s =
        reserve_spillover_raw_refund_budget(net_icusd.to_u64(), available_for_user, rmr);

    // Bound a native-collateral spillover before the icUSD pull. The reserve
    // portion is settled separately, so only the planned remainder is checked.
    // The nominal target is conservative across any within-run allocation and
    // protects the u64 pending-payout/event representation.
    let spillover_plan = if spillover_e8s > 0 {
        refresh_redemption_candidate_prices().await?;
        let plan = read_state(|state| {
            let run = legacy_reserve_spillover_run(state, spillover_e8s)?
                .expect("positive spillover must have a selected run");
            let amount = ICUSD::from(spillover_e8s);
            let fee = amount * state.get_redemption_fee_for(&run.collateral_type, amount);
            let effective = amount - fee; // RMR was applied above.
            let price = Decimal::from_f64_retain(run.price_usd).ok_or_else(|| {
                ProtocolError::TemporarilyUnavailable(
                    "The reserve spillover collateral price is unavailable.".to_string(),
                )
            })?;
            if !theoretical_collateral_target_fits_u64(effective, price, run.decimals) {
                return Err(ProtocolError::TemporarilyUnavailable(
                    "The reserve spillover exceeds the supported native payout range; try a smaller amount.".to_string(),
                ));
            }
            Ok(RedemptionRunSnapshot::capture(&run))
        })?;
        Some(plan)
    } else {
        None
    };

    // Pull icUSD from caller (effectively burns it)
    let icusd_block_index = transfer_icusd_from(icusd_amount, caller)
        .await
        .map_err(|e| ProtocolError::TransferFromError(e, icusd_amount.to_u64()))?;

    // The pull awaited. Revalidate the same run before the first reserve
    // settlement so a changed price/rank can still be compensated in full.
    if let Some(expected_snapshot) = &spillover_plan {
        let refreshed = refresh_redemption_candidate_prices().await;
        let still_representable = refreshed.is_ok()
            && read_state(|state| {
                let Some(run) = current_legacy_reserve_run_for_snapshot(state, expected_snapshot)
                else {
                    return false;
                };
                let amount = ICUSD::from(spillover_e8s);
                let fee = amount * state.get_redemption_fee_for(&run.collateral_type, amount);
                let effective = amount - fee; // RMR was applied above.
                Decimal::from_f64_retain(run.price_usd)
                    .map(|price| {
                        theoretical_collateral_target_fits_u64(effective, price, run.decimals)
                    })
                    .unwrap_or(false)
            });
        if !still_representable {
            return Err(refund_rejected_quoted_redemption(
                caller,
                icusd_amount.to_u64(),
                icusd_block_index,
                ProtocolError::TemporarilyUnavailable(
                    "The reserve spillover ranking or payout range changed; the icUSD refund was started."
                        .to_string(),
                ),
            )
            .await);
        }
    }

    // Transfer ckStable to user from reserves.
    // CRITICAL: If this fails we MUST refund the icUSD, otherwise the user
    // loses funds with nothing received. ICP inter-canister calls are NOT
    // atomic, so we implement the saga/compensation pattern manually.
    //
    // Wave-4 ICC-007: if the inline refund itself fails, persist a
    // `PendingRefund` keyed by `icusd_block_index`; `process_pending_transfer`
    // retries it until success or MAX_PENDING_RETRIES. The op_nonce is minted
    // once and reused across retries (idempotent at the icUSD ledger).
    if available_for_user > 0 {
        if let Err(transfer_err) =
            management::transfer_collateral(available_for_user, caller, stable_ledger).await
        {
            log!(
                crate::INFO,
                "[redeem_reserves] ckStable transfer failed for {}: {:?}. Refunding {} icUSD.",
                caller,
                transfer_err,
                icusd_amount.to_u64()
            );
            let refund_nonce = mutate_state(|s| s.next_op_nonce());
            match management::transfer_icusd_with_nonce(icusd_amount, caller, refund_nonce).await {
                Ok(refund_block) => {
                    log!(
                        crate::INFO,
                        "[redeem_reserves] Refunded {} icUSD to {} (block {})",
                        icusd_amount.to_u64(),
                        caller,
                        refund_block
                    );
                }
                Err(refund_err) => {
                    log!(crate::INFO,
                        "[redeem_reserves] ckStable transfer failed AND inline icUSD refund failed for {}! \
                         Amount: {} icUSD, ckStable error: {:?}, refund error: {:?}. \
                         Enqueueing durable refund (block {}).",
                        caller, icusd_amount.to_u64(), transfer_err, refund_err, icusd_block_index
                    );
                    mutate_state(|s| {
                        s.pending_refunds.insert(
                            icusd_block_index,
                            crate::state::PendingRefund {
                                user: caller,
                                amount_e8s: icusd_amount.to_u64(),
                                retry_count: 0,
                                op_nonce: refund_nonce,
                            },
                        );
                    });
                    ic_cdk_timers::set_timer(std::time::Duration::from_secs(2), || {
                        ic_cdk::spawn(crate::process_pending_transfer())
                    });
                }
            }
            return Err(ProtocolError::GenericError(format!(
                "Reserve transfer failed; your icUSD refund is in flight. Error: {:?}",
                transfer_err
            )));
        }
    }

    // Transfer fee to treasury (if configured), otherwise fee stays in reserves
    if fee_e6s > 0 {
        if let Some(treasury_principal) = treasury {
            if let Err(e) =
                management::transfer_collateral(fee_e6s, treasury_principal, stable_ledger).await
            {
                log!(crate::INFO,
                    "[redeem_reserves] WARNING: treasury fee transfer failed ({} e6s to {}): {:?}. Fee stays in reserves.",
                    fee_e6s, treasury_principal, e
                );
            }
        }
    }

    // Record the reserve redemption event
    crate::event::record_reserve_redemption(
        caller,
        icusd_amount,
        fee_icusd,
        stable_ledger,
        available_for_user,
        fee_e6s,
        icusd_block_index,
    );

    // Wave-8e LIQ-005: route the reserves-portion fee (in icUSD e8s)
    // through deficit repayment. The redeemer's icUSD was burned via
    // `transfer_icusd_from` above, so this is a pure state mutation.
    // The stablecoin fee transfer to treasury (line ~349) is unaffected
    // — that ckUSDT/ckUSDC payment is the actual revenue, the deficit
    // bookkeeping is the foregone-equity offset.
    if fee_icusd.0 > 0 {
        mutate_state(|s| {
            let _routing = crate::treasury::plan_fee_routing(
                s,
                fee_icusd,
                crate::event::FeeSource::RedemptionFee,
            );
        });
    }

    // Handle vault spillover if reserves didn't cover everything
    let refund_e8s = if spillover_e8s > 0 {
        mutate_state(|s| {
            let spillover_icusd = ICUSD::from(spillover_e8s);
            // Stable and treasury ledger calls have awaited. Compare the full
            // pre-pull snapshot before the synchronous event mutation. If any
            // part changed, do not seize and refund only the spillover tail.
            let Some(expected_snapshot) = &spillover_plan else {
                return reserve_spillover_snapshot_mismatch_refund(
                    spillover_e8s,
                    rmr,
                    raw_spillover_refund_budget_e8s,
                );
            };
            let Some(run) = current_legacy_reserve_run_for_snapshot(s, expected_snapshot) else {
                return reserve_spillover_snapshot_mismatch_refund(
                    spillover_e8s,
                    rmr,
                    raw_spillover_refund_budget_e8s,
                );
            };
            // Stable and treasury transfers may span the full oracle freshness
            // window without changing the tuple's numeric price. Recheck the
            // complete candidate set synchronously at the seizure cutpoint.
            let now = ic_cdk::api::time();
            if current_fresh_legacy_reserve_run_for_snapshot(s, expected_snapshot, now).is_none() {
                return reserve_spillover_snapshot_mismatch_refund(
                    spillover_e8s,
                    rmr,
                    raw_spillover_refund_budget_e8s,
                );
            }
            let Some(config) = s.get_collateral_config(&run.collateral_type) else {
                return reserve_spillover_snapshot_mismatch_refund(
                    spillover_e8s,
                    rmr,
                    raw_spillover_refund_budget_e8s,
                );
            };
            let Some(price_decimal) = config
                .last_price
                .and_then(Decimal::from_f64_retain)
                .filter(|price| *price > Decimal::ZERO)
            else {
                return reserve_spillover_snapshot_mismatch_refund(
                    spillover_e8s,
                    rmr,
                    raw_spillover_refund_budget_e8s,
                );
            };
            let current_price = UsdIcp::from(price_decimal);

            // Wave-14b CDP-03: per-collateral fee path (see redeem_collateral
            // for full rationale). The spillover redeems against the selected run,
            // so the base rate is read from and written back to that
            // collateral's config alone.
            let base_fee = s.get_redemption_fee_for(&run.collateral_type, spillover_icusd);
            let vault_fee = spillover_icusd * base_fee;

            // Note: RMR was already applied when computing spillover_e8s (line 160).
            // Do NOT apply it again here — that would double-discount.
            let effective_spillover = spillover_icusd - vault_fee;

            // A ckStable transfer awaited before this point. Cap the remaining
            // native payout against the current price/decimals so the event's
            // historical u64 sum and pending transfer amount stay representable.
            let safe_effective_e8s =
                max_effective_icusd_for_u64_payout(price_decimal, run.decimals);
            let bounded_effective =
                ICUSD::new(effective_spillover.to_u64().min(safe_effective_e8s));
            let Some(simulated) = s.try_simulate_redemption_for_vault_ids(
                bounded_effective,
                current_price,
                &run.collateral_type,
                &run.vault_ids,
            ) else {
                return reserve_spillover_snapshot_mismatch_refund(
                    spillover_e8s,
                    rmr,
                    raw_spillover_refund_budget_e8s,
                );
            };
            if simulated_collateral_total_raw(&simulated).is_none() {
                return reserve_spillover_snapshot_mismatch_refund(
                    spillover_e8s,
                    rmr,
                    raw_spillover_refund_budget_e8s,
                );
            }

            let outcome = match crate::event::record_redemption_on_vault_run(
                s,
                caller,
                bounded_effective,
                vault_fee,
                current_price,
                icusd_block_index,
                run.collateral_type,
                &run.vault_ids,
                None,
            ) {
                Ok(outcome) => outcome,
                Err(error) => {
                    log!(
                        crate::INFO,
                        "[redeem_reserves] Spillover recorder rejected before mutation: {:?}",
                        error
                    );
                    return reserve_spillover_snapshot_mismatch_refund(
                        spillover_e8s,
                        rmr,
                        raw_spillover_refund_budget_e8s,
                    );
                }
            };

            crate::record_per_collateral_redemption_fee(
                s,
                &run.collateral_type,
                base_fee,
                ic_cdk::api::time(),
            );

            // Wave-8e LIQ-005: route the spillover-portion fee through
            // deficit repayment. icUSD already burned via `transfer_icusd_from`.
            let _routing = crate::treasury::plan_fee_routing(
                s,
                vault_fee,
                crate::event::FeeSource::RedemptionFee,
            );

            // Return the raw amount not represented by the settled stable
            // payment, committed vault fee, and actual debt consumption. Apply
            // one aggregate RMR conversion so the legs cannot each round away
            // a separate unit.
            reserve_post_settlement_raw_refund(
                net_icusd.to_u64(),
                available_for_user,
                vault_fee.to_u64(),
                outcome.consumed.to_u64(),
                rmr,
            )
        })
    } else {
        // Even a fully reserve-funded redemption can leave raw icUSD dust
        // because the stable payout is quantized to e6.
        raw_spillover_refund_budget_e8s
    };
    if refund_e8s > 0 {
        let refund_nonce = mutate_state(|s| s.next_op_nonce());
        match management::transfer_icusd_with_nonce(ICUSD::from(refund_e8s), caller, refund_nonce)
            .await
        {
            Ok(refund_block) => {
                log!(
                    crate::INFO,
                    "[redeem_reserves] Refunded {} unconsumed spillover icUSD to {} (block {})",
                    refund_e8s,
                    caller,
                    refund_block
                );
            }
            Err(refund_err) => {
                log!(crate::INFO,
                    "[redeem_reserves] Unconsumed-spillover refund of {} icUSD to {} failed: {:?}. Enqueueing durable refund (block {}).",
                    refund_e8s, caller, refund_err, icusd_block_index
                );
                mutate_state(|s| {
                    s.pending_refunds.insert(
                        icusd_block_index,
                        crate::state::PendingRefund {
                            user: caller,
                            amount_e8s: refund_e8s,
                            retry_count: 0,
                            op_nonce: refund_nonce,
                        },
                    );
                });
            }
        }
    }
    ic_cdk_timers::set_timer(std::time::Duration::from_secs(0), || {
        ic_cdk::spawn(crate::process_pending_transfer())
    });

    log!(INFO, "[redeem_reserves] {} redeemed {} icUSD: {} e6s from reserves, {} e8s vault spillover, fee {} e6s",
        caller, icusd_amount.to_u64(), available_for_user, spillover_e8s, fee_e6s);

    Ok(crate::ReserveRedemptionResult {
        icusd_block_index,
        stable_amount_sent: available_for_user,
        fee_amount: fee_icusd.to_u64(),
        stable_token_used: stable_ledger,
        // Preserve the historical response meaning: effective spillover
        // before the vault fee/capacity outcome, not the amount actually
        // consumed by selected vaults.
        vault_spillover_amount: spillover_e8s,
    })
}

/// Thin wrapper for backward compatibility. Calls `redeem_collateral` with ICP.
pub async fn redeem_icp(icusd_amount: u64) -> Result<SuccessWithFee, ProtocolError> {
    let icp_ledger = read_state(|s| s.icp_collateral_type());
    redeem_collateral(icp_ledger, icusd_amount).await
}

/// Generic collateral redemption: burn icUSD and receive collateral tokens.
/// Currently the redemption logic (vault sorting, pending transfers) is ICP-centric,
/// but the API surface supports any collateral type. The internal logic will be
/// generalized per-collateral when a second collateral type is actually added.
pub async fn redeem_collateral(
    collateral_type: Principal,
    _icusd_amount: u64,
) -> Result<SuccessWithFee, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let _guard_principal = GuardPrincipal::new(caller, "redeem_collateral")?;

    // RED-101 / RED-003: gate redemption on protocol mode at the shared internal
    // entry point, not just at the Candid endpoints. ReadOnly auto-latches on
    // insolvency (total collateral ratio < 100%, or the deficit account over its
    // threshold); redeeming then extracts collateral at oracle face value from an
    // already-insolvent protocol, deepening the bad-debt position. Placed after
    // the guard and before any icUSD is pulled so every redemption surface
    // (redeem_icp, redeem_collateral) is covered by construction. The Wave-9
    // fix lived only in main.rs::validate_mode and the redeem_icp endpoint
    // bypassed it. Same error as that gate via the shared constructor.
    if read_state(|s| s.mode) == Mode::ReadOnly {
        return Err(ProtocolError::read_only_mode());
    }

    let icusd_amount: ICUSD = _icusd_amount.into();

    if icusd_amount < read_state(|s| s.min_icusd_amount) {
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: read_state(|s| s.min_icusd_amount).to_u64(),
        });
    }

    // Input sanity: the caller-supplied collateral type must exist. The type
    // actually seized is the redemption-priority winner resolved below; the
    // caller's argument cannot target a specific collateral (priority order
    // is a protocol-level peg defense).
    if read_state(|s| s.get_collateral_status(&collateral_type)).is_none() {
        return Err(ProtocolError::GenericError(format!(
            "Collateral type {} not found.",
            collateral_type
        )));
    }

    // RED-002 (audit 2026-06-09): resolve the redemption-priority winner
    // BEFORE pulling icUSD, and key every check on it. Previously freshness,
    // fee pricing, and the base-rate bump used the caller-supplied type while
    // the water-fill seized the priority winner, so a redeemer could pass a
    // deep-debt type to floor the dynamic fee while draining a thin type
    // against a price with no staleness gate.
    let selected_run = read_state(|s| s.redemption_runs().into_iter().next());
    let Some(selected_run) = selected_run else {
        return Err(ProtocolError::TemporarilyUnavailable(
            "No eligible collateral vaults are available for redemption.".to_string(),
        ));
    };
    let redeem_ct = selected_run.collateral_type;

    let redeem_status = read_state(|s| s.get_collateral_status(&redeem_ct));
    if let Some(status) = redeem_status {
        if !status.allows_redemption() {
            return Err(ProtocolError::GenericError(format!(
                "Redemption is not allowed for collateral type {}.",
                redeem_ct
            )));
        }
    } else {
        return Err(ProtocolError::GenericError(format!(
            "Collateral type {} not found.",
            redeem_ct
        )));
    }

    // Fail closed on a stale price for the collateral actually being seized
    // (VER-001 ceiling applies inside ensure_fresh_price_for).
    refresh_redemption_candidate_prices().await?;
    let pre_pull_run = read_state(|s| legacy_redemption_run_for_request(s, collateral_type))?;
    let pre_pull_snapshot = RedemptionRunSnapshot::capture(&pre_pull_run);
    let redeem_ct = pre_pull_run.collateral_type;
    let collateral_price = Decimal::from_f64_retain(pre_pull_run.price_usd).ok_or(
        ProtocolError::TemporarilyUnavailable("No price available for collateral".to_string()),
    )?;
    let current_collateral_price = UsdIcp::from(collateral_price);

    // RED-001 (audit 2026-06-09): reject claims that exceed what the
    // water-fill can consume. Without this, the full claim was burned and the
    // payout was computed from the claim rather than the consumed amount,
    // draining co-collateral vaults' shared backing for debt that was never
    // redeemed. The estimate uses the pre-pull fee/RMR; any residual gap
    // (state moving during the icUSD pull) is covered by the unconsumed
    // refund below.
    let (estimated_effective, run_debt, max_input, payout_representable) = read_state(|s| {
        let base_fee = s.get_redemption_fee_for(&redeem_ct, icusd_amount);
        let fee_est = icusd_amount * base_fee;
        let rmr = s.get_redemption_margin_ratio();
        let run = s
            .redemption_runs()
            .into_iter()
            .next()
            .filter(|run| run.collateral_type == redeem_ct);
        let effective = (icusd_amount - fee_est) * rmr;
        let payout_representable = run
            .as_ref()
            .and_then(|run| {
                let price = Decimal::from_f64_retain(run.price_usd)?;
                let simulated = s.try_simulate_redemption_for_vault_ids(
                    effective,
                    UsdIcp::from(price),
                    &run.collateral_type,
                    &run.vault_ids,
                )?;
                simulated_collateral_total_raw(&simulated)
            })
            .is_some();
        (
            effective,
            run.as_ref().map(|run| run.eligible_debt_e8s).unwrap_or(0),
            run.as_ref()
                .map(|run| max_input_for_run(s, run))
                .unwrap_or(0),
            payout_representable,
        )
    });
    if _icusd_amount > max_input || estimated_effective.to_u64() > run_debt {
        return Err(ProtocolError::GenericError(format!(
            "The requested amount exceeds the current first collateral run capacity ({} icUSD e8s). Refresh and review the redemption order.",
            max_input
        )));
    }
    if !payout_representable {
        return Err(ProtocolError::TemporarilyUnavailable(
            "The selected collateral payout exceeds the supported raw-token range.".to_string(),
        ));
    }

    match transfer_icusd_from(icusd_amount, caller).await {
        Ok(block_index) => {
            // The pull is an await boundary. If global rank or eligible capacity
            // changed while it was in flight, refund the full input before any
            // fee/base-rate mutation or vault seizure.
            if let Err(error) = refresh_redemption_candidate_prices().await {
                return Err(refund_rejected_quoted_redemption(
                    caller,
                    icusd_amount.to_u64(),
                    block_index,
                    error,
                )
                .await);
            }
            let post_pull_run = read_state(|state| state.redemption_runs().into_iter().next());
            let Some(post_pull_run) = post_pull_run else {
                return Err(refund_rejected_quoted_redemption(
                    caller,
                    icusd_amount.to_u64(),
                    block_index,
                    ProtocolError::TemporarilyUnavailable(
                        "No eligible collateral run remains after the icUSD pull.".to_string(),
                    ),
                )
                .await);
            };
            if let Some(error) = redemption_run_snapshot_error(&pre_pull_snapshot, &post_pull_run) {
                return Err(refund_rejected_quoted_redemption(
                    caller,
                    icusd_amount.to_u64(),
                    block_index,
                    legacy_redemption_error(error),
                )
                .await);
            }
            let post_pull = match get_redemption_quote(icusd_amount.to_u64()) {
                Ok(quote) => quote,
                Err(error) => {
                    return Err(refund_rejected_quoted_redemption(
                        caller,
                        icusd_amount.to_u64(),
                        block_index,
                        legacy_redemption_error(error),
                    )
                    .await)
                }
            };
            if post_pull.collateral_type != redeem_ct {
                return Err(refund_rejected_quoted_redemption(
                    caller,
                    icusd_amount.to_u64(),
                    block_index,
                    ProtocolError::GenericError(format!(
                        "The redemption priority changed from {} to {}. Refresh and review the redemption order.",
                        redeem_ct, post_pull.collateral_type
                    )),
                )
                .await);
            }
            let run_vault_ids = read_state(|state| {
                state
                    .redemption_runs()
                    .into_iter()
                    .next()
                    .filter(|run| run.collateral_type == redeem_ct)
                    .map(|run| run.vault_ids)
            });
            let Some(run_vault_ids) = run_vault_ids else {
                return Err(refund_rejected_quoted_redemption(
                    caller,
                    icusd_amount.to_u64(),
                    block_index,
                    ProtocolError::TemporarilyUnavailable(
                        "The selected collateral run changed before execution.".to_string(),
                    ),
                )
                .await);
            };
            let mutation_result = mutate_state(|s| {
                // Wave-14b CDP-03: price the fee against the per-collateral
                // base rate, and write the post-redemption rate back to the
                // per-collateral config (NOT the legacy global fields). A
                // redemption against one collateral no longer corrupts the
                // base rate used to price redemptions against any other.
                // RED-002: keyed on the seized collateral, not the caller's.
                let base_fee = s.get_redemption_fee_for(&redeem_ct, icusd_amount);
                let fee_amount = icusd_amount * base_fee;

                // Apply dynamic Redemption Margin Ratio: redeemers get RMR × face value
                let rmr = s.get_redemption_margin_ratio();
                let effective_icusd = (icusd_amount - fee_amount) * rmr;

                let outcome = crate::event::record_redemption_on_vault_run(
                    s,
                    caller,
                    effective_icusd,
                    fee_amount,
                    current_collateral_price,
                    block_index,
                    redeem_ct,
                    &run_vault_ids,
                    None,
                )
                .map_err(|error| {
                    legacy_redemption_error(redemption_record_error_to_protocol(error))
                })?;

                crate::record_per_collateral_redemption_fee(
                    s,
                    &redeem_ct,
                    base_fee,
                    ic_cdk::api::time(),
                );

                // RED-001: refund the unconsumed remainder of the claim in raw
                // icUSD (un-scale by RMR; the fee stays with the protocol as
                // priced). Floor division favors the protocol; capped at the
                // post-fee pull so a refund can never exceed what was taken.
                let refund_e8s = redemption_raw_refund(
                    (icusd_amount - fee_amount).to_u64(),
                    outcome.consumed.to_u64(),
                    rmr,
                    (icusd_amount - fee_amount).to_u64(),
                );

                // Wave-8e LIQ-005: route a configurable fraction of the
                // redemption fee toward deficit repayment. The redeemer's
                // icUSD has already been burned via `transfer_icusd_from`
                // (the protocol's main account is the icUSD minting
                // account), so the supply side is already correct — this
                // is a pure state mutation that decrements the deficit.
                let _routing = crate::treasury::plan_fee_routing(
                    s,
                    fee_amount,
                    crate::event::FeeSource::RedemptionFee,
                );

                Ok((fee_amount, outcome, refund_e8s))
            });
            let (fee_amount, outcome, refund_e8s) = match mutation_result {
                Ok(result) => result,
                Err(error) => {
                    return Err(refund_rejected_quoted_redemption(
                        caller,
                        icusd_amount.to_u64(),
                        block_index,
                        error,
                    )
                    .await)
                }
            };

            // RED-001: pay back the unconsumed icUSD. Same saga as
            // redeem_reserves (Wave-4 ICC-007): inline refund first, durable
            // `pending_refunds` entry (keyed by the unique burn block index,
            // nonce reused across retries) if the inline transfer fails.
            if refund_e8s > 0 {
                let refund_nonce = mutate_state(|s| s.next_op_nonce());
                match management::transfer_icusd_with_nonce(
                    ICUSD::from(refund_e8s),
                    caller,
                    refund_nonce,
                )
                .await
                {
                    Ok(refund_block) => {
                        log!(
                            INFO,
                            "[redeem_collateral] Refunded {} unconsumed icUSD to {} (block {})",
                            refund_e8s,
                            caller,
                            refund_block
                        );
                    }
                    Err(refund_err) => {
                        log!(INFO,
                            "[redeem_collateral] Unconsumed-claim refund of {} icUSD to {} failed: {:?}. Enqueueing durable refund (block {}).",
                            refund_e8s, caller, refund_err, block_index
                        );
                        mutate_state(|s| {
                            s.pending_refunds.insert(
                                block_index,
                                crate::state::PendingRefund {
                                    user: caller,
                                    amount_e8s: refund_e8s,
                                    retry_count: 0,
                                    op_nonce: refund_nonce,
                                },
                            );
                        });
                    }
                }
            }

            ic_cdk_timers::set_timer(std::time::Duration::from_secs(0), || {
                ic_cdk::spawn(crate::process_pending_transfer())
            });
            Ok(SuccessWithFee {
                block_index,
                fee_amount_paid: fee_amount.to_u64(),
                collateral_amount_received: Some(outcome.margin.to_u64()),
                debt_liquidated_e8s: None, // SP-101
                stable_pulled_e6s: None,   // SP-110
                xrp_claim_id: None,
            })
        }
        Err(transfer_from_error) => Err(ProtocolError::TransferFromError(
            transfer_from_error,
            icusd_amount.to_u64(),
        )),
    }
}

/// Execute a vault-only redemption against exactly the first globally ranked
/// same-collateral run. The submitted token and net payout floor are checked
/// again after the icUSD pull and before any vault mutation.
pub async fn redeem_quoted(
    request: RedeemQuotedRequest,
) -> Result<RedemptionResult, RedemptionError> {
    let caller = ic_cdk::api::caller();
    let _guard = GuardPrincipal::new(caller, "redeem_quoted")?;
    if read_state(|state| state.mode) == Mode::ReadOnly {
        return Err(ProtocolError::read_only_mode().into());
    }
    if request.amount_e8s < read_state(|state| state.min_icusd_amount.to_u64()) {
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: read_state(|state| state.min_icusd_amount.to_u64()),
        }
        .into());
    }

    // Refresh only the quoted collateral. After the await, rebuild the global
    // queue so stale rank changes fail closed instead of paying another asset.
    refresh_redemption_candidate_prices().await?;
    let pre_quote = get_redemption_quote(request.amount_e8s)?;
    if pre_quote.collateral_type != request.expected_collateral_type {
        return Err(RedemptionError::RedemptionPriorityChanged {
            expected: request.expected_collateral_type,
            actual: pre_quote.collateral_type,
        });
    }
    if pre_quote.net_collateral_raw < request.min_net_collateral_raw {
        return Err(RedemptionError::RedemptionMinimumNotMet {
            minimum_net_raw: request.min_net_collateral_raw,
            actual_net_raw: pre_quote.net_collateral_raw,
        });
    }

    match transfer_icusd_from(ICUSD::from(request.amount_e8s), caller).await {
        Err(error) => Err(ProtocolError::TransferFromError(error, request.amount_e8s).into()),
        Ok(block_index) => {
            // The pull awaited. A concurrent redemption or a refreshed price can
            // change the global first run. Validate a fresh snapshot before any
            // fee update, event, debt mutation, or collateral seizure.
            if let Err(error) = refresh_redemption_candidate_prices().await {
                return Err(refund_rejected_quoted_redemption(
                    caller,
                    request.amount_e8s,
                    block_index,
                    RedemptionError::from(error),
                )
                .await);
            }
            let post_quote = match get_redemption_quote(request.amount_e8s) {
                Ok(quote) => quote,
                Err(error) => {
                    return Err(refund_rejected_quoted_redemption(
                        caller,
                        request.amount_e8s,
                        block_index,
                        error,
                    )
                    .await)
                }
            };
            if post_quote.collateral_type != request.expected_collateral_type {
                let error = RedemptionError::RedemptionPriorityChanged {
                    expected: request.expected_collateral_type,
                    actual: post_quote.collateral_type,
                };
                return Err(refund_rejected_quoted_redemption(
                    caller,
                    request.amount_e8s,
                    block_index,
                    error,
                )
                .await);
            }
            if post_quote.net_collateral_raw < request.min_net_collateral_raw {
                let error = RedemptionError::RedemptionMinimumNotMet {
                    minimum_net_raw: request.min_net_collateral_raw,
                    actual_net_raw: post_quote.net_collateral_raw,
                };
                return Err(refund_rejected_quoted_redemption(
                    caller,
                    request.amount_e8s,
                    block_index,
                    error,
                )
                .await);
            }

            let mutation_result: Result<
                (ICUSD, crate::event::RedemptionOutcome, u64, String, u8),
                RedemptionError,
            > = mutate_state(|state| {
                // No await separates the post-pull check from this mutation.
                let run = state.redemption_runs().into_iter().next().ok_or_else(|| {
                    RedemptionError::RedemptionQuoteUnavailable(
                        "Eligible collateral run disappeared.".to_string(),
                    )
                })?;
                if run.collateral_type != request.expected_collateral_type {
                    return Err(RedemptionError::RedemptionPriorityChanged {
                        expected: request.expected_collateral_type,
                        actual: run.collateral_type,
                    });
                }
                let max_input = max_input_for_run(state, &run);
                if request.amount_e8s > max_input {
                    return Err(RedemptionError::RedemptionCapacityExceeded {
                        max_input_icusd_e8s: max_input,
                    });
                }
                let amount = ICUSD::from(request.amount_e8s);
                let fee_ratio = state.get_redemption_fee_for(&run.collateral_type, amount);
                let fee = amount * fee_ratio;
                let rmr = state.get_redemption_margin_ratio();
                let effective = (amount - fee) * rmr;
                let price =
                    UsdIcp::from(Decimal::from_f64_retain(run.price_usd).ok_or_else(|| {
                        RedemptionError::RedemptionQuoteUnavailable(
                            "Collateral price is unavailable.".to_string(),
                        )
                    })?);
                let simulated = state
                    .try_simulate_redemption_for_vault_ids(
                        effective,
                        price,
                        &run.collateral_type,
                        &run.vault_ids,
                    )
                    .ok_or_else(|| {
                        RedemptionError::RedemptionQuoteUnavailable(
                            "A selected collateral share exceeds the supported raw-token range."
                                .to_string(),
                        )
                    })?;
                let gross = simulated_collateral_total_raw(&simulated).ok_or_else(|| {
                    RedemptionError::RedemptionQuoteUnavailable(
                        "The selected collateral payout exceeds the supported raw-token range."
                            .to_string(),
                    )
                })?;
                let config = state
                    .get_collateral_config(&run.collateral_type)
                    .ok_or_else(|| {
                        RedemptionError::RedemptionQuoteUnavailable(
                            "Collateral configuration disappeared.".to_string(),
                        )
                    })?;
                let net = gross.saturating_sub(config.ledger_fee);
                if net < request.min_net_collateral_raw {
                    return Err(RedemptionError::RedemptionMinimumNotMet {
                        minimum_net_raw: request.min_net_collateral_raw,
                        actual_net_raw: net,
                    });
                }

                let ct = run.collateral_type;
                let symbol = run.symbol;
                let decimals = run.decimals;
                let outcome = crate::event::record_redemption_on_vault_run(
                    state,
                    caller,
                    effective,
                    fee,
                    price,
                    block_index,
                    ct,
                    &run.vault_ids,
                    Some(request.min_net_collateral_raw),
                )
                .map_err(redemption_record_error_to_protocol)?;
                crate::record_per_collateral_redemption_fee(
                    state,
                    &ct,
                    fee_ratio,
                    ic_cdk::api::time(),
                );
                let refund_e8s = redemption_raw_refund(
                    (amount - fee).to_u64(),
                    outcome.consumed.to_u64(),
                    rmr,
                    (amount - fee).to_u64(),
                );
                if refund_e8s > 0 {
                    let refund_nonce = state.next_op_nonce();
                    state.pending_refunds.insert(
                        block_index,
                        crate::state::PendingRefund {
                            user: caller,
                            amount_e8s: refund_e8s,
                            retry_count: 0,
                            op_nonce: refund_nonce,
                        },
                    );
                }
                let _ = crate::treasury::plan_fee_routing(
                    state,
                    fee,
                    crate::event::FeeSource::RedemptionFee,
                );
                Ok((fee, outcome, net, symbol, decimals))
            });

            let (fee, outcome, net, symbol, decimals) = match mutation_result {
                Ok(values) => values,
                Err(error) => {
                    return Err(refund_rejected_quoted_redemption(
                        caller,
                        request.amount_e8s,
                        block_index,
                        error,
                    )
                    .await)
                }
            };

            ic_cdk_timers::set_timer(std::time::Duration::from_secs(0), || {
                ic_cdk::spawn(crate::process_pending_transfer())
            });
            Ok(RedemptionResult {
                icusd_block_index: block_index,
                fee_paid_e8s: fee.to_u64(),
                collateral_type: post_quote.collateral_type,
                symbol,
                decimals,
                net_collateral_raw: net,
                payout_status: RedemptionPayoutStatus::Queued,
            })
        }
    }
}

/// Request-ID redemption path. The exact fee-free pull tuple and high-water
/// mark are stable before the first await. Reply loss is retried with that
/// exact tuple; successful burns require an exact ICRC-3 burn receipt before
/// any vault accounting can change.
pub async fn redeem_quoted_v2(
    request: crate::state::RedemptionV2Request,
) -> Result<RedemptionResult, RedemptionError> {
    let caller = ic_cdk::api::caller();
    let _guard = GuardPrincipal::new(caller, "redeem_quoted_v2")?;
    if caller == Principal::anonymous() {
        return Err(ProtocolError::AnonymousCallerNotAllowed.into());
    }

    let existing = read_state(|state| {
        if let Some(row) = state.redemption_v2_latest_result.get(&caller) {
            if row.request.request_id == request.request_id {
                return Some((true, row.clone()));
            }
        }
        state
            .redemption_v2_active
            .get(&caller)
            .map(|row| (false, row.clone()))
    });
    if let Some((completed, row)) = existing.as_ref() {
        if row.request != request {
            return Err(ProtocolError::GenericError(
                "redemption request ID is bound to a different payload".into(),
            )
            .into());
        }
        if *completed {
            if let Some(result) = row.result.clone() {
                return Ok(redemption_v2_result_to_public(result));
            }
            return Err(ProtocolError::TemporarilyUnavailable(
                "this redemption request was already refunded".into(),
            )
            .into());
        }
        if row.phase == crate::state::RedemptionV2Phase::RefundPending {
            return resume_redemption_v2_refund(caller, request, row.clone()).await;
        }
    }

    // Admission policy may change after an exact pull was submitted. It must
    // stop new requests, but must not make an existing request unrecoverable.
    if existing.is_none() {
        let (mode, frozen, minimum_amount) = read_state(|state| {
            (state.mode, state.frozen, state.min_icusd_amount.to_u64())
        });
        if mode == Mode::ReadOnly || frozen {
            return Err(ProtocolError::read_only_mode().into());
        }
        if request.amount_e8s < minimum_amount {
            return Err(ProtocolError::AmountTooLow { minimum_amount }.into());
        }
    }

    // Admission itself is synchronous. This includes the exact operation
    // nonce, memo, timestamp, accounts, fee and amount used for every retry.
    let tuple = if let Some((false, row)) = existing.as_ref() {
        row.tuple.clone()
    } else {
        let tuple = mutate_state(|state| {
            let nonce = state.next_op_nonce();
            let backend = ic_cdk::id();
            crate::SpLiquidationStablePullTuple {
                op_nonce: nonce,
                ledger: state.icusd_ledger_principal,
                from: Account {
                    owner: caller,
                    subaccount: None,
                },
                spender: Account {
                    owner: backend,
                    subaccount: None,
                },
                to: Account {
                    owner: backend,
                    subaccount: None,
                },
                amount_raw: request.amount_e8s,
                fee_raw: 0,
                memo: management::nonce_to_memo(nonce).0.to_vec(),
                created_at_time_ns: management::nonce_to_created_at_time(nonce),
            }
        });
        mutate_state(|state| {
            crate::state::admit_redemption_v2(state, caller, request.clone(), tuple.clone())
        })
        .map_err(|message| RedemptionError::from(ProtocolError::TemporarilyUnavailable(message)))?;
        tuple
    };

    // A failed/ambiguous ledger reply is never treated as proof of no effect.
    // Re-entering with the same request resubmits only the persisted tuple.
    let block_index = match management::transfer_from_with_exact_tuple(&tuple).await {
        Ok(index) => index,
        Err(error) => {
            mutate_state(|state| {
                if let Some(row) = state.redemption_v2_active.get_mut(&caller) {
                    row.last_error = Some(format!("exact icUSD pull unresolved: {error:?}"));
                }
                crate::storage::save_state_to_stable(state);
            });
            return Err(ProtocolError::TemporarilyUnavailable(
                "the icUSD burn outcome is unresolved; retry this same request ID".into(),
            )
            .into());
        }
    };
    mutate_state(|state| {
        if let Some(row) = state.redemption_v2_active.get_mut(&caller) {
            row.block_index = Some(block_index);
            row.phase = crate::state::RedemptionV2Phase::BurnProven;
            row.last_error = None;
        }
        crate::storage::save_state_to_stable(state);
    });
    if let Err(error) =
        crate::icrc3_proof::verify_sp_liquidation_icusd_burn_block(&tuple, block_index).await
    {
        mutate_state(|state| {
            if let Some(row) = state.redemption_v2_active.get_mut(&caller) {
                row.last_error = Some(format!("exact ICRC-3 burn proof unresolved: {error}"));
            }
            crate::storage::save_state_to_stable(state);
        });
        return Err(ProtocolError::TemporarilyUnavailable(
            "the icUSD burn is held pending exact ledger proof; retry this same request ID".into(),
        )
        .into());
    }

    if read_state(|state| state.mode == Mode::ReadOnly || state.frozen) {
        return compensate_redemption_v2(
            caller,
            request,
            block_index,
            ProtocolError::read_only_mode().into(),
        )
        .await;
    }

    if let Err(error) = refresh_redemption_candidate_prices().await {
        return compensate_redemption_v2(caller, request, block_index, error.into()).await;
    }
    let post_quote = match get_redemption_quote(request.amount_e8s) {
        Ok(quote) => quote,
        Err(error) => return compensate_redemption_v2(caller, request, block_index, error).await,
    };
    if post_quote.collateral_type != request.expected_collateral_type {
        return compensate_redemption_v2(
            caller,
            request.clone(),
            block_index,
            RedemptionError::RedemptionPriorityChanged {
                expected: request.expected_collateral_type,
                actual: post_quote.collateral_type,
            },
        )
        .await;
    }
    if post_quote.net_collateral_raw < request.min_net_collateral_raw {
        return compensate_redemption_v2(
            caller,
            request.clone(),
            block_index,
            RedemptionError::RedemptionMinimumNotMet {
                minimum_net_raw: request.min_net_collateral_raw,
                actual_net_raw: post_quote.net_collateral_raw,
            },
        )
        .await;
    }

    let committed = mutate_state(|state| {
        if state.mode == Mode::ReadOnly || state.frozen {
            return Err(ProtocolError::read_only_mode().into());
        }
        let journal_matches = state.redemption_v2_active.get(&caller).is_some_and(|row| {
            row.request == request
                && row.tuple == tuple
                && row.block_index == Some(block_index)
                && row.phase == crate::state::RedemptionV2Phase::BurnProven
        });
        if !journal_matches {
            return Err(RedemptionError::RedemptionQuoteUnavailable(
                "the proven burn request is no longer the active redemption intent".into(),
            ));
        }
        let run = state.redemption_runs().into_iter().next().ok_or_else(|| {
            RedemptionError::RedemptionQuoteUnavailable(
                "Eligible collateral run disappeared.".into(),
            )
        })?;
        if run.collateral_type != request.expected_collateral_type {
            return Err(RedemptionError::RedemptionPriorityChanged {
                expected: request.expected_collateral_type,
                actual: run.collateral_type,
            });
        }
        if request.amount_e8s > max_input_for_run(state, &run) {
            return Err(RedemptionError::RedemptionCapacityExceeded {
                max_input_icusd_e8s: max_input_for_run(state, &run),
            });
        }
        let amount = ICUSD::from(request.amount_e8s);
        let fee_ratio = state.get_redemption_fee_for(&run.collateral_type, amount);
        let fee = amount * fee_ratio;
        let rmr = state.get_redemption_margin_ratio();
        let effective = (amount - fee) * rmr;
        let price = UsdIcp::from(Decimal::from_f64_retain(run.price_usd).ok_or_else(|| {
            RedemptionError::RedemptionQuoteUnavailable("Collateral price is unavailable.".into())
        })?);
        let config = state
            .get_collateral_config(&run.collateral_type)
            .ok_or_else(|| {
                RedemptionError::RedemptionQuoteUnavailable(
                    "Collateral configuration disappeared.".into(),
                )
            })?;
        let simulated = state
            .try_simulate_redemption_for_vault_ids(
                effective,
                price,
                &run.collateral_type,
                &run.vault_ids,
            )
            .ok_or_else(|| {
                RedemptionError::RedemptionQuoteUnavailable("Payout is not representable.".into())
            })?;
        let gross = simulated_collateral_total_raw(&simulated).ok_or_else(|| {
            RedemptionError::RedemptionQuoteUnavailable("Payout is not representable.".into())
        })?;
        let net = gross.saturating_sub(config.ledger_fee);
        if net < request.min_net_collateral_raw {
            return Err(RedemptionError::RedemptionMinimumNotMet {
                minimum_net_raw: request.min_net_collateral_raw,
                actual_net_raw: net,
            });
        }
        let ct = run.collateral_type;
        let symbol = run.symbol;
        let decimals = run.decimals;
        let outcome = crate::event::record_redemption_on_vault_run(
            state,
            caller,
            effective,
            fee,
            price,
            block_index,
            ct,
            &run.vault_ids,
            Some(request.min_net_collateral_raw),
        )
        .map_err(redemption_record_error_to_protocol)?;
        crate::record_per_collateral_redemption_fee(state, &ct, fee_ratio, ic_cdk::api::time());
        let refund_e8s = redemption_raw_refund(
            (amount - fee).to_u64(),
            outcome.consumed.to_u64(),
            rmr,
            (amount - fee).to_u64(),
        );
        let mut residual_refund = None;
        if refund_e8s > 0 {
            let refund_nonce = state.next_op_nonce();
            state.pending_refunds.insert(
                block_index,
                crate::state::PendingRefund {
                    user: caller,
                    amount_e8s: refund_e8s,
                    retry_count: 0,
                    op_nonce: refund_nonce,
                },
            );
            residual_refund = Some((refund_e8s, refund_nonce));
        }
        let _ =
            crate::treasury::plan_fee_routing(state, fee, crate::event::FeeSource::RedemptionFee);
        let saved = crate::state::RedemptionV2Result {
            icusd_block_index: block_index,
            fee_paid_e8s: fee.to_u64(),
            collateral_type: ct,
            symbol,
            decimals,
            net_collateral_raw: net,
            payout_queued: true,
        };
        let mut row = state.redemption_v2_active.remove(&caller).ok_or_else(|| {
            RedemptionError::RedemptionQuoteUnavailable(
                "redemption journal disappeared before commit".into(),
            )
        })?;
        row.phase = crate::state::RedemptionV2Phase::Committed;
        row.result = Some(saved.clone());
        row.block_index = Some(block_index);
        if let Some((refund_amount, refund_nonce)) = residual_refund {
            row.refund_amount_e8s = Some(refund_amount);
            row.refund_op_nonce = Some(refund_nonce);
        }
        state.redemption_v2_latest_result.insert(caller, row);
        crate::storage::save_state_to_stable(state);
        Ok((saved, outcome))
    });
    let (saved, _) = match committed {
        Ok(value) => value,
        Err(error) => return compensate_redemption_v2(caller, request, block_index, error).await,
    };
    ic_cdk_timers::set_timer(std::time::Duration::from_secs(0), || {
        ic_cdk::spawn(crate::process_pending_transfer())
    });
    Ok(redemption_v2_result_to_public(saved))
}

fn redemption_v2_result_to_public(saved: crate::state::RedemptionV2Result) -> RedemptionResult {
    RedemptionResult {
        icusd_block_index: saved.icusd_block_index,
        fee_paid_e8s: saved.fee_paid_e8s,
        collateral_type: saved.collateral_type,
        symbol: saved.symbol,
        decimals: saved.decimals,
        net_collateral_raw: saved.net_collateral_raw,
        payout_status: RedemptionPayoutStatus::Queued,
    }
}

async fn compensate_redemption_v2(
    caller: Principal,
    request: crate::state::RedemptionV2Request,
    block_index: u64,
    reason: RedemptionError,
) -> Result<RedemptionResult, RedemptionError> {
    let nonce = mutate_state(|state| {
        if !state.redemption_v2_active.get(&caller).is_some_and(|row| {
            row.request == request
                && row.block_index == Some(block_index)
                && row.phase == crate::state::RedemptionV2Phase::BurnProven
        }) {
            return None;
        }
        let existing = state.pending_refunds.get(&block_index).copied();
        if existing
            .is_some_and(|refund| refund.user != caller || refund.amount_e8s != request.amount_e8s)
        {
            return None;
        }
        let nonce = existing
            .map(|refund| refund.op_nonce)
            .unwrap_or_else(|| state.next_op_nonce());
        state
            .pending_refunds
            .entry(block_index)
            .or_insert(crate::state::PendingRefund {
                user: caller,
                amount_e8s: request.amount_e8s,
                retry_count: 0,
                op_nonce: nonce,
            });
        if let Some(row) = state.redemption_v2_active.get_mut(&caller) {
            row.phase = crate::state::RedemptionV2Phase::RefundPending;
            row.block_index = Some(block_index);
            row.refund_amount_e8s = Some(request.amount_e8s);
            row.refund_op_nonce = Some(nonce);
            row.last_error = Some(format!(
                "redemption rejected; exact compensation queued: {reason:?}"
            ));
        }
        crate::storage::save_state_to_stable(state);
        Some(nonce)
    });
    let Some(nonce) = nonce else {
        return Err(ProtocolError::TemporarilyUnavailable(
            "an existing refund record does not match this redemption; request remains held".into(),
        )
        .into());
    };
    // The generic worker may also dispatch this exact tuple. A ledger result
    // is only a candidate; the request is terminal after an exact ICRC-3 mint
    // receipt is verified and retained in the journal.
    if let Ok(candidate) =
        management::transfer_icusd_with_nonce(ICUSD::from(request.amount_e8s), caller, nonce).await
    {
        mutate_state(|state| {
            let refund = state.pending_refunds.get(&block_index).copied();
            if let Some(refund) = refund {
                if crate::state::record_redemption_v2_refund_candidate(
                    state,
                    block_index,
                    refund,
                    candidate,
                ) {
                    crate::storage::save_state_to_stable(state);
                }
            }
        });
        if let Some(row) = read_state(|state| state.redemption_v2_active.get(&caller).cloned()) {
            let _ = resume_redemption_v2_refund(caller, request, row).await;
        }
    }
    Err(reason)
}

async fn resume_redemption_v2_refund(
    caller: Principal,
    request: crate::state::RedemptionV2Request,
    _row: crate::state::RedemptionV2Journal,
) -> Result<RedemptionResult, RedemptionError> {
    let row = read_state(|state| state.redemption_v2_active.get(&caller).cloned())
        .ok_or_else(|| ProtocolError::TemporarilyUnavailable(
            "redemption compensation journal is missing; held for reconciliation".into(),
        ))?;
    let block_index = row.block_index.ok_or_else(|| ProtocolError::TemporarilyUnavailable(
        "compensation burn block is unresolved; request remains held".into(),
    ))?;
    let (Some(amount), Some(nonce)) = (row.refund_amount_e8s, row.refund_op_nonce) else {
        return Err(ProtocolError::TemporarilyUnavailable(
            "compensation identity is incomplete; request remains held".into(),
        ).into());
    };
    if row.request != request
        || row.phase != crate::state::RedemptionV2Phase::RefundPending
        || amount != request.amount_e8s
    {
        return Err(ProtocolError::TemporarilyUnavailable(
            "compensation journal differs from this request; held for reconciliation".into(),
        ).into());
    }
    let pending = read_state(|state| state.pending_refunds.get(&block_index).copied());
    if pending.is_some_and(|refund| {
        refund.user != caller || refund.amount_e8s != amount || refund.op_nonce != nonce
    }) {
        return Err(ProtocolError::TemporarilyUnavailable(
            "pending compensation tuple differs from its journal; held for reconciliation".into(),
        ).into());
    }
    if row.refund_block_index.is_none() {
        // A missing queue row is not proof of delivery. Only an existing exact
        // tuple may be dispatched, and an unknown reply keeps it pending.
        let Some(refund) = pending else {
            return Err(ProtocolError::TemporarilyUnavailable(
                "compensation queue row is missing without a receipt; held for reconciliation".into(),
            ).into());
        };
        if refund.retry_count >= crate::MAX_PENDING_RETRIES {
            return Err(ProtocolError::TemporarilyUnavailable(
                "exact compensation tuple is held; inspect the owner refund status and reconcile a proven ledger block".into(),
            ).into());
        }
        let candidate = match management::transfer_icusd_with_nonce(
            ICUSD::from(amount), caller, nonce,
        ).await {
            Ok(candidate) => candidate,
            Err(error) => {
                if matches!(error, icrc_ledger_types::icrc1::transfer::TransferError::TooOld) {
                    mutate_state(|state| {
                        if state.pending_refunds.get(&block_index) == Some(&refund) {
                            if let Some(current) = state.pending_refunds.get_mut(&block_index) {
                                current.retry_count = crate::MAX_PENDING_RETRIES;
                            }
                            crate::storage::save_state_to_stable(state);
                        }
                    });
                }
                return Err(ProtocolError::TemporarilyUnavailable(format!(
                    "exact compensation remains queued for request {}: {error:?}", request.request_id
                )).into());
            }
        };
        let recorded = mutate_state(|state| {
            let current = state.pending_refunds.get(&block_index).copied();
            if current != Some(refund)
                || !crate::state::record_redemption_v2_refund_candidate(
                    state, block_index, refund, candidate,
                )
            {
                return false;
            }
            crate::storage::save_state_to_stable(state);
            true
        });
        if !recorded {
            // The timer may have proved and finalized the same exact refund
            // while this ledger call was suspended. Report that terminal
            // outcome rather than a false unresolved conflict.
            if read_state(|state| state.redemption_v2_latest_result.get(&caller).is_some_and(|done| {
                done.request == request
                    && done.block_index == Some(block_index)
                    && done.phase == crate::state::RedemptionV2Phase::Refunded
                    && done.refund_receipt_verified
                    && done.refund_amount_e8s == Some(amount)
                    && done.refund_op_nonce == Some(nonce)
            })) {
                return Err(ProtocolError::TemporarilyUnavailable(
                    "this redemption request was refunded".into(),
                ).into());
            }
            return Err(ProtocolError::TemporarilyUnavailable(
                "compensation changed after ledger dispatch; held for reconciliation".into(),
            ).into());
        }
    }
    verify_redemption_v2_refund_candidate(caller, block_index, false)
        .await
        .map_err(|error| ProtocolError::TemporarilyUnavailable(error))?;
    Err(ProtocolError::TemporarilyUnavailable("this redemption request was refunded".into()).into())
}

/// A ledger reply is a candidate, not settlement. Both a full compensation
/// and the residual refund of a committed partial redemption use the same
/// exact mint proof before the queue entry may be removed.
async fn verify_redemption_v2_refund_candidate(
    owner: Principal,
    burn_block: u64,
    automatic: bool,
) -> Result<(), String> {
    let snapshot = mutate_state(|state| -> Result<_, String> {
        let row = state
            .redemption_v2_active
            .get_mut(&owner)
            .filter(|row| {
                row.phase == crate::state::RedemptionV2Phase::RefundPending
                    && row.block_index == Some(burn_block)
            })
            .or_else(|| {
                state.redemption_v2_latest_result.get_mut(&owner).filter(|row| {
                    row.phase == crate::state::RedemptionV2Phase::Committed
                        && row.block_index == Some(burn_block)
                })
            })
            .ok_or_else(|| "V2 refund journal is missing or has changed".to_string())?;
        if row.refund_receipt_verified {
            return Ok(None);
        }
        let (Some(amount), Some(nonce), Some(candidate)) = (
            row.refund_amount_e8s,
            row.refund_op_nonce,
            row.refund_block_index,
        ) else {
            return Err("V2 refund lacks a pinned amount, nonce, or candidate block".into());
        };
        if automatic {
            if row.refund_receipt_verification_attempts >= 3 {
                return Err("automatic V2 refund proof retries are exhausted".into());
            }
            row.refund_receipt_verification_attempts =
                row.refund_receipt_verification_attempts.saturating_add(1);
        }
        let snapshot = (row.clone(), amount, nonce, candidate);
        if automatic {
            crate::storage::save_state_to_stable(state);
        }
        Ok(Some(snapshot))
    })?;
    let Some((row, amount, nonce, candidate)) = snapshot else {
        return Ok(());
    };
    let receipt = crate::SpLiquidationStableRefundTuple {
        op_nonce: nonce,
        ledger: row.tuple.ledger,
        source: Account { owner: ic_cdk::id(), subaccount: None },
        destination: Account { owner, subaccount: None },
        principal_refund_raw: amount,
        approval_fee_refund_raw: 0,
        pull_fee_refund_raw: 0,
        amount_raw: amount,
        fee_raw: 0,
        memo: management::nonce_to_memo(nonce).0.to_vec(),
        created_at_time_ns: management::nonce_to_created_at_time(nonce),
    };
    crate::icrc3_proof::verify_sp_liquidation_refund_block(&receipt, candidate)
        .await
        .map_err(|error| format!(
            "V2 refund candidate lacks exact ICRC-3 proof; held for reconciliation: {error}"
        ))?;
    mutate_state(|state| -> Result<(), String> {
        let active = state.redemption_v2_active.get(&owner).is_some_and(|current| {
            current.phase == crate::state::RedemptionV2Phase::RefundPending
                && current.block_index == Some(burn_block)
        });
        let current = if active {
            state.redemption_v2_active.get(&owner)
        } else {
            state.redemption_v2_latest_result.get(&owner)
        }
        .ok_or_else(|| "V2 refund journal disappeared before receipt commit".to_string())?;
        if current.request != row.request
            || current.tuple != row.tuple
            || current.phase != row.phase
            || current.block_index != Some(burn_block)
            || current.refund_amount_e8s != Some(amount)
            || current.refund_op_nonce != Some(nonce)
            || current.refund_block_index != Some(candidate)
            || current.refund_receipt_verified
        {
            return Err("V2 refund journal changed before receipt commit".into());
        }
        if state.pending_refunds.get(&burn_block).is_some_and(|refund| {
            refund.user != owner || refund.amount_e8s != amount || refund.op_nonce != nonce
        }) {
            return Err("pending V2 refund tuple changed before receipt commit".into());
        }
        state.pending_refunds.remove(&burn_block);
        if active {
            let mut completed = state.redemption_v2_active.remove(&owner)
                .ok_or_else(|| "V2 refund journal disappeared".to_string())?;
            completed.phase = crate::state::RedemptionV2Phase::Refunded;
            completed.refund_receipt_verified = true;
            completed.last_error = Some("exact compensation receipt verified".into());
            state.redemption_v2_latest_result.insert(owner, completed);
        } else {
            let completed = state.redemption_v2_latest_result.get_mut(&owner)
                .ok_or_else(|| "V2 residual refund journal disappeared".to_string())?;
            completed.refund_receipt_verified = true;
        }
        crate::storage::save_state_to_stable(state);
        Ok(())
    })
}

pub(crate) async fn process_pending_redemption_v2_refund_receipts(limit: usize) {
    let candidates = read_state(|state| {
        state.redemption_v2_active.iter()
            .chain(state.redemption_v2_latest_result.iter())
            .filter_map(|(owner, row)| {
                let eligible_phase = matches!(
                    row.phase,
                    crate::state::RedemptionV2Phase::RefundPending
                        | crate::state::RedemptionV2Phase::Committed
                );
                (eligible_phase
                    && !row.refund_receipt_verified
                    && row.refund_receipt_verification_attempts < 3
                    && row.refund_block_index.is_some())
                    .then(|| row.block_index.map(|block| (*owner, block)))
                    .flatten()
            })
            .take(limit.min(8))
            .collect::<Vec<_>>()
    });
    for (owner, burn_block) in candidates {
        if let Err(error) = verify_redemption_v2_refund_candidate(owner, burn_block, true).await {
            log!(INFO, "[redemption_v2_refund] exact receipt remains unverified for burn block {}: {}", burn_block, error);
        }
    }
}

/// Attach a ledger block discovered after the original refund tuple became
/// too old to resubmit. The owner cannot nominate an arbitrary mint: its
/// exact ledger, source, destination, amount, fee, memo, and timestamp are
/// proved before the candidate is pinned or any obligation is cleared.
pub async fn reconcile_redemption_v2_refund(
    owner: Principal,
    request_id: u128,
    burn_block: u64,
    refund_block: u64,
) -> Result<bool, ProtocolError> {
    let row = read_state(|state| {
        state.redemption_v2_active.get(&owner)
            .filter(|row| row.phase == crate::state::RedemptionV2Phase::RefundPending
                && row.block_index == Some(burn_block))
            .or_else(|| state.redemption_v2_latest_result.get(&owner).filter(|row| {
                row.phase == crate::state::RedemptionV2Phase::Committed
                    && row.block_index == Some(burn_block)
            }))
            .cloned()
    }).ok_or_else(|| ProtocolError::TemporarilyUnavailable(
        "no matching unresolved V2 refund obligation exists".into(),
    ))?;
    let (Some(amount), Some(nonce)) = (row.refund_amount_e8s, row.refund_op_nonce) else {
        return Err(ProtocolError::TemporarilyUnavailable(
            "the V2 refund obligation has no exact persisted identity".into(),
        ));
    };
    if row.request.request_id != request_id
        || row.refund_receipt_verified
        || row.refund_block_index.is_some_and(|candidate| candidate != refund_block)
    {
        return Err(ProtocolError::TemporarilyUnavailable(
            "V2 refund identity differs or has already settled".into(),
        ));
    }
    if read_state(|state| state.pending_refunds.get(&burn_block).copied()).is_some_and(|pending| {
        pending.user != owner || pending.amount_e8s != amount || pending.op_nonce != nonce
    }) {
        return Err(ProtocolError::TemporarilyUnavailable(
            "the V2 refund queue differs from its persisted identity".into(),
        ));
    }
    let receipt = crate::SpLiquidationStableRefundTuple {
        op_nonce: nonce,
        ledger: row.tuple.ledger,
        source: Account { owner: ic_cdk::id(), subaccount: None },
        destination: Account { owner, subaccount: None },
        principal_refund_raw: amount,
        approval_fee_refund_raw: 0,
        pull_fee_refund_raw: 0,
        amount_raw: amount,
        fee_raw: 0,
        memo: management::nonce_to_memo(nonce).0.to_vec(),
        created_at_time_ns: management::nonce_to_created_at_time(nonce),
    };
    crate::icrc3_proof::verify_sp_liquidation_refund_block(&receipt, refund_block)
        .await
        .map_err(|error| ProtocolError::TemporarilyUnavailable(format!(
            "ledger block does not prove the exact V2 refund: {error}"
        )))?;
    mutate_state(|state| -> Result<(), ProtocolError> {
        let current = state.redemption_v2_active.get_mut(&owner)
            .filter(|current| current.phase == crate::state::RedemptionV2Phase::RefundPending
                && current.block_index == Some(burn_block))
            .or_else(|| state.redemption_v2_latest_result.get_mut(&owner).filter(|current| {
                current.phase == crate::state::RedemptionV2Phase::Committed
                    && current.block_index == Some(burn_block)
            }))
            .ok_or_else(|| ProtocolError::TemporarilyUnavailable(
                "V2 refund changed during receipt verification".into(),
            ))?;
        if current.request != row.request
            || current.tuple != row.tuple
            || current.refund_amount_e8s != Some(amount)
            || current.refund_op_nonce != Some(nonce)
            || current.refund_receipt_verified
            || current.refund_block_index.is_some_and(|candidate| candidate != refund_block)
        {
            return Err(ProtocolError::TemporarilyUnavailable(
                "V2 refund identity changed during receipt verification".into(),
            ));
        }
        if state.pending_refunds.get(&burn_block).is_some_and(|pending| {
            pending.user != owner || pending.amount_e8s != amount || pending.op_nonce != nonce
        }) {
            return Err(ProtocolError::TemporarilyUnavailable(
                "V2 refund queue changed during receipt verification".into(),
            ));
        }
        current.refund_block_index = Some(refund_block);
        crate::storage::save_state_to_stable(state);
        Ok(())
    })?;
    verify_redemption_v2_refund_candidate(owner, burn_block, false)
        .await
        .map_err(ProtocolError::TemporarilyUnavailable)?;
    Ok(true)
}

async fn refund_rejected_quoted_redemption<E>(
    caller: Principal,
    amount_e8s: u64,
    original_block: u64,
    reason: E,
) -> E {
    let refund_nonce = mutate_state(|state| state.next_op_nonce());
    match management::transfer_icusd_with_nonce(ICUSD::from(amount_e8s), caller, refund_nonce).await
    {
        Ok(_) => reason,
        Err(refund_error) => {
            mutate_state(|state| {
                persist_rejected_redemption_refund(
                    state,
                    caller,
                    original_block,
                    amount_e8s,
                    refund_nonce,
                );
            });
            log!(INFO,
                "[redeem_quoted] Full icUSD refund for rejected redemption {} is queued after transfer failure: {:?}",
                original_block, refund_error
            );
            reason
        }
    }
}

enum InboundCollateralApplied {
    Open { vault_id: u64, block_index: u64 },
    Margin { block_index: u64 },
}

fn same_inbound_collateral_intent(
    saved: &crate::state::InboundCollateralOperation,
    requested: &crate::state::InboundCollateralOperation,
) -> bool {
    match (saved, requested) {
        (
            crate::state::InboundCollateralOperation::Open {
                collateral_type: a, ..
            },
            crate::state::InboundCollateralOperation::Open {
                collateral_type: b, ..
            },
        ) => a == b,
        (
            crate::state::InboundCollateralOperation::AddMargin {
                vault_id: a,
                vault_snapshot: av,
            },
            crate::state::InboundCollateralOperation::AddMargin {
                vault_id: b,
                vault_snapshot: bv,
            },
        ) => {
            let _ = (av, bv); // Snapshot is a commit precondition, not request identity.
            a == b
        }
        _ => false,
    }
}

/// Validate caller-funded collateral ingress using the proof adapter pinned to
/// the configured ledger identity. Native ICP uses its `query_blocks` ABI;
/// other ICRC ledgers continue to require the exact ICRC-3 transfer_from block.
async fn verify_inbound_collateral_transfer_from_receipt(
    tuple: &crate::SpLiquidationStablePullTuple,
    proof_kind: crate::state::PayoutProofKind,
    block_index: u64,
) -> Result<(), String> {
    match proof_kind {
        crate::state::PayoutProofKind::NativeIcp => {
            crate::treasury::verify_native_icp_transfer_from_receipt(tuple, block_index).await
        }
        crate::state::PayoutProofKind::Icrc3 => {
            crate::icrc3_proof::verify_icrc3_transfer_from_block(tuple, block_index).await
        }
    }
}

async fn settle_inbound_collateral(
    owner: Principal,
    ledger: Principal,
    request_id: u128,
    amount_raw: u64,
    operation: crate::state::InboundCollateralOperation,
) -> Result<InboundCollateralApplied, ProtocolError> {
    // ICP ledger identity is immutable after Init. Capture its proof adapter
    // before any await and carry it through this attempt; commit rechecks the
    // same identity-derived adapter after proof verification.
    let proof_kind = read_state(|state| state.payout_proof_kind_for_ledger(ledger));
    let _dispatch_guard = crate::guard::InboundCollateralDispatchGuard::new(owner, ledger)?;
    let key = (owner, ledger);
    if let Some(done) = read_state(|s| s.inbound_collateral_latest_result.get(&key).cloned()) {
        if done.request_id == request_id {
            if done.amount_raw != amount_raw
                || !same_inbound_collateral_intent(&done.operation, &operation)
            {
                return Err(ProtocolError::GenericError(
                    "request ID was already used for a different collateral intent".into(),
                ));
            }
            return match done.result {
                crate::state::InboundCollateralResult::Open {
                    vault_id,
                    block_index,
                } => Ok(InboundCollateralApplied::Open {
                    vault_id,
                    block_index,
                }),
                crate::state::InboundCollateralResult::AddMargin { block_index } => {
                    Ok(InboundCollateralApplied::Margin { block_index })
                }
                crate::state::InboundCollateralResult::Rejected { message } => {
                    Err(ProtocolError::TemporarilyUnavailable(message))
                }
            };
        }
    }
    let mut row = if let Some(row) =
        read_state(|s| crate::state::inbound_collateral_journal(s, owner, ledger))
    {
        if row.request_id != request_id
            || row.tuple.amount_raw != amount_raw
            || !same_inbound_collateral_intent(&row.operation, &operation)
        {
            return Err(ProtocolError::TemporarilyUnavailable(
                "another collateral transfer for this owner and ledger is unresolved".into(),
            ));
        }
        row
    } else {
        let fee = management::get_ledger_fee(ledger)
            .await
            .map_err(ProtocolError::GenericError)?;
        let nonce = mutate_state(|s| s.next_op_nonce());
        let backend = ic_cdk::id();
        let tuple = crate::SpLiquidationStablePullTuple {
            op_nonce: nonce,
            ledger,
            from: Account {
                owner,
                subaccount: None,
            },
            spender: Account {
                owner: backend,
                subaccount: None,
            },
            to: Account {
                owner: backend,
                subaccount: None,
            },
            amount_raw,
            fee_raw: fee,
            memo: management::nonce_to_memo(nonce).0.to_vec(),
            created_at_time_ns: management::nonce_to_created_at_time(nonce),
        };
        mutate_state(|s| {
            crate::state::admit_inbound_collateral(
                s,
                crate::state::InboundCollateralJournal {
                    owner,
                    request_id,
                    operation: operation.clone(),
                    tuple,
                    candidate_block_index: None,
                    candidate_attach_window_start_ns: 0,
                    candidate_attach_attempts: 0,
                    had_ambiguous_attempt: false,
                    last_error: None,
                },
            )
        })
        .map_err(ProtocolError::GenericError)?;
        read_state(|s| crate::state::inbound_collateral_journal(s, owner, ledger)).ok_or_else(
            || ProtocolError::GenericError("inbound journal failed to persist".into()),
        )?
    };
    let operation = row.operation.clone();

    let block = if let Some(candidate) = row.candidate_block_index {
        candidate
    } else {
        let prior_ambiguity = row.had_ambiguous_attempt;
        mutate_state(|s| {
            crate::state::set_inbound_collateral_attempt(
                s, owner, ledger, &row.tuple, None, true, None,
            )
        })
        .map_err(ProtocolError::GenericError)?;
        match management::transfer_from_with_exact_tuple_outcome(&row.tuple).await {
            management::ExactTransferFromOutcome::Applied(block) => block,
            management::ExactTransferFromOutcome::ProvenNoEffect(error) if !prior_ambiguity => {
                if let icrc_ledger_types::icrc2::transfer_from::TransferFromError::BadFee {
                    expected_fee,
                } = &error
                {
                    if let Ok(expected_fee) = u64::try_from(expected_fee.0.clone()) {
                        let nonce = mutate_state(|s| s.next_op_nonce());
                        row.tuple.op_nonce = nonce;
                        row.tuple.fee_raw = expected_fee;
                        row.tuple.memo = management::nonce_to_memo(nonce).0.to_vec();
                        row.tuple.created_at_time_ns = management::nonce_to_created_at_time(nonce);
                        row.had_ambiguous_attempt = false;
                        row.last_error = Some(
                            "first dispatch proved no effect; exact fee tuple repriced".into(),
                        );
                        mutate_state(|s| {
                            s.pending_inbound_collateral.insert(key, row.clone());
                            crate::storage::save_state_to_stable(s);
                        });
                        return Err(ProtocolError::TemporarilyUnavailable(
                            "collateral fee changed; exact tuple was safely repriced for retry"
                                .into(),
                        ));
                    }
                }
                mutate_state(|s| {
                    s.pending_inbound_collateral.remove(&key);
                    s.inbound_collateral_latest_result.insert(
                        key,
                        crate::state::CompletedInboundCollateral {
                            request_id,
                            operation: operation.clone(),
                            tuple: row.tuple.clone(),
                            amount_raw,
                            result: crate::state::InboundCollateralResult::Rejected {
                                message: format!("typed no-effect: {error:?}"),
                            },
                        },
                    );
                    crate::storage::save_state_to_stable(s);
                });
                return Err(ProtocolError::TransferFromError(error, amount_raw));
            }
            management::ExactTransferFromOutcome::ProvenNoEffect(error) => {
                mutate_state(|s| {
                    crate::state::set_inbound_collateral_attempt(
                        s,
                        owner,
                        ledger,
                        &row.tuple,
                        None,
                        true,
                        Some(format!(
                            "typed no-effect after earlier ambiguous dispatch: {error:?}"
                        )),
                    )
                })
                .map_err(ProtocolError::GenericError)?;
                return Err(ProtocolError::TemporarilyUnavailable(
                    "an earlier collateral pull may have committed; exact tuple remains held"
                        .into(),
                ));
            }
            management::ExactTransferFromOutcome::AmbiguousLedgerError(error) => {
                mutate_state(|s| {
                    crate::state::set_inbound_collateral_attempt(
                        s,
                        owner,
                        ledger,
                        &row.tuple,
                        None,
                        true,
                        Some(format!("ambiguous ICRC-2 response: {error:?}")),
                    )
                })
                .map_err(ProtocolError::GenericError)?;
                return Err(ProtocolError::TemporarilyUnavailable(
                    "collateral pull outcome is ambiguous; exact tuple is held for retry".into(),
                ));
            }
            management::ExactTransferFromOutcome::CallRejected { code, message } => {
                mutate_state(|s| {
                    crate::state::set_inbound_collateral_attempt(
                        s,
                        owner,
                        ledger,
                        &row.tuple,
                        None,
                        true,
                        Some(format!("call rejected {code}: {message}")),
                    )
                })
                .map_err(ProtocolError::GenericError)?;
                return Err(ProtocolError::TemporarilyUnavailable(
                    "collateral pull call was rejected after dispatch; exact tuple is held".into(),
                ));
            }
            management::ExactTransferFromOutcome::InvalidBlockIndex => {
                mutate_state(|s| {
                    crate::state::set_inbound_collateral_attempt(
                        s,
                        owner,
                        ledger,
                        &row.tuple,
                        None,
                        true,
                        Some("ledger returned an unrepresentable block index".into()),
                    )
                })
                .map_err(ProtocolError::GenericError)?;
                return Err(ProtocolError::TemporarilyUnavailable(
                    "collateral pull receipt index is unavailable; exact tuple is held".into(),
                ));
            }
        }
    };

    mutate_state(|s| {
        crate::state::set_inbound_collateral_attempt(
            s,
            owner,
            ledger,
            &row.tuple,
            Some(block),
            true,
            None,
        )
    })
    .map_err(ProtocolError::GenericError)?;
    if let Err(error) =
        verify_inbound_collateral_transfer_from_receipt(&row.tuple, proof_kind, block).await
    {
        mutate_state(|s| {
            crate::state::set_inbound_collateral_attempt(
                s,
                owner,
                ledger,
                &row.tuple,
                Some(block),
                true,
                Some(format!("exact collateral transfer proof pending: {error}")),
            )
        })
        .map_err(ProtocolError::GenericError)?;
        return Err(ProtocolError::TemporarilyUnavailable(
            "collateral transfer has a candidate receipt but exact transfer proof is pending"
                .into(),
        ));
    }

    mutate_state(|s| -> Result<InboundCollateralApplied, String> {
        let current = s
            .pending_inbound_collateral
            .get(&key)
            .ok_or_else(|| "inbound collateral journal disappeared before credit".to_string())?;
        if current.tuple != row.tuple
            || current.candidate_block_index != Some(block)
            || current.operation != operation
            || current.owner != owner
            || s.payout_proof_kind_for_ledger(current.tuple.ledger) != proof_kind
        {
            return Err("inbound collateral journal changed before credit".into());
        }
        let result = match &operation {
            crate::state::InboundCollateralOperation::Open {
                collateral_type,
                reserved_vault_id,
            } => {
                let vault_id = *reserved_vault_id;
                if s.vault_id_to_vaults.contains_key(&vault_id) {
                    return Err("reserved vault ID was consumed before open settlement".into());
                }
                crate::event::record_open_vault(
                    s,
                    Vault {
                        owner,
                        borrowed_icusd_amount: 0.into(),
                        collateral_amount: amount_raw,
                        vault_id,
                        collateral_type: *collateral_type,
                        last_accrual_time: ic_cdk::api::time(),
                        accrued_interest: ICUSD::new(0),
                        bot_processing: false,
                    },
                    block,
                );
                InboundCollateralApplied::Open {
                    vault_id,
                    block_index: block,
                }
            }
            crate::state::InboundCollateralOperation::AddMargin {
                vault_id,
                vault_snapshot,
            } => {
                let Some(current_vault) = s.vault_id_to_vaults.get(vault_id) else {
                    return Err(
                        "vault closed while collateral pull was pending; exact receipt is held"
                            .into(),
                    );
                };
                if current_vault.owner != vault_snapshot.owner
                    || current_vault.collateral_type != vault_snapshot.collateral_type
                    || current_vault.collateral_amount != vault_snapshot.collateral_amount
                {
                    return Err(
                        "vault changed while collateral pull was pending; exact receipt is held"
                            .into(),
                    );
                }
                if checked_margin_balance(vault_snapshot.collateral_amount, amount_raw).is_none() {
                    return Err(
                        "margin receipt exceeds representable vault balance; exact receipt is held"
                            .into(),
                    );
                }
                crate::event::record_add_margin_to_vault_for(
                    s,
                    *vault_id,
                    ICP::from(amount_raw),
                    block,
                    owner,
                );
                InboundCollateralApplied::Margin { block_index: block }
            }
        };
        let saved_result = match &result {
            InboundCollateralApplied::Open {
                vault_id,
                block_index,
            } => crate::state::InboundCollateralResult::Open {
                vault_id: *vault_id,
                block_index: *block_index,
            },
            InboundCollateralApplied::Margin { block_index } => {
                crate::state::InboundCollateralResult::AddMargin {
                    block_index: *block_index,
                }
            }
        };
        s.pending_inbound_collateral.remove(&key);
        s.inbound_collateral_latest_result.insert(
            key,
            crate::state::CompletedInboundCollateral {
                request_id,
                operation: operation.clone(),
                tuple: row.tuple.clone(),
                amount_raw,
                result: saved_result,
            },
        );
        crate::storage::save_state_to_stable(s);
        Ok(result)
    })
    .map_err(ProtocolError::TemporarilyUnavailable)
}

pub async fn resume_pending_inbound_collateral() {
    let (selected_keys, pending) = read_state(|s| {
        let keys = s
            .pending_inbound_collateral
            .keys()
            .copied()
            .collect::<Vec<_>>();
        let after = s.inbound_collateral_resume_cursor;
        let mut selected = keys
            .iter()
            .copied()
            .filter(|key| after.is_none_or(|cursor| *key > cursor))
            .take(8)
            .collect::<Vec<_>>();
        if selected.is_empty() {
            selected = keys.into_iter().take(8).collect();
        }
        let rows = selected
            .iter()
            .filter_map(|key| s.pending_inbound_collateral.get(key).cloned())
            .collect::<Vec<_>>();
        (selected, rows)
    });
    if let Some(cursor) = selected_keys.last().copied() {
        mutate_state(|s| {
            s.inbound_collateral_resume_cursor = Some(cursor);
            crate::storage::save_state_to_stable(s);
        });
    }
    for row in pending {
        let amount = row.tuple.amount_raw;
        match settle_inbound_collateral(
            row.owner,
            row.tuple.ledger,
            row.request_id,
            amount,
            row.operation.clone(),
        )
        .await
        {
            Ok(_) => {}
            Err(error) => log!(
                INFO,
                "[inbound collateral recovery] owner={} ledger={} remains pending: {:?}",
                row.owner,
                row.tuple.ledger,
                error
            ),
        }
    }
}

/// Attach a caller-supplied positive receipt candidate to an exact pending
/// request. The caller cannot clear the row: the full pinned tuple must first
/// verify through ICRC-3, and settlement remains the normal idempotent path.
pub async fn attach_inbound_collateral_receipt_candidate(
    owner: Principal,
    ledger: Principal,
    request_id: u128,
    block_index: u64,
) -> Result<(), ProtocolError> {
    let row = read_state(|s| {
        s.pending_inbound_collateral
            .get(&(owner, ledger))
            .filter(|row| row.request_id == request_id)
            .cloned()
    })
    .ok_or_else(|| ProtocolError::GenericError("no matching active collateral request".into()))?;
    let _dispatch_guard = crate::guard::InboundCollateralDispatchGuard::new(owner, ledger)?;
    let current = read_state(|s| {
        s.pending_inbound_collateral
            .get(&(owner, ledger))
            .filter(|current| current.request_id == request_id && current.tuple == row.tuple)
            .cloned()
    })
    .ok_or_else(|| {
        ProtocolError::TemporarilyUnavailable(
            "collateral request changed during receipt attachment".into(),
        )
    })?;
    let proof_kind = read_state(|s| s.payout_proof_kind_for_ledger(current.tuple.ledger));
    if let Some(existing) = current.candidate_block_index {
        return if existing == block_index {
            Ok(())
        } else {
            Err(ProtocolError::GenericError(
                "a different candidate receipt is already attached".into(),
            ))
        };
    }
    mutate_state(|s| {
        crate::state::reserve_inbound_candidate_verification_attempt(
            s,
            owner,
            ledger,
            request_id,
            &current.tuple,
            ic_cdk::api::time(),
        )
    })
    .map_err(ProtocolError::TemporarilyUnavailable)?;
    verify_inbound_collateral_transfer_from_receipt(&current.tuple, proof_kind, block_index)
        .await
        .map_err(|error| {
            ProtocolError::GenericError(format!(
                "candidate block does not prove the pinned collateral pull: {error}"
            ))
        })?;
    mutate_state(|s| {
        let latest = s
            .pending_inbound_collateral
            .get(&(owner, ledger))
            .ok_or_else(|| {
                "pending collateral request disappeared after receipt proof".to_string()
            })?;
        if latest.request_id != request_id
            || latest.tuple != current.tuple
            || latest.operation != current.operation
            || s.payout_proof_kind_for_ledger(latest.tuple.ledger) != proof_kind
        {
            return Err("pending collateral request changed after receipt proof".to_string());
        }
        if latest
            .candidate_block_index
            .is_some_and(|existing| existing != block_index)
        {
            return Err("a different candidate receipt is already attached".to_string());
        }
        crate::state::set_inbound_collateral_attempt(
            s,
            owner,
            ledger,
            &current.tuple,
            Some(block_index),
            current.had_ambiguous_attempt,
            None,
        )
    })
    .map_err(ProtocolError::GenericError)
}

pub async fn open_vault_with_request_id(
    request_id: u128,
    collateral_amount_raw: u64,
    collateral_type_opt: Option<Principal>,
) -> Result<OpenVaultSuccess, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let requested_collateral_type =
        collateral_type_opt.unwrap_or_else(|| read_state(|s| s.icp_collateral_type()));
    let requested_ledger = read_state(|s| {
        s.get_collateral_config(&requested_collateral_type)
            .map(|config| config.ledger_canister_id)
    })
    .ok_or_else(|| ProtocolError::GenericError("Collateral type not supported.".into()))?;
    if let Some(done) = read_state(|s| {
        crate::state::completed_inbound_collateral(s, caller, requested_ledger, request_id)
    }) {
        let row = done;
        if row.amount_raw != collateral_amount_raw
            || !matches!(&row.operation, crate::state::InboundCollateralOperation::Open { collateral_type, .. }
                if *collateral_type == requested_collateral_type)
        {
            return Err(ProtocolError::GenericError(
                "request ID was already used for a different collateral intent".into(),
            ));
        }
        return match row.result {
            crate::state::InboundCollateralResult::Open {
                vault_id,
                block_index,
            } => Ok(OpenVaultSuccess {
                vault_id,
                block_index,
            }),
            crate::state::InboundCollateralResult::Rejected { message } => {
                Err(ProtocolError::TemporarilyUnavailable(message))
            }
            crate::state::InboundCollateralResult::AddMargin { .. } => Err(
                ProtocolError::GenericError("request ID operation kind mismatch".into()),
            ),
        };
    }
    // Pass operation name to guard for better tracking
    let guard_principal = match GuardPrincipal::new(caller, "open_vault") {
        Ok(guard) => guard,
        Err(GuardError::AlreadyProcessing) => {
            log!(
                INFO,
                "[open_vault] Principal {:?} already has an ongoing operation",
                caller
            );
            return Err(ProtocolError::AlreadyProcessing);
        }
        Err(GuardError::StaleOperation) => {
            log!(
                INFO,
                "[open_vault] Principal {:?} has a stale operation that's being cleaned up",
                caller
            );
            return Err(ProtocolError::TemporarilyUnavailable(
                "Previous operation is being cleaned up. Please try again in a few seconds."
                    .to_string(),
            ));
        }
        Err(err) => return Err(err.into()),
    };

    // Resolve collateral type: default to ICP if not specified
    let collateral_type = requested_collateral_type;

    // Look up CollateralConfig; check status is Active
    let (config_ledger, config_status, min_deposit, is_native_xrp) =
        read_state(|s| match s.get_collateral_config(&collateral_type) {
            Some(config) => Ok((
                config.ledger_canister_id,
                config.status,
                config.min_collateral_deposit,
                config.is_native_xrp(),
            )),
            None => Err(ProtocolError::GenericError(
                "Collateral type not supported.".to_string(),
            )),
        })?;

    // P2: native-XRP collateral is custodied on the XRP Ledger (chains::xrp), not
    // pulled via an ICRC `transfer_from`. Its deposit flow (open-then-verify) is
    // wired in P3; until then reject opens through this ICRC path so XRP collateral
    // can never be silently mishandled as an ICRC token.
    if is_native_xrp {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "Native-XRP collateral uses the XRP deposit flow (not yet enabled).".to_string(),
        ));
    }

    if !config_status.allows_open() {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "Collateral type is not accepting new vaults.".to_string(),
        ));
    }

    let icp_margin_amount: ICP = collateral_amount_raw.into();

    if min_deposit > 0 && icp_margin_amount < ICP::new(min_deposit) {
        guard_principal.fail();
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: min_deposit,
        });
    }

    match settle_inbound_collateral(
        caller,
        config_ledger,
        request_id,
        collateral_amount_raw,
        crate::state::InboundCollateralOperation::Open {
            collateral_type,
            reserved_vault_id: 0,
        },
    )
    .await
    {
        Ok(InboundCollateralApplied::Open {
            vault_id,
            block_index,
        }) => {
            guard_principal.complete();
            Ok(OpenVaultSuccess {
                vault_id,
                block_index,
            })
        }
        Ok(InboundCollateralApplied::Margin { .. }) => {
            guard_principal.fail();
            Err(ProtocolError::GenericError(
                "inbound operation kind mismatch".into(),
            ))
        }
        Err(error) => {
            guard_principal.fail();
            Err(error)
        }
    }
}

/// Legacy no-ID opens cannot distinguish an intentional equal deposit from a
/// retry after a lost successful canister reply. Keep them fail-closed.
pub async fn open_vault(
    _collateral_amount_raw: u64,
    _collateral_type_opt: Option<Principal>,
) -> Result<OpenVaultSuccess, ProtocolError> {
    Err(ProtocolError::TemporarilyUnavailable(
        "use open_vault_v2 with a stable request ID".into(),
    ))
}

/// Compound open-vault-and-borrow in a single canister call.
///
/// Uses ICRC-2 `transfer_from` (like `open_vault`) to pull collateral, creates
/// the vault, then immediately borrows `borrow_amount_raw` icUSD — all under a
/// single guard.  This allows Oisy / ICRC-112 signer wallets to batch
/// `icrc2_approve` + `open_vault_and_borrow` into **one** popup instead of the
/// three sequential popups that separate `open_vault` + `borrow_from_vault`
/// would require.
/// P3 return value for `open_xrp_vault`: the reserved vault id and the XRPL custody
/// address the user funds.
#[derive(candid::CandidType, Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct XrpVaultOpenInfo {
    pub vault_id: u64,
    pub custody_address: String,
    pub reserve_base_drops: u64,
}

fn require_xrp_production_key() -> Result<(), ProtocolError> {
    let configured_key = crate::chains::xrp::config::xrp_schnorr_key_name();
    if crate::chains::xrp::config::is_xrp_production_key_name(&configured_key) {
        return Ok(());
    }
    Err(ProtocolError::GenericError(format!(
        "native-XRP operations require production Schnorr key key_1 (configured: {configured_key})"
    )))
}

/// P3 (native-XRP collateral): open a vault in the open-then-verify staging area.
/// Derives the per-vault XRPL custody address (threshold Ed25519), records an
/// `XrpPendingDeposit` under a freshly reserved vault_id, and returns the address
/// for the user to fund. NO collateral is credited and NO icUSD is minted until
/// `confirm_xrp_deposit` verifies the deposit. Errors if native-XRP collateral is
/// not registered (P5) or is not accepting new vaults.
pub async fn open_xrp_vault() -> Result<XrpVaultOpenInfo, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let guard_principal = GuardPrincipal::new(caller, "open_xrp_vault")?;
    if let Err(e) = require_xrp_production_key() {
        guard_principal.fail();
        return Err(e);
    }

    let xrp_ct = crate::state::xrp_collateral_principal();
    let cfg = read_state(|s| {
        s.get_collateral_config(&xrp_ct)
            .map(|c| (c.status, c.is_native_xrp()))
    });
    match cfg {
        Some((status, true)) => {
            if !status.allows_open() {
                guard_principal.fail();
                return Err(ProtocolError::GenericError(
                    "XRP collateral is not accepting new vaults.".to_string(),
                ));
            }
        }
        Some((_, false)) => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(
                "XRP collateral is misconfigured (custody is not native-XRP).".to_string(),
            ));
        }
        None => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(
                "XRP collateral is not registered.".to_string(),
            ));
        }
    }

    // Hardening (P3/P4 review): bound per-caller pending deposits so a caller can't
    // spam unfunded opens (each would consume a vault_id + a threshold derivation +
    // a persisted state entry).
    const MAX_XRP_PENDING_PER_CALLER: usize = 10;
    // Global cap bounds total persisted pending-deposit state (and the O(N) per-caller
    // scan below) across all callers — safe (refuses NEW opens when full; never
    // orphans an existing entry, unlike a TTL prune of a maybe-funded deposit).
    const MAX_XRP_PENDING_GLOBAL: usize = 10_000;
    let (global_pending, caller_pending) = read_state(|s| {
        (
            s.xrp_pending_deposits.len(),
            s.xrp_pending_deposits
                .values()
                .filter(|d| d.owner == caller)
                .count(),
        )
    });
    if global_pending >= MAX_XRP_PENDING_GLOBAL {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "XRP deposit staging is full; please retry after pending deposits clear.".to_string(),
        ));
    }
    if caller_pending >= MAX_XRP_PENDING_PER_CALLER {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "Too many open XRP deposits; confirm or settle existing ones first.".to_string(),
        ));
    }

    let reserve_base_drops = match crate::chains::xrp::xrp_rpc::fetch_reserve_base().await {
        Ok(r) => match u64::try_from(r) {
            Ok(drops) => drops,
            Err(_) => {
                guard_principal.fail();
                return Err(ProtocolError::GenericError(
                    "xrp reserve base exceeds u64 drops".to_string(),
                ));
            }
        },
        Err(e) => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(format!(
                "xrp server_state failed: {e}"
            )));
        }
    };

    // Reserve a vault_id (also the threshold-derivation nonce). A derive failure
    // below just leaves a gap in the id sequence, which is harmless.
    let vault_id = mutate_state(|s| s.increment_vault_id());

    let path = crate::chains::xrp::ted25519::custody_derivation_path(
        crate::chains::xrp::XRP_CHAIN_ID,
        caller,
        vault_id,
    );
    let custody_address = match crate::chains::xrp::ted25519::derive_xrp_address(path).await {
        Ok((_pubkey, addr)) => addr,
        Err(e) => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(format!(
                "xrp custody derive failed: {e}"
            )));
        }
    };

    let opened_at_ns = ic_cdk::api::time();
    mutate_state(|s| {
        s.xrp_pending_deposits.insert(
            vault_id,
            crate::state::XrpPendingDeposit {
                owner: caller,
                custody_address: custody_address.clone(),
                derivation_nonce: vault_id,
                opened_at_ns,
                reserve_base_drops,
            },
        );
    });

    guard_principal.complete();
    Ok(XrpVaultOpenInfo {
        vault_id,
        custody_address,
        reserve_base_drops,
    })
}

/// Pure: collateral drops to credit from a verified XRP custody balance, net of the
/// base reserve the user funds. Errors if nothing is creditable (balance ≤ reserve),
/// the net exceeds u64 drops, or the net is below the per-collateral minimum.
pub(crate) fn xrp_credit_amount(
    balance_drops: u128,
    reserve_base: u128,
    min_deposit: u64,
) -> Result<u64, ProtocolError> {
    let net = balance_drops.saturating_sub(reserve_base);
    let credited = u64::try_from(net)
        .map_err(|_| ProtocolError::GenericError("XRP balance exceeds u64 drops".to_string()))?;
    if credited == 0 || credited < min_deposit {
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: min_deposit.max(1),
        });
    }
    Ok(credited)
}

/// P3 (native-XRP collateral): verify the user's XRP deposit to the vault's custody
/// address and credit it as collateral, creating a real `Vault` with zero debt. The
/// user then borrows icUSD via the normal `borrow_from_vault` (the borrow→mint path
/// is collateral-generic and mints on the IC). Owner-only and idempotent: the
/// pending entry is removed on success, so a second call errors. Credits
/// `balance - reserve_base` drops — the user funds the XRPL base reserve returned
/// by server_state, which stays locked at the custody account. Returns the credited
/// drops.
pub async fn confirm_xrp_deposit(vault_id: u64) -> Result<u64, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let guard_principal =
        GuardPrincipal::new(caller, &format!("confirm_xrp_deposit_{}", vault_id))?;
    if let Err(e) = require_xrp_production_key() {
        guard_principal.fail();
        return Err(e);
    }

    let pending = match read_state(|s| s.xrp_pending_deposits.get(&vault_id).cloned()) {
        Some(p) => p,
        None => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(
                "No pending XRP deposit for this vault (already confirmed or unknown).".to_string(),
            ));
        }
    };
    if pending.owner != caller {
        guard_principal.fail();
        return Err(ProtocolError::CallerNotOwner);
    }

    let xrp_ct = crate::state::xrp_collateral_principal();
    let min_deposit = read_state(|s| {
        s.get_collateral_config(&xrp_ct)
            .map(|c| c.min_collateral_deposit)
            .unwrap_or(0)
    });

    // Verify on the XRP Ledger (consensus-retry-wrapped reads).
    let acct = match crate::chains::xrp::xrp_rpc::fetch_account_info(&pending.custody_address).await
    {
        Ok(a) => a,
        Err(e) => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(format!(
                "xrp account_info failed: {e}"
            )));
        }
    };
    if !acct.exists {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "XRP custody account is unfunded; deposit not yet received.".to_string(),
        ));
    }
    let reserve = if pending.reserve_base_drops > 0 {
        u128::from(pending.reserve_base_drops)
    } else {
        match crate::chains::xrp::xrp_rpc::fetch_reserve_base().await {
            Ok(r) => r,
            Err(e) => {
                guard_principal.fail();
                return Err(ProtocolError::GenericError(format!(
                    "xrp server_state failed: {e}"
                )));
            }
        }
    };

    // Credit balance net of the reserve quoted when the deposit address was
    // prepared. Legacy pending deposits did not store it, so they fall back to a
    // live reserve read above.
    let credited = match xrp_credit_amount(acct.balance_drops, reserve, min_deposit) {
        Ok(c) => c,
        Err(e) => {
            guard_principal.fail();
            return Err(e);
        }
    };

    // Atomically: re-check the pending entry still exists (no concurrent confirm
    // slipped in during the awaits), create the vault, and clear the pending entry.
    let created = mutate_state(|s| {
        if !s.xrp_pending_deposits.contains_key(&vault_id) {
            return false;
        }
        record_open_vault(
            s,
            Vault {
                owner: caller,
                borrowed_icusd_amount: 0.into(),
                collateral_amount: credited,
                vault_id,
                collateral_type: xrp_ct,
                last_accrual_time: ic_cdk::api::time(),
                accrued_interest: ICUSD::new(0),
                bot_processing: false,
            },
            acct.ledger_index as u64,
        );
        s.xrp_pending_deposits.remove(&vault_id);
        true
    });

    if !created {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "XRP deposit was already confirmed concurrently.".to_string(),
        ));
    }

    guard_principal.complete();
    Ok(credited)
}

/// P4: record an unsettled XRP collateral claim and return its id. The OUT-paths
/// (withdraw / liquidation / redemption) call this instead of an ICRC transfer when
/// the collateral is native-XRP. `custody_owner`+`custody_nonce` (the source vault's
/// owner + id) locate the XRPL custody address the protocol later pays the claimant
/// from via `settle_xrp_claim`.
pub(crate) fn record_xrp_claim(
    s: &mut crate::state::State,
    claimant: Principal,
    custody_owner: Principal,
    custody_nonce: u64,
    drops: u64,
    now_ns: u64,
) -> u64 {
    let claim_id = s.next_xrp_claim_id;
    s.next_xrp_claim_id = s.next_xrp_claim_id.wrapping_add(1);
    s.xrp_claims.insert(
        claim_id,
        crate::state::XrpClaim {
            claimant,
            drops,
            custody_owner,
            custody_nonce,
            created_at_ns: now_ns,
            settlement: None,
            quarantine_reason: None,
        },
    );
    claim_id
}

/// P4: queue a collateral payout to `recipient`. ICRC collateral -> a
/// PendingMarginTransfer (the ICRC transfer machinery pays it). Native-XRP -> an
/// XrpClaim instead (settled later via settle_xrp_claim from the vault's custody
/// address); native-XRP therefore never enters the ICRC pending-transfer flow.
/// `custody_owner` is the SOURCE vault's owner (its threshold key controls the
/// custody address), captured while the vault is in hand — safe even when
/// cleanup_if_drained removes the vault immediately after.
type PendingPayoutRef = (crate::event::PendingPayoutKind, u128);

fn queue_collateral_payout(
    s: &mut crate::state::State,
    vault_id: u64,
    custody_owner: Principal,
    recipient: Principal,
    margin: ICP,
    collateral_type: Principal,
    op_nonce: u128,
    now_ns: u64,
    immediate_payouts: &mut Vec<PendingPayoutRef>,
) -> Option<u64> {
    let is_xrp = s
        .get_collateral_config(&collateral_type)
        .map(|c| c.is_native_xrp())
        .unwrap_or(false);
    if is_xrp {
        Some(record_xrp_claim(
            s,
            recipient,
            custody_owner,
            vault_id,
            margin.to_u64(),
            now_ns,
        ))
    } else {
        let (ledger, fee) = s
            .get_collateral_config(&collateral_type)
            .map(|config| (config.ledger_canister_id, config.ledger_fee))
            .unwrap_or((s.icp_ledger_principal, s.icp_ledger_fee.to_u64()));
        let transfer_amount_raw = margin.to_u64().saturating_sub(fee);
        let transfer = PendingMarginTransfer {
            vault_id,
            owner: recipient,
            margin,
            collateral_type,
            retry_count: 0,
            op_nonce,
            ledger: Some(ledger),
            transfer_amount_raw: Some(transfer_amount_raw),
            redemption_transfer: None,
            held_for_manual_retry: transfer_amount_raw == 0,
            reconciliation_required: transfer_amount_raw == 0,
            min_net_collateral_raw: None,
        };
        crate::event::record_pending_payout_queued(
            s,
            op_nonce,
            crate::event::PendingPayoutKind::Margin,
            transfer,
        );
        immediate_payouts.push((crate::event::PendingPayoutKind::Margin, op_nonce));
        None
    }
}

pub const XRP_SP_ABSORB_PREFLIGHT_TTL_NS: u64 = 15 * 60 * 1_000_000_000;

#[derive(Clone, Debug, PartialEq, Eq)]
struct XrpSpAbsorbSizing {
    preflight: crate::XrpSpAbsorbPreflight,
    total_to_seize_drops: u64,
}

fn ensure_registered_sp(
    state: &crate::state::State,
    caller: Principal,
) -> Result<(), ProtocolError> {
    if state.stability_pool_canister != Some(caller) {
        return Err(ProtocolError::GenericError(
            "Caller is not the registered stability pool canister".to_string(),
        ));
    }
    Ok(())
}

pub fn stability_pool_xrp_claim_outstanding_in_state(
    state: &crate::state::State,
    caller: Principal,
    claim_id: u64,
    claimant: Principal,
) -> Result<bool, ProtocolError> {
    ensure_registered_sp(state, caller)?;
    match state.xrp_claims.get(&claim_id) {
        Some(claim) if claim.claimant == claimant => Ok(true),
        Some(_) => Err(ProtocolError::GenericError(format!(
            "XRP claim #{claim_id} belongs to a different claimant"
        ))),
        None => Ok(false),
    }
}

/// Pure validation for `stability_pool_settle_xrp_claim` (the SP-driven
/// auto-settlement sweep): the caller must be the registered stability pool,
/// the claim must exist and belong to the claimant the SP is settling for, and
/// quarantined claims are refused (F-03 — possibly already paid under a
/// divergent hash; only admin_resolve_xrp_claim may touch them).
///
/// The "No such XRP claim" wording matches the claimant path so the SP sweep's
/// outstanding-check semantics stay consistent: missing == settled-or-unknown.
pub fn validate_sp_settle_xrp_claim_in_state(
    state: &crate::state::State,
    caller: Principal,
    claim_id: u64,
    claimant: Principal,
) -> Result<(), ProtocolError> {
    ensure_registered_sp(state, caller)?;
    let claim = state.xrp_claims.get(&claim_id).ok_or_else(|| {
        ProtocolError::GenericError("No such XRP claim (already settled or unknown).".to_string())
    })?;
    if claim.claimant != claimant {
        return Err(ProtocolError::GenericError(format!(
            "XRP claim #{claim_id} belongs to a different claimant"
        )));
    }
    if let Some(reason) = &claim.quarantine_reason {
        return Err(ProtocolError::GenericError(format!(
            "XRP claim #{claim_id} is quarantined ({reason}); awaiting admin reconciliation."
        )));
    }
    Ok(())
}

fn xrp_sp_absorb_sizing(
    state: &crate::state::State,
    vault_id: u64,
    expected_icusd_burn_e8s: u64,
) -> Result<XrpSpAbsorbSizing, ProtocolError> {
    if expected_icusd_burn_e8s == 0 {
        return Err(ProtocolError::GenericError(
            "XRP SP absorb burn amount must be non-zero".to_string(),
        ));
    }
    let vault = state
        .vault_id_to_vaults
        .get(&vault_id)
        .ok_or_else(|| ProtocolError::GenericError(format!("Vault #{vault_id} not found")))?;
    let cfg = state
        .get_collateral_config(&vault.collateral_type)
        .ok_or_else(|| {
            ProtocolError::GenericError(format!("No collateral config for vault #{vault_id}"))
        })?;
    if !cfg.is_native_xrp() {
        return Err(ProtocolError::GenericError(
            "XRP SP absorb requires a native-XRP vault".to_string(),
        ));
    }
    if !cfg.status.allows_liquidation() {
        return Err(ProtocolError::GenericError(
            "Liquidation is not allowed for this collateral type.".to_string(),
        ));
    }
    if vault.borrowed_icusd_amount.to_u64() != expected_icusd_burn_e8s {
        return Err(ProtocolError::GenericError(format!(
            "XRP SP absorb burn {} does not match live debt {} for vault {}",
            expected_icusd_burn_e8s,
            vault.borrowed_icusd_amount.to_u64(),
            vault_id
        )));
    }
    let price = state
        .get_collateral_price_decimal(&vault.collateral_type)
        .ok_or_else(|| {
            ProtocolError::GenericError(
                "No price available for collateral. Price feed may be down.".to_string(),
            )
        })?;
    let price_usd = UsdIcp::from(price);
    let cr = compute_collateral_ratio(vault, price_usd, state);
    let min_liq = state.get_min_liquidation_ratio_for(&vault.collateral_type);
    if cr >= min_liq {
        return Err(ProtocolError::GenericError(format!(
            "native-XRP vault {vault_id} is no longer liquidatable"
        )));
    }

    let liquidation_amount = ICUSD::new(expected_icusd_burn_e8s);
    let collateral_raw =
        crate::numeric::try_icusd_to_collateral_amount(liquidation_amount, price, cfg.decimals)
            .ok_or_else(|| {
                ProtocolError::GenericError(
                    "Cannot size native-XRP absorb collateral: conversion is unrepresentable"
                        .to_string(),
                )
            })?;
    let collateral_with_bonus =
        ICP::from(collateral_raw) * state.get_liquidation_bonus_for(&vault.collateral_type);
    let total_to_seize = collateral_with_bonus.min(ICP::from(vault.collateral_amount));
    let total_to_seize_drops = total_to_seize.to_u64();
    let bonus_portion = total_to_seize_drops.saturating_sub(collateral_raw);
    let protocol_cut = (Decimal::from(bonus_portion) * state.get_liquidation_protocol_share().0)
        .to_u64()
        .unwrap_or(0)
        .min(total_to_seize_drops);
    let collateral_received_drops = total_to_seize_drops.saturating_sub(protocol_cut);
    if collateral_received_drops == 0 {
        return Err(ProtocolError::GenericError(
            "XRP SP absorb would receive zero collateral".to_string(),
        ));
    }

    Ok(XrpSpAbsorbSizing {
        preflight: crate::XrpSpAbsorbPreflight {
            vault_id,
            icusd_burn_e8s: expected_icusd_burn_e8s,
            collateral_received_drops,
            collateral_price_e8s: price_usd.to_e8s(),
            expires_at_ns: 0,
        },
        total_to_seize_drops,
    })
}

pub fn stability_pool_preflight_xrp_absorb_in_state(
    state: &mut crate::state::State,
    caller: Principal,
    vault_id: u64,
    expected_icusd_burn_e8s: u64,
    now_ns: u64,
) -> Result<crate::XrpSpAbsorbPreflight, ProtocolError> {
    ensure_registered_sp(state, caller)?;
    if state.frozen {
        return Err(ProtocolError::TemporarilyUnavailable(
            "Protocol is frozen. All operations are suspended pending admin review.".to_string(),
        ));
    }
    if state.liquidation_frozen {
        return Err(ProtocolError::TemporarilyUnavailable(
            "Liquidations are currently frozen by admin.".to_string(),
        ));
    }
    if state.sp_writedown_disabled {
        return Err(ProtocolError::TemporarilyUnavailable(
            "SP writedown path is disabled by admin".to_string(),
        ));
    }
    if crate::guard::is_vault_liquidating(vault_id) {
        return Err(ProtocolError::TemporarilyUnavailable(format!(
            "Vault #{vault_id} has another operation in flight; retry shortly"
        )));
    }

    let sizing = xrp_sp_absorb_sizing(state, vault_id, expected_icusd_burn_e8s)?;
    let mut preflight = sizing.preflight;
    preflight.expires_at_ns = now_ns.saturating_add(XRP_SP_ABSORB_PREFLIGHT_TTL_NS);
    state.sp_xrp_absorb_preflights.insert(
        vault_id,
        crate::state::StoredXrpSpAbsorbPreflight {
            caller,
            vault_id,
            icusd_burn_e8s: expected_icusd_burn_e8s,
            total_to_seize_drops: sizing.total_to_seize_drops,
            collateral_received_drops: preflight.collateral_received_drops,
            collateral_price_e8s: preflight.collateral_price_e8s,
            expires_at_ns: preflight.expires_at_ns,
        },
    );
    Ok(preflight)
}

/// Hand back an unburned native-XRP absorb reservation.
///
/// A reservation blocks every vault-mutating entry point for `vault_id`
/// (including the manual liquidation paths) until it is consumed by a
/// successful absorb or expires after `XRP_SP_ABSORB_PREFLIGHT_TTL_NS`. When
/// the SP reserves and then gives up BEFORE burning any icUSD -- e.g. the
/// payout allocation build finds no opted-in depositor holding icUSD, or the
/// fan-out exceeds the allocation cap -- that window would otherwise leave an
/// underwater vault unliquidatable by any path for a full 15 minutes.
///
/// Only the registered SP may release, and only under the exact
/// `icusd_burn_e8s` it reserved: the reservation is the sizing snapshot a
/// post-burn submit resolves against, so dropping the wrong one would strand
/// an in-flight burn. A successful absorb removes the reservation itself, so
/// releasing a completed absorb is a no-op rather than an error.
///
/// Returns whether a reservation was actually cleared, so a retried cleanup
/// after a lost reply is idempotent rather than a failure.
pub fn stability_pool_release_xrp_absorb_preflight_in_state(
    state: &mut crate::state::State,
    caller: Principal,
    vault_id: u64,
    icusd_burn_e8s: u64,
) -> Result<bool, ProtocolError> {
    ensure_registered_sp(state, caller)?;
    let Some(preflight) = state.sp_xrp_absorb_preflights.get(&vault_id) else {
        return Ok(false);
    };
    if preflight.caller != caller {
        return Err(ProtocolError::GenericError(format!(
            "Vault #{vault_id} reservation belongs to another caller"
        )));
    }
    if preflight.icusd_burn_e8s != icusd_burn_e8s {
        return Err(ProtocolError::GenericError(format!(
            "XRP SP absorb release {} does not match the reserved burn {} for vault {}",
            icusd_burn_e8s, preflight.icusd_burn_e8s, vault_id
        )));
    }
    state.sp_xrp_absorb_preflights.remove(&vault_id);
    Ok(true)
}

/// Resolve the persisted preflight reservation for a POST-BURN submit.
///
/// Deliberately does NOT require the reservation to be unexpired. The SP burns
/// icUSD before this submit, so the reservation is the sizing snapshot the SP
/// already committed to by burning. Requiring an unexpired reservation here meant a
/// backend outage longer than the preflight TTL after the burn would permanently
/// strand the burned icUSD with no recovery (review blocker B-2). Honoring an
/// expired-but-present reservation is safe: the reservation persists in state until
/// the submit consumes it, the burn proof is independently verified and single-use
/// (`consumed_writedown_proofs`), the caller must be the registered SP, and the
/// submit guards that the vault still holds at least the reserved gross seizure
/// before minting any claim. Expiry still blocks NEW vault mutations via
/// `ensure_no_active_xrp_sp_absorb_preflight`, and a fresh preflight overwrites a
/// stale one for the same vault.
fn matching_xrp_absorb_preflight(
    state: &crate::state::State,
    vault_id: u64,
    icusd_burned_e8s: u64,
    caller: Principal,
) -> Option<crate::state::StoredXrpSpAbsorbPreflight> {
    let preflight = state.sp_xrp_absorb_preflights.get(&vault_id)?;
    if preflight.caller == caller && preflight.icusd_burn_e8s == icusd_burned_e8s {
        Some(preflight.clone())
    } else {
        None
    }
}

fn ensure_no_active_xrp_sp_absorb_preflight(
    state: &crate::state::State,
    vault_id: u64,
    now_ns: u64,
) -> Result<(), ProtocolError> {
    if let Some(preflight) = state.sp_xrp_absorb_preflights.get(&vault_id) {
        if preflight.expires_at_ns >= now_ns {
            return Err(ProtocolError::TemporarilyUnavailable(format!(
                "Vault #{vault_id} has a pending native-XRP stability-pool liquidation reservation; retry after it expires or completes"
            )));
        }
    }
    Ok(())
}

fn reject_active_xrp_sp_absorb_preflight(vault_id: u64, now_ns: u64) -> Result<(), ProtocolError> {
    read_state(|s| ensure_no_active_xrp_sp_absorb_preflight(s, vault_id, now_ns))
}

fn ensure_xrp_sp_absorb_preflight_vault(
    state: &crate::state::State,
    vault_id: u64,
) -> Result<&Vault, ProtocolError> {
    let vault = state
        .vault_id_to_vaults
        .get(&vault_id)
        .ok_or_else(|| ProtocolError::GenericError(format!("Vault #{vault_id} not found")))?;
    let cfg = state
        .get_collateral_config(&vault.collateral_type)
        .ok_or_else(|| {
            ProtocolError::GenericError(format!("No collateral config for vault #{vault_id}"))
        })?;
    if !cfg.is_native_xrp() {
        return Err(ProtocolError::GenericError(
            "XRP SP absorb requires a native-XRP vault".to_string(),
        ));
    }
    Ok(vault)
}

fn canonical_xrp_allocations(
    allocations: &[crate::XrpSpPayoutAllocation],
) -> Vec<crate::XrpSpPayoutAllocation> {
    let mut sorted = allocations.to_vec();
    sorted.sort_by(|a, b| {
        a.claimant
            .as_slice()
            .cmp(b.claimant.as_slice())
            .then_with(|| a.payout_address.cmp(&b.payout_address))
            .then_with(|| a.destination_tag.cmp(&b.destination_tag))
            .then_with(|| a.drops.cmp(&b.drops))
    });
    sorted
}

fn validate_xrp_sp_allocations(
    allocations: &[crate::XrpSpPayoutAllocation],
    expected_drops: u64,
) -> Result<Vec<crate::XrpSpPayoutAllocation>, ProtocolError> {
    if allocations.is_empty() {
        return Err(ProtocolError::GenericError(
            "XRP SP absorb requires at least one payout allocation".to_string(),
        ));
    }
    if allocations.len() > crate::MAX_XRP_SP_PAYOUT_ALLOCATIONS {
        return Err(ProtocolError::GenericError(format!(
            "XRP SP absorb supports at most {} payout allocations",
            crate::MAX_XRP_SP_PAYOUT_ALLOCATIONS
        )));
    }
    let mut sum: u128 = 0;
    for allocation in allocations {
        if allocation.payout_address.trim().is_empty() {
            return Err(ProtocolError::GenericError(
                "XRP SP absorb payout address is required".to_string(),
            ));
        }
        if allocation.drops == 0 {
            return Err(ProtocolError::GenericError(
                "XRP SP absorb payout allocation drops must be non-zero".to_string(),
            ));
        }
        sum = sum.saturating_add(u128::from(allocation.drops));
    }
    if sum != u128::from(expected_drops) {
        return Err(ProtocolError::GenericError(format!(
            "XRP SP absorb allocation sum {} does not match collateral received {}",
            sum, expected_drops
        )));
    }
    Ok(canonical_xrp_allocations(allocations))
}

fn hash_len_prefixed(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

fn xrp_sp_allocation_fingerprint(
    caller: Principal,
    request: &crate::XrpSpAbsorbRequest,
    allocations: &[crate::XrpSpPayoutAllocation],
) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hash_len_prefixed(&mut hasher, caller.as_slice());
    hasher.update(request.vault_id.to_be_bytes());
    hasher.update(request.icusd_burned_e8s.to_be_bytes());
    let proof_kind = match request.proof.ledger_kind {
        crate::icrc3_proof::SpProofLedger::IcusdBurn => 0u8,
        crate::icrc3_proof::SpProofLedger::ThreePoolTransfer => 1u8,
        crate::icrc3_proof::SpProofLedger::ThreePoolTransferDefault => 2u8,
    };
    hasher.update([proof_kind]);
    hasher.update(request.proof.block_index.to_be_bytes());
    hasher.update(request.proof.vault_id_memo.to_be_bytes());
    hasher.update((allocations.len() as u64).to_be_bytes());
    for allocation in allocations {
        hash_len_prefixed(&mut hasher, allocation.claimant.as_slice());
        hash_len_prefixed(&mut hasher, allocation.payout_address.as_bytes());
        match allocation.destination_tag {
            Some(tag) => {
                hasher.update([1u8]);
                hasher.update(tag.to_be_bytes());
            }
            None => hasher.update([0u8]),
        }
        hasher.update(allocation.drops.to_be_bytes());
    }
    hasher.finalize().to_vec()
}

fn stored_xrp_sp_absorb_matches_retry(
    stored: &crate::state::StoredXrpSpAbsorbResult,
    caller: Principal,
    request: &crate::XrpSpAbsorbRequest,
    allocation_fingerprint: &[u8],
) -> bool {
    stored.caller == caller
        && stored.vault_id == request.vault_id
        && stored.icusd_burned_e8s == request.icusd_burned_e8s
        && stored.proof_ledger == request.proof.ledger_kind
        && stored.proof_block_index == request.proof.block_index
        && stored.allocation_fingerprint == allocation_fingerprint
}

pub fn record_sp_xrp_absorb_result_bounded(
    state: &mut crate::state::State,
    proof_key: (crate::icrc3_proof::SpProofLedger, u64),
    stored: crate::state::StoredXrpSpAbsorbResult,
) {
    state
        .sp_xrp_absorb_results_by_proof
        .insert(proof_key, stored);
    while state.sp_xrp_absorb_results_by_proof.len()
        > crate::state::MAX_SP_XRP_ABSORB_RESULTS_BY_PROOF
    {
        let Some(oldest_key) = state
            .sp_xrp_absorb_results_by_proof
            .keys()
            .copied()
            .find(|key| *key != proof_key)
        else {
            break;
        };
        state.sp_xrp_absorb_results_by_proof.remove(&oldest_key);
    }
}

pub fn xrp_sp_absorb_cached_replay_result(
    state: &crate::state::State,
    caller: Principal,
    request: &crate::XrpSpAbsorbRequest,
) -> Option<Result<crate::XrpSpAbsorbResult, ProtocolError>> {
    let proof_key = (request.proof.ledger_kind, request.proof.block_index);
    let stored = state.sp_xrp_absorb_results_by_proof.get(&proof_key)?;
    Some(
        validate_xrp_sp_allocations(&request.allocations, stored.result.collateral_received_drops)
            .and_then(|allocations| {
                let fingerprint = xrp_sp_allocation_fingerprint(caller, request, &allocations);
                if stored_xrp_sp_absorb_matches_retry(stored, caller, request, &fingerprint) {
                    Ok(stored.result.clone())
                } else {
                    Err(ProtocolError::GenericError(format!(
                        "SP XRP absorb proof replay rejected: ({:?}, block {}) already consumed for a different request",
                        request.proof.ledger_kind, request.proof.block_index
                    )))
                }
            }),
    )
}

/// Look up the durable outcome of one exact XRP SP absorb request without
/// performing proof verification, liquidation, or any other state transition.
pub fn xrp_sp_absorb_status_in_state(
    state: &crate::state::State,
    caller: Principal,
    request: &crate::XrpSpAbsorbRequest,
) -> Result<crate::XrpSpAbsorbStatus, ProtocolError> {
    ensure_registered_sp(state, caller)?;

    let proof_key = (request.proof.ledger_kind, request.proof.block_index);
    let refund = state.sp_burn_refunds_by_proof.get(&proof_key);
    let absorb = state.sp_xrp_absorb_results_by_proof.get(&proof_key);
    // If both terminal records exist, do not choose one: the proof has
    // contradictory backend outcomes and requires recovery.
    if refund.is_some() && absorb.is_some() {
        return Ok(crate::XrpSpAbsorbStatus::ConsumedWithoutResult);
    }
    let consumed = state.consumed_writedown_proofs.contains(&proof_key);
    if let Some(refund) = refund {
        let exact_refund = consumed
            && request.proof.ledger_kind == crate::icrc3_proof::SpProofLedger::IcusdBurn
            && request.proof.vault_id_memo == request.vault_id
            && refund.caller == caller
            && refund.vault_id == request.vault_id
            && refund.amount_e8s == request.icusd_burned_e8s
            && refund.ledger == state.icusd_ledger_principal
            && refund.burn_block_index == request.proof.block_index;
        return Ok(if exact_refund {
            crate::XrpSpAbsorbStatus::RefundJournaled
        } else {
            crate::XrpSpAbsorbStatus::ConsumedWithoutResult
        });
    }
    let Some(stored) = absorb else {
        return Ok(if consumed {
            crate::XrpSpAbsorbStatus::ConsumedWithoutResult
        } else {
            crate::XrpSpAbsorbStatus::Unseen
        });
    };

    // A cached result is authoritative only when the replay tombstone agrees,
    // the persisted record is bound to the lookup key, and the full canonical
    // request fingerprint matches. Any inconsistency remains fail-closed.
    if !consumed
        || stored.proof_ledger != request.proof.ledger_kind
        || stored.proof_block_index != request.proof.block_index
        || !stored.result.success
        || stored.result.vault_id != request.vault_id
        || stored.result.block_index != request.proof.block_index
        || stored.result.liquidated_debt_e8s != request.icusd_burned_e8s
    {
        return Ok(crate::XrpSpAbsorbStatus::ConsumedWithoutResult);
    }

    let matches = validate_xrp_sp_allocations(
        &request.allocations,
        stored.result.collateral_received_drops,
    )
    .map(|allocations| {
        let fingerprint = xrp_sp_allocation_fingerprint(caller, request, &allocations);
        request.proof.vault_id_memo == request.vault_id
            && stored_xrp_sp_absorb_matches_retry(stored, caller, request, &fingerprint)
    })
    .unwrap_or(false);

    Ok(if matches {
        crate::XrpSpAbsorbStatus::Accepted(stored.result.clone())
    } else {
        crate::XrpSpAbsorbStatus::ConsumedWithoutResult
    })
}

pub fn stability_pool_liquidate_xrp_vault_in_state(
    state: &mut crate::state::State,
    caller: Principal,
    request: crate::XrpSpAbsorbRequest,
    now_ns: u64,
) -> Result<crate::XrpSpAbsorbResult, ProtocolError> {
    ensure_registered_sp(state, caller)?;
    let proof_key = (request.proof.ledger_kind, request.proof.block_index);

    if let Some(stored) = state.sp_xrp_absorb_results_by_proof.get(&proof_key) {
        let allocations = validate_xrp_sp_allocations(
            &request.allocations,
            stored.result.collateral_received_drops,
        )?;
        let fingerprint = xrp_sp_allocation_fingerprint(caller, &request, &allocations);
        if stored_xrp_sp_absorb_matches_retry(stored, caller, &request, &fingerprint) {
            return Ok(stored.result.clone());
        }
        return Err(ProtocolError::GenericError(format!(
            "SP XRP absorb proof replay rejected: ({:?}, block {}) already consumed for a different request",
            request.proof.ledger_kind, request.proof.block_index
        )));
    }

    if request.proof.ledger_kind != crate::icrc3_proof::SpProofLedger::IcusdBurn {
        return Err(ProtocolError::GenericError(
            "XRP SP absorb requires an icUSD burn proof".to_string(),
        ));
    }
    if request.proof.vault_id_memo != request.vault_id {
        return Err(ProtocolError::GenericError(format!(
            "SP writedown proof vault_id_memo {} does not match call vault_id {}",
            request.proof.vault_id_memo, request.vault_id
        )));
    }
    if state.consumed_writedown_proofs.contains(&proof_key) {
        return Err(ProtocolError::GenericError(format!(
            "SP writedown proof replay rejected: ({:?}, block {}) already consumed",
            request.proof.ledger_kind, request.proof.block_index
        )));
    }

    let preflight =
        matching_xrp_absorb_preflight(state, request.vault_id, request.icusd_burned_e8s, caller)
            .ok_or_else(|| {
                ProtocolError::GenericError(
                    "XRP SP absorb requires a matching preflight reservation".to_string(),
                )
            })?;

    let allocations =
        validate_xrp_sp_allocations(&request.allocations, preflight.collateral_received_drops)?;
    let fingerprint = xrp_sp_allocation_fingerprint(caller, &request, &allocations);
    let (custody_owner, vault_collateral, vault_borrowed) = {
        let vault = ensure_xrp_sp_absorb_preflight_vault(state, request.vault_id)?;
        (
            vault.owner,
            vault.collateral_amount,
            vault.borrowed_icusd_amount,
        )
    };
    // Conservation guard (collateral side): the depositor allocations plus the developer
    // protocol-fee claim below sum to the gross `total_to_seize_drops`, which is what we
    // debit from the vault. Now that an expired reservation is honored on submit (B-2),
    // the active-preflight lock can lapse and another path (e.g. a manual partial
    // liquidation) could seize this vault in between. If the vault's collateral fell
    // below the reserved gross seizure, abort BEFORE any mutation rather than minting
    // claims that exceed the seized XRP.
    if vault_collateral < preflight.total_to_seize_drops {
        return Err(ProtocolError::GenericError(format!(
            "XRP SP absorb vault {} collateral {} fell below the reserved seizure {} since preflight; aborting before mutation",
            request.vault_id, vault_collateral, preflight.total_to_seize_drops
        )));
    }
    // Conservation guard (debt side): the SP irrevocably burned `icusd_burned_e8s`
    // before this submit. If a manual liquidation reduced the vault's debt below that
    // burn during the same window, the silent `.min(borrowed)` cap at the writedown
    // would clear less debt than was burned, over-burning icUSD with no offsetting debt
    // (a direct loss to SP depositors). Abort before any mutation. The check is
    // directional: a pool-limited PARTIAL absorb has `borrowed > burn` and is allowed,
    // and price recovery does not change `borrowed`, so this does not regress B-2's
    // intended post-burn finalization.
    if vault_borrowed < ICUSD::new(request.icusd_burned_e8s) {
        return Err(ProtocolError::GenericError(format!(
            "XRP SP absorb vault {} live debt {:?} fell below the reserved burn {} since preflight; aborting before mutation",
            request.vault_id, vault_borrowed, request.icusd_burned_e8s
        )));
    }

    let mut payout_claims = Vec::with_capacity(allocations.len());
    for allocation in &allocations {
        let claim_id = record_xrp_claim(
            state,
            allocation.claimant,
            custody_owner,
            request.vault_id,
            allocation.drops,
            now_ns,
        );
        payout_claims.push(crate::XrpSpPayoutClaim {
            claimant: allocation.claimant,
            claim_id,
            payout_address: allocation.payout_address.clone(),
            destination_tag: allocation.destination_tag,
            drops: allocation.drops,
        });
    }

    // B-1: route the protocol's liquidation-fee cut to a developer-settleable
    // XrpClaim, mirroring the manual native-XRP liquidation paths (see
    // `liquidate_vault_partial`). The vault is debited the GROSS `total_to_seize_drops`
    // below, but the depositor allocations only sum to the NET
    // `collateral_received_drops`. Without this claim the protocol cut would be removed
    // from the vault yet have no claim referencing it — stranded in custody and
    // breaking `sum(claims) == collateral seized`.
    let protocol_cut = preflight
        .total_to_seize_drops
        .saturating_sub(preflight.collateral_received_drops);
    if protocol_cut > 0 {
        let developer = state.developer_principal;
        record_xrp_claim(
            state,
            developer,
            custody_owner,
            request.vault_id,
            protocol_cut,
            now_ns,
        );
    }

    let mut interest_share = ICUSD::new(0);
    if let Some(vault) = state.vault_id_to_vaults.get_mut(&request.vault_id) {
        if vault.accrued_interest.0 > 0 && vault.borrowed_icusd_amount.0 > 0 {
            interest_share = ICUSD::new(crate::numeric::proportional_interest_share(
                request.icusd_burned_e8s,
                vault.accrued_interest.0,
                vault.borrowed_icusd_amount.0,
            ));
        }
        let debt_applied = ICUSD::new(request.icusd_burned_e8s).min(vault.borrowed_icusd_amount);
        let collateral_applied = preflight.total_to_seize_drops.min(vault.collateral_amount);
        vault.borrowed_icusd_amount = vault.borrowed_icusd_amount.saturating_sub(debt_applied);
        vault.collateral_amount = vault.collateral_amount.saturating_sub(collateral_applied);
        vault.accrued_interest = vault.accrued_interest.saturating_sub(interest_share);
    }
    crate::state::record_recent_liquidation(state, request.icusd_burned_e8s, now_ns);
    state.cleanup_if_drained(request.vault_id);
    state.consumed_writedown_proofs.insert(proof_key);
    state.sp_xrp_absorb_preflights.remove(&request.vault_id);

    let result = crate::XrpSpAbsorbResult {
        success: true,
        vault_id: request.vault_id,
        liquidated_debt_e8s: request.icusd_burned_e8s,
        collateral_received_drops: preflight.collateral_received_drops,
        payout_claims,
        block_index: request.proof.block_index,
        collateral_price_e8s: preflight.collateral_price_e8s,
    };
    let stored = crate::state::StoredXrpSpAbsorbResult {
        caller,
        vault_id: request.vault_id,
        icusd_burned_e8s: request.icusd_burned_e8s,
        proof_ledger: request.proof.ledger_kind,
        proof_block_index: request.proof.block_index,
        allocation_fingerprint: fingerprint,
        result: result.clone(),
        accepted_at_ns: now_ns,
    };
    record_sp_xrp_absorb_result_bounded(state, proof_key, stored);
    Ok(result)
}

/// True iff `vault_id` is a native-XRP-collateral vault (custody on the XRP
/// Ledger). Such vaults are settled through claim-based paths only: the
/// stability pool absorbs them via the dedicated native-XRP absorb flow
/// (`stability_pool_preflight_xrp_absorb` + `stability_pool_liquidate_xrp_vault`,
/// which mint `XrpClaim`s for opted-in depositors), and external liquidators use
/// `liquidate_vault_partial` / `partial_liquidate_vault`. This predicate guards
/// the entry points that CANNOT settle an XrpClaim — the bot, and the generic
/// ICRC/3USD stability-pool write-down paths.
pub fn vault_is_native_xrp(vault_id: u64) -> bool {
    read_state(|s| {
        s.vault_id_to_vaults
            .get(&vault_id)
            .and_then(|v| s.get_collateral_config(&v.collateral_type))
            .map(|c| c.is_native_xrp())
            .unwrap_or(false)
    })
}

/// Pure: drops to actually send when settling a claim — the claimant bears the XRPL
/// network fee (sends `drops - fee`). Errors if the claim cannot cover the fee.
pub(crate) fn xrp_claim_send_amount(drops: u64, fee: u64) -> Result<u64, ProtocolError> {
    match drops.checked_sub(fee) {
        Some(n) if n > 0 => Ok(n),
        _ => Err(ProtocolError::AmountTooLow {
            minimum_amount: fee.saturating_add(1),
        }),
    }
}

pub(crate) fn xrp_unresolved_claim_drops_for_custody(
    s: &crate::state::State,
    custody_owner: Principal,
    custody_nonce: u64,
) -> Result<u128, ProtocolError> {
    s.xrp_claims
        .values()
        .filter(|claim| {
            claim.custody_owner == custody_owner && claim.custody_nonce == custody_nonce
        })
        .try_fold(0u128, |total, claim| {
            total.checked_add(u128::from(claim.drops)).ok_or_else(|| {
                ProtocolError::GenericError(
                    "Aggregate XRP claims exceed supported drops range".to_string(),
                )
            })
        })
}

pub(crate) fn xrp_inflight_claims_for_custody(
    s: &crate::state::State,
    current_claim_id: u64,
    custody_owner: Principal,
    custody_nonce: u64,
) -> Vec<(u64, crate::state::XrpSettlement)> {
    s.xrp_claims
        .iter()
        .filter_map(|(claim_id, claim)| {
            if *claim_id != current_claim_id
                && claim.custody_owner == custody_owner
                && claim.custody_nonce == custody_nonce
            {
                claim
                    .settlement
                    .as_ref()
                    .map(|settlement| (*claim_id, settlement.clone()))
            } else {
                None
            }
        })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum XrpSettlementReconciliation {
    Paid,
    FailedFeeCharged,
    ExpiredNotFound,
}

pub(crate) fn reconcile_xrp_settlement_snapshot(
    s: &mut crate::state::State,
    claim_id: u64,
    tx_hash: &str,
    outcome: XrpSettlementReconciliation,
) -> bool {
    let hash_matches = s
        .xrp_claims
        .get(&claim_id)
        .and_then(|claim| claim.settlement.as_ref())
        .map(|settlement| settlement.tx_hash == tx_hash)
        .unwrap_or(false);
    if !hash_matches {
        return false;
    }

    match outcome {
        XrpSettlementReconciliation::Paid => {
            s.xrp_claims.remove(&claim_id);
        }
        XrpSettlementReconciliation::FailedFeeCharged => {
            if let Some(claim) = s.xrp_claims.get_mut(&claim_id) {
                claim.drops = claim
                    .drops
                    .saturating_sub(crate::chains::xrp::adapter::XRP_FEE_DROPS);
                claim.settlement = None;
            }
        }
        XrpSettlementReconciliation::ExpiredNotFound => {
            if let Some(claim) = s.xrp_claims.get_mut(&claim_id) {
                claim.settlement = None;
            }
        }
    }
    true
}

/// F-03: durably flag a claim as quarantined because a settlement divergence is
/// suspected (its custody Sequence advanced past the recorded `source_sequence` while
/// the recorded tx_hash is NotFound on-ledger). Idempotent: keeps the first reason set,
/// so repeated detection does not overwrite the most precise diagnostic. While a claim
/// is quarantined `settle_xrp_claim` refuses to sign; an admin clears it via
/// `admin_resolve_xrp_claim`. Returns true if the claim exists.
pub(crate) fn quarantine_xrp_claim_snapshot(
    s: &mut crate::state::State,
    claim_id: u64,
    reason: &str,
) -> bool {
    if let Some(claim) = s.xrp_claims.get_mut(&claim_id) {
        if claim.quarantine_reason.is_none() {
            claim.quarantine_reason = Some(reason.to_string());
        }
        true
    } else {
        false
    }
}

/// F-03: apply an admin resolution to a quarantined claim after off-ledger
/// reconciliation. `confirm_paid = true` means the admin verified the divergent Payment
/// DID deliver -> remove the claim (no re-pay). `false` means it did NOT deliver -> clear
/// the quarantine + settlement so the claimant can retry settle and be paid exactly once.
/// Errors WITHOUT mutating if the claim is absent or not quarantined, so a resolve can
/// never silently drop a healthy, still-settle-able claim. `pub` for the main.rs endpoint.
pub fn resolve_quarantined_xrp_claim_snapshot(
    s: &mut crate::state::State,
    claim_id: u64,
    confirm_paid: bool,
) -> Result<(), ProtocolError> {
    match s.xrp_claims.get(&claim_id) {
        Some(c) if c.quarantine_reason.is_some() => {}
        Some(_) => {
            return Err(ProtocolError::GenericError(format!(
                "XRP claim #{claim_id} is not quarantined; refusing to resolve a healthy claim"
            )))
        }
        None => {
            return Err(ProtocolError::GenericError(format!(
                "No such XRP claim #{claim_id}"
            )))
        }
    }
    if confirm_paid {
        s.xrp_claims.remove(&claim_id);
    } else if let Some(c) = s.xrp_claims.get_mut(&claim_id) {
        c.quarantine_reason = None;
        c.settlement = None;
    }
    Ok(())
}

async fn reconcile_xrp_other_inflight_claims(
    current_claim_id: u64,
    custody_owner: Principal,
    custody_nonce: u64,
    acct: &crate::chains::xrp::xrp_rpc::XrpAccountInfo,
) -> Result<Option<u64>, ProtocolError> {
    let in_flight = read_state(|s| {
        xrp_inflight_claims_for_custody(s, current_claim_id, custody_owner, custody_nonce)
    });
    for (other_claim_id, settlement) in in_flight {
        let status = crate::chains::xrp::xrp_rpc::fetch_tx_status(&settlement.tx_hash)
            .await
            .map_err(|e| {
                ProtocolError::GenericError(format!(
                    "xrp tx status for claim #{other_claim_id} failed: {e}"
                ))
            })?;
        match xrp_sibling_reconcile_decision(&status, &settlement, acct) {
            XrpSiblingReconcileDecision::Paid => {
                mutate_state(|s| {
                    reconcile_xrp_settlement_snapshot(
                        s,
                        other_claim_id,
                        &settlement.tx_hash,
                        XrpSettlementReconciliation::Paid,
                    );
                });
            }
            XrpSiblingReconcileDecision::FailedFeeCharged => {
                mutate_state(|s| {
                    reconcile_xrp_settlement_snapshot(
                        s,
                        other_claim_id,
                        &settlement.tx_hash,
                        XrpSettlementReconciliation::FailedFeeCharged,
                    );
                });
            }
            XrpSiblingReconcileDecision::StillInFlight => {
                return Ok(Some(other_claim_id));
            }
            XrpSiblingReconcileDecision::ExpiredSafeToClear => {
                mutate_state(|s| {
                    reconcile_xrp_settlement_snapshot(
                        s,
                        other_claim_id,
                        &settlement.tx_hash,
                        XrpSettlementReconciliation::ExpiredNotFound,
                    );
                });
            }
            // F-03: the sibling's Payment consumed its source Sequence under a hash that
            // differs from the one we recorded, so it may already have paid out. Durably
            // quarantine the sibling (so future settles refuse it too), refuse to clear the
            // blocker (which would let it be re-signed and double-paid), and fail the
            // current settle closed; an admin resolves the sibling via admin_resolve_xrp_claim.
            XrpSiblingReconcileDecision::QuarantineDiverged => {
                let reason = format!(
                    "settlement diverged: custody account sequence advanced past source \
                     sequence while tx_hash {} is NotFound on-ledger; Payment may already \
                     have settled under a different hash",
                    settlement.tx_hash
                );
                mutate_state(|s| {
                    quarantine_xrp_claim_snapshot(s, other_claim_id, &reason);
                });
                return Err(ProtocolError::GenericError(format!(
                    "xrp sibling claim #{other_claim_id} quarantined ({reason}). Refusing to \
                     clear the blocker to avoid a double-pay; manual reconciliation required."
                )));
            }
        }
    }
    Ok(None)
}

pub(crate) fn ensure_xrp_claim_aggregate_solvency(
    acct: &crate::chains::xrp::xrp_rpc::XrpAccountInfo,
    reserve_drops: u128,
    unresolved_claim_drops: u128,
) -> Result<(), ProtocolError> {
    if !acct.exists {
        return Err(ProtocolError::GenericError(
            "XRP custody account is unfunded; cannot settle claim.".to_string(),
        ));
    }
    let required = reserve_drops
        .checked_add(unresolved_claim_drops)
        .ok_or_else(|| {
            ProtocolError::GenericError(
                "Aggregate XRP claims plus reserve exceed supported drops range".to_string(),
            )
        })?;
    if acct.balance_drops < required {
        return Err(ProtocolError::GenericError(format!(
            "insufficient XRP for unresolved claims: balance {} drops < aggregate claims {} + reserve {}",
            acct.balance_drops, unresolved_claim_drops, reserve_drops
        )));
    }
    Ok(())
}

pub(crate) fn ensure_xrp_replacement_sequence_safe(
    prev: &crate::state::XrpSettlement,
    acct: &crate::chains::xrp::xrp_rpc::XrpAccountInfo,
) -> Result<(), ProtocolError> {
    let Some(source_sequence) = prev.source_sequence else {
        return Err(ProtocolError::GenericError(
            "Cannot replace XRP settlement from legacy state without source sequence.".to_string(),
        ));
    };
    if acct.sequence > source_sequence {
        return Err(ProtocolError::GenericError(format!(
            "Cannot replace XRP settlement: source sequence advanced from {} to {}.",
            source_sequence, acct.sequence
        )));
    }
    if acct.sequence < source_sequence {
        return Err(ProtocolError::GenericError(format!(
            "Cannot replace XRP settlement: live source sequence {} is behind stored sequence {}.",
            acct.sequence, source_sequence
        )));
    }
    Ok(())
}

/// How to reconcile an OTHER in-flight sibling settlement (one sharing a custody
/// address with the claim currently being settled), given the sibling's on-ledger tx
/// status and the live custody account. Split out of `reconcile_xrp_other_inflight_claims`
/// so the F-03 sibling-divergence guard is unit-testable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum XrpSiblingReconcileDecision {
    /// Sibling Payment validated on-ledger -> finalize (remove the sibling claim).
    Paid,
    /// Sibling Payment validated but failed (`tec*`) -> charge the fee once, clear blocker.
    FailedFeeCharged,
    /// Sibling Payment may still land (its LastLedgerSequence has not passed) -> back off
    /// until it confirms or expires.
    StillInFlight,
    /// Sibling Payment expired AND the live custody Sequence still equals its
    /// `source_sequence`, proving nothing consumed that Sequence (the Payment never
    /// applied under ANY hash) -> safe to clear the blocker.
    ExpiredSafeToClear,
    /// Sibling Payment is NotFound by our local hash and expired, but the live custody
    /// Sequence has ADVANCED past its `source_sequence`. On the XRPL a sequence is
    /// consumed strictly in order and only by a transaction from that account, so the
    /// sibling's own Payment provably consumed it — under a hash that differs from the one
    /// we recorded (the F-03 codec/canonicalization divergence). It may already have paid
    /// the claimant, so the blocker must NOT be cleared (clearing would let the sibling be
    /// re-signed and double-paid). Quarantine for manual reconciliation.
    QuarantineDiverged,
}

/// Pure F-03 guard. The NotFound branch is gated on `ensure_xrp_replacement_sequence_safe`
/// exactly like the primary settle path (`settle_xrp_claim_with_tag`); previously the
/// sibling-reconcile path cleared the blocker on expiry WITHOUT a Sequence check, which
/// let a diverged-hash sibling be reset to `settlement = None` and re-paid a second time.
pub(crate) fn xrp_sibling_reconcile_decision(
    status: &crate::chains::xrp::xrp_rpc::XrpTxStatus,
    settlement: &crate::state::XrpSettlement,
    acct: &crate::chains::xrp::xrp_rpc::XrpAccountInfo,
) -> XrpSiblingReconcileDecision {
    use crate::chains::xrp::xrp_rpc::XrpTxStatus;
    match status {
        XrpTxStatus::Validated { .. } => XrpSiblingReconcileDecision::Paid,
        XrpTxStatus::Failed => XrpSiblingReconcileDecision::FailedFeeCharged,
        XrpTxStatus::NotFound => {
            if acct.ledger_index <= settlement.last_ledger_sequence {
                XrpSiblingReconcileDecision::StillInFlight
            } else if ensure_xrp_replacement_sequence_safe(settlement, acct).is_ok() {
                XrpSiblingReconcileDecision::ExpiredSafeToClear
            } else {
                XrpSiblingReconcileDecision::QuarantineDiverged
            }
        }
    }
}

pub(crate) fn remove_xrp_pending_deposit_if_unfunded_snapshot(
    s: &mut crate::state::State,
    vault_id: u64,
    expected: &crate::state::XrpPendingDeposit,
    acct: &crate::chains::xrp::xrp_rpc::XrpAccountInfo,
) -> Result<bool, ProtocolError> {
    if acct.exists {
        return Err(ProtocolError::GenericError(
            "XRP custody account is funded; confirm the deposit instead of cancelling.".to_string(),
        ));
    }
    match s.xrp_pending_deposits.get(&vault_id) {
        Some(current) if current == expected => {
            s.xrp_pending_deposits.remove(&vault_id);
            Ok(true)
        }
        Some(_) | None => Ok(false),
    }
}

const XRP_PENDING_CLEANUP_MIN_AGE_NS: u64 = 10 * 60 * 1_000_000_000;

pub(crate) fn ensure_xrp_pending_cleanup_age(
    pending: &crate::state::XrpPendingDeposit,
    now_ns: u64,
) -> Result<(), ProtocolError> {
    let age_ns = now_ns.saturating_sub(pending.opened_at_ns);
    if age_ns < XRP_PENDING_CLEANUP_MIN_AGE_NS {
        return Err(ProtocolError::GenericError(
            "XRP pending deposit is too new to cancel; wait for the XRPL funding window to pass."
                .to_string(),
        ));
    }
    Ok(())
}

/// XRP-006: owner cleanup for an unfunded native-XRP open. This never removes a
/// funded custody account; users must confirm funded deposits into real vaults.
pub async fn cancel_xrp_pending_open(vault_id: u64) -> Result<(), ProtocolError> {
    let caller = ic_cdk::api::caller();
    let guard_principal =
        GuardPrincipal::new(caller, &format!("cancel_xrp_pending_open_{}", vault_id))?;

    let pending = match read_state(|s| s.xrp_pending_deposits.get(&vault_id).cloned()) {
        Some(p) => p,
        None => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(
                "No pending XRP deposit for this vault.".to_string(),
            ));
        }
    };
    if pending.owner != caller {
        guard_principal.fail();
        return Err(ProtocolError::CallerNotOwner);
    }
    if let Err(e) = ensure_xrp_pending_cleanup_age(&pending, ic_cdk::api::time()) {
        guard_principal.fail();
        return Err(e);
    }

    let acct = match crate::chains::xrp::xrp_rpc::fetch_account_info(&pending.custody_address).await
    {
        Ok(a) => a,
        Err(e) => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(format!(
                "xrp account_info failed: {e}"
            )));
        }
    };

    let removed = mutate_state(|s| {
        ensure_xrp_pending_cleanup_age(&pending, ic_cdk::api::time())?;
        remove_xrp_pending_deposit_if_unfunded_snapshot(s, vault_id, &pending, &acct)
    })?;
    if !removed {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "XRP pending deposit changed concurrently; refresh and retry.".to_string(),
        ));
    }

    guard_principal.complete();
    Ok(())
}

/// XRP-006: developer cleanup for abandoned unfunded native-XRP opens. This is
/// deliberately unfunded-only; if XRP has reached the custody address the entry
/// must remain confirmable by its owner.
pub async fn sweep_xrp_pending_open(vault_id: u64) -> Result<(), ProtocolError> {
    let caller = ic_cdk::api::caller();
    if !read_state(|s| s.developer_principal == caller) {
        return Err(ProtocolError::GenericError(
            "Only the developer can sweep XRP pending opens.".to_string(),
        ));
    }
    let guard_principal =
        GuardPrincipal::new(caller, &format!("sweep_xrp_pending_open_{}", vault_id))?;

    let pending = match read_state(|s| s.xrp_pending_deposits.get(&vault_id).cloned()) {
        Some(p) => p,
        None => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(
                "No pending XRP deposit for this vault.".to_string(),
            ));
        }
    };
    if let Err(e) = ensure_xrp_pending_cleanup_age(&pending, ic_cdk::api::time()) {
        guard_principal.fail();
        return Err(e);
    }

    let acct = match crate::chains::xrp::xrp_rpc::fetch_account_info(&pending.custody_address).await
    {
        Ok(a) => a,
        Err(e) => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(format!(
                "xrp account_info failed: {e}"
            )));
        }
    };

    let removed = mutate_state(|s| {
        ensure_xrp_pending_cleanup_age(&pending, ic_cdk::api::time())?;
        remove_xrp_pending_deposit_if_unfunded_snapshot(s, vault_id, &pending, &acct)
    })?;
    if !removed {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "XRP pending deposit changed concurrently; refresh and retry.".to_string(),
        ));
    }

    guard_principal.complete();
    Ok(())
}

/// P4: settle an XRP claim — sign + submit a `Payment` from the source vault's
/// custody address (re-derived from the claim) to `destination`, for
/// `claim.drops - fee` (the claimant bears the fee). Claimant-only. One in-flight
/// Payment per custody address (sequence serialization, keyed on the source vault
/// id). On success the claim is removed; returns the locally computed tx hash.
pub async fn settle_xrp_claim(claim_id: u64, destination: String) -> Result<String, ProtocolError> {
    settle_xrp_claim_with_tag(claim_id, destination, None).await
}

pub async fn settle_xrp_claim_with_tag(
    claim_id: u64,
    destination: String,
    destination_tag: Option<u32>,
) -> Result<String, ProtocolError> {
    settle_xrp_claim_as(
        ic_cdk::api::caller(),
        claim_id,
        destination,
        destination_tag,
    )
    .await
}

/// Settlement body with an explicit acting claimant. The claimant entry points
/// pass `ic_cdk::caller()`; `stability_pool_settle_xrp_claim` passes the
/// depositor the SP is settling for (after `validate_sp_settle_xrp_claim_in_state`
/// has established the caller is the registered SP and the claimant matches).
/// The per-claim guard is keyed by the acting claimant either way, so an SP
/// sweep and the depositor clicking settle serialize on the same custody lock
/// and the confirm-before-sign idempotency check below.
pub async fn settle_xrp_claim_as(
    caller: Principal,
    claim_id: u64,
    destination: String,
    destination_tag: Option<u32>,
) -> Result<String, ProtocolError> {
    require_xrp_production_key()?;
    let mut claim = match read_state(|s| s.xrp_claims.get(&claim_id).cloned()) {
        Some(c) => c,
        None => {
            return Err(ProtocolError::GenericError(
                "No such XRP claim (already settled or unknown).".to_string(),
            ))
        }
    };
    let mut replacement_destination = destination;
    let mut replacement_destination_tag = destination_tag;
    if claim.claimant != caller {
        return Err(ProtocolError::CallerNotOwner);
    }

    // F-03: a quarantined claim may already have been paid under a divergent hash.
    // Refuse to sign anything until an admin resolves it (admin_resolve_xrp_claim).
    if let Some(reason) = claim.quarantine_reason.clone() {
        return Err(ProtocolError::GenericError(format!(
            "XRP claim #{claim_id} is quarantined ({reason}); awaiting admin reconciliation."
        )));
    }

    let guard_principal = GuardPrincipal::new(caller, &format!("settle_xrp_claim_{}", claim_id))?;
    // Per-custody-address sequence serialization: custody_nonce == the source vault
    // id, so this per-vault lock prevents two concurrent Payments from one custody
    // address colliding on the XRPL Sequence.
    let _seq_guard = match VaultLiquidationGuard::new(claim.custody_nonce) {
        Ok(g) => g,
        Err(e) => {
            guard_principal.fail();
            return Err(e);
        }
    };

    let path = crate::chains::xrp::ted25519::custody_derivation_path(
        crate::chains::xrp::XRP_CHAIN_ID,
        claim.custody_owner,
        claim.custody_nonce,
    );

    // Idempotency (anti double-pay): if a settlement Payment was already
    // signed+submitted for this claim, CONFIRM it before signing a new one. A submit
    // outcall can error AFTER rippled already broadcast the tx; without this check a
    // retry would read the bumped account Sequence and send a second distinct
    // Payment, paying the claimant twice out of the custody address.
    if let Some(prev) = claim.settlement.clone() {
        match crate::chains::xrp::xrp_rpc::fetch_tx_status(&prev.tx_hash).await {
            Ok(crate::chains::xrp::xrp_rpc::XrpTxStatus::Validated { .. }) => {
                // Already paid on-chain — finalize by removing the claim.
                mutate_state(|s| {
                    s.xrp_claims.remove(&claim_id);
                });
                guard_principal.complete();
                return Ok(prev.tx_hash);
            }
            Ok(crate::chains::xrp::xrp_rpc::XrpTxStatus::NotFound) => {
                // Not validated yet. Only sign a fresh tx if the prior one can NEVER
                // apply anymore (its LastLedgerSequence has passed); otherwise it may
                // still land, so refuse to sign a second one.
                let addr =
                    match crate::chains::xrp::ted25519::derive_xrp_address(path.clone()).await {
                        Ok((_pk, addr)) => addr,
                        Err(e) => {
                            guard_principal.fail();
                            return Err(ProtocolError::GenericError(format!(
                                "xrp derive failed: {e}"
                            )));
                        }
                    };
                let acct = match crate::chains::xrp::xrp_rpc::fetch_account_info(&addr).await {
                    Ok(a) => a,
                    Err(e) => {
                        guard_principal.fail();
                        return Err(ProtocolError::GenericError(format!(
                            "xrp account_info failed: {e}"
                        )));
                    }
                };
                if acct.ledger_index <= prev.last_ledger_sequence {
                    guard_principal.fail();
                    return Err(ProtocolError::GenericError(
                        "XRP settlement already in flight; retry once it confirms or expires."
                            .to_string(),
                    ));
                }
                if let Err(e) = ensure_xrp_replacement_sequence_safe(&prev, &acct) {
                    // F-03: the prior Payment's source Sequence was consumed (under some
                    // hash) yet our recorded tx_hash is NotFound on-ledger — a divergence.
                    // Durably quarantine so future settles short-circuit, then fail closed.
                    let reason = format!(
                        "settlement diverged: {:?} (tx_hash {} NotFound on-ledger)",
                        e, prev.tx_hash
                    );
                    mutate_state(|s| {
                        quarantine_xrp_claim_snapshot(s, claim_id, &reason);
                    });
                    guard_principal.fail();
                    return Err(e);
                }
                if replacement_destination.trim().is_empty() {
                    replacement_destination = match prev.destination.clone() {
                        Some(dest) => dest,
                        None => {
                            guard_principal.fail();
                            return Err(ProtocolError::GenericError(
                                "XRP settlement replacement requires a destination address."
                                    .to_string(),
                            ));
                        }
                    };
                }
                if replacement_destination_tag.is_none() {
                    replacement_destination_tag = prev.destination_tag;
                }
                // Expired and never applied -> safe to sign a fresh settlement.
            }
            Ok(crate::chains::xrp::xrp_rpc::XrpTxStatus::Failed) => {
                // Validated but failed -> funds did not move, but XRPL still
                // consumed the source-account fee. Charge it to this claim once,
                // clear the failed settlement, and allow a replacement.
                let fee = crate::chains::xrp::adapter::XRP_FEE_DROPS;
                claim.drops = claim.drops.saturating_sub(fee);
                mutate_state(|s| {
                    if let Some(c) = s.xrp_claims.get_mut(&claim_id) {
                        if c.settlement
                            .as_ref()
                            .map(|settlement| settlement.tx_hash == prev.tx_hash)
                            .unwrap_or(false)
                        {
                            c.drops = claim.drops;
                            c.settlement = None;
                        }
                    }
                });
                if replacement_destination.trim().is_empty() {
                    replacement_destination = prev.destination.clone().unwrap_or_default();
                }
                if replacement_destination_tag.is_none() {
                    replacement_destination_tag = prev.destination_tag;
                }
            }
            Err(e) => {
                guard_principal.fail();
                return Err(ProtocolError::GenericError(format!(
                    "xrp tx status failed: {e}"
                )));
            }
        }
    }

    // Sign a fresh settlement Payment (claimant bears the XRPL fee).
    let send_drops =
        match xrp_claim_send_amount(claim.drops, crate::chains::xrp::adapter::XRP_FEE_DROPS) {
            Ok(n) => n,
            Err(e) => {
                guard_principal.fail();
                return Err(e);
            }
        };

    let (source_address, acct) =
        match crate::chains::xrp::ted25519::derive_xrp_address(path.clone()).await {
            Ok((_pk, addr)) => match crate::chains::xrp::xrp_rpc::fetch_account_info(&addr).await {
                Ok(a) => (addr, a),
                Err(e) => {
                    guard_principal.fail();
                    return Err(ProtocolError::GenericError(format!(
                        "xrp account_info failed: {e}"
                    )));
                }
            },
            Err(e) => {
                guard_principal.fail();
                return Err(ProtocolError::GenericError(format!(
                    "xrp derive failed: {e}"
                )));
            }
        };
    let reserve = match crate::chains::xrp::xrp_rpc::fetch_reserve_base().await {
        Ok(r) => r,
        Err(e) => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(format!(
                "xrp server_state failed: {e}"
            )));
        }
    };
    let blocking_other_claim = match reconcile_xrp_other_inflight_claims(
        claim_id,
        claim.custody_owner,
        claim.custody_nonce,
        &acct,
    )
    .await
    {
        Ok(v) => v,
        Err(e) => {
            guard_principal.fail();
            return Err(e);
        }
    };
    if let Some(other_claim_id) = blocking_other_claim {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(format!(
            "XRP settlement for claim #{other_claim_id} is already in flight for this custody address; confirm it before settling another claim."
        )));
    }
    let unresolved_claim_drops = match read_state(|s| {
        xrp_unresolved_claim_drops_for_custody(s, claim.custody_owner, claim.custody_nonce)
    }) {
        Ok(drops) => drops,
        Err(e) => {
            guard_principal.fail();
            return Err(e);
        }
    };
    if let Err(e) = ensure_xrp_claim_aggregate_solvency(&acct, reserve, unresolved_claim_drops) {
        log!(
            INFO,
            "[settle_xrp_claim] aggregate solvency rejected claim #{} from {}: {:?}",
            claim_id,
            source_address,
            e
        );
        guard_principal.fail();
        return Err(e);
    }

    let adapter = crate::chains::xrp::adapter::XrpAdapter::new(crate::chains::xrp::XRP_CHAIN_ID);
    let payment = match adapter
        .sign_xrp_payment_from(
            path,
            &replacement_destination,
            send_drops as u128,
            replacement_destination_tag,
        )
        .await
    {
        Ok(v) => v,
        Err(e) => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(format!(
                "xrp claim sign failed: {e:?}"
            )));
        }
    };
    let signed = payment.signed;

    // Record the in-flight settlement BEFORE submitting, so a submit whose outcall
    // errors after rippled broadcast is reconciled on the next settle (confirm) call
    // rather than double-paid.
    mutate_state(|s| {
        if let Some(c) = s.xrp_claims.get_mut(&claim_id) {
            c.settlement = Some(crate::state::XrpSettlement {
                tx_hash: signed.tx_hash.clone(),
                last_ledger_sequence: payment.last_ledger_sequence,
                source_sequence: Some(payment.source_sequence),
                destination: Some(replacement_destination.clone()),
                destination_tag: replacement_destination_tag,
            });
        }
    });

    if let Err(e) =
        crate::chains::xrp::xrp_rpc::submit_blob(&hex::encode_upper(&signed.raw_tx)).await
    {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(format!(
            "xrp claim submit failed (call settle again to confirm or retry): {e}"
        )));
    }

    // Submitted. The claim is removed only once the tx is confirmed Validated on a
    // later settle call; until then it keeps `settlement` set so a retry confirms
    // instead of re-paying. Return the locally computed hash.
    guard_principal.complete();
    Ok(signed.tx_hash)
}

#[cfg(test)]
mod xrp_p4_tests {
    use super::*;

    fn claim(
        claimant: Principal,
        custody_owner: Principal,
        custody_nonce: u64,
        drops: u64,
    ) -> crate::state::XrpClaim {
        crate::state::XrpClaim {
            claimant,
            drops,
            custody_owner,
            custody_nonce,
            created_at_ns: 0,
            settlement: None,
            quarantine_reason: None,
        }
    }

    fn settlement(tx_hash: &str) -> crate::state::XrpSettlement {
        crate::state::XrpSettlement {
            tx_hash: tx_hash.to_string(),
            last_ledger_sequence: 9_000_000,
            source_sequence: Some(41),
            destination: Some("rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh".to_string()),
            destination_tag: None,
        }
    }

    #[test]
    fn claim_send_amount_subtracts_fee() {
        assert_eq!(xrp_claim_send_amount(1_000_000, 20).unwrap(), 999_980);
    }

    #[test]
    fn claim_send_amount_rejects_at_or_below_fee() {
        assert!(matches!(
            xrp_claim_send_amount(20, 20),
            Err(ProtocolError::AmountTooLow { .. })
        ));
        assert!(matches!(
            xrp_claim_send_amount(5, 20),
            Err(ProtocolError::AmountTooLow { .. })
        ));
    }

    #[test]
    fn record_xrp_claim_allocates_incrementing_ids() {
        let mut s = crate::state::State::default();
        let owner = Principal::from_slice(&[0xaa; 16]);
        let liq = Principal::from_slice(&[0xbb; 16]);
        let id0 = record_xrp_claim(&mut s, liq, owner, 7, 4_000_000, 100);
        let id1 = record_xrp_claim(&mut s, owner, owner, 8, 1_000_000, 200);
        assert_eq!(id0, 0);
        assert_eq!(id1, 1);
        assert_eq!(s.next_xrp_claim_id, 2);
        let c0 = s.xrp_claims.get(&id0).unwrap();
        assert_eq!(c0.claimant, liq);
        assert_eq!(c0.custody_owner, owner);
        assert_eq!(c0.custody_nonce, 7);
        assert_eq!(c0.drops, 4_000_000);
    }

    #[test]
    fn sp_xrp_claim_status_requires_registered_pool() {
        let mut s = crate::state::State::default();
        let sp = Principal::from_slice(&[0x5a; 16]);
        let claimant = Principal::from_slice(&[0x11; 16]);
        let owner = Principal::from_slice(&[0xaa; 16]);
        s.stability_pool_canister = Some(sp);
        s.xrp_claims.insert(7, claim(claimant, owner, 1, 2_000_000));

        let err = stability_pool_xrp_claim_outstanding_in_state(
            &s,
            Principal::from_slice(&[0xee; 16]),
            7,
            claimant,
        )
        .unwrap_err();
        assert!(matches!(err, ProtocolError::GenericError(_)));
    }

    #[test]
    fn sp_xrp_claim_status_reports_matching_claim_only() {
        let mut s = crate::state::State::default();
        let sp = Principal::from_slice(&[0x5a; 16]);
        let claimant = Principal::from_slice(&[0x11; 16]);
        let other_claimant = Principal::from_slice(&[0x22; 16]);
        let owner = Principal::from_slice(&[0xaa; 16]);
        s.stability_pool_canister = Some(sp);
        s.xrp_claims.insert(7, claim(claimant, owner, 1, 2_000_000));

        assert_eq!(
            stability_pool_xrp_claim_outstanding_in_state(&s, sp, 7, claimant).unwrap(),
            true
        );
        assert_eq!(
            stability_pool_xrp_claim_outstanding_in_state(&s, sp, 8, claimant).unwrap(),
            false
        );
        assert!(stability_pool_xrp_claim_outstanding_in_state(&s, sp, 7, other_claimant).is_err());
    }

    #[test]
    fn native_xrp_withdraw_and_close_policy_preserves_custody_vault() {
        assert_eq!(
            withdraw_close_completion_policy(true),
            WithdrawCloseCompletionPolicy::KeepNativeXrpVaultOpen
        );
        assert_eq!(
            withdraw_close_completion_policy(false),
            WithdrawCloseCompletionPolicy::CloseVault
        );
    }

    #[test]
    fn unresolved_claim_drops_aggregates_only_same_custody_address() {
        let mut s = crate::state::State::default();
        let claimant = Principal::from_slice(&[0x11; 16]);
        let owner = Principal::from_slice(&[0xaa; 16]);
        let other_owner = Principal::from_slice(&[0xbb; 16]);
        s.xrp_claims.insert(0, claim(claimant, owner, 7, 2_000_000));
        s.xrp_claims.insert(1, claim(claimant, owner, 7, 3_000_000));
        s.xrp_claims.insert(2, claim(claimant, owner, 8, 5_000_000));
        s.xrp_claims
            .insert(3, claim(claimant, other_owner, 7, 7_000_000));

        assert_eq!(
            xrp_unresolved_claim_drops_for_custody(&s, owner, 7).unwrap(),
            5_000_000
        );
    }

    #[test]
    fn inflight_claims_for_custody_detects_same_custody_only() {
        let mut s = crate::state::State::default();
        let claimant = Principal::from_slice(&[0x11; 16]);
        let owner = Principal::from_slice(&[0xaa; 16]);
        let other_owner = Principal::from_slice(&[0xbb; 16]);
        s.xrp_claims.insert(0, claim(claimant, owner, 7, 2_000_000));
        s.xrp_claims.insert(1, {
            let mut c = claim(claimant, owner, 7, 3_000_000);
            c.settlement = Some(settlement("ABC"));
            c
        });
        s.xrp_claims.insert(2, {
            let mut c = claim(claimant, other_owner, 7, 5_000_000);
            c.settlement = Some(crate::state::XrpSettlement {
                tx_hash: "DEF".to_string(),
                last_ledger_sequence: 9_000_000,
                source_sequence: Some(12),
                destination: Some("rLUEXYuLiQptky37CqLcm9USQpPiz5rkpD".to_string()),
                destination_tag: Some(99),
            });
            c
        });

        assert_eq!(
            xrp_inflight_claims_for_custody(&s, 0, owner, 7)
                .into_iter()
                .map(|(id, _)| id)
                .collect::<Vec<_>>(),
            vec![1]
        );
        assert!(xrp_inflight_claims_for_custody(&s, 1, owner, 7).is_empty());
        assert!(xrp_inflight_claims_for_custody(&s, 0, owner, 8).is_empty());
    }

    #[test]
    fn inflight_claims_for_custody_returns_settlement_snapshots() {
        let mut s = crate::state::State::default();
        let claimant = Principal::from_slice(&[0x11; 16]);
        let owner = Principal::from_slice(&[0xaa; 16]);
        s.xrp_claims.insert(0, claim(claimant, owner, 7, 2_000_000));
        s.xrp_claims.insert(1, {
            let mut c = claim(claimant, owner, 7, 3_000_000);
            c.settlement = Some(settlement("ABC"));
            c
        });

        let in_flight = xrp_inflight_claims_for_custody(&s, 0, owner, 7);

        assert_eq!(in_flight.len(), 1);
        assert_eq!(in_flight[0].0, 1);
        assert_eq!(in_flight[0].1.tx_hash, "ABC");
    }

    #[test]
    fn reconcile_paid_settlement_removes_claim_only_for_matching_hash() {
        let mut s = crate::state::State::default();
        let claimant = Principal::from_slice(&[0x11; 16]);
        let owner = Principal::from_slice(&[0xaa; 16]);
        let mut c = claim(claimant, owner, 7, 3_000_000);
        c.settlement = Some(settlement("ABC"));
        s.xrp_claims.insert(1, c);

        assert!(!reconcile_xrp_settlement_snapshot(
            &mut s,
            1,
            "DEF",
            XrpSettlementReconciliation::Paid,
        ));
        assert!(s.xrp_claims.contains_key(&1));
        assert!(reconcile_xrp_settlement_snapshot(
            &mut s,
            1,
            "ABC",
            XrpSettlementReconciliation::Paid,
        ));
        assert!(!s.xrp_claims.contains_key(&1));
    }

    #[test]
    fn reconcile_failed_settlement_charges_fee_once_and_clears_blocker() {
        let mut s = crate::state::State::default();
        let claimant = Principal::from_slice(&[0x11; 16]);
        let owner = Principal::from_slice(&[0xaa; 16]);
        let mut c = claim(claimant, owner, 7, 3_000_000);
        c.settlement = Some(settlement("ABC"));
        s.xrp_claims.insert(1, c);

        assert!(reconcile_xrp_settlement_snapshot(
            &mut s,
            1,
            "ABC",
            XrpSettlementReconciliation::FailedFeeCharged,
        ));
        let claim = s.xrp_claims.get(&1).unwrap();
        assert_eq!(
            claim.drops,
            3_000_000 - crate::chains::xrp::adapter::XRP_FEE_DROPS
        );
        assert!(claim.settlement.is_none());
    }

    #[test]
    fn reconcile_expired_not_found_clears_blocker_without_charging_fee() {
        let mut s = crate::state::State::default();
        let claimant = Principal::from_slice(&[0x11; 16]);
        let owner = Principal::from_slice(&[0xaa; 16]);
        let mut c = claim(claimant, owner, 7, 3_000_000);
        c.settlement = Some(settlement("ABC"));
        s.xrp_claims.insert(1, c);

        assert!(reconcile_xrp_settlement_snapshot(
            &mut s,
            1,
            "ABC",
            XrpSettlementReconciliation::ExpiredNotFound,
        ));
        let claim = s.xrp_claims.get(&1).unwrap();
        assert_eq!(claim.drops, 3_000_000);
        assert!(claim.settlement.is_none());
    }

    #[test]
    fn aggregate_solvency_rejects_balance_that_only_covers_current_claim() {
        let acct = crate::chains::xrp::xrp_rpc::XrpAccountInfo {
            exists: true,
            sequence: 41,
            balance_drops: 4_000_020,
            ledger_index: 9_000_000,
        };

        assert!(matches!(
            ensure_xrp_claim_aggregate_solvency(&acct, 1_000_000, 5_000_000),
            Err(ProtocolError::GenericError(_))
        ));
    }

    #[test]
    fn aggregate_solvency_accepts_exact_balance_for_all_unresolved_claims() {
        let acct = crate::chains::xrp::xrp_rpc::XrpAccountInfo {
            exists: true,
            sequence: 41,
            balance_drops: 6_000_000,
            ledger_index: 9_000_000,
        };

        assert!(ensure_xrp_claim_aggregate_solvency(&acct, 1_000_000, 5_000_000).is_ok());
    }

    #[test]
    fn replacement_rejects_missing_prior_source_sequence() {
        let prev = crate::state::XrpSettlement {
            tx_hash: "ABC".to_string(),
            last_ledger_sequence: 9_000_000,
            source_sequence: None,
            destination: Some("rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh".to_string()),
            destination_tag: None,
        };
        let acct = crate::chains::xrp::xrp_rpc::XrpAccountInfo {
            exists: true,
            sequence: 41,
            balance_drops: 10_000_000,
            ledger_index: 9_000_100,
        };

        assert!(matches!(
            ensure_xrp_replacement_sequence_safe(&prev, &acct),
            Err(ProtocolError::GenericError(_))
        ));
    }

    #[test]
    fn replacement_rejects_when_live_sequence_advanced_past_prior_source_sequence() {
        let prev = crate::state::XrpSettlement {
            tx_hash: "ABC".to_string(),
            last_ledger_sequence: 9_000_000,
            source_sequence: Some(41),
            destination: Some("rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh".to_string()),
            destination_tag: None,
        };
        let acct = crate::chains::xrp::xrp_rpc::XrpAccountInfo {
            exists: true,
            sequence: 42,
            balance_drops: 10_000_000,
            ledger_index: 9_000_100,
        };

        assert!(matches!(
            ensure_xrp_replacement_sequence_safe(&prev, &acct),
            Err(ProtocolError::GenericError(_))
        ));
    }

    #[test]
    fn replacement_allows_same_live_sequence_after_expiry() {
        let prev = crate::state::XrpSettlement {
            tx_hash: "ABC".to_string(),
            last_ledger_sequence: 9_000_000,
            source_sequence: Some(41),
            destination: Some("rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh".to_string()),
            destination_tag: None,
        };
        let acct = crate::chains::xrp::xrp_rpc::XrpAccountInfo {
            exists: true,
            sequence: 41,
            balance_drops: 10_000_000,
            ledger_index: 9_000_100,
        };

        assert!(ensure_xrp_replacement_sequence_safe(&prev, &acct).is_ok());
    }

    #[test]
    fn settle_xrp_claim_with_tag_is_exposed_at_vault_level() {
        let _ = settle_xrp_claim_with_tag;
    }

    // ── F-03 sibling-reconcile divergence guard ──────────────────────────────
    //
    // `reconcile_xrp_other_inflight_claims` clears an expired, NotFound-by-local-hash
    // SIBLING settlement to `None` so the current claim can proceed. Before the guard,
    // that clear ran unconditionally on expiry (see `reconcile_expired_not_found_clears_
    // blocker_without_charging_fee`, which still encodes the pure snapshot behavior). The
    // primary settle path already refuses to re-sign once the custody Sequence advances
    // (`replacement_rejects_when_live_sequence_advanced_past_prior_source_sequence`); the
    // sibling path did not. These tests pin the now-symmetric decision.

    fn sibling_acct(
        sequence: u32,
        ledger_index: u32,
    ) -> crate::chains::xrp::xrp_rpc::XrpAccountInfo {
        crate::chains::xrp::xrp_rpc::XrpAccountInfo {
            exists: true,
            sequence,
            balance_drops: 100_000_000,
            ledger_index,
        }
    }

    #[test]
    fn sibling_reconcile_validated_is_paid() {
        let st = settlement("SIB"); // last_ledger_sequence = 9_000_000
        let acct = sibling_acct(41, 9_000_100);
        let status = crate::chains::xrp::xrp_rpc::XrpTxStatus::Validated {
            ledger_index: 9_000_050,
            delivered_drops: 1_000_000,
        };
        assert_eq!(
            xrp_sibling_reconcile_decision(&status, &st, &acct),
            XrpSiblingReconcileDecision::Paid
        );
    }

    #[test]
    fn sibling_reconcile_failed_charges_fee() {
        let st = settlement("SIB");
        let acct = sibling_acct(41, 9_000_100);
        let status = crate::chains::xrp::xrp_rpc::XrpTxStatus::Failed;
        assert_eq!(
            xrp_sibling_reconcile_decision(&status, &st, &acct),
            XrpSiblingReconcileDecision::FailedFeeCharged
        );
    }

    #[test]
    fn sibling_reconcile_notfound_unexpired_is_still_in_flight() {
        let st = settlement("SIB"); // last_ledger_sequence = 9_000_000
        let acct = sibling_acct(41, 9_000_000); // ledger_index == LLS -> not yet expired
        let status = crate::chains::xrp::xrp_rpc::XrpTxStatus::NotFound;
        assert_eq!(
            xrp_sibling_reconcile_decision(&status, &st, &acct),
            XrpSiblingReconcileDecision::StillInFlight
        );
    }

    #[test]
    fn sibling_reconcile_expired_sequence_unchanged_is_safe_to_clear() {
        let st = settlement("SIB"); // source_sequence = Some(41)
        let acct = sibling_acct(41, 9_000_100); // expired, sequence UNCHANGED -> never applied
        let status = crate::chains::xrp::xrp_rpc::XrpTxStatus::NotFound;
        assert_eq!(
            xrp_sibling_reconcile_decision(&status, &st, &acct),
            XrpSiblingReconcileDecision::ExpiredSafeToClear
        );
    }

    /// THE F-03 sibling double-pay guard. A sibling whose tx is NotFound by our local
    /// hash but whose custody Sequence ADVANCED (its Payment consumed the sequence under a
    /// diverged hash) must be QUARANTINED, not cleared. Pre-fix this case cleared the
    /// blocker, after which the sibling was treated as fresh, re-signed, and double-paid.
    #[test]
    fn sibling_reconcile_expired_sequence_advanced_quarantines_diverged() {
        let st = settlement("SIB"); // source_sequence = Some(41)
        let acct = sibling_acct(42, 9_000_100); // expired, sequence ADVANCED past source
        let status = crate::chains::xrp::xrp_rpc::XrpTxStatus::NotFound;
        assert_eq!(
            xrp_sibling_reconcile_decision(&status, &st, &acct),
            XrpSiblingReconcileDecision::QuarantineDiverged
        );
    }

    #[test]
    fn sibling_reconcile_legacy_no_source_sequence_quarantines() {
        let mut st = settlement("SIB");
        st.source_sequence = None; // legacy settlement cannot prove its sequence -> never clear
        let acct = sibling_acct(41, 9_000_100);
        let status = crate::chains::xrp::xrp_rpc::XrpTxStatus::NotFound;
        assert_eq!(
            xrp_sibling_reconcile_decision(&status, &st, &acct),
            XrpSiblingReconcileDecision::QuarantineDiverged
        );
    }

    // ── F-03 quarantine set + admin resolve ──────────────────────────────────

    #[test]
    fn quarantine_snapshot_sets_reason_idempotently() {
        let mut s = crate::state::State::default();
        let claimant = Principal::from_slice(&[0x11; 16]);
        let owner = Principal::from_slice(&[0xaa; 16]);
        s.xrp_claims.insert(7, claim(claimant, owner, 7, 2_000_000));

        assert!(quarantine_xrp_claim_snapshot(&mut s, 7, "first reason"));
        assert_eq!(
            s.xrp_claims.get(&7).unwrap().quarantine_reason.as_deref(),
            Some("first reason")
        );
        // Idempotent: a second detection keeps the first (most precise) reason.
        assert!(quarantine_xrp_claim_snapshot(&mut s, 7, "second reason"));
        assert_eq!(
            s.xrp_claims.get(&7).unwrap().quarantine_reason.as_deref(),
            Some("first reason")
        );
    }

    #[test]
    fn quarantine_snapshot_missing_claim_is_noop_false() {
        let mut s = crate::state::State::default();
        assert!(!quarantine_xrp_claim_snapshot(&mut s, 999, "x"));
    }

    #[test]
    fn resolve_confirm_paid_removes_quarantined_claim() {
        let mut s = crate::state::State::default();
        let claimant = Principal::from_slice(&[0x11; 16]);
        let owner = Principal::from_slice(&[0xaa; 16]);
        let mut c = claim(claimant, owner, 7, 2_000_000);
        c.settlement = Some(settlement("ABC"));
        c.quarantine_reason = Some("diverged".to_string());
        s.xrp_claims.insert(7, c);

        assert!(resolve_quarantined_xrp_claim_snapshot(&mut s, 7, true).is_ok());
        assert!(!s.xrp_claims.contains_key(&7));
    }

    #[test]
    fn resolve_release_for_retry_clears_quarantine_and_settlement() {
        let mut s = crate::state::State::default();
        let claimant = Principal::from_slice(&[0x11; 16]);
        let owner = Principal::from_slice(&[0xaa; 16]);
        let mut c = claim(claimant, owner, 7, 2_000_000);
        c.settlement = Some(settlement("ABC"));
        c.quarantine_reason = Some("diverged".to_string());
        s.xrp_claims.insert(7, c);

        assert!(resolve_quarantined_xrp_claim_snapshot(&mut s, 7, false).is_ok());
        let c = s.xrp_claims.get(&7).unwrap();
        assert!(c.quarantine_reason.is_none());
        assert!(c.settlement.is_none());
        assert_eq!(c.drops, 2_000_000); // claim preserved for a clean retry
    }

    #[test]
    fn resolve_refuses_healthy_claim() {
        let mut s = crate::state::State::default();
        let claimant = Principal::from_slice(&[0x11; 16]);
        let owner = Principal::from_slice(&[0xaa; 16]);
        s.xrp_claims.insert(7, claim(claimant, owner, 7, 2_000_000)); // not quarantined

        assert!(matches!(
            resolve_quarantined_xrp_claim_snapshot(&mut s, 7, true),
            Err(ProtocolError::GenericError(_))
        ));
        // The healthy claim is untouched (not dropped).
        assert!(s.xrp_claims.contains_key(&7));
    }

    #[test]
    fn resolve_missing_claim_errors() {
        let mut s = crate::state::State::default();
        assert!(matches!(
            resolve_quarantined_xrp_claim_snapshot(&mut s, 404, true),
            Err(ProtocolError::GenericError(_))
        ));
    }
}

#[cfg(test)]
mod xrp_p3_tests {
    use super::*;

    fn pending(owner: Principal, custody_address: &str) -> crate::state::XrpPendingDeposit {
        crate::state::XrpPendingDeposit {
            owner,
            custody_address: custody_address.to_string(),
            derivation_nonce: 7,
            opened_at_ns: 123,
            reserve_base_drops: 1_000_000,
        }
    }

    #[test]
    fn credit_nets_the_base_reserve() {
        // 5 XRP balance, 1 XRP reserve -> 4 XRP (drops) credited.
        assert_eq!(
            xrp_credit_amount(5_000_000, 1_000_000, 0).unwrap(),
            4_000_000
        );
    }

    #[test]
    fn credit_rejects_balance_at_or_below_reserve() {
        assert!(matches!(
            xrp_credit_amount(900_000, 1_000_000, 0),
            Err(ProtocolError::AmountTooLow { .. })
        ));
        assert!(matches!(
            xrp_credit_amount(1_000_000, 1_000_000, 0),
            Err(ProtocolError::AmountTooLow { .. })
        ));
    }

    #[test]
    fn pending_cleanup_removes_only_when_live_account_is_unfunded() {
        let owner = Principal::from_slice(&[0x44; 16]);
        let dep = pending(owner, "rLUEXYuLiQptky37CqLcm9USQpPiz5rkpD");
        let mut s = crate::state::State::default();
        s.xrp_pending_deposits.insert(7, dep.clone());
        let acct = crate::chains::xrp::xrp_rpc::XrpAccountInfo {
            exists: false,
            sequence: 0,
            balance_drops: 0,
            ledger_index: 9_000_000,
        };

        assert_eq!(
            remove_xrp_pending_deposit_if_unfunded_snapshot(&mut s, 7, &dep, &acct).unwrap(),
            true
        );
        assert!(!s.xrp_pending_deposits.contains_key(&7));
    }

    #[test]
    fn pending_cleanup_refuses_funded_account() {
        let owner = Principal::from_slice(&[0x44; 16]);
        let dep = pending(owner, "rLUEXYuLiQptky37CqLcm9USQpPiz5rkpD");
        let mut s = crate::state::State::default();
        s.xrp_pending_deposits.insert(7, dep.clone());
        let acct = crate::chains::xrp::xrp_rpc::XrpAccountInfo {
            exists: true,
            sequence: 1,
            balance_drops: 1_000_000,
            ledger_index: 9_000_000,
        };

        assert!(matches!(
            remove_xrp_pending_deposit_if_unfunded_snapshot(&mut s, 7, &dep, &acct),
            Err(ProtocolError::GenericError(_))
        ));
        assert!(s.xrp_pending_deposits.contains_key(&7));
    }

    #[test]
    fn pending_cleanup_rechecks_snapshot_after_await_before_removing() {
        let owner = Principal::from_slice(&[0x44; 16]);
        let dep = pending(owner, "rLUEXYuLiQptky37CqLcm9USQpPiz5rkpD");
        let changed = pending(owner, "rHb9CJAWyB4rj91VRWn96DkukG4bwdtyTh");
        let mut s = crate::state::State::default();
        s.xrp_pending_deposits.insert(7, changed);
        let acct = crate::chains::xrp::xrp_rpc::XrpAccountInfo {
            exists: false,
            sequence: 0,
            balance_drops: 0,
            ledger_index: 9_000_000,
        };

        assert_eq!(
            remove_xrp_pending_deposit_if_unfunded_snapshot(&mut s, 7, &dep, &acct).unwrap(),
            false
        );
        assert!(s.xrp_pending_deposits.contains_key(&7));
    }

    #[test]
    fn pending_cleanup_age_rejects_recent_entries() {
        let owner = Principal::from_slice(&[0x44; 16]);
        let dep = pending(owner, "rLUEXYuLiQptky37CqLcm9USQpPiz5rkpD");
        assert!(matches!(
            ensure_xrp_pending_cleanup_age(&dep, dep.opened_at_ns + 1),
            Err(ProtocolError::GenericError(_))
        ));
    }

    #[test]
    fn pending_cleanup_age_accepts_old_entries() {
        let owner = Principal::from_slice(&[0x44; 16]);
        let dep = pending(owner, "rLUEXYuLiQptky37CqLcm9USQpPiz5rkpD");
        assert!(ensure_xrp_pending_cleanup_age(
            &dep,
            dep.opened_at_ns + XRP_PENDING_CLEANUP_MIN_AGE_NS
        )
        .is_ok());
    }

    #[test]
    fn credit_rejects_net_below_min_deposit() {
        // net 500k but min 1M -> too low
        assert!(matches!(
            xrp_credit_amount(1_500_000, 1_000_000, 1_000_000),
            Err(ProtocolError::AmountTooLow { .. })
        ));
    }

    #[test]
    fn credit_ok_exactly_at_min_deposit() {
        assert_eq!(
            xrp_credit_amount(2_000_000, 1_000_000, 1_000_000).unwrap(),
            1_000_000
        );
    }

    #[test]
    fn credit_rejects_u64_overflow() {
        assert!(matches!(
            xrp_credit_amount(u128::MAX, 0, 0),
            Err(ProtocolError::GenericError(_))
        ));
    }
}

pub async fn open_vault_and_borrow(
    collateral_amount_raw: u64,
    borrow_amount_raw: u64,
    collateral_type_opt: Option<Principal>,
) -> Result<OpenVaultSuccess, ProtocolError> {
    // This compound route must keep the collateral ingress journal linked to
    // the borrow mint saga through one terminal response. Until that combined
    // recovery state machine is implemented, fail closed before any pull.
    if !open_vault_and_borrow_ingress_enabled() {
        return Err(ProtocolError::TemporarilyUnavailable(
            "compound open-and-borrow is paused while exact collateral ingress recovery is added; use open_vault, then borrow separately".into(),
        ));
    }
    let caller = ic_cdk::api::caller();
    let guard_principal = match GuardPrincipal::new(caller, "open_vault_and_borrow") {
        Ok(guard) => guard,
        Err(GuardError::AlreadyProcessing) => {
            log!(
                INFO,
                "[open_vault_and_borrow] Principal {:?} already has an ongoing operation",
                caller
            );
            return Err(ProtocolError::AlreadyProcessing);
        }
        Err(GuardError::StaleOperation) => {
            log!(
                INFO,
                "[open_vault_and_borrow] Principal {:?} has a stale operation being cleaned up",
                caller
            );
            return Err(ProtocolError::TemporarilyUnavailable(
                "Previous operation is being cleaned up. Please try again in a few seconds."
                    .to_string(),
            ));
        }
        Err(err) => return Err(err.into()),
    };

    // Resolve collateral type: default to ICP if not specified
    let collateral_type =
        collateral_type_opt.unwrap_or_else(|| read_state(|s| s.icp_collateral_type()));

    // Look up CollateralConfig; check status is Active
    let (config_ledger, config_status, min_deposit, is_native_xrp) =
        read_state(|s| match s.get_collateral_config(&collateral_type) {
            Some(config) => Ok((
                config.ledger_canister_id,
                config.status,
                config.min_collateral_deposit,
                config.is_native_xrp(),
            )),
            None => Err(ProtocolError::GenericError(
                "Collateral type not supported.".to_string(),
            )),
        })?;

    // P2: native-XRP collateral is custodied on the XRP Ledger (chains::xrp), not
    // pulled via an ICRC `transfer_from`. Its deposit flow (open-then-verify) is
    // wired in P3; until then reject opens through this ICRC path so XRP collateral
    // can never be silently mishandled as an ICRC token.
    if is_native_xrp {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "Native-XRP collateral uses the XRP deposit flow (not yet enabled).".to_string(),
        ));
    }

    if !config_status.allows_open() {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "Collateral type is not accepting new vaults.".to_string(),
        ));
    }

    let icp_margin_amount: ICP = collateral_amount_raw.into();

    if min_deposit > 0 && icp_margin_amount < ICP::new(min_deposit) {
        guard_principal.fail();
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: min_deposit,
        });
    }

    // Pull collateral via ICRC-2 transfer_from (caller must have approved first)
    let block_index =
        match transfer_collateral_from(collateral_amount_raw, caller, config_ledger).await {
            Ok(bi) => bi,
            Err(transfer_from_error) => {
                guard_principal.fail();
                if let TransferFromError::BadFee { expected_fee } = transfer_from_error.clone() {
                    mutate_state(|s| {
                        if let Ok(fee) = u64::try_from(expected_fee.0) {
                            if let Some(config) = s.get_collateral_config_mut(&collateral_type) {
                                config.ledger_fee = fee;
                            }
                        }
                    });
                };
                return Err(ProtocolError::TransferFromError(
                    transfer_from_error,
                    icp_margin_amount.to_u64(),
                ));
            }
        };

    // Create the vault
    let vault_id = mutate_state(|s| {
        let vault_id = s.increment_vault_id();
        record_open_vault(
            s,
            Vault {
                owner: caller,
                borrowed_icusd_amount: 0.into(),
                collateral_amount: collateral_amount_raw,
                vault_id,
                collateral_type,
                last_accrual_time: ic_cdk::api::time(),
                accrued_interest: ICUSD::new(0),
                bot_processing: false,
            },
            block_index,
        );
        vault_id
    });

    log!(
        INFO,
        "[open_vault_and_borrow] opened vault {vault_id}, now borrowing {borrow_amount_raw}"
    );

    // Borrow icUSD — reuse internal fn to avoid guard conflict
    if borrow_amount_raw > 0 {
        // AR-B-003: per-vault op lock across the borrow's mint await.
        let _vault_op_guard = match VaultLiquidationGuard::new(vault_id) {
            Ok(g) => g,
            Err(e) => {
                guard_principal.fail();
                return Err(ProtocolError::GenericError(format!(
                    "Vault created (id={}) but borrow of {} failed: {:?}. You can borrow separately.",
                    vault_id, borrow_amount_raw, e
                )));
            }
        };
        match borrow_from_vault_internal(
            caller,
            VaultArg {
                vault_id,
                amount: borrow_amount_raw,
            },
        )
        .await
        {
            Ok(borrow_result) => {
                log!(
                    INFO,
                    "[open_vault_and_borrow] vault {} borrow of {} succeeded (fee: {})",
                    vault_id,
                    borrow_amount_raw,
                    borrow_result.fee_amount_paid
                );
            }
            Err(e) => {
                guard_principal.fail();
                return Err(ProtocolError::GenericError(format!(
                    "Vault created (id={}) but borrow of {} failed: {:?}. You can borrow separately.",
                    vault_id, borrow_amount_raw, e
                )));
            }
        }
    }

    guard_principal.complete();
    Ok(OpenVaultSuccess {
        vault_id,
        block_index,
    })
}

fn open_vault_and_borrow_ingress_enabled() -> bool {
    false
}

/// Internal borrow logic without guard management.
/// Called by both `borrow_from_vault` (which acquires its own guard) and
/// `open_vault_with_deposit` (which already holds a guard for the same principal).
const BORROW_MINT_RETRY_WINDOW_NS: u64 = 23 * 60 * 60 * 1_000_000_000;
const BORROW_MINT_MAX_ATTEMPTS: u8 = 60;

fn checked_margin_balance(current: u64, add: u64) -> Option<u64> {
    current.checked_add(add)
}

fn validate_borrow_mint_receipt(
    row: &crate::state::PendingBorrowMint,
    block: &crate::icrc3_proof::DecodedBlock,
) -> Result<(), String> {
    let expected_to = icrc_ledger_types::icrc1::account::Account {
        owner: row.owner,
        subaccount: None,
    };
    let expected_memo = management::nonce_to_memo(row.op_nonce).0;
    if !matches!(block.btype.as_deref(), None | Some("1mint"))
        || block.op != "mint"
        || block.from.is_some()
        || block.to.as_ref() != Some(&expected_to)
        || block.spender.is_some()
        || block.amount != row.net_amount_e8s as u128
        || block.transaction_fee.unwrap_or(0) != 0
        || block.fee.unwrap_or(0) != 0
        || block.memo.as_deref() != Some(expected_memo.as_slice())
        || block.created_at_time != Some(row.created_at_time_ns)
        || block.expected_allowance.is_some()
        || block.expires_at.is_some()
    {
        return Err("borrow receipt is not the exact persisted ICRC-3 mint tuple".into());
    }
    Ok(())
}

async fn dispatch_pending_borrow_mint(
    row: crate::state::PendingBorrowMint,
) -> Result<u64, ProtocolError> {
    let Some(_dispatch_guard) = BorrowMintDispatchGuard::try_new(row.op_nonce) else {
        return Err(ProtocolError::TemporarilyUnavailable(
            "this borrow mint is already being dispatched".into(),
        ));
    };
    let now = ic_cdk::api::time();
    if row.held_reason.is_some()
        || now.saturating_sub(row.created_at_time_ns) >= BORROW_MINT_RETRY_WINDOW_NS
    {
        mutate_state(|s| {
            if let Some(live) = s.pending_borrow_mints.get_mut(&row.op_nonce) {
                live.held_reason.get_or_insert_with(|| "mint tuple reached safe retry horizon; exact receipt reconciliation required".into());
            }
        });
        return Err(ProtocolError::GenericError(
            "borrow mint is held for exact receipt reconciliation".into(),
        ));
    }
    let updated = mutate_state(|s| {
        let Some(live) = s.pending_borrow_mints.get_mut(&row.op_nonce) else {
            return Err(ProtocolError::GenericError(
                "borrow mint journal disappeared".into(),
            ));
        };
        if live != &row || live.held_reason.is_some() {
            return Err(ProtocolError::GenericError(
                "borrow mint journal changed; reconciliation required".into(),
            ));
        }
        live.attempts = live.attempts.saturating_add(1);
        live.last_attempt_at_ns = now;
        Ok(live.clone())
    })?;
    let to = icrc_ledger_types::icrc1::account::Account {
        owner: updated.owner,
        subaccount: None,
    };
    let result = management::mint_icusd_with_nonce(
        updated.ledger,
        ICUSD::from(updated.net_amount_e8s),
        to,
        updated.op_nonce,
    )
    .await;
    let block_index = match result {
        Ok(block) => block,
        Err(error) => {
            let held = matches!(
                error,
                icrc_ledger_types::icrc1::transfer::TransferError::TooOld
            ) || updated.attempts >= BORROW_MINT_MAX_ATTEMPTS
                || ic_cdk::api::time().saturating_sub(updated.created_at_time_ns)
                    >= BORROW_MINT_RETRY_WINDOW_NS;
            if held {
                mutate_state(|s| {
                    if let Some(live) = s.pending_borrow_mints.get_mut(&updated.op_nonce) {
                        live.held_reason = Some(format!(
                            "borrow mint requires manual exact receipt reconciliation: {error:?}"
                        ));
                    }
                });
            }
            return Err(ProtocolError::TransferError(error));
        }
    };
    let block = crate::icrc3_proof::fetch_icrc3_block(updated.ledger, block_index)
        .await
        .map_err(|e| {
            ProtocolError::GenericError(format!(
                "borrow mint receipt unavailable; operation remains fenced: {e}"
            ))
        })?;
    validate_borrow_mint_receipt(&updated, &block).map_err(|e| {
        mutate_state(|s| {
            if let Some(live) = s.pending_borrow_mints.get_mut(&updated.op_nonce) {
                live.held_reason = Some(e.clone());
            }
        });
        ProtocolError::GenericError(format!(
            "borrow mint receipt rejected; operation remains fenced: {e}"
        ))
    })?;
    let treasury_operation = mutate_state(|s| {
        if s.pending_borrow_mints.get(&updated.op_nonce) != Some(&updated) {
            return Err(ProtocolError::GenericError(
                "borrow journal changed after mint proof; operation remains fenced".into(),
            ));
        }
        let Some(vault) = s.vault_id_to_vaults.get(&updated.vault_id) else {
            return Err(ProtocolError::GenericError(
                "borrowed vault disappeared; operation remains fenced".into(),
            ));
        };
        if vault.owner != updated.owner
            || vault.collateral_type != updated.collateral_type
            || vault
                .borrowed_icusd_amount
                .to_u64()
                .checked_add(updated.gross_amount_e8s)
                .is_none()
        {
            return Err(ProtocolError::GenericError(
                "borrow vault identity or debt changed; operation remains fenced".into(),
            ));
        }
        record_borrow_from_vault(
            s,
            updated.vault_id,
            updated.owner,
            ICUSD::from(updated.gross_amount_e8s),
            ICUSD::from(updated.fee_e8s),
            block_index,
        );
        let treasury_operation =
            crate::treasury::queue_borrowing_fee_in_state(s, ICUSD::from(updated.fee_e8s));
        s.pending_borrow_mints.remove(&updated.op_nonce);
        Ok(treasury_operation)
    })?;
    if let Some(operation_id) = treasury_operation {
        crate::treasury::process_queued_treasury_payment(operation_id).await;
    }
    Ok(block_index)
}

pub async fn process_pending_borrow_mints() {
    let rows = mutate_state(|s| s.next_pending_borrow_mint_batch(16));
    for row in rows {
        let _ = dispatch_pending_borrow_mint(row).await;
    }
}

/// Reconcile a held borrow only from a caller-supplied block that proves the
/// persisted mint tuple. This path never dispatches or substitutes a nonce.
pub async fn reconcile_pending_borrow_mint_receipt(
    op_nonce: u128,
    block_index: u64,
) -> Result<bool, String> {
    let Some(_dispatch_guard) = BorrowMintDispatchGuard::try_new(op_nonce) else {
        return Err("borrow mint is currently being dispatched".into());
    };
    let row = read_state(|s| s.pending_borrow_mints.get(&op_nonce).cloned())
        .ok_or_else(|| "no pending borrow mint found".to_string())?;
    let block = crate::icrc3_proof::fetch_icrc3_block(row.ledger, block_index).await?;
    validate_borrow_mint_receipt(&row, &block)?;
    let treasury_operation = mutate_state(|s| {
        if s.pending_borrow_mints.get(&op_nonce) != Some(&row) {
            return Err(
                "borrow mint changed during receipt verification; retry reconciliation".to_string(),
            );
        }
        let Some(vault) = s.vault_id_to_vaults.get(&row.vault_id) else {
            return Err("borrow vault disappeared; receipt is proven but debt remains held".into());
        };
        if vault.owner != row.owner || vault.collateral_type != row.collateral_type {
            return Err(
                "borrow vault identity changed; receipt is proven but debt remains held".into(),
            );
        }
        record_borrow_from_vault(
            s,
            row.vault_id,
            row.owner,
            ICUSD::from(row.gross_amount_e8s),
            ICUSD::from(row.fee_e8s),
            block_index,
        );
        let treasury_operation =
            crate::treasury::queue_borrowing_fee_in_state(s, ICUSD::from(row.fee_e8s));
        s.pending_borrow_mints.remove(&op_nonce);
        Ok(treasury_operation)
    })?;
    if let Some(operation_id) = treasury_operation {
        crate::treasury::process_queued_treasury_payment(operation_id).await;
    }
    Ok(true)
}

pub fn has_pending_borrow_mints() -> bool {
    read_state(|s| !s.pending_borrow_mints.is_empty())
}

async fn borrow_from_vault_internal(
    caller: Principal,
    arg: VaultArg,
) -> Result<SuccessWithFee, ProtocolError> {
    let amount: ICUSD = arg.amount.into();

    if amount < read_state(|s| s.min_icusd_amount) {
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: read_state(|s| s.min_icusd_amount).to_u64(),
        });
    }

    // Accrue interest on this vault before borrowing so CR check uses up-to-date debt.
    let now = ic_cdk::api::time();
    reject_active_xrp_sp_absorb_preflight(arg.vault_id, now)?;
    mutate_state(|s| s.accrue_single_vault(arg.vault_id, now));

    let (vault, collateral_price, config_decimals, is_native_xrp) =
        read_state(|s| match s.vault_id_to_vaults.get(&arg.vault_id) {
            Some(vault) => {
                let price = s
                    .get_collateral_price_decimal(&vault.collateral_type)
                    .ok_or("No price available for collateral. Price feed may be down.")?;
                let config = s
                    .get_collateral_config(&vault.collateral_type)
                    .ok_or("Collateral type not configured.")?;
                Ok((
                    vault.clone(),
                    price,
                    config.decimals,
                    config.is_native_xrp(),
                ))
            }
            None => Err("Vault not found. Please check the vault ID."),
        })
        .map_err(|msg: &str| ProtocolError::GenericError(msg.to_string()))?;

    require_vault_not_processing(&vault)?;

    // Check collateral status allows borrowing
    let collateral_status = read_state(|s| s.get_collateral_status(&vault.collateral_type));
    if let Some(status) = collateral_status {
        if !status.allows_borrow() {
            return Err(ProtocolError::GenericError(
                "Borrowing is not allowed for this collateral type.".to_string(),
            ));
        }
    }
    if is_native_xrp {
        require_xrp_production_key()?;
    }

    if caller != vault.owner {
        return Err(ProtocolError::CallerNotOwner);
    }

    // Check debt ceiling + global mint cap AND reserve the headroom atomically.
    //
    // BK-003 (audit 2026-06-05): these caps are checked here but the debt is not
    // recorded until after the `mint_icusd().await` below. Two borrows from
    // DIFFERENT owners both pass this check against the same committed aggregate,
    // both mint, and jointly exceed the cap (the per-caller GuardPrincipal does
    // not serialize distinct owners against the aggregate). The reservation guard
    // counts every in-flight borrow in the check and is held across the mint, so
    // a concurrent borrow sees this one's reserved amount. Released on Drop
    // (return or continuation-trap via ic-cdk cleanup).
    let (current_debt, pending_collateral, total_borrowed, pending_global, global_cap) =
        read_state(|s| {
            (
                s.total_debt_for_collateral(&vault.collateral_type).to_u64(),
                s.pending_borrow_reserved_for(&vault.collateral_type),
                s.total_borrowed_icusd_amount().to_u64(),
                s.pending_borrow_reserved_global(),
                s.global_icusd_mint_cap,
            )
        });
    let debt_ceiling = read_state(|s| {
        s.get_collateral_config(&vault.collateral_type)
            .map(|c| c.debt_ceiling)
            .unwrap_or(u64::MAX)
    });
    let current_debt = current_debt.saturating_add(pending_collateral);
    let total_borrowed = total_borrowed.saturating_add(pending_global);
    let _borrow_reservation = crate::guard::BorrowReservationGuard::try_reserve(
        vault.collateral_type,
        amount.to_u64(),
        current_debt,
        debt_ceiling,
        total_borrowed,
        global_cap,
    )
    .map_err(ProtocolError::GenericError)?;

    let collateral_value = crate::numeric::collateral_usd_value(
        vault.collateral_amount,
        collateral_price,
        config_decimals,
    );
    let min_ratio = read_state(|s| {
        let base = s.get_min_collateral_ratio_for(&vault.collateral_type);
        if s.mode == Mode::Recovery {
            let recovery_cr = s.get_recovery_cr_for(&vault.collateral_type);
            if recovery_cr > base {
                recovery_cr
            } else {
                base
            }
        } else {
            base
        }
    });
    let max_borrowable_amount: ICUSD = collateral_value / min_ratio;

    if vault.borrowed_icusd_amount + amount > max_borrowable_amount {
        return Err(ProtocolError::GenericError(format!(
            "failed to borrow from vault, max borrowable: {max_borrowable_amount}, borrowed: {}, requested: {amount}",
            vault.borrowed_icusd_amount
        )));
    }

    // Compute projected vault CR after this borrow (for dynamic fee multiplier)
    let new_total_debt = vault.borrowed_icusd_amount + amount;
    let projected_cr = if new_total_debt.to_u64() == 0 {
        Ratio::new(dec!(999))
    } else {
        Ratio::from(
            Decimal::from_u64(collateral_value.to_u64()).unwrap_or(Decimal::ZERO)
                / Decimal::from_u64(new_total_debt.to_u64()).unwrap_or(Decimal::ONE),
        )
    };

    let fee: ICUSD = read_state(|s| {
        let base_fee = s.get_borrowing_fee_for(&vault.collateral_type);
        let multiplier = s.get_borrowing_fee_multiplier(projected_cr);
        let raw_fee: ICUSD = amount * base_fee * multiplier;
        // INT-003: clamp so `amount - fee >= 1 e8s`. Defense in depth: the
        // curve validator caps the multiplier, but a legacy or migrated curve
        // (or any future code path that writes the fee state outside
        // `set_borrowing_fee_curve`) cannot panic the borrow path.
        clamp_borrow_fee(amount, raw_fee)
    });

    crate::storage::mark_borrow_mint_journal_used()
        .map_err(ProtocolError::TemporarilyUnavailable)?;
    let pending = mutate_state(|s| {
        let debt_now = s.total_debt_for_collateral(&vault.collateral_type).to_u64();
        let collateral_reserved = s.pending_borrow_reserved_for(&vault.collateral_type);
        let total_now = s.total_borrowed_icusd_amount().to_u64();
        let global_reserved = s.pending_borrow_reserved_global();
        let live_debt_ceiling = s
            .get_collateral_config(&vault.collateral_type)
            .map(|config| config.debt_ceiling)
            .unwrap_or(u64::MAX);
        if debt_now
            .checked_add(collateral_reserved)
            .and_then(|v| v.checked_add(amount.to_u64()))
            .is_none_or(|v| v > live_debt_ceiling)
            || total_now
                .checked_add(global_reserved)
                .and_then(|v| v.checked_add(amount.to_u64()))
                .is_none_or(|v| v > s.global_icusd_mint_cap)
        {
            return Err(ProtocolError::GenericError(
                "borrow capacity changed before mint dispatch".into(),
            ));
        }
        let Some(live_vault) = s.vault_id_to_vaults.get(&arg.vault_id) else {
            return Err(ProtocolError::GenericError(
                "vault disappeared before mint dispatch".into(),
            ));
        };
        if live_vault.owner != caller || live_vault.collateral_type != vault.collateral_type {
            return Err(ProtocolError::GenericError(
                "vault identity changed before mint dispatch".into(),
            ));
        }
        let op_nonce = s.next_op_nonce();
        let pending = crate::state::PendingBorrowMint {
            op_nonce,
            vault_id: arg.vault_id,
            owner: caller,
            collateral_type: vault.collateral_type,
            ledger: s.icusd_ledger_principal,
            gross_amount_e8s: amount.to_u64(),
            fee_e8s: fee.to_u64(),
            net_amount_e8s: (amount - fee).to_u64(),
            created_at_time_ns: management::nonce_to_created_at_time(op_nonce),
            created_at_ns: ic_cdk::api::time(),
            attempts: 0,
            last_attempt_at_ns: 0,
            held_reason: None,
        };
        s.pending_borrow_mints.insert(op_nonce, pending.clone());
        Ok(pending)
    })?;

    match dispatch_pending_borrow_mint(pending).await {
        Ok(block_index) => {
            Ok(SuccessWithFee {
                block_index,
                fee_amount_paid: fee.to_u64(),
                collateral_amount_received: None,
                debt_liquidated_e8s: None, // SP-101
                stable_pulled_e6s: None,   // SP-110
                xrp_claim_id: None,
            })
        }
        Err(error) => Err(error),
    }
}

pub async fn borrow_from_vault(arg: VaultArg) -> Result<SuccessWithFee, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let guard_principal =
        match GuardPrincipal::new(caller, &format!("borrow_vault_{}", arg.vault_id)) {
            Ok(guard) => guard,
            Err(GuardError::AlreadyProcessing) => {
                log!(
                    INFO,
                    "[borrow_from_vault] Principal {:?} already has an ongoing operation",
                    caller
                );
                return Err(ProtocolError::AlreadyProcessing);
            }
            Err(err) => return Err(err.into()),
        };

    // AR-B-003 (audit 2026-06-09): per-vault op lock. The per-caller guard
    // above does not exclude a concurrent liquidation/redemption of this
    // vault; this lock does (and the redemption water-fill skips locked
    // vaults). See guard.rs::VaultLiquidationGuard.
    let _vault_op_guard = match VaultLiquidationGuard::new(arg.vault_id) {
        Ok(g) => g,
        Err(e) => {
            guard_principal.fail();
            return Err(e);
        }
    };

    match borrow_from_vault_internal(caller, arg).await {
        Ok(result) => {
            guard_principal.complete();
            Ok(result)
        }
        Err(e) => {
            guard_principal.fail();
            Err(e)
        }
    }
}

/// Internal repay logic without guard management.
///
/// Called by both `repay_to_vault` (which acquires its own `repay_vault_{id}`
/// guard) and `repay_and_close_vault` (which holds a single
/// `repay_and_close_{id}` guard spanning repay + withdraw + close).
///
/// Performs interest accrual, validates caller/state, pulls icUSD via
/// `icrc2_transfer_from`, records the repayment, and distributes the interest
/// share to treasury.
///
/// `is_full_close` signals that the caller is the `repay_and_close_vault`
/// compound endpoint and intends to zero the vault's debt in this call. When
/// true, the `MIN_ICUSD_AMOUNT` floor is bypassed so vaults stuck in the
/// `(DUST_DEBT_THRESHOLD, MIN_ICUSD_AMOUNT)` zone can be cleared. The floor
/// stays in force for the regular `repay_to_vault` path as an anti-spam
/// guarantee — an explicit flag (rather than `amount == debt` equality) avoids
/// brittleness from interest accruing between the caller's debt fetch and
/// this helper's read.
async fn repay_to_vault_internal(
    caller: Principal,
    arg: VaultArg,
    is_full_close: bool,
) -> Result<u64, ProtocolError> {
    let amount: ICUSD = arg.amount.into();

    // Accrue interest before repayment so the correct debt balance is used.
    let now = ic_cdk::api::time();
    reject_active_xrp_sp_absorb_preflight(arg.vault_id, now)?;
    mutate_state(|s| s.accrue_single_vault(arg.vault_id, now));

    let vault = read_state(|s| s.vault_id_to_vaults.get(&arg.vault_id).cloned())
        .ok_or_else(|| ProtocolError::GenericError("Vault not found".to_string()))?;

    require_vault_not_processing(&vault)?;

    // Check collateral status allows repayment
    let collateral_status = read_state(|s| s.get_collateral_status(&vault.collateral_type));
    if let Some(status) = collateral_status {
        if !status.allows_repay() {
            return Err(ProtocolError::GenericError(
                "Repayment is not allowed for this collateral type.".to_string(),
            ));
        }
    }

    if caller != vault.owner {
        return Err(ProtocolError::CallerNotOwner);
    }

    if !is_full_close && amount < read_state(|s| s.min_icusd_amount) {
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: read_state(|s| s.min_icusd_amount).to_u64(),
        });
    }

    // Cap repay amount to actual debt. Interest accrued between when the
    // frontend read the balance and now can push debt slightly above what
    // the user entered. If the requested amount exceeds or nearly matches
    // the current debt (within 1% or 0.01 icUSD — whichever is larger),
    // treat it as a full repayment to avoid leaving un-repayable dust.
    let debt = vault.borrowed_icusd_amount;
    let dust_threshold = std::cmp::max(debt.0 / 100, 1_000_000); // 1% or 0.01 icUSD
    let amount = if amount > debt {
        debt
    } else if debt.0.saturating_sub(amount.0) <= dust_threshold {
        debt
    } else {
        amount
    };

    check_min_vault_debt_after_repay(&vault, amount)?;

    match transfer_icusd_from(amount, caller).await {
        Ok(block_index) => {
            let interest_share =
                mutate_state(|s| record_repayed_to_vault(s, arg.vault_id, amount, block_index));
            // IC-B-002 (audit 2026-06-09): re-queue any unminted interest share so the
            // next flush retries it instead of silently dropping treasury revenue.
            let unminted_interest =
                crate::treasury::distribute_interest(interest_share, vault.collateral_type).await;
            if unminted_interest.to_u64() > 0 {
                mutate_state(|s| {
                    s.restore_pending_interest_for_pool(
                        vault.collateral_type,
                        unminted_interest.to_u64(),
                    )
                });
            }
            Ok(block_index)
        }
        Err(transfer_from_error) => Err(ProtocolError::TransferFromError(
            transfer_from_error,
            amount.to_u64(),
        )),
    }
}

fn repayment_v2_payload_matches(
    row: &crate::state::RepaymentV2Journal,
    caller: Principal,
    request_id: u128,
    arg: &VaultArg,
    close_after_repay: bool,
) -> bool {
    row.owner == caller
        && row.request_id == request_id
        && row.vault_id == arg.vault_id
        && row.requested_amount_raw == arg.amount
        && row.close_after_repay == close_after_repay
}

fn repayment_v2_tuple_for_proof(
    tuple: &crate::RepaymentV2PullTuple,
) -> crate::SpLiquidationStablePullTuple {
    crate::SpLiquidationStablePullTuple {
        op_nonce: tuple.op_nonce,
        ledger: tuple.ledger,
        from: tuple.from.clone(),
        spender: tuple.spender.clone(),
        to: tuple.to.clone(),
        amount_raw: tuple.amount_raw,
        fee_raw: tuple.fee_raw,
        memo: tuple.memo.clone(),
        created_at_time_ns: tuple.created_at_time_ns,
    }
}

fn repayment_v2_failure(
    row: &mut crate::state::RepaymentV2Journal,
    message: String,
    ambiguous: bool,
) {
    row.last_error = Some(message);
    row.had_ambiguous_attempt |= ambiguous;
    row.phase = crate::RepaymentV2Phase::HeldPull;
}

/// Durable caller-ID repayment. The pull tuple is saved before dispatch and a
/// positive exact ICRC-3 burn receipt is required before debt/event mutation.
pub async fn repay_v2(
    request_id: u128,
    arg: VaultArg,
    close_after_repay: bool,
) -> Result<crate::RepaymentV2StatusView, ProtocolError> {
    let caller = ic_cdk::api::caller();
    if request_id == 0 || arg.amount == 0 {
        return Err(ProtocolError::GenericError(
            "repayment request ID and amount must be nonzero".into(),
        ));
    }

    // Exact replay/status is resolved before reading live vault state. This is
    // essential after a successful debt commit and lost outer reply.
    let existing = read_state(|s| {
        s.repayment_v2_active
            .get(&caller)
            .cloned()
            .or_else(|| s.repayment_v2_latest_result.get(&caller).cloned())
    });
    if let Some(row) = existing {
        if row.request_id == request_id {
            if !repayment_v2_payload_matches(&row, caller, request_id, &arg, close_after_repay) {
                return Err(ProtocolError::GenericError(
                    "repayment request ID is already bound to a different payload".into(),
                ));
            }
            if matches!(
                row.phase,
                crate::RepaymentV2Phase::Complete
                    | crate::RepaymentV2Phase::Rejected
                    | crate::RepaymentV2Phase::CloseNeedsAdditionalRepayment
            ) {
                return Ok(row.status_view());
            }
        } else if read_state(|s| s.repayment_v2_active.contains_key(&caller)) {
            return Err(ProtocolError::AlreadyProcessing);
        } else if request_id <= row.request_id {
            return Err(ProtocolError::GenericError(
                "repayment request ID is older than the retained result; it cannot be replayed"
                    .into(),
            ));
        }
    }

    let _vault_guard = VaultLiquidationGuard::new(arg.vault_id)?;
    let mut row = match read_state(|s| s.repayment_v2_active.get(&caller).cloned()) {
        Some(row) => {
            if !repayment_v2_payload_matches(&row, caller, request_id, &arg, close_after_repay) {
                return Err(ProtocolError::GenericError(
                    "repayment request ID is unresolved or bound to a different payload".into(),
                ));
            }
            row
        }
        None => {
            let next_id = read_state(|s| {
                s.repayment_v2_high_water
                    .get(&caller)
                    .copied()
                    .unwrap_or(0)
                    .checked_add(1)
            })
            .ok_or_else(|| {
                ProtocolError::GenericError("repayment request ID sequence exhausted".into())
            })?;
            if request_id != next_id {
                return Err(ProtocolError::GenericError(format!(
                    "repayment request ID must be the next sequence value ({next_id})"
                )));
            }

            let now = ic_cdk::api::time();
            reject_active_xrp_sp_absorb_preflight(arg.vault_id, now)?;
            mutate_state(|s| s.accrue_single_vault(arg.vault_id, now));
            let vault = read_state(|s| s.vault_id_to_vaults.get(&arg.vault_id).cloned())
                .ok_or_else(|| ProtocolError::GenericError("Vault not found".into()))?;
            require_vault_not_processing(&vault)?;
            if read_state(|s| s.pending_collateral_withdrawals.contains_key(&arg.vault_id)) {
                return Err(ProtocolError::TemporarilyUnavailable(
                    "vault has an unresolved collateral withdrawal".into(),
                ));
            }
            if caller != vault.owner {
                return Err(ProtocolError::CallerNotOwner);
            }
            if let Some(status) = read_state(|s| s.get_collateral_status(&vault.collateral_type)) {
                if !status.allows_repay() {
                    return Err(ProtocolError::GenericError(
                        "Repayment is not allowed for this collateral type.".into(),
                    ));
                }
            }
            let min_amount = read_state(|s| s.min_icusd_amount);
            if !close_after_repay && ICUSD::from(arg.amount) < min_amount {
                return Err(ProtocolError::AmountTooLow {
                    minimum_amount: min_amount.to_u64(),
                });
            }
            let debt = vault.borrowed_icusd_amount;
            let requested = ICUSD::from(arg.amount);
            let dust_threshold = std::cmp::max(debt.0 / 100, 1_000_000);
            let effective = if requested > debt {
                debt
            } else if debt.0.saturating_sub(requested.0) <= dust_threshold {
                debt
            } else {
                requested
            };
            check_min_vault_debt_after_repay(&vault, effective)?;

            let ledger = read_state(|s| s.icusd_ledger_principal);
            crate::sp_burn_refund::verify_mint_authority(ledger).await?;
            // Recheck owner/debt/config after the authority await; only interest
            // accrual is allowed to have changed the debt upward.
            let current = read_state(|s| s.vault_id_to_vaults.get(&arg.vault_id).cloned())
                .ok_or_else(|| {
                    ProtocolError::TemporarilyUnavailable(
                        "vault disappeared during repayment admission".into(),
                    )
                })?;
            if current.owner != caller
                || current.collateral_type != vault.collateral_type
                || read_state(|s| s.icusd_ledger_principal) != ledger
                || read_state(|s| s.pending_collateral_withdrawals.contains_key(&arg.vault_id))
            {
                return Err(ProtocolError::TemporarilyUnavailable(
                    "repayment plan changed during admission; retry with a new request ID".into(),
                ));
            }
            require_vault_not_processing(&current)?;
            let current_debt = current.borrowed_icusd_amount;
            let current_dust_threshold = std::cmp::max(current_debt.0 / 100, 1_000_000);
            let effective = if requested > current_debt {
                current_debt
            } else if current_debt.0.saturating_sub(requested.0) <= current_dust_threshold {
                current_debt
            } else {
                requested
            };
            check_min_vault_debt_after_repay(&current, effective)?;
            if close_after_repay
                && current_debt.0.saturating_sub(effective.0) > crate::state::DUST_DEBT_THRESHOLD
            {
                return Err(ProtocolError::GenericError(
                    "repay-and-close requires a full repayment; the requested amount would leave debt above the forgivable dust threshold".into(),
                ));
            }

            // ICRC-2 transfer_from to the verified minting account is a
            // fee-free burn on the official ledger. Pin explicit zero fee.
            let tuple = mutate_state(|s| {
                let op_nonce = s.next_op_nonce();
                let memo = management::nonce_to_memo(op_nonce).0.to_vec();
                crate::RepaymentV2PullTuple {
                    op_nonce,
                    ledger,
                    from: Account {
                        owner: caller,
                        subaccount: None,
                    },
                    spender: Account {
                        owner: ic_cdk::id(),
                        subaccount: None,
                    },
                    to: Account {
                        owner: ic_cdk::id(),
                        subaccount: None,
                    },
                    amount_raw: effective.to_u64(),
                    fee_raw: 0,
                    memo,
                    created_at_time_ns: management::nonce_to_created_at_time(op_nonce),
                }
            });
            let new_row = crate::state::RepaymentV2Journal {
                owner: caller,
                request_id,
                vault_id: arg.vault_id,
                requested_amount_raw: arg.amount,
                effective_amount_raw: effective.to_u64(),
                pinned_debt_raw: current.borrowed_icusd_amount.to_u64(),
                close_after_repay,
                collateral_type: current.collateral_type,
                ledger,
                tuple,
                phase: crate::RepaymentV2Phase::PendingPull,
                candidate_block_index: None,
                candidate_attach_window_start_ns: 0,
                candidate_attach_attempts: 0,
                had_ambiguous_attempt: false,
                dispatch_attempts: 0,
                repay_block_index: None,
                collateral_return_block_index: None,
                interest_share_raw: 0,
                last_error: None,
            };
            let backend = ic_cdk::id();
            let admitted =
                mutate_state(|s| crate::state::admit_repayment_v2(s, new_row.clone(), backend));
            admitted.map_err(ProtocolError::GenericError)?;
            new_row
        }
    };

    if matches!(
        row.phase,
        crate::RepaymentV2Phase::PendingPull | crate::RepaymentV2Phase::HeldPull
    ) && row.candidate_block_index.is_none()
    {
        // Recheck that the pinned ledger is still the configured icUSD ledger
        // and still names this backend as mint authority before each dispatch.
        crate::sp_burn_refund::verify_mint_authority(row.ledger).await?;
        if read_state(|s| {
            s.repayment_v2_active
                .get(&caller)
                .is_some_and(|saved| saved.request_id == request_id && saved.tuple == row.tuple)
        }) == false
        {
            return Err(ProtocolError::TemporarilyUnavailable(
                "repayment journal changed before pull dispatch".into(),
            ));
        }
        // A persisted attempt counter proves a previous dispatch may have
        // committed even if the callback trapped before recording ambiguity.
        let prior_ambiguous = row.had_ambiguous_attempt || row.dispatch_attempts > 0;
        row.dispatch_attempts = row.dispatch_attempts.saturating_add(1);
        let attempt = row.dispatch_attempts;
        row.last_error = None;
        mutate_state(|s| crate::state::save_repayment_v2(s, row.clone()))
            .map_err(ProtocolError::GenericError)?;
        let tuple = repayment_v2_tuple_for_proof(&row.tuple);
        let outcome = management::transfer_from_with_exact_tuple(&tuple).await;
        match outcome {
            Ok(block_index) => {
                row.candidate_block_index = Some(block_index);
                row.had_ambiguous_attempt = prior_ambiguous;
                row.last_error = None;
                mutate_state(|s| crate::state::save_repayment_v2(s, row.clone()))
                    .map_err(ProtocolError::GenericError)?;
            }
            Err(error) => {
                let no_effect = matches!(
                    error,
                    TransferFromError::BadFee { .. }
                        | TransferFromError::BadBurn { .. }
                        | TransferFromError::InsufficientFunds { .. }
                        | TransferFromError::InsufficientAllowance { .. }
                        | TransferFromError::TooOld
                        | TransferFromError::CreatedInFuture { .. }
                );
                let message = format!("repayment transfer_from returned {error:?}");
                if no_effect && !prior_ambiguous && attempt == 1 {
                    row.phase = crate::RepaymentV2Phase::Rejected;
                    row.last_error = Some(message);
                    mutate_state(|s| {
                        crate::state::save_repayment_v2(s, row.clone())?;
                        crate::state::finish_repayment_v2(s, caller, request_id)
                    })
                    .map_err(ProtocolError::GenericError)?;
                    return Ok(row.status_view());
                }
                repayment_v2_failure(&mut row, message, !no_effect || prior_ambiguous);
                mutate_state(|s| crate::state::save_repayment_v2(s, row.clone()))
                    .map_err(ProtocolError::GenericError)?;
                return Ok(row.status_view());
            }
        }
    }

    if row.repay_block_index.is_none() {
        let Some(block_index) = row.candidate_block_index else {
            return Ok(row.status_view());
        };
        if let Err(error) = crate::icrc3_proof::verify_sp_liquidation_icusd_burn_block(
            &repayment_v2_tuple_for_proof(&row.tuple),
            block_index,
        )
        .await
        {
            repayment_v2_failure(
                &mut row,
                format!("repayment receipt proof failed: {error}"),
                true,
            );
            mutate_state(|s| crate::state::save_repayment_v2(s, row.clone()))
                .map_err(ProtocolError::GenericError)?;
            return Ok(row.status_view());
        }
        let exact_row = read_state(|s| {
            s.repayment_v2_active.get(&caller).is_some_and(|saved| {
                saved.request_id == request_id
                    && saved.tuple == row.tuple
                    && saved.candidate_block_index == Some(block_index)
            })
        });
        if !exact_row {
            return Err(ProtocolError::TemporarilyUnavailable(
                "repayment journal changed during receipt verification".into(),
            ));
        }
        let commit_result = mutate_state(|s| {
            let saved_row = s
                .repayment_v2_active
                .get(&caller)
                .filter(|saved| {
                    saved.request_id == request_id
                        && saved.tuple == row.tuple
                        && saved.candidate_block_index == Some(block_index)
                        && saved.repay_block_index.is_none()
                })
                .ok_or_else(|| "repayment row changed before debt commit".to_string())?;
            let vault = s
                .vault_id_to_vaults
                .get(&row.vault_id)
                .ok_or_else(|| "vault disappeared before debt commit".to_string())?;
            if vault.owner != caller
                || vault.collateral_type != row.collateral_type
                || vault.borrowed_icusd_amount.0 < row.effective_amount_raw
                || s.pending_collateral_withdrawals.contains_key(&row.vault_id)
            {
                return Err(
                    "vault state changed before debt commit; proven pull remains held".into(),
                );
            }
            let close_after_repay = saved_row.close_after_repay;
            let interest = record_repayed_to_vault(
                s,
                row.vault_id,
                ICUSD::from(row.effective_amount_raw),
                block_index,
            );
            // Queue exactly once in the same atomic mutation as the debt/event
            // commit. The regular durable interest flusher owns delivery.
            s.restore_pending_interest_for_pool(row.collateral_type, interest.to_u64());
            let saved = s
                .repayment_v2_active
                .get_mut(&caller)
                .ok_or_else(|| "repayment row disappeared during debt commit".to_string())?;
            saved.repay_block_index = Some(block_index);
            saved.interest_share_raw = interest.to_u64();
            saved.phase = if close_after_repay {
                crate::RepaymentV2Phase::ClosePending
            } else {
                crate::RepaymentV2Phase::RepayCommitted
            };
            saved.last_error = None;
            crate::storage::save_state_to_stable(s);
            Ok::<_, String>(interest)
        });
        let interest_share = match commit_result {
            Ok(interest) => interest,
            Err(message) => {
                repayment_v2_failure(&mut row, message, true);
                mutate_state(|s| crate::state::save_repayment_v2(s, row.clone()))
                    .map_err(ProtocolError::GenericError)?;
                return Ok(row.status_view());
            }
        };
        row.repay_block_index = Some(block_index);
        row.interest_share_raw = interest_share.to_u64();
        row.phase = if row.close_after_repay {
            crate::RepaymentV2Phase::ClosePending
        } else {
            crate::RepaymentV2Phase::RepayCommitted
        };
    }

    if row.close_after_repay && row.phase == crate::RepaymentV2Phase::ClosePending {
        // The periodic accrual timer may have run while the exact burn receipt
        // was being fetched. Never leave a committed repayment active forever
        // when that new interest makes the original close amount insufficient;
        // publish the receipt-bearing outcome and allow a fresh ID for the
        // remaining debt. Once the collateral withdrawal row is installed,
        // accrual skips the vault until close settlement.
        let remaining_debt = read_state(|s| {
            s.vault_id_to_vaults
                .get(&row.vault_id)
                .map(|vault| vault.borrowed_icusd_amount.to_u64())
        });
        if remaining_debt.is_some_and(|amount| amount > crate::state::DUST_DEBT_THRESHOLD) {
            row.phase = crate::RepaymentV2Phase::CloseNeedsAdditionalRepayment;
            row.last_error = Some(
                "repayment receipt is committed, but new accrued debt remains; submit a fresh repayment request before closing".into(),
            );
            mutate_state(|s| {
                crate::state::save_repayment_v2(s, row.clone())?;
                crate::state::finish_repayment_v2(s, caller, request_id)
            })
            .map_err(ProtocolError::GenericError)?;
            return Ok(row.status_view());
        }
        let repay_block_index = row.repay_block_index.unwrap_or_default();
        match withdraw_and_close_vault_internal(
            caller,
            row.vault_id,
            Some(repay_block_index),
            Some(request_id),
        )
        .await
        {
            Ok(collateral_index) => {
                if let Some(completed) = read_state(|s| {
                    s.repayment_v2_latest_result
                        .get(&caller)
                        .filter(|saved| saved.request_id == request_id)
                        .cloned()
                }) {
                    row = completed;
                } else {
                    row.collateral_return_block_index = collateral_index;
                    row.phase = crate::RepaymentV2Phase::Complete;
                    row.last_error = None;
                    mutate_state(|s| {
                        crate::state::save_repayment_v2(s, row.clone())?;
                        crate::state::finish_repayment_v2(s, caller, request_id)
                    })
                    .map_err(ProtocolError::GenericError)?;
                }
            }
            Err(error) => {
                row.last_error = Some(format!(
                    "repay committed; close remains recoverable: {error:?}"
                ));
                mutate_state(|s| crate::state::save_repayment_v2(s, row.clone()))
                    .map_err(ProtocolError::GenericError)?;
                return Ok(row.status_view());
            }
        }
    } else if row.phase == crate::RepaymentV2Phase::RepayCommitted {
        row.phase = crate::RepaymentV2Phase::Complete;
        mutate_state(|s| {
            crate::state::save_repayment_v2(s, row.clone())?;
            crate::state::finish_repayment_v2(s, caller, request_id)
        })
        .map_err(ProtocolError::GenericError)?;
    }

    Ok(row.status_view())
}

pub async fn attach_repayment_v2_candidate(
    request_id: u128,
    block_index: u64,
) -> Result<crate::RepaymentV2StatusView, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let snapshot = read_state(|s| s.repayment_v2_active.get(&caller).cloned())
        .filter(|row| row.request_id == request_id)
        .ok_or_else(|| ProtocolError::GenericError("active repayment request not found".into()))?;
    let _vault_guard = VaultLiquidationGuard::new(snapshot.vault_id)?;
    let row = mutate_state(|s| {
        crate::state::reserve_repayment_v2_candidate_attempt(
            s,
            caller,
            request_id,
            ic_cdk::api::time(),
        )
    })
    .map_err(ProtocolError::GenericError)?;
    crate::icrc3_proof::verify_sp_liquidation_icusd_burn_block(
        &repayment_v2_tuple_for_proof(&row.tuple),
        block_index,
    )
    .await
    .map_err(|message| {
        ProtocolError::GenericError(format!(
            "repayment candidate is not an exact receipt: {message}"
        ))
    })?;
    let updated = mutate_state(|s| {
        let current = s
            .repayment_v2_active
            .get_mut(&caller)
            .filter(|current| {
                current.request_id == request_id
                    && current.tuple == row.tuple
                    && current.repay_block_index.is_none()
                    && current.candidate_block_index.is_none()
            })
            .ok_or_else(|| "repayment row changed during candidate proof".to_string())?;
        current.candidate_block_index = Some(block_index);
        current.last_error = None;
        let result = current.clone();
        crate::storage::save_state_to_stable(s);
        Ok::<_, String>(result)
    })
    .map_err(ProtocolError::GenericError)?;
    Ok(updated.status_view())
}

pub async fn repay_to_vault(arg: VaultArg) -> Result<u64, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let guard_principal = GuardPrincipal::new(caller, &format!("repay_vault_{}", arg.vault_id))?;
    // AR-B-003: per-vault op lock; see guard.rs::VaultLiquidationGuard.
    let _vault_op_guard = match VaultLiquidationGuard::new(arg.vault_id) {
        Ok(g) => g,
        Err(e) => {
            guard_principal.fail();
            return Err(e);
        }
    };

    match repay_to_vault_internal(caller, arg, false).await {
        Ok(block_index) => {
            guard_principal.complete();
            Ok(block_index)
        }
        Err(e) => {
            guard_principal.fail();
            Err(e)
        }
    }
}

/// Repay vault debt using ckUSDT or ckUSDC (1:1 with icUSD, plus configurable fee)
fn stable_repayment_v2_matches(
    row: &crate::state::StableRepaymentV2Journal,
    owner: Principal,
    request_id: u128,
    arg: &VaultArgWithToken,
) -> bool {
    row.owner == owner
        && row.request_id == request_id
        && row.vault_id == arg.vault_id
        && row.requested_amount_e8 == arg.amount
        && row.token_type == arg.token_type
}

async fn settle_stable_repayment_v2(
    mut row: crate::state::StableRepaymentV2Journal,
) -> Result<crate::StableRepaymentV2StatusView, ProtocolError> {
    let owner = row.owner;
    let request_id = row.request_id;
    if matches!(
        row.phase,
        crate::StableRepaymentV2Phase::Complete | crate::StableRepaymentV2Phase::Rejected
    ) {
        return Ok(row.status_view());
    }

    if row.candidate_block_index.is_none() {
        // Attempt count is persisted before the await. On a callback trap or
        // GenericError, retries use the exact same source/amount/fee/memo/time.
        row.dispatch_attempts = row.dispatch_attempts.saturating_add(1);
        let attempt = row.dispatch_attempts;
        let prior_possible_effect = row.had_ambiguous_attempt || attempt > 1;
        row.last_error = None;
        mutate_state(|s| crate::state::save_stable_repayment_v2(s, row.clone()))
            .map_err(ProtocolError::GenericError)?;

        match management::transfer_from_with_exact_tuple_outcome(&row.tuple).await {
            management::ExactTransferFromOutcome::Applied(block_index) => {
                row.candidate_block_index = Some(block_index);
                mutate_state(|s| crate::state::save_stable_repayment_v2(s, row.clone()))
                    .map_err(ProtocolError::GenericError)?;
            }
            management::ExactTransferFromOutcome::ProvenNoEffect(error)
                if !prior_possible_effect && attempt == 1 =>
            {
                row.phase = crate::StableRepaymentV2Phase::Rejected;
                row.last_error = Some(format!("stable transfer_from had no effect: {error:?}"));
                mutate_state(|s| {
                    crate::state::save_stable_repayment_v2(s, row.clone())?;
                    crate::state::finish_stable_repayment_v2(s, owner, request_id)
                })
                .map_err(ProtocolError::GenericError)?;
                return Ok(row.status_view());
            }
            outcome => {
                let (message, ambiguous) = match outcome {
                    management::ExactTransferFromOutcome::ProvenNoEffect(error) =>
                        (format!("stable transfer_from no-effect after prior possible dispatch: {error:?}"), true),
                    management::ExactTransferFromOutcome::AmbiguousLedgerError(error) =>
                        (format!("stable transfer_from outcome is ambiguous: {error:?}"), true),
                    management::ExactTransferFromOutcome::CallRejected { code, message } =>
                        (format!("stable transfer_from call rejected ({code}): {message}"), true),
                    management::ExactTransferFromOutcome::InvalidBlockIndex =>
                        ("stable transfer_from returned an invalid block index".into(), true),
                    management::ExactTransferFromOutcome::Applied(_) => unreachable!(),
                };
                row.phase = crate::StableRepaymentV2Phase::HeldPull;
                row.had_ambiguous_attempt |= ambiguous;
                row.last_error = Some(message);
                mutate_state(|s| crate::state::save_stable_repayment_v2(s, row.clone()))
                    .map_err(ProtocolError::GenericError)?;
                return Ok(row.status_view());
            }
        }
    }

    if row.result.is_none() {
        let block_index = row.candidate_block_index.ok_or_else(|| {
            ProtocolError::TemporarilyUnavailable(
                "stable repayment has no verified receipt candidate".into(),
            )
        })?;
        if let Err(error) =
            crate::icrc3_proof::verify_icrc3_transfer_from_block(&row.tuple, block_index).await
        {
            row.phase = crate::StableRepaymentV2Phase::HeldPull;
            row.had_ambiguous_attempt = true;
            row.last_error = Some(format!("stable repayment receipt proof failed: {error}"));
            mutate_state(|s| crate::state::save_stable_repayment_v2(s, row.clone()))
                .map_err(ProtocolError::GenericError)?;
            return Ok(row.status_view());
        }

        let commit = mutate_state(|s| {
            let saved = s
                .stable_repayment_v2_active
                .get(&owner)
                .filter(|saved| {
                    saved.request_id == request_id
                        && saved.tuple == row.tuple
                        && saved.candidate_block_index == Some(block_index)
                        && saved.result.is_none()
                })
                .ok_or_else(|| {
                    "stable repayment row changed before receipt-backed debt commit".to_string()
                })?;
            let routing_plan = saved.interest_routing_plan.clone();
            let vault = s
                .vault_id_to_vaults
                .get(&row.vault_id)
                .ok_or_else(|| "vault disappeared before stable repayment commit".to_string())?;
            if vault.owner != owner
                || vault.collateral_type != row.collateral_type
                || vault.borrowed_icusd_amount.0 < row.effective_debt_reduction_e8
                || vault.bot_processing
                || s.pending_collateral_withdrawals.contains_key(&row.vault_id)
                || s.pending_borrow_mints
                    .values()
                    .any(|pending| pending.vault_id == row.vault_id)
            {
                return Err("vault context changed; stable pull remains held for receipt-backed reconciliation".into());
            }
            let current_debt = vault.borrowed_icusd_amount.0;
            let effective = row.effective_debt_reduction_e8.min(current_debt);
            let preview_interest = if vault.accrued_interest.0 > 0 && current_debt > 0 {
                ((u128::from(effective) * u128::from(vault.accrued_interest.0))
                    / u128::from(current_debt))
                .min(u128::from(u64::MAX)) as u64
            } else {
                0
            }
            .min(vault.accrued_interest.0)
            .min(effective);
            let routing_plan = routing_plan.as_ref().ok_or_else(|| {
                "stable repayment has no pinned interest routing plan".to_string()
            })?;
            // This helper validates the entire pinned split before writing any
            // outbox. An unavailable required destination leaves the proven
            // pull held and no debt/event mutation is applied.
            crate::treasury::pin_stablecoin_interest_distribution_in_state(
                s,
                preview_interest,
                row.collateral_type,
                row.token_type.clone(),
                row.ledger,
                routing_plan,
            )?;
            // Stable ICRC ledgers have their own block namespace. Keep this
            // receipt in the V2 journal/result and update debt directly; the
            // legacy public `Event` enum intentionally cannot encode it
            // without breaking frozen Candid clients or mislabeling its block.
            let (interest, _) =
                s.repay_to_vault(row.vault_id, ICUSD::from(row.effective_debt_reduction_e8));
            let surcharge_payment_id = if row.surcharge_e6 > 0 {
                let treasury = s.treasury_principal;
                let asset = match row.token_type {
                    crate::StableTokenType::CKUSDT => crate::treasury::AssetType::CKUSDT,
                    crate::StableTokenType::CKUSDC => crate::treasury::AssetType::CKUSDC,
                };
                Some(crate::treasury::queue_stablecoin_surcharge_in_state(
                    s,
                    row.ledger,
                    treasury,
                    row.surcharge_e6,
                    asset,
                ))
            } else {
                None
            };
            let result = crate::StableRepaymentV2Result {
                ledger: row.ledger,
                block_index,
                effective_debt_reduction_e8: row.effective_debt_reduction_e8,
                interest_share_e8: interest.to_u64(),
                surcharge_payment_id,
            };
            // The active row was validated above and no await occurs in this
            // mutation. Update and compact it without a fallible helper after
            // events/outboxes/debt have changed.
            let saved = s.stable_repayment_v2_active.get_mut(&owner).expect(
                "validated stable repayment row cannot disappear during synchronous commit",
            );
            saved.result = Some(result);
            saved.phase = crate::StableRepaymentV2Phase::Complete;
            saved.last_error = None;
            let completed = s
                .stable_repayment_v2_active
                .remove(&owner)
                .expect("completed stable repayment row remains present");
            s.stable_repayment_v2_latest_result.insert(owner, completed);
            crate::storage::save_state_to_stable(s);
            Ok::<_, String>(())
        });
        if let Err(message) = commit {
            row.phase = crate::StableRepaymentV2Phase::HeldPull;
            row.had_ambiguous_attempt = true;
            row.last_error = Some(message);
            mutate_state(|s| crate::state::save_stable_repayment_v2(s, row.clone()))
                .map_err(ProtocolError::GenericError)?;
            return Ok(row.status_view());
        }
        return read_state(|s| {
            s.stable_repayment_v2_latest_result
                .get(&owner)
                .map(|r| r.status_view())
        })
        .ok_or_else(|| {
            ProtocolError::TemporarilyUnavailable(
                "stable repayment committed but terminal result is not readable".into(),
            )
        });
    }
    Ok(row.status_view())
}

pub async fn repay_to_vault_with_stable_v2(
    request_id: u128,
    arg: VaultArgWithToken,
) -> Result<crate::StableRepaymentV2StatusView, ProtocolError> {
    let owner = ic_cdk::api::caller();
    if request_id == 0 || arg.amount == 0 {
        return Err(ProtocolError::GenericError(
            "stable repayment request ID and amount must be nonzero".into(),
        ));
    }
    let saved = read_state(|s| {
        s.stable_repayment_v2_active
            .get(&owner)
            .cloned()
            .or_else(|| s.stable_repayment_v2_latest_result.get(&owner).cloned())
    });
    if let Some(row) = saved {
        if row.request_id == request_id {
            if !stable_repayment_v2_matches(&row, owner, request_id, &arg) {
                return Err(ProtocolError::GenericError(
                    "stable repayment request ID is bound to a different payload".into(),
                ));
            }
            if matches!(
                row.phase,
                crate::StableRepaymentV2Phase::Complete | crate::StableRepaymentV2Phase::Rejected
            ) {
                return Ok(row.status_view());
            }
        } else if read_state(|s| s.stable_repayment_v2_active.contains_key(&owner)) {
            return Err(ProtocolError::AlreadyProcessing);
        } else if request_id <= row.request_id {
            return Err(ProtocolError::GenericError(
                "stable repayment request ID is older than the retained result".into(),
            ));
        }
    }

    let _vault_guard = VaultLiquidationGuard::new(arg.vault_id)?;
    let row = match read_state(|s| s.stable_repayment_v2_active.get(&owner).cloned()) {
        Some(row) => {
            if !stable_repayment_v2_matches(&row, owner, request_id, &arg) {
                return Err(ProtocolError::GenericError(
                    "stable repayment request is unresolved or has a different payload".into(),
                ));
            }
            row
        }
        None => {
            let next_id = read_state(|s| {
                s.stable_repayment_v2_high_water
                    .get(&owner)
                    .copied()
                    .unwrap_or(0)
                    .checked_add(1)
            })
            .ok_or_else(|| {
                ProtocolError::GenericError("stable repayment request ID sequence exhausted".into())
            })?;
            if request_id != next_id {
                return Err(ProtocolError::GenericError(format!(
                    "stable repayment request ID must be the next sequence value ({next_id})"
                )));
            }
            let now = ic_cdk::api::time();
            reject_active_xrp_sp_absorb_preflight(arg.vault_id, now)?;
            crate::xrc::ensure_stable_not_depegged(&arg.token_type).await?;
            let ledger = read_state(|s| match arg.token_type {
                crate::StableTokenType::CKUSDT => s.ckusdt_ledger_principal,
                crate::StableTokenType::CKUSDC => s.ckusdc_ledger_principal,
            })
            .ok_or_else(|| {
                ProtocolError::GenericError("selected stable ledger is not configured".into())
            })?;
            let enabled = read_state(|s| match arg.token_type {
                crate::StableTokenType::CKUSDT => s.ckusdt_enabled,
                crate::StableTokenType::CKUSDC => s.ckusdc_enabled,
            });
            if !enabled {
                return Err(ProtocolError::GenericError(format!(
                    "{:?} repayments are currently disabled",
                    arg.token_type
                )));
            }
            mutate_state(|s| s.accrue_single_vault(arg.vault_id, now));
            let vault = read_state(|s| s.vault_id_to_vaults.get(&arg.vault_id).cloned())
                .ok_or_else(|| ProtocolError::GenericError("Vault not found".into()))?;
            require_vault_not_processing(&vault)?;
            if vault.owner != owner {
                return Err(ProtocolError::CallerNotOwner);
            }
            if let Some(status) = read_state(|s| s.get_collateral_status(&vault.collateral_type)) {
                if !status.allows_repay() {
                    return Err(ProtocolError::GenericError(
                        "Repayment is not allowed for this collateral type.".into(),
                    ));
                }
            }
            let amount_e8 = arg.amount - arg.amount % 100;
            let requested = ICUSD::from(amount_e8);
            if requested < read_state(|s| s.min_icusd_amount) {
                return Err(ProtocolError::AmountTooLow {
                    minimum_amount: read_state(|s| s.min_icusd_amount).to_u64(),
                });
            }
            let debt = vault.borrowed_icusd_amount;
            let dust = std::cmp::max(debt.0 / 100, 1_000_000);
            let effective = if requested > debt {
                debt
            } else if debt.0.saturating_sub(requested.0) <= dust {
                debt
            } else {
                requested
            };
            check_min_vault_debt_after_repay(&vault, effective)?;
            let fee_rate = read_state(|s| s.ckstable_repay_fee);
            let (principal_pull_e6, surcharge_e6, total_pull_e6) =
                stable_repay_pull_e6s(effective, fee_rate)?;
            let ledger_fee = management::get_ledger_fee(ledger)
                .await
                .map_err(ProtocolError::GenericError)?;
            let current = read_state(|s| s.vault_id_to_vaults.get(&arg.vault_id).cloned())
                .ok_or_else(|| {
                    ProtocolError::TemporarilyUnavailable(
                        "vault disappeared during stable repayment preflight".into(),
                    )
                })?;
            let configured_ledger = read_state(|s| match arg.token_type {
                crate::StableTokenType::CKUSDT => s.ckusdt_ledger_principal,
                crate::StableTokenType::CKUSDC => s.ckusdc_ledger_principal,
            });
            let current_minimum = read_state(|s| s.min_icusd_amount);
            if current.owner != owner
                || current.collateral_type != vault.collateral_type
                || configured_ledger != Some(ledger)
                || read_state(|s| match arg.token_type {
                    crate::StableTokenType::CKUSDT => !s.ckusdt_enabled,
                    crate::StableTokenType::CKUSDC => !s.ckusdc_enabled,
                })
                || read_state(|s| s.ckstable_repay_fee != fee_rate)
                || requested < current_minimum
                || current.borrowed_icusd_amount.0 < effective.to_u64()
                || read_state(|s| {
                    s.get_collateral_status(&current.collateral_type)
                        .is_some_and(|status| !status.allows_repay())
                })
            {
                return Err(ProtocolError::TemporarilyUnavailable(
                    "stable repayment plan changed during preflight".into(),
                ));
            }
            require_vault_not_processing(&current)?;
            check_min_vault_debt_after_repay(&current, effective)?;
            let routing_plan = read_state(|s| crate::state::StableRepaymentV2InterestRoutingPlan {
                split: s.interest_split.clone(),
                stable_treasury: s.treasury_principal,
                icusd_ledger: s.icusd_ledger_principal,
                stability_pool: s.stability_pool_canister,
                three_pool: s.three_pool_canister,
                amm1: s.amm1_canister,
                amm1_pool_id: s.amm1_pool_id.clone(),
            });
            let split_bps = routing_plan
                .split
                .iter()
                .try_fold(0u64, |sum, recipient| sum.checked_add(recipient.bps));
            if split_bps != Some(10_000) {
                return Err(ProtocolError::TemporarilyUnavailable(
                    "stable repayment is paused until the interest split is valid".into(),
                ));
            }
            let requires_icusd_route = routing_plan.split.iter().any(|recipient| {
                recipient.bps > 0
                    && matches!(
                        recipient.destination,
                        crate::state::InterestDestination::StabilityPool
                            | crate::state::InterestDestination::ThreePool
                            | crate::state::InterestDestination::Amm1
                    )
            });
            if requires_icusd_route && routing_plan.icusd_ledger == Principal::anonymous() {
                return Err(ProtocolError::TemporarilyUnavailable(
                    "stable repayment is paused until the icUSD ledger is configured for pinned interest delivery".into(),
                ));
            }
            if routing_plan.split.iter().any(|recipient| {
                recipient.bps > 0
                    && recipient.destination == crate::state::InterestDestination::StabilityPool
            }) && routing_plan.stability_pool.is_none()
            {
                return Err(ProtocolError::TemporarilyUnavailable(
                    "stable repayment is paused until the Stability Pool interest route is configured".into(),
                ));
            }
            let journal = mutate_state(|s| {
                let op_nonce = s.next_op_nonce();
                let tuple = crate::SpLiquidationStablePullTuple {
                    op_nonce,
                    ledger,
                    from: Account {
                        owner,
                        subaccount: None,
                    },
                    spender: Account {
                        owner: ic_cdk::id(),
                        subaccount: None,
                    },
                    to: Account {
                        owner: ic_cdk::id(),
                        subaccount: None,
                    },
                    amount_raw: total_pull_e6,
                    fee_raw: ledger_fee,
                    memo: management::nonce_to_memo(op_nonce).0.to_vec(),
                    created_at_time_ns: management::nonce_to_created_at_time(op_nonce),
                };
                let journal = crate::state::StableRepaymentV2Journal {
                    owner,
                    request_id,
                    vault_id: arg.vault_id,
                    token_type: arg.token_type.clone(),
                    requested_amount_e8: arg.amount,
                    effective_debt_reduction_e8: effective.to_u64(),
                    principal_pull_e6,
                    surcharge_e6,
                    pinned_debt_e8: current.borrowed_icusd_amount.to_u64(),
                    collateral_type: current.collateral_type,
                    ledger,
                    interest_routing_plan: Some(routing_plan.clone()),
                    tuple: tuple.clone(),
                    phase: crate::StableRepaymentV2Phase::PendingPull,
                    candidate_block_index: None,
                    candidate_attach_window_start_ns: 0,
                    candidate_attach_attempts: 0,
                    had_ambiguous_attempt: false,
                    dispatch_attempts: 0,
                    result: None,
                    last_error: None,
                };
                journal
            });
            mutate_state(|s| {
                crate::state::admit_stable_repayment_v2(s, journal.clone(), ic_cdk::id())
            })
            .map_err(ProtocolError::GenericError)?;
            journal
        }
    };
    // From here on do not query live depeg/configuration state: a previous
    // dispatch may already have committed and exact receipt recovery wins.
    settle_stable_repayment_v2(row).await
}

pub async fn attach_stable_repayment_v2_candidate(
    request_id: u128,
    block_index: u64,
) -> Result<crate::StableRepaymentV2StatusView, ProtocolError> {
    let owner = ic_cdk::api::caller();
    let initial = read_state(|s| s.stable_repayment_v2_active.get(&owner).cloned())
        .filter(|row| row.request_id == request_id)
        .ok_or_else(|| {
            ProtocolError::GenericError("active stable repayment request not found".into())
        })?;
    let _guard = VaultLiquidationGuard::new(initial.vault_id)?;
    let row = mutate_state(|s| {
        crate::state::reserve_stable_repayment_candidate_attempt(
            s,
            owner,
            request_id,
            ic_cdk::api::time(),
        )
    })
    .map_err(ProtocolError::GenericError)?;
    crate::icrc3_proof::verify_icrc3_transfer_from_block(&row.tuple, block_index)
        .await
        .map_err(|error| {
            ProtocolError::GenericError(format!(
                "candidate block does not prove stable repayment tuple: {error}"
            ))
        })?;
    let mut attached = row;
    attached.candidate_block_index = Some(block_index);
    attached.last_error = None;
    mutate_state(|s| {
        let active = s
            .stable_repayment_v2_active
            .get(&owner)
            .filter(|saved| {
                saved.request_id == request_id
                    && saved.tuple == attached.tuple
                    && saved.had_ambiguous_attempt
                    && saved.candidate_block_index.is_none()
                    && saved.result.is_none()
            })
            .ok_or_else(|| {
                "stable repayment row changed during candidate verification".to_string()
            })?;
        let _ = active;
        crate::state::save_stable_repayment_v2(s, attached.clone())
    })
    .map_err(ProtocolError::GenericError)?;
    settle_stable_repayment_v2(attached).await
}

pub async fn repay_to_vault_with_stable(arg: VaultArgWithToken) -> Result<u64, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let guard_principal =
        GuardPrincipal::new(caller, &format!("repay_vault_stable_{}", arg.vault_id))?;
    // AR-B-003: per-vault op lock; see guard.rs::VaultLiquidationGuard.
    let _vault_op_guard = match VaultLiquidationGuard::new(arg.vault_id) {
        Ok(g) => g,
        Err(e) => {
            guard_principal.fail();
            return Err(e);
        }
    };

    // Check if the selected stable token is enabled
    let is_enabled = read_state(|s| match arg.token_type {
        StableTokenType::CKUSDT => s.ckusdt_enabled,
        StableTokenType::CKUSDC => s.ckusdc_enabled,
    });
    if !is_enabled {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(format!(
            "{:?} repayments are currently disabled",
            arg.token_type
        )));
    }

    let now = ic_cdk::api::time();
    if let Err(e) = reject_active_xrp_sp_absorb_preflight(arg.vault_id, now) {
        guard_principal.fail();
        return Err(e);
    }

    // Depeg protection: fetch fresh stablecoin price and reject if outside $0.95–$1.05
    if let Err(e) = crate::xrc::ensure_stable_not_depegged(&arg.token_type).await {
        guard_principal.fail();
        return Err(e);
    }

    // Truncate to nearest 100 e8s for clean 8→6 decimal conversion
    let raw_amount_e8s = arg.amount - (arg.amount % 100);
    let amount: ICUSD = raw_amount_e8s.into();

    // Accrue interest before repayment so the correct debt balance is used.
    mutate_state(|s| s.accrue_single_vault(arg.vault_id, now));

    let vault = match read_state(|s| s.vault_id_to_vaults.get(&arg.vault_id).cloned()) {
        Some(v) => v,
        None => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError("Vault not found".to_string()));
        }
    };

    if caller != vault.owner {
        guard_principal.fail();
        return Err(ProtocolError::CallerNotOwner);
    }

    if let Err(e) = require_vault_not_processing(&vault) {
        guard_principal.fail();
        return Err(e);
    }

    // Check collateral status allows repayment
    let collateral_status = read_state(|s| s.get_collateral_status(&vault.collateral_type));
    if let Some(status) = collateral_status {
        if !status.allows_repay() {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(
                "Repayment is not allowed for this collateral type.".to_string(),
            ));
        }
    }

    if caller != vault.owner {
        guard_principal.fail();
        return Err(ProtocolError::CallerNotOwner);
    }

    if amount < read_state(|s| s.min_icusd_amount) {
        guard_principal.fail();
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: read_state(|s| s.min_icusd_amount).to_u64(),
        });
    }

    // Cap repay amount to actual debt. Interest accrued between when the
    // frontend read the balance and now can push debt slightly above what
    // the user entered. If the requested amount exceeds or nearly matches
    // the current debt (within 1% or 0.01 icUSD — whichever is larger),
    // treat it as a full repayment to avoid leaving un-repayable dust.
    let debt = vault.borrowed_icusd_amount;
    let dust_threshold = std::cmp::max(debt.0 / 100, 1_000_000); // 1% or 0.01 icUSD
    let amount = if amount > debt {
        debt
    } else if debt.0.saturating_sub(amount.0) <= dust_threshold {
        debt
    } else {
        amount
    };

    if let Err(e) = check_min_vault_debt_after_repay(&vault, amount) {
        guard_principal.fail();
        return Err(e);
    }

    // Derive the stable pull from finalized retired debt, including any
    // near-full-debt snap. Ceiling the conversion prevents retiring more
    // icUSD debt than the stable principal can cover.
    let fee_rate = read_state(|s| s.ckstable_repay_fee);
    let (_base_stable_e6s, fee_e6s, total_pull_e6s) = match stable_repay_pull_e6s(amount, fee_rate)
    {
        Ok(pull) => pull,
        Err(error) => {
            guard_principal.fail();
            return Err(error);
        }
    };

    // Transfer the stable token from user (in 6-decimal units)
    match transfer_stable_from(arg.token_type.clone(), total_pull_e6s, caller).await {
        Ok(block_index) => {
            let interest_share =
                mutate_state(|s| record_repayed_to_vault(s, arg.vault_id, amount, block_index));
            // The debt has already changed. Persist the exact fee obligation
            // before the interest distribution's first await can interrupt us.
            let fee_payment_id = (fee_e6s > 0).then(|| {
                crate::treasury::queue_stablecoin_surcharge_obligation(
                    fee_e6s,
                    arg.token_type.clone(),
                    crate::state::TreasuryPaymentKind::StablecoinRepaySurcharge,
                )
            });

            // Route interest via N-way split (stablecoin-denominated)
            if interest_share.to_u64() > 0 {
                crate::treasury::distribute_stablecoin_interest(
                    interest_share.to_u64(),
                    vault.collateral_type,
                    arg.token_type.clone(),
                )
                .await;
            }

            if let Some(operation_id) = fee_payment_id {
                crate::treasury::dispatch_pending_treasury_payment(operation_id).await;
            }

            guard_principal.complete();
            Ok(block_index)
        }
        Err(transfer_from_error) => {
            guard_principal.fail();
            Err(ProtocolError::TransferFromError(
                transfer_from_error,
                total_pull_e6s,
            ))
        }
    }
}

pub async fn add_margin_with_request_id(
    request_id: u128,
    arg: VaultArg,
) -> Result<u64, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let requested_ledger = read_state(|s| {
        let vault = s.vault_id_to_vaults.get(&arg.vault_id)?;
        Some(
            s.get_collateral_config(&vault.collateral_type)?
                .ledger_canister_id,
        )
    })
    .ok_or_else(|| ProtocolError::GenericError("Vault or collateral type not found".into()))?;
    if let Some(row) = read_state(|s| {
        crate::state::completed_inbound_collateral(s, caller, requested_ledger, request_id)
    }) {
        if row.amount_raw != arg.amount
            || !matches!(&row.operation, crate::state::InboundCollateralOperation::AddMargin { vault_id, .. } if *vault_id == arg.vault_id)
        {
            return Err(ProtocolError::GenericError(
                "request ID was already used for a different collateral intent".into(),
            ));
        }
        return match row.result {
            crate::state::InboundCollateralResult::AddMargin { block_index } => Ok(block_index),
            crate::state::InboundCollateralResult::Rejected { message } => {
                Err(ProtocolError::TemporarilyUnavailable(message))
            }
            crate::state::InboundCollateralResult::Open { .. } => Err(ProtocolError::GenericError(
                "request ID operation kind mismatch".into(),
            )),
        };
    }
    let guard_principal =
        GuardPrincipal::new(caller, &format!("add_margin_vault_{}", arg.vault_id))?;
    let amount: ICP = arg.amount.into();

    let now = ic_cdk::api::time();
    if let Err(e) = reject_active_xrp_sp_absorb_preflight(arg.vault_id, now) {
        guard_principal.fail();
        return Err(e);
    }

    let (vault, config_ledger, min_deposit, is_native_xrp) =
        match read_state(|s| match s.vault_id_to_vaults.get(&arg.vault_id) {
            Some(v) => {
                let config = s
                    .get_collateral_config(&v.collateral_type)
                    .ok_or("Collateral type not configured")?;
                Ok((
                    v.clone(),
                    config.ledger_canister_id,
                    config.min_collateral_deposit,
                    config.is_native_xrp(),
                ))
            }
            None => Err("Vault not found"),
        }) {
            Ok(result) => result,
            Err(msg) => {
                guard_principal.fail();
                return Err(ProtocolError::GenericError(msg.to_string()));
            }
        };

    // Reacquire the durable ingress fence for this owner's pending add-margin
    // tuple; every other vault mutator is rejected while that row exists.
    let _vault_op_guard = match VaultLiquidationGuard::new_for_inbound_collateral(
        arg.vault_id,
        caller,
        config_ledger,
    ) {
        Ok(g) => g,
        Err(e) => {
            guard_principal.fail();
            return Err(e);
        }
    };

    // P2: native-XRP collateral is not custodied via ICRC; its add-collateral flow
    // is wired with the XRP deposit path (P3). Reject so XRP collateral can never be
    // pulled as an ICRC token. (Latent until P5 enables XRP registration.)
    if is_native_xrp {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "Native-XRP collateral uses the XRP deposit flow (not yet enabled).".to_string(),
        ));
    }

    if caller != vault.owner {
        guard_principal.fail();
        return Err(ProtocolError::CallerNotOwner);
    }

    if let Err(e) = require_vault_not_processing(&vault) {
        guard_principal.fail();
        return Err(e);
    }

    if min_deposit > 0 && amount < ICP::new(min_deposit) {
        guard_principal.fail();
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: min_deposit,
        });
    }

    // Check collateral status allows adding collateral
    let collateral_status = read_state(|s| s.get_collateral_status(&vault.collateral_type));
    if let Some(status) = collateral_status {
        if !status.allows_add_collateral() {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(
                "Adding collateral is not allowed for this collateral type.".to_string(),
            ));
        }
    }

    if caller != vault.owner {
        guard_principal.fail();
        return Err(ProtocolError::CallerNotOwner);
    }

    if checked_margin_balance(vault.collateral_amount, amount.to_u64()).is_none() {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "added collateral would exceed the vault's representable u64 balance".into(),
        ));
    }

    match settle_inbound_collateral(
        caller,
        config_ledger,
        request_id,
        arg.amount,
        crate::state::InboundCollateralOperation::AddMargin {
            vault_id: arg.vault_id,
            vault_snapshot: vault,
        },
    )
    .await
    {
        Ok(InboundCollateralApplied::Margin { block_index }) => {
            guard_principal.complete();
            Ok(block_index)
        }
        Ok(InboundCollateralApplied::Open { .. }) => {
            guard_principal.fail();
            Err(ProtocolError::GenericError(
                "inbound operation kind mismatch".into(),
            ))
        }
        Err(error) => {
            guard_principal.fail();
            Err(error)
        }
    }
}

/// Legacy no-ID add-margin cannot safely classify response-loss retries.
pub async fn add_margin_to_vault(_arg: VaultArg) -> Result<u64, ProtocolError> {
    Err(ProtocolError::TemporarilyUnavailable(
        "use add_margin_v2 with a stable request ID".into(),
    ))
}

// ─── Push-deposit vault operations (Oisy wallet integration) ───
//
// These mirror open_vault / add_margin_to_vault but instead of pulling funds
// via ICRC-2 transfer_from, they sweep funds that the user already pushed to
// a deterministic deposit subaccount. This avoids sequential signer popups
// that Oisy's ICRC-21/25 consent flow may trigger (whether ICRC-2 approve
// actually works through Oisy is unconfirmed).

fn push_deposit_amount_minimum_error(amount: u64, min_deposit: u64) -> Option<ProtocolError> {
    if min_deposit > 0 && amount < min_deposit {
        Some(ProtocolError::AmountTooLow {
            minimum_amount: min_deposit,
        })
    } else {
        None
    }
}

fn push_deposit_balance_minimum_error(
    balance: u64,
    ledger_fee: u64,
    min_deposit: u64,
) -> Option<ProtocolError> {
    // Leave empty and fee-only balances to the sweep helper, which owns those
    // existing diagnostics. Otherwise compare the exact amount it would sweep.
    if balance > ledger_fee {
        push_deposit_amount_minimum_error(balance - ledger_fee, min_deposit)
    } else {
        None
    }
}

const CANONICAL_THREE_USD_LP_LEDGER: &str = "fohh4-yyaaa-aaaap-qtkpa-cai";

fn is_three_usd_lp_ledger(ledger: Principal, configured_three_pool: Option<Principal>) -> bool {
    let canonical =
        Principal::from_text(CANONICAL_THREE_USD_LP_LEDGER).expect("canonical 3pool principal");
    ledger == canonical || configured_three_pool == Some(ledger)
}

fn reject_three_usd_lp_push_deposit(
    ledger: Principal,
    configured_three_pool: Option<Principal>,
) -> Result<(), ProtocolError> {
    if is_three_usd_lp_ledger(ledger, configured_three_pool) {
        return Err(ProtocolError::GenericError(
            "3USD LP collateral cannot use push-deposit: its ledger keys balances by principal and ignores subaccounts.".to_string(),
        ));
    }
    Ok(())
}

/// Serialize sweeps from one caller's deterministic deposit subaccount across
/// awaits. `GuardPrincipal` has a stale-operation recovery window, so it alone
/// does not prove that an earlier future cannot still sweep after a later
/// operation's balance preflight.
thread_local! {
    static PUSH_DEPOSIT_SWEEP_LOCKS: RefCell<std::collections::HashSet<Principal>> =
        RefCell::new(std::collections::HashSet::new());
}

struct PushDepositSweepGuard(Principal);

impl PushDepositSweepGuard {
    fn new(caller: Principal) -> Result<Self, ProtocolError> {
        PUSH_DEPOSIT_SWEEP_LOCKS.with(|locks| {
            if !locks.borrow_mut().insert(caller) {
                return Err(ProtocolError::TemporarilyUnavailable(
                    "A push-deposit sweep for this caller is already in flight; retry shortly"
                        .to_string(),
                ));
            }
            Ok(Self(caller))
        })
    }
}

impl Drop for PushDepositSweepGuard {
    fn drop(&mut self) {
        PUSH_DEPOSIT_SWEEP_LOCKS.with(|locks| {
            locks.borrow_mut().remove(&self.0);
        });
    }
}

async fn check_push_deposit_minimum_before_sweep(
    caller: &Principal,
    ledger: Principal,
    ledger_fee: u64,
    min_deposit: u64,
) -> Result<(), ProtocolError> {
    if min_deposit == 0 {
        return Ok(());
    }

    let balance = management::get_balance_of(management::get_deposit_account_for(caller), ledger)
        .await
        .map_err(|error| {
            ProtocolError::GenericError(format!("Push-deposit balance check failed: {error}"))
        })?;

    if let Some(error) = push_deposit_balance_minimum_error(balance, ledger_fee, min_deposit) {
        return Err(error);
    }

    Ok(())
}

fn same_push_deposit_intent(
    saved: &crate::state::PushDepositSweepOperation,
    requested: &crate::state::PushDepositSweepOperation,
) -> bool {
    match (saved, requested) {
        (
            crate::state::PushDepositSweepOperation::Open {
                collateral_type: a,
                borrow_amount_raw: ab,
                ..
            },
            crate::state::PushDepositSweepOperation::Open {
                collateral_type: b,
                borrow_amount_raw: bb,
                ..
            },
        ) => a == b && ab == bb,
        (
            crate::state::PushDepositSweepOperation::AddMargin { vault_id: a, .. },
            crate::state::PushDepositSweepOperation::AddMargin { vault_id: b, .. },
        ) => a == b,
        _ => false,
    }
}

async fn verify_push_deposit_sweep_receipt(
    tuple: &crate::state::PushDepositSweepTuple,
    block_index: u64,
) -> Result<(), String> {
    match tuple.proof_kind.unwrap_or_else(|| read_state(|s| s.payout_proof_kind_for_ledger(tuple.ledger))) {
        crate::state::PayoutProofKind::NativeIcp => {
            match crate::treasury::verify_native_icp_direct_account_transfer_receipt(
                tuple.ledger, &tuple.from, &tuple.to, tuple.amount_raw,
                tuple.expected_fee_raw, &tuple.memo, tuple.created_at_time_ns, block_index,
            ).await {
                Ok(()) => Ok(()),
                Err(native_error) => crate::icrc3_proof::verify_icrc3_direct_transfer_block_with_fee(
                    tuple.ledger, block_index, tuple.from.clone(), tuple.to.clone(),
                    tuple.amount_raw, tuple.expected_fee_raw, Some(&tuple.memo),
                    Some(tuple.created_at_time_ns),
                ).await.map_err(|icrc3_error| format!("native ICP proof failed ({native_error}); exact ICRC-3 proof failed ({icrc3_error})")),
            }
        },
        crate::state::PayoutProofKind::Icrc3 => crate::icrc3_proof::verify_icrc3_direct_transfer_block_with_fee(
            tuple.ledger, block_index, tuple.from.clone(), tuple.to.clone(),
            tuple.amount_raw, tuple.expected_fee_raw, Some(&tuple.memo),
            Some(tuple.created_at_time_ns),
        ).await,
    }
}

/// Settle a request-ID-bound push-deposit sweep. The journal pins the whole
/// balance snapshot and exact ICRC-1 dedup tuple before the ledger call.
async fn settle_push_deposit_sweep(
    owner: Principal,
    ledger: Principal,
    ledger_fee: u64,
    min_deposit: u64,
    request_id: Option<u128>,
    operation: crate::state::PushDepositSweepOperation,
) -> Result<(crate::state::PushDepositSweepResult, bool), ProtocolError> {
    let key = (owner, ledger);
    if let Some(done) = read_state(|s| s.completed_push_deposit_sweeps.get(&key).cloned()) {
        if request_id == Some(done.request_id)
            && same_push_deposit_intent(&done.operation, &operation)
        {
            return match done.result {
                crate::state::PushDepositSweepResult::Rejected { message } => {
                    Err(ProtocolError::TemporarilyUnavailable(message))
                }
                result => Ok((result, false)),
            };
        }
    }

    let row = if let Some(row) = read_state(|s| s.pending_push_deposit_sweeps.get(&key).cloned()) {
        if request_id != Some(row.request_id)
            || !same_push_deposit_intent(&row.operation, &operation)
        {
            return Err(ProtocolError::TemporarilyUnavailable(
                "another push-deposit sweep is unresolved; recover its exact request first".into(),
            ));
        }
        row
    } else {
        let Some(request_id) = request_id else {
            return Err(ProtocolError::TemporarilyUnavailable(
                "legacy push-deposit calls cannot start a sweep; use the request-ID V2 endpoint"
                    .into(),
            ));
        };
        let high_water = read_state(|s| {
            s.push_deposit_sweep_high_water
                .get(&key)
                .copied()
                .unwrap_or(0)
        });
        let expected_id = high_water.checked_add(1).ok_or_else(|| {
            ProtocolError::GenericError("push-deposit request ID sequence exhausted".into())
        })?;
        if request_id != expected_id {
            return Err(ProtocolError::GenericError(format!(
                "push-deposit request ID must be {expected_id}"
            )));
        }
        check_push_deposit_minimum_before_sweep(&owner, ledger, ledger_fee, min_deposit).await?;
        let from = management::get_deposit_account_for(&owner);
        let balance = management::get_balance_of(from.clone(), ledger)
            .await
            .map_err(|e| {
                ProtocolError::GenericError(format!("Push-deposit balance check failed: {e}"))
            })?;
        if balance == 0 || balance <= ledger_fee {
            return Err(ProtocolError::GenericError(format!(
                "Deposit balance ({balance}) is not enough to cover ledger fee ({ledger_fee})"
            )));
        }
        let amount_raw = balance - ledger_fee;
        if min_deposit > 0 && amount_raw < min_deposit {
            return Err(ProtocolError::GenericError(format!(
                "Net push-deposit amount ({amount_raw}) is below minimum ({min_deposit})"
            )));
        }
        let op_nonce = mutate_state(|s| s.next_op_nonce());
        let tuple = crate::state::PushDepositSweepTuple {
            op_nonce,
            ledger,
            from,
            to: icrc_ledger_types::icrc1::account::Account {
                owner: ic_cdk::id(),
                subaccount: None,
            },
            amount_raw,
            fee_raw: Some(ledger_fee),
            expected_fee_raw: ledger_fee,
            proof_kind: Some(read_state(|s| s.payout_proof_kind_for_ledger(ledger))),
            memo: management::nonce_to_memo(op_nonce).0.to_vec(),
            created_at_time_ns: management::nonce_to_created_at_time(op_nonce),
        };
        let mut operation = operation;
        if let crate::state::PushDepositSweepOperation::Open {
            reserved_vault_id, ..
        } = &mut operation
        {
            *reserved_vault_id = 0;
        }
        mutate_state(|s| {
            if s.frozen {
                return Err(
                    "protocol is frozen; no new push-deposit transfer may be admitted".to_string(),
                );
            }
            let collateral_type = match &operation {
                crate::state::PushDepositSweepOperation::Open {
                    collateral_type, ..
                } => *collateral_type,
                crate::state::PushDepositSweepOperation::AddMargin { vault_snapshot, .. } => {
                    vault_snapshot.collateral_type
                }
            };
            if matches!(
                &operation,
                crate::state::PushDepositSweepOperation::Open { .. }
            ) && s.mode == crate::state::Mode::ReadOnly
            {
                return Err("protocol is read-only; no new vault may be opened".to_string());
            }
            let current_config = s.get_collateral_config(&collateral_type).ok_or_else(|| {
                "collateral configuration changed before sweep admission".to_string()
            })?;
            if current_config.ledger_canister_id != ledger
                || current_config.ledger_fee != ledger_fee
            {
                return Err("collateral ledger or fee changed before sweep admission".to_string());
            }
            if tuple.proof_kind != Some(s.payout_proof_kind_for_ledger(ledger)) {
                return Err("ledger receipt adapter changed before sweep admission".to_string());
            }
            if matches!(
                &operation,
                crate::state::PushDepositSweepOperation::Open { .. }
            ) && !current_config.status.allows_open()
            {
                return Err("collateral status no longer allows opening a vault".to_string());
            }
            if matches!(
                &operation,
                crate::state::PushDepositSweepOperation::AddMargin { .. }
            ) && s
                .get_collateral_status(&collateral_type)
                .is_some_and(|status| !status.allows_add_collateral())
            {
                return Err("collateral status no longer allows adding collateral".to_string());
            }
            if let crate::state::PushDepositSweepOperation::AddMargin { vault_id, .. } = &operation
            {
                let current = s
                    .vault_id_to_vaults
                    .get(vault_id)
                    .ok_or_else(|| "vault closed before sweep admission".to_string())?;
                if current.owner != owner || current.collateral_type != collateral_type {
                    return Err(
                        "vault owner or collateral changed before sweep admission".to_string()
                    );
                }
                if current.bot_processing
                    || s.pending_collateral_withdrawals.contains_key(vault_id)
                    || s.vault_has_pending_inbound_margin(*vault_id)
                    || s.pending_borrow_mints
                        .values()
                        .any(|row| row.vault_id == *vault_id)
                    || s.sp_liquidation_v2_journals
                        .values()
                        .any(|journal| journal.request.vault_id == *vault_id)
                    || s.repayment_v2_active
                        .values()
                        .any(|row| row.vault_id == *vault_id)
                    || s.stable_repayment_v2_active
                        .values()
                        .any(|row| row.vault_id == *vault_id)
                {
                    return Err(
                        "vault acquired an unresolved processing fence before sweep admission"
                            .to_string(),
                    );
                }
            }
            crate::state::admit_push_deposit_sweep(
                s,
                crate::state::PushDepositSweepJournal {
                    owner,
                    request_id,
                    operation,
                    tuple,
                    observed_balance_raw: balance,
                    had_ambiguous_attempt: false,
                    candidate_block_index: None,
                    last_error: None,
                },
            )
            .map(|_| ())
        })
        .map_err(ProtocolError::GenericError)?;
        read_state(|s| s.pending_push_deposit_sweeps.get(&key).cloned()).ok_or_else(|| {
            ProtocolError::GenericError("push-deposit journal failed to persist".into())
        })?
    };

    let mut candidate = row.candidate_block_index;
    if candidate.is_none() {
        let prior_ambiguity = mutate_state(|s| -> Result<bool, String> {
            if s.frozen {
                return Err(
                    "protocol is frozen; push-deposit recovery resumes after unfreeze".into(),
                );
            }
            let current = s.pending_push_deposit_sweeps.get_mut(&key).ok_or_else(|| {
                "pending push-deposit journal disappeared before dispatch".to_string()
            })?;
            if current.owner != owner
                || current.request_id != row.request_id
                || current.tuple != row.tuple
                || current.operation != row.operation
            {
                return Err("pending push-deposit journal changed before dispatch".into());
            }
            let prior_ambiguity = current.had_ambiguous_attempt;
            current.had_ambiguous_attempt = true;
            crate::storage::save_state_to_stable(s);
            Ok(prior_ambiguity)
        })
        .map_err(ProtocolError::GenericError)?;
        match management::transfer_push_deposit_with_exact_tuple(&row.tuple).await {
            management::ExactPushDepositTransferOutcome::Applied(block) => {
                candidate = Some(block);
                mutate_state(|s| {
                    if let Some(current) = s.pending_push_deposit_sweeps.get_mut(&key) {
                        current.candidate_block_index = Some(block);
                        current.last_error = None;
                        crate::storage::save_state_to_stable(s);
                    }
                });
            }
            management::ExactPushDepositTransferOutcome::ProvenNoEffect(error)
                if !prior_ambiguity =>
            {
                let message = format!("typed ICRC-1 no-effect: {error:?}");
                mutate_state(|s| {
                    s.pending_push_deposit_sweeps.remove(&key);
                    s.completed_push_deposit_sweeps.insert(
                        key,
                        crate::state::CompletedPushDepositSweep {
                            request_id: row.request_id,
                            operation: row.operation.clone(),
                            tuple: row.tuple.clone(),
                            result: crate::state::PushDepositSweepResult::Rejected { message },
                        },
                    );
                    crate::storage::save_state_to_stable(s);
                });
                return Err(ProtocolError::TransferError(error));
            }
            management::ExactPushDepositTransferOutcome::ProvenNoEffect(error) => {
                let message = format!("typed no-effect after earlier ambiguous sweep: {error:?}");
                mutate_state(|s| {
                    if let Some(current) = s.pending_push_deposit_sweeps.get_mut(&key) {
                        current.had_ambiguous_attempt = true;
                        current.last_error = Some(message.clone());
                        crate::storage::save_state_to_stable(s);
                    }
                });
                return Err(ProtocolError::TemporarilyUnavailable(message));
            }
            management::ExactPushDepositTransferOutcome::AmbiguousLedgerError(error) => {
                let message = format!("ambiguous ICRC-1 response: {error:?}");
                mutate_state(|s| {
                    if let Some(current) = s.pending_push_deposit_sweeps.get_mut(&key) {
                        current.had_ambiguous_attempt = true;
                        current.last_error = Some(message.clone());
                        crate::storage::save_state_to_stable(s);
                    }
                });
                return Err(ProtocolError::TemporarilyUnavailable(message));
            }
            management::ExactPushDepositTransferOutcome::CallRejected { code, message } => {
                let detail = format!("ICRC-1 call rejected after dispatch ({code}): {message}");
                mutate_state(|s| {
                    if let Some(current) = s.pending_push_deposit_sweeps.get_mut(&key) {
                        current.had_ambiguous_attempt = true;
                        current.last_error = Some(detail.clone());
                        crate::storage::save_state_to_stable(s);
                    }
                });
                return Err(ProtocolError::TemporarilyUnavailable(detail));
            }
            management::ExactPushDepositTransferOutcome::InvalidBlockIndex => {
                let message = "ledger returned an unrepresentable block index".to_string();
                mutate_state(|s| {
                    if let Some(current) = s.pending_push_deposit_sweeps.get_mut(&key) {
                        current.had_ambiguous_attempt = true;
                        current.last_error = Some(message.clone());
                        crate::storage::save_state_to_stable(s);
                    }
                });
                return Err(ProtocolError::TemporarilyUnavailable(message));
            }
        }
    }
    let block = candidate.ok_or_else(|| {
        ProtocolError::TemporarilyUnavailable("push-deposit candidate receipt missing".into())
    })?;
    if let Err(error) = verify_push_deposit_sweep_receipt(&row.tuple, block).await {
        let message = format!("exact push-deposit receipt proof pending: {error}");
        mutate_state(|s| {
            if let Some(current) = s.pending_push_deposit_sweeps.get_mut(&key) {
                current.candidate_block_index = Some(block);
                current.had_ambiguous_attempt = true;
                current.last_error = Some(message.clone());
                crate::storage::save_state_to_stable(s);
            }
        });
        return Err(ProtocolError::TemporarilyUnavailable(message));
    }

    mutate_state(|s| -> Result<crate::state::PushDepositSweepResult, String> {
        if s.frozen { return Err("protocol is frozen; verified push-deposit receipt remains held until unfreeze".into()); }
        let current = s.pending_push_deposit_sweeps.get(&key).ok_or_else(|| "push-deposit journal disappeared before credit".to_string())?;
        if current.request_id != row.request_id || current.owner != owner || current.tuple != row.tuple || current.operation != row.operation || current.candidate_block_index != Some(block) {
            return Err("push-deposit journal changed before credit".into());
        }
        let result = match &row.operation {
            crate::state::PushDepositSweepOperation::Open { collateral_type, reserved_vault_id, .. } => {
                if s.vault_id_to_vaults.contains_key(reserved_vault_id) { return Err("reserved push-deposit vault ID was already consumed".into()); }
                crate::event::record_open_vault(s, Vault { owner, borrowed_icusd_amount: 0.into(), collateral_amount: row.tuple.amount_raw, vault_id: *reserved_vault_id, collateral_type: *collateral_type, last_accrual_time: ic_cdk::api::time(), accrued_interest: ICUSD::new(0), bot_processing: false }, block);
                crate::state::PushDepositSweepResult::Open { vault_id: *reserved_vault_id, block_index: block }
            }
            crate::state::PushDepositSweepOperation::AddMargin { vault_id, vault_snapshot } => {
                let current_vault = s.vault_id_to_vaults.get(vault_id).ok_or_else(|| "vault closed while sweep was pending; exact receipt held".to_string())?;
                if current_vault.owner != owner || current_vault.owner != vault_snapshot.owner || current_vault.collateral_type != vault_snapshot.collateral_type { return Err("vault owner or collateral changed while sweep was pending; exact receipt held".into()); }
                if checked_margin_balance(current_vault.collateral_amount, row.tuple.amount_raw).is_none() { return Err("push-deposit margin exceeds representable vault balance; receipt held".into()); }
                crate::event::record_add_margin_to_vault_for(s, *vault_id, ICP::from(row.tuple.amount_raw), block, owner);
                crate::state::PushDepositSweepResult::AddMargin { block_index: block }
            }
        };
        s.pending_push_deposit_sweeps.remove(&key);
        s.completed_push_deposit_sweeps.insert(key, crate::state::CompletedPushDepositSweep { request_id: row.request_id, operation: row.operation.clone(), tuple: row.tuple.clone(), result: result.clone() });
        crate::storage::save_state_to_stable(s);
        Ok(result)
    }).map(|result| (result, true)).map_err(ProtocolError::GenericError)
}

pub fn get_push_deposit_sweep_status(
    owner: Principal,
    ledger: Principal,
) -> Option<crate::PushDepositSweepStatusView> {
    read_state(|s| {
        if let Some(row) = s.pending_push_deposit_sweeps.get(&(owner, ledger)) {
            let operation = match &row.operation {
                crate::state::PushDepositSweepOperation::Open {
                    collateral_type, ..
                } => crate::PushDepositSweepOperationKind::Open {
                    collateral_type: *collateral_type,
                },
                crate::state::PushDepositSweepOperation::AddMargin { vault_id, .. } => {
                    crate::PushDepositSweepOperationKind::AddMargin {
                        vault_id: *vault_id,
                    }
                }
            };
            return Some(crate::PushDepositSweepStatusView {
                owner,
                ledger,
                request_id: row.request_id,
                operation,
                phase: if row.had_ambiguous_attempt || row.candidate_block_index.is_some() {
                    crate::PushDepositSweepPhase::Held
                } else {
                    crate::PushDepositSweepPhase::Pending
                },
                amount_raw: row.tuple.amount_raw,
                fee_raw: row.tuple.fee_raw,
                expected_fee_raw: row.tuple.expected_fee_raw,
                memo: row.tuple.memo.clone(),
                created_at_time_ns: row.tuple.created_at_time_ns,
                candidate_block_index: row.candidate_block_index,
                result: None,
                had_ambiguous_attempt: row.had_ambiguous_attempt,
                last_error: row.last_error.clone(),
            });
        }
        let done = s.completed_push_deposit_sweeps.get(&(owner, ledger))?;
        let operation = match &done.operation {
            crate::state::PushDepositSweepOperation::Open {
                collateral_type, ..
            } => crate::PushDepositSweepOperationKind::Open {
                collateral_type: *collateral_type,
            },
            crate::state::PushDepositSweepOperation::AddMargin { vault_id, .. } => {
                crate::PushDepositSweepOperationKind::AddMargin {
                    vault_id: *vault_id,
                }
            }
        };
        let result = match &done.result {
            crate::state::PushDepositSweepResult::Open {
                vault_id,
                block_index,
            } => crate::PushDepositSweepResultView::Open {
                vault_id: *vault_id,
                block_index: *block_index,
            },
            crate::state::PushDepositSweepResult::AddMargin { block_index } => {
                crate::PushDepositSweepResultView::AddMargin {
                    block_index: *block_index,
                }
            }
            crate::state::PushDepositSweepResult::Rejected { message } => {
                crate::PushDepositSweepResultView::Rejected {
                    message: message.clone(),
                }
            }
        };
        Some(crate::PushDepositSweepStatusView {
            owner,
            ledger,
            request_id: done.request_id,
            operation,
            phase: if matches!(
                &done.result,
                crate::state::PushDepositSweepResult::Rejected { .. }
            ) {
                crate::PushDepositSweepPhase::Rejected
            } else {
                crate::PushDepositSweepPhase::Complete
            },
            amount_raw: done.tuple.amount_raw,
            fee_raw: done.tuple.fee_raw,
            expected_fee_raw: done.tuple.expected_fee_raw,
            memo: done.tuple.memo.clone(),
            created_at_time_ns: done.tuple.created_at_time_ns,
            candidate_block_index: match &done.result {
                crate::state::PushDepositSweepResult::Open { block_index, .. }
                | crate::state::PushDepositSweepResult::AddMargin { block_index } => {
                    Some(*block_index)
                }
                crate::state::PushDepositSweepResult::Rejected { .. } => None,
            },
            result: Some(result),
            had_ambiguous_attempt: false,
            last_error: None,
        })
    })
}

/// Discover pending sweeps and the latest retained result per ledger even when
/// the currently configured collateral ledger has rotated since admission.
/// Results are owner-scoped, cursor-ordered, and bounded.
pub fn list_push_deposit_sweep_statuses(
    owner: Principal,
    after_ledger: Option<Principal>,
    limit: u16,
) -> Vec<crate::PushDepositSweepStatusView> {
    let limit = usize::from(limit.clamp(1, 100));
    read_state(|s| {
        let mut statuses = std::collections::BTreeMap::new();
        let lower = after_ledger
            .map(|ledger| std::ops::Bound::Excluded((owner, ledger)))
            .unwrap_or_else(|| {
                std::ops::Bound::Included((owner, Principal::management_canister()))
            });
        for ((_, ledger), row) in s
            .pending_push_deposit_sweeps
            .range((lower.clone(), std::ops::Bound::Unbounded))
            .take_while(|((pending_owner, _), _)| *pending_owner == owner)
            .take(limit)
        {
            let operation = match &row.operation {
                crate::state::PushDepositSweepOperation::Open {
                    collateral_type, ..
                } => crate::PushDepositSweepOperationKind::Open {
                    collateral_type: *collateral_type,
                },
                crate::state::PushDepositSweepOperation::AddMargin { vault_id, .. } => {
                    crate::PushDepositSweepOperationKind::AddMargin {
                        vault_id: *vault_id,
                    }
                }
            };
            statuses.insert(
                *ledger,
                crate::PushDepositSweepStatusView {
                    owner,
                    ledger: *ledger,
                    request_id: row.request_id,
                    operation,
                    phase: if row.had_ambiguous_attempt || row.candidate_block_index.is_some() {
                        crate::PushDepositSweepPhase::Held
                    } else {
                        crate::PushDepositSweepPhase::Pending
                    },
                    amount_raw: row.tuple.amount_raw,
                    fee_raw: row.tuple.fee_raw,
                    expected_fee_raw: row.tuple.expected_fee_raw,
                    memo: row.tuple.memo.clone(),
                    created_at_time_ns: row.tuple.created_at_time_ns,
                    candidate_block_index: row.candidate_block_index,
                    result: None,
                    had_ambiguous_attempt: row.had_ambiguous_attempt,
                    last_error: row.last_error.clone(),
                },
            );
        }
        for ((_, ledger), done) in s
            .completed_push_deposit_sweeps
            .range((lower, std::ops::Bound::Unbounded))
            .take_while(|((done_owner, _), _)| *done_owner == owner)
            .take(limit)
        {
            if statuses.contains_key(ledger) {
                continue;
            }
            let operation = match &done.operation {
                crate::state::PushDepositSweepOperation::Open {
                    collateral_type, ..
                } => crate::PushDepositSweepOperationKind::Open {
                    collateral_type: *collateral_type,
                },
                crate::state::PushDepositSweepOperation::AddMargin { vault_id, .. } => {
                    crate::PushDepositSweepOperationKind::AddMargin {
                        vault_id: *vault_id,
                    }
                }
            };
            let result = match &done.result {
                crate::state::PushDepositSweepResult::Open {
                    vault_id,
                    block_index,
                } => crate::PushDepositSweepResultView::Open {
                    vault_id: *vault_id,
                    block_index: *block_index,
                },
                crate::state::PushDepositSweepResult::AddMargin { block_index } => {
                    crate::PushDepositSweepResultView::AddMargin {
                        block_index: *block_index,
                    }
                }
                crate::state::PushDepositSweepResult::Rejected { message } => {
                    crate::PushDepositSweepResultView::Rejected {
                        message: message.clone(),
                    }
                }
            };
            let block_index = match &done.result {
                crate::state::PushDepositSweepResult::Open { block_index, .. }
                | crate::state::PushDepositSweepResult::AddMargin { block_index } => *block_index,
                crate::state::PushDepositSweepResult::Rejected { .. } => 0,
            };
            statuses.insert(
                *ledger,
                crate::PushDepositSweepStatusView {
                    owner,
                    ledger: *ledger,
                    request_id: done.request_id,
                    operation,
                    phase: if matches!(
                        &done.result,
                        crate::state::PushDepositSweepResult::Rejected { .. }
                    ) {
                        crate::PushDepositSweepPhase::Rejected
                    } else {
                        crate::PushDepositSweepPhase::Complete
                    },
                    amount_raw: done.tuple.amount_raw,
                    fee_raw: done.tuple.fee_raw,
                    expected_fee_raw: done.tuple.expected_fee_raw,
                    memo: done.tuple.memo.clone(),
                    created_at_time_ns: done.tuple.created_at_time_ns,
                    candidate_block_index: match &done.result {
                        crate::state::PushDepositSweepResult::Open { block_index, .. }
                        | crate::state::PushDepositSweepResult::AddMargin { block_index } => {
                            Some(*block_index)
                        }
                        crate::state::PushDepositSweepResult::Rejected { .. } => None,
                    },
                    result: Some(result),
                    had_ambiguous_attempt: false,
                    last_error: None,
                },
            );
        }
        statuses.into_values().take(limit).collect()
    })
}

pub async fn recover_push_deposit_sweep(
    owner: Principal,
    ledger: Principal,
    request_id: u128,
) -> Result<crate::state::PushDepositSweepResult, ProtocolError> {
    let _deposit_sweep_guard = PushDepositSweepGuard::new(owner)?;
    recover_push_deposit_sweep_inner(owner, ledger, request_id).await
}

async fn recover_push_deposit_sweep_inner(
    owner: Principal,
    ledger: Principal,
    request_id: u128,
) -> Result<crate::state::PushDepositSweepResult, ProtocolError> {
    let row = read_state(|s| s.pending_push_deposit_sweeps.get(&(owner, ledger)).cloned());
    let Some(row) = row else {
        return match read_state(|s| {
            s.completed_push_deposit_sweeps
                .get(&(owner, ledger))
                .cloned()
        }) {
            Some(done) if done.request_id == request_id => match done.result {
                crate::state::PushDepositSweepResult::Rejected { message } => {
                    Err(ProtocolError::TemporarilyUnavailable(message))
                }
                result => Ok(result),
            },
            _ => Err(ProtocolError::GenericError(
                "no push-deposit request with that ID is pending or retained".into(),
            )),
        };
    };
    if row.request_id != request_id {
        return Err(ProtocolError::GenericError(
            "request ID does not match the pending push-deposit tuple".into(),
        ));
    }
    let _vault_op_guard = match &row.operation {
        crate::state::PushDepositSweepOperation::AddMargin { vault_id, .. } => {
            Some(VaultLiquidationGuard::new(*vault_id)?)
        }
        crate::state::PushDepositSweepOperation::Open { .. } => None,
    };
    let (result, _) = settle_push_deposit_sweep(
        owner,
        ledger,
        row.tuple.expected_fee_raw,
        0,
        Some(request_id),
        row.operation,
    )
    .await?;
    Ok(result)
}

pub async fn attach_push_deposit_sweep_receipt(
    owner: Principal,
    ledger: Principal,
    request_id: u128,
    block_index: u64,
) -> Result<crate::state::PushDepositSweepResult, ProtocolError> {
    let _deposit_sweep_guard = PushDepositSweepGuard::new(owner)?;
    let row = read_state(|s| s.pending_push_deposit_sweeps.get(&(owner, ledger)).cloned())
        .ok_or_else(|| {
            ProtocolError::GenericError("no pending push-deposit sweep exists".into())
        })?;
    if row.request_id != request_id {
        return Err(ProtocolError::GenericError(
            "request ID does not match the pending push-deposit tuple".into(),
        ));
    }
    verify_push_deposit_sweep_receipt(&row.tuple, block_index)
        .await
        .map_err(ProtocolError::TemporarilyUnavailable)?;
    mutate_state(|s| {
        let current = s
            .pending_push_deposit_sweeps
            .get_mut(&(owner, ledger))
            .ok_or_else(|| "pending push-deposit sweep disappeared".to_string())?;
        if current.request_id != request_id || current.tuple != row.tuple {
            return Err("pending push-deposit tuple changed before receipt attachment".to_string());
        }
        if current
            .candidate_block_index
            .is_some_and(|saved| saved != block_index)
        {
            return Err("a different candidate receipt is already pinned".to_string());
        }
        current.candidate_block_index = Some(block_index);
        current.had_ambiguous_attempt = true;
        current.last_error = None;
        crate::storage::save_state_to_stable(s);
        Ok(())
    })
    .map_err(ProtocolError::GenericError)?;
    recover_push_deposit_sweep_inner(owner, ledger, request_id).await
}

pub async fn open_vault_with_deposit(
    borrow_amount_raw: u64,
    collateral_type_opt: Option<Principal>,
) -> Result<OpenVaultSuccess, ProtocolError> {
    open_vault_with_deposit_inner(borrow_amount_raw, collateral_type_opt, None).await
}

pub async fn open_vault_with_deposit_v2(
    borrow_amount_raw: u64,
    collateral_type_opt: Option<Principal>,
    request_id: u128,
) -> Result<OpenVaultSuccess, ProtocolError> {
    if borrow_amount_raw != 0 {
        return Err(ProtocolError::GenericError(
            "request-ID push-deposit V2 opens with zero borrow; borrow separately after the vault is confirmed".into(),
        ));
    }
    open_vault_with_deposit_inner(borrow_amount_raw, collateral_type_opt, Some(request_id)).await
}

async fn open_vault_with_deposit_inner(
    borrow_amount_raw: u64,
    collateral_type_opt: Option<Principal>,
    request_id: Option<u128>,
) -> Result<OpenVaultSuccess, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let guard_principal = match GuardPrincipal::new(caller, "open_vault_with_deposit") {
        Ok(guard) => guard,
        Err(GuardError::AlreadyProcessing) => {
            log!(
                INFO,
                "[open_vault_with_deposit] Principal {:?} already has an ongoing operation",
                caller
            );
            return Err(ProtocolError::AlreadyProcessing);
        }
        Err(err) => return Err(err.into()),
    };
    let _deposit_sweep_guard = match PushDepositSweepGuard::new(caller) {
        Ok(guard) => guard,
        Err(error) => {
            guard_principal.fail();
            return Err(error);
        }
    };

    // Resolve collateral type: default to ICP if not specified
    let collateral_type =
        collateral_type_opt.unwrap_or_else(|| read_state(|s| s.icp_collateral_type()));

    // Look up CollateralConfig
    let (config_ledger, config_status, config_fee, min_deposit, is_native_xrp) =
        read_state(|s| match s.get_collateral_config(&collateral_type) {
            Some(config) => Ok((
                config.ledger_canister_id,
                config.status,
                config.ledger_fee,
                config.min_collateral_deposit,
                config.is_native_xrp(),
            )),
            None => Err(ProtocolError::GenericError(
                "Collateral type not supported.".to_string(),
            )),
        })?;

    let configured_three_pool = read_state(|s| s.three_pool_canister);
    if let Err(error) = reject_three_usd_lp_push_deposit(config_ledger, configured_three_pool) {
        guard_principal.fail();
        return Err(error);
    }

    // P2: native-XRP collateral is custodied on the XRP Ledger (chains::xrp), not
    // swept from an ICRC deposit subaccount. Reject until the XRP deposit flow (P3).
    if is_native_xrp {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "Native-XRP collateral uses the XRP deposit flow (not yet enabled).".to_string(),
        ));
    }

    if !config_status.allows_open() {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "Collateral type is not accepting new vaults.".to_string(),
        ));
    }

    let operation = crate::state::PushDepositSweepOperation::Open {
        collateral_type,
        reserved_vault_id: 0,
        borrow_amount_raw,
    };
    let (sweep_result, newly_credited) = match settle_push_deposit_sweep(
        caller,
        config_ledger,
        config_fee,
        min_deposit,
        request_id,
        operation,
    )
    .await
    {
        Ok(result) => result,
        Err(error) => {
            guard_principal.fail();
            return Err(error);
        }
    };
    let (vault_id, sweep_block_index) = match sweep_result {
        crate::state::PushDepositSweepResult::Open {
            vault_id,
            block_index,
        } => (vault_id, block_index),
        _ => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(
                "push-deposit request result kind mismatch".into(),
            ));
        }
    };
    let collateral_amount = read_state(|s| {
        s.vault_id_to_vaults
            .get(&vault_id)
            .map(|v| v.collateral_amount)
            .unwrap_or(0)
    });

    log!(INFO, "[open_vault_with_deposit] opened vault {} for {} with {} collateral via push-deposit (sweep block {})",
        vault_id, caller, collateral_amount, sweep_block_index);

    // If the caller also requested an initial borrow, do it now.
    // Use borrow_from_vault_internal to avoid GuardPrincipal conflict —
    // this function already holds the guard for `caller`.
    if newly_credited && borrow_amount_raw > 0 {
        // AR-B-003: per-vault op lock across the borrow's mint await.
        let _vault_op_guard = VaultLiquidationGuard::new(vault_id)?;
        match borrow_from_vault_internal(
            caller,
            VaultArg {
                vault_id,
                amount: borrow_amount_raw,
            },
        )
        .await
        {
            Ok(borrow_result) => {
                log!(
                    INFO,
                    "[open_vault_with_deposit] vault {} initial borrow of {} succeeded (fee: {})",
                    vault_id,
                    borrow_amount_raw,
                    borrow_result.fee_amount_paid
                );
            }
            Err(e) => {
                guard_principal.fail();
                return Err(ProtocolError::GenericError(format!(
                    "Vault created (id={}) but initial borrow of {} failed: {:?}. You can borrow in a separate call.",
                    vault_id, borrow_amount_raw, e
                )));
            }
        }
    }

    guard_principal.complete();
    Ok(OpenVaultSuccess {
        vault_id,
        block_index: sweep_block_index,
    })
}

pub async fn add_margin_with_deposit(vault_id: u64) -> Result<u64, ProtocolError> {
    add_margin_with_deposit_inner(vault_id, None).await
}

pub async fn add_margin_with_deposit_v2(
    vault_id: u64,
    request_id: u128,
) -> Result<u64, ProtocolError> {
    add_margin_with_deposit_inner(vault_id, Some(request_id)).await
}

async fn add_margin_with_deposit_inner(
    vault_id: u64,
    request_id: Option<u128>,
) -> Result<u64, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let guard_principal = GuardPrincipal::new(caller, &format!("add_margin_deposit_{}", vault_id))?;
    let _deposit_sweep_guard = match PushDepositSweepGuard::new(caller) {
        Ok(guard) => guard,
        Err(error) => {
            guard_principal.fail();
            return Err(error);
        }
    };
    // AR-B-003: per-vault op lock; see guard.rs::VaultLiquidationGuard.
    let _vault_op_guard = match VaultLiquidationGuard::new(vault_id) {
        Ok(g) => g,
        Err(e) => {
            guard_principal.fail();
            return Err(e);
        }
    };

    let now = ic_cdk::api::time();
    if let Err(e) = reject_active_xrp_sp_absorb_preflight(vault_id, now) {
        guard_principal.fail();
        return Err(e);
    }

    let (vault, config_ledger, config_fee, min_deposit, is_native_xrp) =
        match read_state(|s| match s.vault_id_to_vaults.get(&vault_id) {
            Some(v) => {
                let config = s
                    .get_collateral_config(&v.collateral_type)
                    .ok_or("Collateral type not configured")?;
                Ok((
                    v.clone(),
                    config.ledger_canister_id,
                    config.ledger_fee,
                    config.min_collateral_deposit,
                    config.is_native_xrp(),
                ))
            }
            None => Err("Vault not found"),
        }) {
            Ok(result) => result,
            Err(msg) => {
                guard_principal.fail();
                return Err(ProtocolError::GenericError(msg.to_string()));
            }
        };

    let configured_three_pool = read_state(|s| s.three_pool_canister);
    if let Err(error) = reject_three_usd_lp_push_deposit(config_ledger, configured_three_pool) {
        guard_principal.fail();
        return Err(error);
    }

    // P2: native-XRP collateral is not custodied via ICRC; its add-collateral flow
    // is wired with the XRP deposit path (P3). Reject so XRP collateral can never be
    // swept as an ICRC token. (Latent until P5 enables XRP registration.)
    if is_native_xrp {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "Native-XRP collateral uses the XRP deposit flow (not yet enabled).".to_string(),
        ));
    }

    if caller != vault.owner {
        guard_principal.fail();
        return Err(ProtocolError::CallerNotOwner);
    }

    if let Err(e) = require_vault_not_processing_except(
        &vault,
        None,
        request_id.map(|request_id| (caller, config_ledger, request_id)),
    ) {
        guard_principal.fail();
        return Err(e);
    }

    // Check collateral status
    let collateral_status = read_state(|s| s.get_collateral_status(&vault.collateral_type));
    if let Some(status) = collateral_status {
        if !status.allows_add_collateral() {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(
                "Adding collateral is not allowed for this collateral type.".to_string(),
            ));
        }
    }

    let operation = crate::state::PushDepositSweepOperation::AddMargin {
        vault_id,
        vault_snapshot: vault.clone(),
    };
    let (sweep_result, _newly_credited) = match settle_push_deposit_sweep(
        caller,
        config_ledger,
        config_fee,
        min_deposit,
        request_id,
        operation,
    )
    .await
    {
        Ok(result) => result,
        Err(error) => {
            guard_principal.fail();
            return Err(error);
        }
    };
    let sweep_block_index = match sweep_result {
        crate::state::PushDepositSweepResult::AddMargin { block_index } => block_index,
        _ => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(
                "push-deposit request result kind mismatch".into(),
            ));
        }
    };
    let collateral_amount = read_state(|s| {
        s.vault_id_to_vaults
            .get(&vault_id)
            .map(|v| v.collateral_amount.saturating_sub(vault.collateral_amount))
            .unwrap_or(0)
    });

    log!(INFO, "[add_margin_with_deposit] added {} collateral to vault {} via push-deposit (sweep block {})",
        collateral_amount, vault_id, sweep_block_index);

    guard_principal.complete();
    Ok(sweep_block_index)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WithdrawCloseCompletionPolicy {
    CloseVault,
    KeepNativeXrpVaultOpen,
}

fn withdraw_close_completion_policy(is_native_xrp: bool) -> WithdrawCloseCompletionPolicy {
    if is_native_xrp {
        WithdrawCloseCompletionPolicy::KeepNativeXrpVaultOpen
    } else {
        WithdrawCloseCompletionPolicy::CloseVault
    }
}

fn native_xrp_reserve_locked_message() -> String {
    "Native-XRP vaults stay open because the XRP account reserve remains locked on XRPL."
        .to_string()
}

/// Releases the in-flight close counter on every function exit. Request
/// timestamps remain recorded for rate limiting; only the concurrency slot is
/// released here.
struct CloseVaultRequestGuard;

impl Drop for CloseVaultRequestGuard {
    fn drop(&mut self) {
        mutate_state(|s| s.complete_close_vault_request());
    }
}

fn record_close_request_with_concurrency_guard(caller: Principal) -> CloseVaultRequestGuard {
    mutate_state(|s| s.record_close_vault_request(caller));
    CloseVaultRequestGuard
}

#[cfg(test)]
mod p01_vault_regression_tests {
    use super::*;

    #[test]
    fn push_deposit_minimum_is_checked_against_net_credit_before_sweep() {
        assert!(matches!(
            push_deposit_balance_minimum_error(100, 10, 91),
            Some(ProtocolError::AmountTooLow { minimum_amount: 91 })
        ));
        assert!(push_deposit_balance_minimum_error(100, 10, 90).is_none());
        // Preserve sweep_deposit's existing empty/fee-only balance diagnostics.
        assert!(push_deposit_balance_minimum_error(10, 10, 1).is_none());
        assert!(push_deposit_balance_minimum_error(0, 10, 1).is_none());
        assert!(push_deposit_balance_minimum_error(100, 10, 0).is_none());
    }

    #[test]
    fn three_usd_lp_ledger_is_rejected_from_push_deposit() {
        let canonical = Principal::from_text(CANONICAL_THREE_USD_LP_LEDGER).unwrap();
        let configured = Principal::from_slice(&[0xa6]);
        let ordinary = Principal::from_slice(&[0xa7]);

        assert!(reject_three_usd_lp_push_deposit(canonical, None).is_err());
        assert!(reject_three_usd_lp_push_deposit(configured, Some(configured)).is_err());
        assert!(reject_three_usd_lp_push_deposit(ordinary, None).is_ok());
    }

    #[test]
    fn linked_reserve_payout_retry_respects_held_and_reconciliation_state() {
        let caller = Principal::from_slice(&[0xa8]);
        let ledger = Principal::from_slice(&[0xa9]);
        let mut transfer = PendingMarginTransfer {
            vault_id: 1,
            owner: caller,
            margin: ICP::new(100),
            collateral_type: Principal::anonymous(),
            retry_count: 0,
            op_nonce: 11,
            ledger: Some(ledger),
            transfer_amount_raw: Some(90),
            redemption_transfer: None,
            held_for_manual_retry: false,
            reconciliation_required: false,
            min_net_collateral_raw: None,
        };

        assert!(linked_three_usd_payout_is_retryable(11, 11, &transfer));
        transfer.held_for_manual_retry = true;
        assert!(!linked_three_usd_payout_is_retryable(11, 11, &transfer));
        transfer.held_for_manual_retry = false;
        transfer.reconciliation_required = true;
        assert!(!linked_three_usd_payout_is_retryable(11, 11, &transfer));
        assert!(!linked_three_usd_payout_is_retryable(12, 11, &transfer));
    }

    #[test]
    fn push_deposit_sweep_lock_excludes_a_stale_same_caller_operation() {
        let caller = Principal::from_slice(&[0xa2]);
        let first = PushDepositSweepGuard::new(caller).unwrap();
        assert!(matches!(
            PushDepositSweepGuard::new(caller),
            Err(ProtocolError::TemporarilyUnavailable(_))
        ));
        drop(first);
        assert!(PushDepositSweepGuard::new(caller).is_ok());
    }

    #[test]
    fn pending_push_margin_fence_survives_state_roundtrip_and_blocks_close() {
        let owner = Principal::from_slice(&[0xb1]);
        let ledger_a = Principal::from_slice(&[0xb2]);
        let ledger_b = Principal::from_slice(&[0xb3]);
        let other_owner = Principal::from_slice(&[0xb4]);
        let vault = Vault {
            owner,
            borrowed_icusd_amount: ICUSD::new(0),
            collateral_amount: 100,
            vault_id: 7,
            collateral_type: ledger_a,
            last_accrual_time: 0,
            accrued_interest: ICUSD::new(0),
            bot_processing: false,
        };
        let operation = crate::state::PushDepositSweepOperation::AddMargin {
            vault_id: 7,
            vault_snapshot: vault.clone(),
        };
        let backend = Principal::management_canister();
        let tuple = crate::state::PushDepositSweepTuple {
            op_nonce: 1,
            ledger: ledger_b,
            from: Account {
                owner: backend,
                subaccount: Some([1; 32]),
            },
            to: Account {
                owner: backend,
                subaccount: None,
            },
            amount_raw: 50,
            fee_raw: Some(10),
            expected_fee_raw: 10,
            proof_kind: Some(crate::state::PayoutProofKind::Icrc3),
            memo: vec![1],
            created_at_time_ns: 1,
        };
        let journal = crate::state::PushDepositSweepJournal {
            owner,
            request_id: 2,
            operation: operation.clone(),
            tuple: tuple.clone(),
            observed_balance_raw: 60,
            had_ambiguous_attempt: true,
            candidate_block_index: None,
            last_error: Some("reply was ambiguous".into()),
        };
        let mut state = crate::state::State::default();
        state.vault_id_to_vaults.insert(7, vault);
        state
            .pending_push_deposit_sweeps
            .insert((owner, ledger_b), journal);
        state.completed_push_deposit_sweeps.insert(
            (owner, ledger_a),
            crate::state::CompletedPushDepositSweep {
                request_id: 1,
                operation: crate::state::PushDepositSweepOperation::Open {
                    collateral_type: ledger_a,
                    reserved_vault_id: 3,
                    borrow_amount_raw: 0,
                },
                tuple: crate::state::PushDepositSweepTuple {
                    ledger: ledger_a,
                    ..tuple.clone()
                },
                result: crate::state::PushDepositSweepResult::Open {
                    vault_id: 3,
                    block_index: 4,
                },
            },
        );
        state.pending_push_deposit_sweeps.insert(
            (other_owner, ledger_a),
            crate::state::PushDepositSweepJournal {
                owner: other_owner,
                request_id: 1,
                operation: crate::state::PushDepositSweepOperation::Open {
                    collateral_type: ledger_a,
                    reserved_vault_id: 4,
                    borrow_amount_raw: 0,
                },
                tuple: crate::state::PushDepositSweepTuple {
                    ledger: ledger_a,
                    ..tuple.clone()
                },
                observed_balance_raw: 60,
                had_ambiguous_attempt: true,
                candidate_block_index: None,
                last_error: None,
            },
        );

        let mut encoded = Vec::new();
        ciborium::ser::into_writer(&state, &mut encoded)
            .expect("serialize state with held push margin");
        let restored: crate::state::State = ciborium::de::from_reader(encoded.as_slice())
            .expect("restore state with held push margin");
        assert!(restored.vault_has_pending_inbound_margin(7));
        let mut query_encoded = Vec::new();
        ciborium::ser::into_writer(&restored, &mut query_encoded).expect("serialize query fixture");
        let query_state: crate::state::State =
            ciborium::de::from_reader(query_encoded.as_slice()).expect("restore query fixture");
        crate::state::replace_state(query_state);
        let first_page = list_push_deposit_sweep_statuses(owner, None, 1);
        assert_eq!(first_page.len(), 1);
        assert_eq!(first_page[0].ledger, ledger_a);
        assert_eq!(first_page[0].request_id, 1);
        assert!(matches!(
            first_page[0].phase,
            crate::PushDepositSweepPhase::Complete
        ));
        let second_page = list_push_deposit_sweep_statuses(owner, Some(ledger_a), 1);
        assert_eq!(second_page.len(), 1);
        assert_eq!(second_page[0].ledger, ledger_b);
        assert_eq!(second_page[0].request_id, 2);
        assert!(matches!(
            second_page[0].phase,
            crate::PushDepositSweepPhase::Held
        ));

        let mut restored = restored;
        let close = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            restored.remove_vault_and_unindex(7)
        }));
        assert!(
            close.is_err(),
            "canonical removal must be fenced while push margin is pending"
        );
        restored
            .pending_push_deposit_sweeps
            .remove(&(owner, ledger_b));
        assert!(!restored.vault_has_pending_inbound_margin(7));
        assert!(
            restored.remove_vault_and_unindex(7).is_some(),
            "exact settlement clears the durable fence"
        );
    }

    #[test]
    fn close_vault_concurrency_slot_is_released_on_early_return() {
        let caller = Principal::from_slice(&[0xa1]);
        let mut state = crate::state::State::default();
        state.close_vault_requests.insert(caller, vec![0]);
        state.global_close_requests.push_back(0);
        state.concurrent_close_operations = 1;
        crate::state::replace_state(state);

        let result = (|| -> Result<(), ProtocolError> {
            let _close_request_guard = CloseVaultRequestGuard;
            Err(ProtocolError::CallerNotOwner)
        })();

        assert!(matches!(result, Err(ProtocolError::CallerNotOwner)));
        crate::state::read_state(|s| {
            assert_eq!(s.concurrent_close_operations, 0);
            assert_eq!(s.close_vault_requests.get(&caller).unwrap().len(), 1);
            assert_eq!(s.global_close_requests.len(), 1);
        });
    }
}

pub async fn close_vault(vault_id: u64) -> Result<Option<u64>, ProtocolError> {
    let caller = ic_cdk::caller();
    let _guard_principal = GuardPrincipal::new(caller, &format!("close_vault_{}", vault_id))?;
    // AR-B-003: per-vault op lock; see guard.rs::VaultLiquidationGuard.
    let _vault_op_guard = VaultLiquidationGuard::new(vault_id)?;
    reject_active_xrp_sp_absorb_preflight(vault_id, ic_cdk::api::time())?;

    // Check rate limits first
    mutate_state(|s| s.check_close_vault_rate_limit(caller))?;

    // Record the close request for rate limiting
    let _close_request_guard = record_close_request_with_concurrency_guard(caller);

    // Accrue interest before closing so the full repayment amount is accurate.
    let now = ic_cdk::api::time();
    mutate_state(|s| s.accrue_single_vault(vault_id, now));

    // Check if the vault exists first
    let vault_exists = read_state(|s| s.vault_id_to_vaults.contains_key(&vault_id));

    if !vault_exists {
        log!(
            INFO,
            "[close_vault] Vault #{} not found for principal {}",
            vault_id,
            caller
        );
        return Err(ProtocolError::GenericError(format!(
            "Vault #{} not found",
            vault_id
        )));
    }

    // Get the vault
    let vault = read_state(|s| {
        s.vault_id_to_vaults
            .get(&vault_id)
            .cloned()
            .ok_or(ProtocolError::GenericError("Vault not found".to_string()))
    })?;

    require_vault_not_processing(&vault)?;

    // Check collateral status allows closing
    let collateral_status = read_state(|s| s.get_collateral_status(&vault.collateral_type));
    if let Some(status) = collateral_status {
        if !status.allows_close() {
            return Err(ProtocolError::GenericError(
                "Closing vaults is not allowed for this collateral type.".to_string(),
            ));
        }
    }

    // Verify caller is the owner
    if caller != vault.owner {
        log!(
            INFO,
            "[close_vault] Principal {} is not the owner of vault #{}",
            caller,
            vault_id
        );
        return Err(ProtocolError::CallerNotOwner);
    }

    // Handle dust amounts - if debt is very small, forgive it
    if vault.borrowed_icusd_amount <= DUST_THRESHOLD {
        log!(
            INFO,
            "[close_vault] Forgiving dust debt of {} icUSD for vault #{}",
            vault.borrowed_icusd_amount,
            vault_id
        );

        // Record dust forgiveness (no real payment, no treasury routing)
        mutate_state(|s| {
            s.dust_forgiven_total += vault.borrowed_icusd_amount;
            let _ = s.repay_to_vault(vault_id, vault.borrowed_icusd_amount);
        });

        // Record dust forgiveness event
        crate::storage::record_event(&crate::event::Event::DustForgiven {
            vault_id,
            amount: vault.borrowed_icusd_amount,
            timestamp: Some(ic_cdk::api::time()),
        });
    } else if vault.borrowed_icusd_amount > ICUSD::new(0) {
        log!(
            INFO,
            "[close_vault] Cannot close vault #{} with outstanding debt: {}",
            vault_id,
            vault.borrowed_icusd_amount
        );
        return Err(ProtocolError::GenericError(
            "Cannot close vault with outstanding debt. Repay all debt first.".to_string(),
        ));
    }

    // Verify there's no remaining collateral
    if vault.collateral_amount > 0 {
        log!(
            INFO,
            "[close_vault] Cannot close vault #{} with remaining collateral: {}",
            vault_id,
            vault.collateral_amount
        );
        return Err(ProtocolError::GenericError(
            "Cannot close vault with remaining collateral. Withdraw collateral first.".to_string(),
        ));
    }

    let is_native_xrp = read_state(|s| {
        s.get_collateral_config(&vault.collateral_type)
            .map(|config| config.is_native_xrp())
            .unwrap_or(false)
    });
    if withdraw_close_completion_policy(is_native_xrp)
        == WithdrawCloseCompletionPolicy::KeepNativeXrpVaultOpen
    {
        log!(
            INFO,
            "[close_vault] Keeping native-XRP vault #{} open because the XRPL reserve remains locked",
            vault_id
        );
        return Err(ProtocolError::GenericError(
            native_xrp_reserve_locked_message(),
        ));
    }

    // Simply close the vault - no transfers needed
    mutate_state(|s| {
        // Make sure vault exists before attempting to remove
        if s.vault_id_to_vaults.contains_key(&vault_id) {
            // The vault must still exist when record_close_vault runs:
            // state::close_vault inside it performs the removal (primary map
            // + every secondary index) and traps on an unknown vault. An
            // earlier version removed the vault inline here first, so the
            // recorder's close always hit that trap and rolled the whole
            // call back — the endpoint could never succeed.
            crate::event::record_close_vault(s, vault_id, None);

            log!(
                INFO,
                "[close_vault] Successfully closed vault #{} for principal {}",
                vault_id,
                caller
            );
        } else {
            // Log that we tried to close a vault that was already gone
            log!(
                INFO,
                "[close_vault] Attempted to close vault #{} that was already removed",
                vault_id
            );
        }
    });

    // Return success with no block index (since no transfer was made)
    Ok(None)
}

pub const COLLATERAL_WITHDRAWAL_DEDUP_WINDOW_NS: u64 = 23 * 60 * 60 * 1_000_000_000;

fn reserve_collateral_withdrawal(
    vault: &Vault,
    owner: Principal,
    ledger: Principal,
    fee_raw: u64,
    gross_amount_raw: u64,
    action: crate::state::CollateralWithdrawalAction,
    forgive_dust_debt: bool,
    repay_block_index: Option<u64>,
) -> Result<crate::state::PendingCollateralWithdrawal, ProtocolError> {
    let net_amount_raw = gross_amount_raw
        .checked_sub(fee_raw)
        .filter(|n| *n > 0)
        .ok_or_else(|| {
            ProtocolError::GenericError(
                "Collateral withdrawal must exceed the configured ledger fee.".into(),
            )
        })?;
    mutate_state(|state| {
        if let Some(existing) = state.pending_collateral_withdrawals.get(&vault.vault_id) {
            return Err(ProtocolError::GenericError(format!(
                "Vault has unresolved collateral withdrawal operation {} ({:?}, gross {}). Recover that exact obligation first.",
                existing.operation_id, existing.action, existing.gross_amount_raw
            )));
        }
        let live_vault = state
            .vault_id_to_vaults
            .get(&vault.vault_id)
            .ok_or_else(|| ProtocolError::GenericError("Vault no longer exists".into()))?;
        if live_vault.owner != vault.owner
            || live_vault.collateral_type != vault.collateral_type
            || live_vault.collateral_amount != vault.collateral_amount
            || live_vault.borrowed_icusd_amount != vault.borrowed_icusd_amount
            || live_vault.accrued_interest != vault.accrued_interest
            || live_vault.last_accrual_time != vault.last_accrual_time
            || live_vault.bot_processing
        {
            return Err(ProtocolError::GenericError(
                "Vault changed during collateral withdrawal preflight; retry with a fresh quote."
                    .into(),
            ));
        }
        if live_vault.owner != owner || live_vault.collateral_amount < gross_amount_raw {
            return Err(ProtocolError::GenericError(
                "Vault owner or collateral no longer matches withdrawal request.".into(),
            ));
        }
        let operation_id = state.next_op_nonce();
        let transfer = crate::state::PendingCollateralWithdrawal {
            operation_id,
            vault_id: vault.vault_id,
            owner,
            ledger,
            action,
            gross_amount_raw,
            net_amount_raw,
            fee_raw,
            memo: operation_id,
            created_at_time_ns: management::nonce_to_created_at_time(operation_id),
            phase: crate::state::CollateralWithdrawalPhase::Reserved,
            dispatch_attempts: 0,
            last_error: None,
            forgive_dust_debt,
            repay_block_index,
        };
        crate::event::record_collateral_withdrawal_queued(state, transfer.clone());
        Ok(transfer)
    })
}

async fn dispatch_collateral_withdrawal(
    vault_id: u64,
    operation_id: u128,
) -> Result<u64, ProtocolError> {
    let now = ic_cdk::api::time();
    let tuple = mutate_state(|state| {
        let row = state
            .pending_collateral_withdrawals
            .get(&vault_id)
            .cloned()
            .ok_or_else(|| {
                ProtocolError::GenericError("No pending collateral withdrawal".into())
            })?;
        if row.operation_id != operation_id {
            return Err(ProtocolError::GenericError(
                "Collateral withdrawal identity changed".into(),
            ));
        }
        if now < row.created_at_time_ns
            || now.saturating_sub(row.created_at_time_ns) > COLLATERAL_WITHDRAWAL_DEDUP_WINDOW_NS
        {
            crate::event::record_collateral_withdrawal_held(
                state,
                vault_id,
                operation_id,
                "ICRC deduplication window expired; exact history reconciliation is required"
                    .into(),
            );
            return Err(ProtocolError::GenericError(
                "Collateral withdrawal is held because its deduplication window expired; reconcile its exact ledger receipt.".into(),
            ));
        }
        if row.dispatch_attempts >= 60 {
            crate::event::record_collateral_withdrawal_held(
                state,
                vault_id,
                operation_id,
                "automatic exact-tuple retry cap reached; exact history reconciliation is required"
                    .into(),
            );
            return Err(ProtocolError::GenericError(
                "Collateral withdrawal is held at the retry cap; reconcile its exact ledger receipt.".into(),
            ));
        }
        let row =
            crate::event::record_collateral_withdrawal_dispatching(state, vault_id, operation_id)
                .ok_or_else(|| {
                ProtocolError::GenericError("Collateral withdrawal is held or retry-capped".into())
            })?;
        Ok(row)
    })?;
    match management::transfer_collateral_with_exact_outcome(
        tuple.ledger,
        tuple.owner,
        tuple.net_amount_raw,
        tuple.fee_raw,
        tuple.memo,
        tuple.created_at_time_ns,
    )
    .await
    {
        management::ExactCollateralTransferOutcome::Applied(block_index) => {
            let proof_tuple = crate::state::PinnedRedemptionTransfer {
                op_nonce: tuple.operation_id,
                ledger: tuple.ledger,
                recipient: tuple.owner,
                amount_raw: tuple.net_amount_raw,
                fee_raw: tuple.fee_raw,
                fee_is_explicit: true,
                memo: tuple.memo,
                created_at_time_ns: tuple.created_at_time_ns,
            };
            if let Err(reason) =
                management::verify_pinned_redemption_receipt(proof_tuple, block_index).await
            {
                mutate_state(|state| {
                    crate::event::record_collateral_withdrawal_ambiguous(
                        state,
                        vault_id,
                        operation_id,
                        format!("receipt verification failed: {reason}"),
                    )
                });
                return Err(ProtocolError::GenericError(
                    "Collateral transfer response is unverified; withdrawal remains held for reconciliation.".into(),
                ));
            }
            let settled = mutate_state(|state| {
                crate::event::record_collateral_withdrawal_settled(
                    state,
                    vault_id,
                    operation_id,
                    block_index,
                )
            });
            if settled {
                Ok(block_index)
            } else {
                Err(ProtocolError::GenericError(
                    "Exact receipt verified but vault accounting could not be reconciled; withdrawal remains held.".into(),
                ))
            }
        }
        management::ExactCollateralTransferOutcome::LedgerError(error) => {
            mutate_state(|state| {
                crate::event::record_collateral_withdrawal_held(
                    state,
                    vault_id,
                    operation_id,
                    format!("ledger returned {error:?}"),
                )
            });
            Err(ProtocolError::TransferError(error))
        }
        management::ExactCollateralTransferOutcome::ProvenNoEffect(error) => {
            let bad_fee = match &error {
                icrc_ledger_types::icrc1::transfer::TransferError::BadFee { expected_fee } => {
                    expected_fee.0.to_u64()
                }
                _ => None,
            };
            if tuple.dispatch_attempts == 1 {
                if let Some(expected_fee) = bad_fee {
                    let repriced = mutate_state(|state| {
                        crate::event::record_collateral_withdrawal_repriced(
                            state,
                            vault_id,
                            operation_id,
                            expected_fee,
                        )
                    });
                    if repriced.is_some() {
                        return Err(ProtocolError::GenericError(
                            "The ledger proved the first transfer had no effect; the withdrawal was safely re-pinned to its current fee and will retry.".into(),
                        ));
                    }
                }
                mutate_state(|state| {
                    crate::event::record_collateral_withdrawal_no_effect(
                        state,
                        vault_id,
                        operation_id,
                        format!("first typed ledger rejection proved no effect: {error:?}"),
                    )
                });
            } else {
                mutate_state(|state| {
                    crate::event::record_collateral_withdrawal_held(
                        state,
                        vault_id,
                        operation_id,
                        format!("later typed ledger rejection; exact payout held: {error:?}"),
                    )
                });
            }
            Err(ProtocolError::TransferError(error))
        }
        management::ExactCollateralTransferOutcome::CallRejected { code, message } => {
            mutate_state(|state| {
                crate::event::record_collateral_withdrawal_ambiguous(
                    state,
                    vault_id,
                    operation_id,
                    format!("ambiguous call rejection {code}: {message}"),
                )
            });
            Err(ProtocolError::GenericError(
                "Collateral transfer outcome is ambiguous; the exact payout remains fenced for recovery.".into(),
            ))
        }
        management::ExactCollateralTransferOutcome::InvalidBlockIndex => {
            mutate_state(|state| {
                crate::event::record_collateral_withdrawal_ambiguous(
                    state,
                    vault_id,
                    operation_id,
                    "ledger returned an unrepresentable block index".into(),
                )
            });
            Err(ProtocolError::GenericError(
                "Collateral transfer receipt is unrepresentable; exact payout remains held.".into(),
            ))
        }
    }
}

pub async fn retry_pending_collateral_withdrawal(vault_id: u64) -> Result<u64, ProtocolError> {
    let caller = ic_cdk::caller();
    let _vault_op_guard = VaultLiquidationGuard::new(vault_id)?;
    let row = read_state(|s| s.pending_collateral_withdrawals.get(&vault_id).cloned())
        .ok_or_else(|| ProtocolError::GenericError("No pending collateral withdrawal".into()))?;
    if caller != row.owner && !ic_cdk::api::is_controller(&caller) {
        return Err(ProtocolError::CallerNotOwner);
    }
    dispatch_collateral_withdrawal(vault_id, row.operation_id).await
}

pub async fn process_pending_collateral_withdrawals() {
    let now = ic_cdk::api::time();
    let pending = read_state(|state| {
        state
            .pending_collateral_withdrawals
            .values()
            .filter(|row| {
                !matches!(
                    row.phase,
                    crate::state::CollateralWithdrawalPhase::Held
                        | crate::state::CollateralWithdrawalPhase::NoEffect
                ) && row.dispatch_attempts < 60
                    && now.saturating_sub(row.created_at_time_ns)
                        <= COLLATERAL_WITHDRAWAL_DEDUP_WINDOW_NS
            })
            .take(10)
            .map(|row| (row.vault_id, row.operation_id))
            .collect::<Vec<_>>()
    });
    for (vault_id, operation_id) in pending {
        let Ok(_vault_op_guard) = VaultLiquidationGuard::new(vault_id) else {
            continue;
        };
        let _ = dispatch_collateral_withdrawal(vault_id, operation_id).await;
    }
}

pub async fn reconcile_pending_collateral_withdrawal(
    vault_id: u64,
    block_index: u64,
) -> Result<bool, ProtocolError> {
    let caller = ic_cdk::caller();
    let _vault_op_guard = VaultLiquidationGuard::new(vault_id)?;
    let row = read_state(|s| s.pending_collateral_withdrawals.get(&vault_id).cloned())
        .ok_or_else(|| ProtocolError::GenericError("No pending collateral withdrawal".into()))?;
    if caller != row.owner && !ic_cdk::api::is_controller(&caller) {
        return Err(ProtocolError::CallerNotOwner);
    }
    let proof_tuple = crate::state::PinnedRedemptionTransfer {
        op_nonce: row.operation_id,
        ledger: row.ledger,
        recipient: row.owner,
        amount_raw: row.net_amount_raw,
        fee_raw: row.fee_raw,
        fee_is_explicit: true,
        memo: row.memo,
        created_at_time_ns: row.created_at_time_ns,
    };
    management::verify_pinned_redemption_receipt(proof_tuple, block_index)
        .await
        .map_err(|reason| {
            ProtocolError::GenericError(format!(
                "Exact collateral withdrawal receipt did not verify: {reason}"
            ))
        })?;
    Ok(mutate_state(|state| {
        crate::event::record_collateral_withdrawal_settled(
            state,
            vault_id,
            row.operation_id,
            block_index,
        )
    }))
}

pub async fn withdraw_collateral(vault_id: u64) -> Result<u64, ProtocolError> {
    let caller = ic_cdk::caller();
    let _guard_principal =
        GuardPrincipal::new(caller, &format!("withdraw_collateral_{}", vault_id))?;
    // AR-B-003: per-vault op lock; see guard.rs::VaultLiquidationGuard.
    let _vault_op_guard = VaultLiquidationGuard::new(vault_id)?;
    if let Some(pending) = read_state(|s| s.pending_collateral_withdrawals.get(&vault_id).cloned())
    {
        if pending.owner != caller
            || pending.action != crate::state::CollateralWithdrawalAction::Full
        {
            return Err(ProtocolError::GenericError(
                "Vault has a different unresolved collateral withdrawal; use its recovery route."
                    .into(),
            ));
        }
        return dispatch_collateral_withdrawal(vault_id, pending.operation_id).await;
    }
    reject_active_xrp_sp_absorb_preflight(vault_id, ic_cdk::api::time())?;

    log!(
        INFO,
        "[withdraw_collateral] Request to withdraw collateral from vault #{} by principal {}",
        vault_id,
        caller
    );

    // Check vault exists and caller is owner
    let vault = read_state(|state| {
        state
            .vault_id_to_vaults
            .get(&vault_id)
            .cloned()
            .ok_or(ProtocolError::GenericError("Vault not found".to_string()))
    })?;

    require_vault_not_processing(&vault)?;

    // Check collateral status allows withdrawal
    let collateral_status = read_state(|s| s.get_collateral_status(&vault.collateral_type));
    if let Some(status) = collateral_status {
        if !status.allows_withdraw() {
            return Err(ProtocolError::GenericError(
                "Withdrawal is not allowed for this collateral type.".to_string(),
            ));
        }
    }

    if caller != vault.owner {
        log!(
            INFO,
            "[withdraw_collateral] Caller {} is not the owner of vault #{}",
            caller,
            vault_id
        );
        return Err(ProtocolError::CallerNotOwner);
    }

    // Check there's no debt
    if vault.borrowed_icusd_amount > ICUSD::new(0) {
        log!(
            INFO,
            "[withdraw_collateral] Vault #{} has outstanding debt of {} icUSD",
            vault_id,
            vault.borrowed_icusd_amount
        );
        return Err(ProtocolError::GenericError(format!(
            "Vault has {} icUSD debt. You must repay all debt before withdrawing collateral.",
            vault.borrowed_icusd_amount
        )));
    }

    // Check there's collateral to withdraw
    if vault.collateral_amount == 0 {
        log!(
            INFO,
            "[withdraw_collateral] Vault #{} has no collateral to withdraw",
            vault_id
        );
        return Err(ProtocolError::GenericError(
            "No collateral to withdraw".to_string(),
        ));
    }

    // Look up per-collateral config (incl. custody kind for P4 native-XRP routing).
    let (ledger_canister_id, ledger_fee, is_native_xrp) =
        read_state(|s| {
            let config = s.get_collateral_config(&vault.collateral_type).ok_or(
                ProtocolError::GenericError("Collateral type not configured".to_string()),
            )?;
            Ok::<_, ProtocolError>((
                config.ledger_canister_id,
                config.ledger_fee,
                config.is_native_xrp(),
            ))
        })?;

    // Get the amount to transfer
    let amount_to_transfer = ICP::from(vault.collateral_amount);
    log!(
        INFO,
        "[withdraw_collateral] Withdrawing {} from vault #{}",
        amount_to_transfer,
        vault_id
    );

    // P4: native-XRP collateral leaves the vault into an XrpClaim (settled later via
    // settle_xrp_claim, signed from the vault's custody address) instead of an ICRC
    // transfer. Collateral is already zeroed above; the XRPL fee is taken at settle
    // time (claimant-bears-fee), so the full amount becomes the claim.
    if is_native_xrp {
        let now_ns = ic_cdk::api::time();
        let claim_id = mutate_state(|s| {
            if let Some(vault) = s.vault_id_to_vaults.get_mut(&vault_id) {
                vault.collateral_amount = 0;
            }
            s.reindex_vault_cr(vault_id);
            crate::event::record_collateral_withdrawn(s, vault_id, amount_to_transfer, 0);
            record_xrp_claim(
                s,
                caller,
                caller,
                vault_id,
                amount_to_transfer.to_u64(),
                now_ns,
            )
        });
        log!(
            INFO,
            "[withdraw_collateral] vault #{} native-XRP collateral -> XRP claim #{}",
            vault_id,
            claim_id
        );
        return Ok(claim_id);
    }

    let transfer = reserve_collateral_withdrawal(
        &vault,
        caller,
        ledger_canister_id,
        ledger_fee,
        amount_to_transfer.to_u64(),
        crate::state::CollateralWithdrawalAction::Full,
        false,
        None,
    )?;
    dispatch_collateral_withdrawal(vault_id, transfer.operation_id).await
}
pub async fn withdraw_partial_collateral(vault_id: u64, amount: u64) -> Result<u64, ProtocolError> {
    let caller = ic_cdk::caller();
    let _guard_principal = GuardPrincipal::new(caller, &format!("withdraw_partial_{}", vault_id))?;
    // AR-B-003: per-vault op lock. The post-await commit debits the vault by
    // the pre-await withdraw amount; without this lock a concurrent
    // liquidation/redemption could shrink the vault first, leaving phantom
    // collateral on the books after the transfer already paid the owner.
    let _vault_op_guard = VaultLiquidationGuard::new(vault_id)?;
    if let Some(pending) = read_state(|s| s.pending_collateral_withdrawals.get(&vault_id).cloned())
    {
        if pending.owner != caller
            || pending.action != crate::state::CollateralWithdrawalAction::Partial
            || pending.gross_amount_raw != amount
        {
            return Err(ProtocolError::GenericError(
                "Vault has a different unresolved collateral withdrawal; use its recovery route."
                    .into(),
            ));
        }
        return dispatch_collateral_withdrawal(vault_id, pending.operation_id).await;
    }

    let withdraw_amount: ICP = ICP::new(amount);

    // INT-004: accrue interest on this vault before any CR-relevant read so
    // the withdrawal headroom is computed against fresh debt. Mirrors the
    // pattern already used in `borrow_from_vault_internal` and both
    // `repay_to_vault` entry points. `accrue_single_vault` is a no-op when
    // the vault has no debt or when the elapsed window is zero.
    let now = ic_cdk::api::time();
    reject_active_xrp_sp_absorb_preflight(vault_id, now)?;
    mutate_state(|s| s.accrue_single_vault(vault_id, now));

    // Read vault, per-collateral price + config from state
    let (
        vault,
        collateral_price,
        config_decimals,
        ledger_canister_id,
        ledger_fee,
        min_deposit,
        is_native_xrp,
    ) = match read_state(|s| match s.vault_id_to_vaults.get(&vault_id) {
        Some(vault) => {
            let price = s
                .get_collateral_price_decimal(&vault.collateral_type)
                .ok_or("No price available for collateral. Price feed may be down.")?;
            let config = s
                .get_collateral_config(&vault.collateral_type)
                .ok_or("Collateral type not configured.")?;
            Ok((
                vault.clone(),
                price,
                config.decimals,
                config.ledger_canister_id,
                config.ledger_fee,
                config.min_collateral_deposit,
                config.is_native_xrp(),
            ))
        }
        None => Err("Vault not found. Please check the vault ID."),
    }) {
        Ok(result) => result,
        Err(msg) => return Err(ProtocolError::GenericError(msg.to_string())),
    };

    require_vault_not_processing(&vault)?;

    if min_deposit > 0 && withdraw_amount < ICP::new(min_deposit) {
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: min_deposit,
        });
    }

    log!(
        INFO,
        "[withdraw_partial_collateral] Request to withdraw {} from vault #{} by principal {}",
        withdraw_amount,
        vault_id,
        caller
    );

    // Check collateral status allows withdrawal
    let collateral_status = read_state(|s| s.get_collateral_status(&vault.collateral_type));
    if let Some(status) = collateral_status {
        if !status.allows_withdraw() {
            return Err(ProtocolError::GenericError(
                "Withdrawal is not allowed for this collateral type.".to_string(),
            ));
        }
    }

    if caller != vault.owner {
        return Err(ProtocolError::CallerNotOwner);
    }

    let vault_collateral = ICP::from(vault.collateral_amount);

    if vault_collateral == ICP::new(0) {
        return Err(ProtocolError::GenericError(
            "No collateral to withdraw".to_string(),
        ));
    }

    // Forgive dust debt: if remaining debt is below threshold, zero it out
    let has_dust = vault.borrowed_icusd_amount.0 > 0
        && vault.borrowed_icusd_amount.0 <= crate::state::DUST_DEBT_THRESHOLD;
    if has_dust && is_native_xrp {
        log!(
            INFO,
            "[withdraw_partial_collateral] Forgiving dust debt of {} on vault #{}",
            vault.borrowed_icusd_amount,
            vault_id
        );
        mutate_state(|s| {
            if let Some(v) = s.vault_id_to_vaults.get_mut(&vault_id) {
                v.borrowed_icusd_amount = ICUSD::new(0);
                v.accrued_interest = ICUSD::new(0);
            }
            // Wave-8b LIQ-002: dust forgiveness changes debt → re-key.
            s.reindex_vault_cr(vault_id);
        });
    }

    // Calculate max withdrawable amount that keeps CR >= minimum
    let max_withdrawable = if vault.borrowed_icusd_amount == ICUSD::new(0) || has_dust {
        // No debt (or dust forgiven) — can withdraw everything
        vault_collateral
    } else {
        // min_collateral_value = debt * min_ratio
        // min_collateral_amount = icusd_to_collateral_amount(min_collateral_value, price, decimals)
        // max_withdrawable = current_collateral - min_collateral_amount
        let min_ratio = read_state(|s| {
            let base = s.get_min_collateral_ratio_for(&vault.collateral_type);
            if s.mode == Mode::Recovery {
                let recovery_cr = s.get_recovery_cr_for(&vault.collateral_type);
                if recovery_cr > base {
                    recovery_cr
                } else {
                    base
                }
            } else {
                base
            }
        });
        let min_collateral_value: ICUSD = vault.borrowed_icusd_amount * min_ratio;
        let min_collateral_raw = crate::numeric::try_icusd_to_collateral_amount(
            min_collateral_value,
            collateral_price,
            config_decimals,
        )
        .ok_or_else(|| {
            ProtocolError::GenericError(
                "Cannot safely calculate the minimum collateral required for this debt."
                    .to_string(),
            )
        })?;
        let min_collateral = ICP::from(min_collateral_raw);

        if vault_collateral <= min_collateral {
            return Err(ProtocolError::GenericError(
                "No excess collateral to withdraw. Your vault is already at or below the minimum collateral ratio.".to_string()
            ));
        }

        vault_collateral - min_collateral
    };

    if withdraw_amount > max_withdrawable {
        return Err(ProtocolError::GenericError(format!(
            "Withdrawal amount exceeds maximum. Max withdrawable: {} (keeps CR above minimum).",
            max_withdrawable
        )));
    }

    log!(
        INFO,
        "[withdraw_partial_collateral] Max withdrawable: {}, requested: {} from vault #{}",
        max_withdrawable,
        withdraw_amount,
        vault_id
    );

    // Note: margin is reduced in record_partial_collateral_withdrawn (via remove_margin_from_vault)
    // after the transfer succeeds. Do NOT also subtract here — that would double-deduct.

    // P4: native-XRP collateral leaves into an XrpClaim instead of an ICRC transfer.
    // Reduce the vault collateral (same as the ICRC success path) and record the
    // claim; the full withdraw_amount becomes the claim (XRPL fee taken at settle).
    if is_native_xrp {
        let now_ns = ic_cdk::api::time();
        let claim_id = mutate_state(|s| {
            crate::event::record_partial_collateral_withdrawn(s, vault_id, withdraw_amount, 0);
            record_xrp_claim(
                s,
                caller,
                caller,
                vault_id,
                withdraw_amount.to_u64(),
                now_ns,
            )
        });
        log!(
            INFO,
            "[withdraw_partial_collateral] vault #{} native-XRP collateral -> XRP claim #{}",
            vault_id,
            claim_id
        );
        return Ok(claim_id);
    }

    let transfer = reserve_collateral_withdrawal(
        &vault,
        caller,
        ledger_canister_id,
        ledger_fee,
        withdraw_amount.to_u64(),
        crate::state::CollateralWithdrawalAction::Partial,
        has_dust,
        None,
    )?;
    dispatch_collateral_withdrawal(vault_id, transfer.operation_id).await
}

/// Internal withdraw-collateral-and-close logic without guard management.
///
/// Called by both `withdraw_and_close_vault` (which acquires its own
/// `withdraw_and_close_{id}` guard) and `repay_and_close_vault` (which holds
/// a single `repay_and_close_{id}` guard spanning repay + withdraw + close).
///
/// Forgives eligible dust debt and validates collateral status. ICRC
/// collateral is reserved in the durable exact-tuple outbox and debited/closed
/// only after receipt verification; native-XRP collateral creates a claim and
/// leaves the vault open because the XRPL account reserve stays locked.
async fn withdraw_and_close_vault_internal(
    caller: Principal,
    vault_id: u64,
    repay_block_index: Option<u64>,
    repayment_v2_request_id: Option<u128>,
) -> Result<Option<u64>, ProtocolError> {
    if let Some(pending) = read_state(|s| s.pending_collateral_withdrawals.get(&vault_id).cloned())
    {
        if pending.owner != caller
            || pending.action != crate::state::CollateralWithdrawalAction::Close
            || repayment_v2_request_id.is_some() && pending.repay_block_index != repay_block_index
        {
            return Err(ProtocolError::GenericError(
                "Vault has a different unresolved collateral withdrawal or repayment receipt; use its exact recovery route."
                    .into(),
            ));
        }
        return dispatch_collateral_withdrawal(vault_id, pending.operation_id)
            .await
            .map(Some);
    }
    log!(
        INFO,
        "[withdraw_and_close] Request for vault #{} by principal {}",
        vault_id,
        caller
    );
    reject_active_xrp_sp_absorb_preflight(vault_id, ic_cdk::api::time())?;

    // Check if the vault exists first
    let vault = read_state(|s| {
        s.vault_id_to_vaults
            .get(&vault_id)
            .cloned()
            .ok_or(ProtocolError::GenericError(format!(
                "Vault #{} not found",
                vault_id
            )))
    })?;

    require_vault_not_processing_except(
        &vault,
        repayment_v2_request_id.map(|request_id| (caller, request_id)),
        None,
    )?;

    // Check collateral status allows withdraw + close
    let collateral_status = read_state(|s| s.get_collateral_status(&vault.collateral_type));
    if let Some(status) = collateral_status {
        if !status.allows_withdraw() || !status.allows_close() {
            return Err(ProtocolError::GenericError(
                "Withdraw-and-close is not allowed for this collateral type.".to_string(),
            ));
        }
    }

    // Verify caller is the owner
    if caller != vault.owner {
        log!(
            INFO,
            "[withdraw_and_close] Principal {} is not the owner of vault #{}",
            caller,
            vault_id
        );
        return Err(ProtocolError::CallerNotOwner);
    }

    // Look up the collateral route before deciding whether dust forgiveness
    // can be applied synchronously (native XRP claim) or must wait for the
    // receipt-backed ICRC settlement.
    let (ledger_canister_id, ledger_fee, is_native_xrp) =
        read_state(|s| {
            let config = s.get_collateral_config(&vault.collateral_type).ok_or(
                ProtocolError::GenericError("Collateral type not configured".to_string()),
            )?;
            Ok::<_, ProtocolError>((
                config.ledger_canister_id,
                config.ledger_fee,
                config.is_native_xrp(),
            ))
        })?;

    // Forgive dust debt before checking
    let forgive_dust_debt = vault.borrowed_icusd_amount.0 > 0
        && vault.borrowed_icusd_amount.0 <= crate::state::DUST_DEBT_THRESHOLD;
    if forgive_dust_debt && (is_native_xrp || vault.collateral_amount == 0) {
        log!(
            INFO,
            "[withdraw_and_close] Forgiving dust debt of {} on vault #{}",
            vault.borrowed_icusd_amount,
            vault_id
        );
        mutate_state(|s| {
            if let Some(v) = s.vault_id_to_vaults.get_mut(&vault_id) {
                v.borrowed_icusd_amount = ICUSD::new(0);
                v.accrued_interest = ICUSD::new(0);
            }
            // Wave-8b LIQ-002: dust forgiveness changes debt → re-key.
            s.reindex_vault_cr(vault_id);
        });
    } else if vault.borrowed_icusd_amount > ICUSD::new(0) && !forgive_dust_debt {
        log!(
            INFO,
            "[withdraw_and_close] Vault #{} has outstanding debt of {} icUSD",
            vault_id,
            vault.borrowed_icusd_amount
        );
        return Err(ProtocolError::GenericError(format!(
            "Cannot close vault while it has outstanding debt of {} icUSD. Please repay all debt first.",
            vault.borrowed_icusd_amount
        )));
    }

    // If there's collateral, withdraw it first
    let mut block_index: Option<u64> = None;
    let amount_to_transfer = ICP::from(vault.collateral_amount);

    if amount_to_transfer > ICP::new(0) {
        log!(
            INFO,
            "[withdraw_and_close] Withdrawing {} from vault #{}",
            amount_to_transfer,
            vault_id
        );

        // P4: native-XRP collateral leaves into an XrpClaim, not an ICRC transfer.
        if is_native_xrp {
            let now_ns = ic_cdk::api::time();
            let claim_id = mutate_state(|s| {
                if let Some(vault) = s.vault_id_to_vaults.get_mut(&vault_id) {
                    vault.collateral_amount = 0;
                }
                s.reindex_vault_cr(vault_id);
                crate::event::record_collateral_withdrawn(s, vault_id, amount_to_transfer, 0);
                record_xrp_claim(
                    s,
                    caller,
                    caller,
                    vault_id,
                    amount_to_transfer.to_u64(),
                    now_ns,
                )
            });
            log!(
                INFO,
                "[withdraw_and_close] vault #{} native-XRP collateral -> XRP claim #{}",
                vault_id,
                claim_id
            );
            block_index = Some(claim_id);
        } else {
            let transfer = reserve_collateral_withdrawal(
                &vault,
                caller,
                ledger_canister_id,
                ledger_fee,
                amount_to_transfer.to_u64(),
                crate::state::CollateralWithdrawalAction::Close,
                forgive_dust_debt,
                repay_block_index,
            )?;
            return dispatch_collateral_withdrawal(vault_id, transfer.operation_id)
                .await
                .map(Some);
        } // end native-XRP `else` (the ICRC transfer path)
    } else {
        log!(
            INFO,
            "[withdraw_and_close] Vault #{} has no collateral to withdraw",
            vault_id
        );
    };

    if withdraw_close_completion_policy(is_native_xrp)
        == WithdrawCloseCompletionPolicy::KeepNativeXrpVaultOpen
    {
        log!(
            INFO,
            "[withdraw_and_close] Keeping native-XRP vault #{} open because the XRPL reserve remains locked",
            vault_id
        );
        return Ok(block_index);
    }

    // Now close the vault - only if we've successfully transferred any funds
    // or if there were no funds to transfer
    mutate_state(|s| {
        // Make sure vault exists before attempting to remove
        if s.vault_id_to_vaults.contains_key(&vault_id) {
            // Record the combined withdraw and close event
            crate::event::record_withdraw_and_close_vault(
                s,
                vault_id,
                amount_to_transfer,
                block_index,
            );

            log!(
                INFO,
                "[withdraw_and_close] Successfully closed vault #{} for principal {}",
                vault_id,
                caller
            );
        } else {
            // Log that we tried to close a vault that was already gone
            log!(
                INFO,
                "[withdraw_and_close] Attempted to close vault #{} that was already removed",
                vault_id
            );
        }
    });

    // Return the block index if we did a transfer, otherwise None
    Ok(block_index)
}

pub async fn withdraw_and_close_vault(vault_id: u64) -> Result<Option<u64>, ProtocolError> {
    let caller = ic_cdk::caller();
    // Use a specific name for better tracking
    let _guard_principal =
        GuardPrincipal::new(caller, &format!("withdraw_and_close_{}", vault_id))?;
    // AR-B-003: per-vault op lock; see guard.rs::VaultLiquidationGuard.
    let _vault_op_guard = VaultLiquidationGuard::new(vault_id)?;

    withdraw_and_close_vault_internal(caller, vault_id, None, None).await
}

/// Compound repay + withdraw + close in a single canister call.
///
/// Pulls icUSD via `icrc2_transfer_from` to zero the vault's debt, then
/// withdraws all collateral and deletes the vault — all under a single
/// `repay_and_close_{vault_id}` guard. This lets Oisy / ICRC-49 signer
/// wallets close a borrowed vault with 2 consent screens (approve + this
/// call) instead of 4 (approve + repay + approve + withdraw_and_close)
/// when calling the separate methods sequentially.
///
/// `arg.amount` is the icUSD amount to repay. Per `repay_to_vault_internal`,
/// the amount is capped to actual debt and snaps to full-repayment if within
/// 1% / 0.01 icUSD dust. If repay leaves any debt, the close phase fails
/// and the vault stays open (but the partial repay is preserved on-chain).
///
/// Returns the icUSD repay block index and the optional collateral-return
/// block index (None if the vault had no collateral, e.g. due to liquidation).
#[derive(candid::CandidType, candid::Deserialize, Clone, Debug)]
pub struct RepayAndCloseSuccess {
    pub repay_block_index: u64,
    pub collateral_return_block_index: Option<u64>,
}

pub async fn repay_and_close_vault(arg: VaultArg) -> Result<RepayAndCloseSuccess, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let vault_id = arg.vault_id;
    let guard_principal = GuardPrincipal::new(caller, &format!("repay_and_close_{}", vault_id))?;
    // AR-B-003: per-vault op lock spanning repay + withdraw + close.
    let _vault_op_guard = match VaultLiquidationGuard::new(vault_id) {
        Ok(g) => g,
        Err(e) => {
            guard_principal.fail();
            return Err(e);
        }
    };

    if let Some(pending) = read_state(|s| s.pending_collateral_withdrawals.get(&vault_id).cloned())
    {
        if pending.owner != caller
            || pending.action != crate::state::CollateralWithdrawalAction::Close
            || pending.repay_block_index.is_none()
        {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(
                "Vault has a different unresolved collateral withdrawal; recover it before repaying or closing.".into(),
            ));
        }
        let repay_block_index = pending.repay_block_index.unwrap_or_default();
        return match dispatch_collateral_withdrawal(vault_id, pending.operation_id).await {
            Ok(collateral_return_block_index) => {
                guard_principal.complete();
                Ok(RepayAndCloseSuccess {
                    repay_block_index,
                    collateral_return_block_index: Some(collateral_return_block_index),
                })
            }
            Err(error) => {
                guard_principal.fail();
                Err(error)
            }
        };
    }

    // Phase 1: repay. On failure the guard fails and we propagate the error —
    // no collateral movement attempted. `is_full_close=true` lets vaults stuck
    // in the (DUST_DEBT_THRESHOLD, MIN_ICUSD_AMOUNT) zone clear their debt
    // here, since the close phase below will zero the vault entirely.
    let repay_block_index = match repay_to_vault_internal(caller, arg, true).await {
        Ok(idx) => idx,
        Err(e) => {
            guard_principal.fail();
            return Err(e);
        }
    };

    // Phase 2: withdraw + close. If this fails (e.g. collateral transfer
    // bounces), the repay is already on-chain — the vault stays open with
    // debt=0 and full collateral, recoverable via the existing
    // `withdraw_and_close_vault` endpoint. Surface a descriptive error.
    match withdraw_and_close_vault_internal(caller, vault_id, Some(repay_block_index), None).await {
        Ok(collateral_return_block_index) => {
            guard_principal.complete();
            Ok(RepayAndCloseSuccess {
                repay_block_index,
                collateral_return_block_index,
            })
        }
        Err(e) => {
            guard_principal.fail();
            log!(
                INFO,
                "[repay_and_close_vault] Repay succeeded (block {}) but withdraw/close failed for vault #{}: {:?}. Vault is recoverable via withdraw_and_close_vault.",
                repay_block_index,
                vault_id,
                e
            );
            Err(e)
        }
    }
}

/// Execute or resume one approval-backed Stability Pool liquidation. The
/// request ID and all ledger tuples are persisted before dispatch; callers
/// retry this exact request after reply loss.
pub async fn stability_pool_liquidate_v2(
    request: crate::SpLiquidationV2Request,
) -> Result<crate::SpLiquidationV2StatusView, ProtocolError> {
    if !crate::SP_LIQUIDATION_V2_ENABLED {
        return Err(ProtocolError::TemporarilyUnavailable(
            "Stability Pool liquidation V2 is disabled pending coordinated release gates".into(),
        ));
    }
    let pool = ic_cdk::api::caller();
    let _ = stability_pool_liquidate_v2_inner(request.clone(), pool, true).await;
    let request_id = request.request_id;
    let (row, acknowledged) = read_state(|s| {
        (
            s.sp_liquidation_v2_journals
                .get(&(pool, request_id))
                .cloned(),
            s.sp_liquidation_v2_acknowledged_through
                .get(&pool)
                .is_some_and(|floor| request_id <= *floor),
        )
    });
    match row {
        Some(row) => Ok(crate::SpLiquidationV2StatusView {
            stability_pool: pool,
            request_id,
            request: (!matches!(row.status, crate::SpLiquidationV2Status::Acknowledged))
                .then_some(row.request),
            status: row.status,
        }),
        None if acknowledged => Ok(crate::SpLiquidationV2StatusView {
            stability_pool: pool,
            request_id,
            request: None,
            status: crate::SpLiquidationV2Status::Acknowledged,
        }),
        None => Err(ProtocolError::GenericError(
            "V2 request did not produce a durable status row".into(),
        )),
    }
}

fn sp_liquidation_v2_approval_fee_mint_refund(
    request: &crate::SpLiquidationV2Request,
    ledger: Principal,
    pool: Principal,
    backend: Principal,
) -> crate::SpLiquidationStableRefundTuple {
    let op_nonce = mutate_state(|s| s.next_op_nonce());
    crate::SpLiquidationStableRefundTuple {
        op_nonce,
        ledger,
        source: icrc_ledger_types::icrc1::account::Account {
            owner: backend,
            subaccount: None,
        },
        destination: icrc_ledger_types::icrc1::account::Account {
            owner: pool,
            subaccount: None,
        },
        principal_refund_raw: 0,
        approval_fee_refund_raw: request.approval.tuple.fee_raw,
        pull_fee_refund_raw: 0,
        amount_raw: request.approval.tuple.fee_raw,
        fee_raw: 0,
        memo: [
            b"RUMI-SP-LIQ-V2-MINT:".as_slice(),
            &request.request_id.to_be_bytes(),
            &op_nonce.to_be_bytes(),
        ]
        .concat(),
        created_at_time_ns: ic_cdk::api::time(),
    }
}

fn sp_liquidation_v2_payout_successor(
    predecessor: &crate::SpLiquidationPayoutTuple,
    evidence: &crate::SpLiquidationPayoutNoEffectEvidence,
) -> Result<crate::SpLiquidationPayoutTuple, String> {
    let fee_raw = match evidence {
        crate::SpLiquidationPayoutNoEffectEvidence::BadFee { expected_fee_raw } => {
            *expected_fee_raw
        }
        crate::SpLiquidationPayoutNoEffectEvidence::InsufficientFunds { .. } => predecessor.fee_raw,
    };
    let net_amount_raw = predecessor
        .gross_amount_raw
        .checked_sub(fee_raw)
        .filter(|net| *net > 0)
        .ok_or_else(|| "typed no-effect evidence leaves no positive-net payout".to_string())?;
    if matches!(
        evidence,
        crate::SpLiquidationPayoutNoEffectEvidence::InsufficientFunds { .. }
    ) && (fee_raw != predecessor.fee_raw || net_amount_raw != predecessor.net_amount_raw)
    {
        return Err(
            "InsufficientFunds successor must preserve the exact fee and net amount".into(),
        );
    }
    let op_nonce = mutate_state(|s| s.next_op_nonce());
    if op_nonce <= predecessor.op_nonce {
        return Err("payout successor nonce did not advance".into());
    }
    Ok(crate::SpLiquidationPayoutTuple {
        op_nonce,
        ledger: predecessor.ledger,
        source: predecessor.source.clone(),
        destination: predecessor.destination.clone(),
        gross_amount_raw: predecessor.gross_amount_raw,
        net_amount_raw,
        fee_raw,
        memo: management::nonce_to_memo(op_nonce).0.to_vec(),
        created_at_time_ns: management::nonce_to_created_at_time(op_nonce),
        collateral_type: predecessor.collateral_type,
    })
}

async fn sp_legacy_liquidation_in_flight(pool: Principal, vault_id: u64) -> Result<bool, String> {
    let response: Result<(Result<bool, crate::SpLegacyFenceError>,), _> =
        ic_cdk::call(pool, "has_legacy_liquidation_in_flight", (vault_id,)).await;
    let (result,) = response.map_err(|(code, message)| {
        format!("legacy liquidation fence call rejected ({code:?}): {message}")
    })?;
    result.map_err(|error| format!("legacy liquidation fence returned an error: {error:?}"))
}

async fn stability_pool_liquidate_v2_inner(
    request: crate::SpLiquidationV2Request,
    pool: Principal,
    enforce_caller: bool,
) -> Result<SuccessWithFee, ProtocolError> {
    use crate::management::{ExactCollateralTransferOutcome, ExactTransferFromOutcome};
    use crate::SpLiquidationV2Status as Status;
    use icrc_ledger_types::icrc1::account::Account;

    if !crate::SP_LIQUIDATION_V2_ENABLED {
        return Err(ProtocolError::TemporarilyUnavailable(
            "Stability Pool liquidation V2 is disabled pending coordinated release gates".into(),
        ));
    }

    let backend = ic_cdk::id();
    if (enforce_caller && pool != ic_cdk::api::caller())
        || pool == Principal::anonymous()
        || !read_state(|s| s.stability_pool_canister == Some(pool))
    {
        return Err(ProtocolError::GenericError(
            "Caller is not the registered stability pool canister".into(),
        ));
    }
    let _vault_liq_guard = VaultLiquidationGuard::new(request.vault_id)?;
    if let Some(row) = read_state(|s| {
        s.sp_liquidation_v2_journals
            .get(&(pool, request.request_id))
            .cloned()
    }) {
        if row.request != request {
            return Err(ProtocolError::GenericError(
                "request ID is already bound to a different liquidation payload".into(),
            ));
        }
        match row.status {
            Status::Complete { result, .. } => return Ok(result),
            Status::StablePullRefunded { .. } | Status::Rejected { .. } | Status::Acknowledged => {
                return Err(ProtocolError::GenericError(
                    "SP liquidation request already reached a terminal non-success status".into(),
                ));
            }
            Status::StablePullRefundPending {
                stable_pull_receipt,
                tuple,
                candidate_block_index,
                ..
            } => {
                if read_state(|s| s.icusd_ledger_principal != tuple.ledger)
                    || crate::sp_burn_refund::verify_mint_authority(tuple.ledger)
                        .await
                        .is_err()
                {
                    return Err(ProtocolError::TemporarilyUnavailable(
                        "icUSD mint refund is held until the pinned ledger/minting account can be verified".into(),
                    ));
                }
                let still_pinned = read_state(|s| {
                    s.sp_liquidation_v2_journals
                        .get(&(pool, request.request_id))
                        .is_some_and(|current| {
                            current.request == request
                                && matches!(
                                    &current.status,
                                    Status::StablePullRefundPending { tuple: current_tuple, .. }
                                        if current_tuple == &tuple
                                )
                        })
                });
                if !still_pinned {
                    return Err(ProtocolError::TemporarilyUnavailable(
                        "icUSD mint refund journal changed during authority verification".into(),
                    ));
                }
                let block = match candidate_block_index {
                    Some(block) => Some(block),
                    None => match management::transfer_sp_liquidation_refund(&tuple).await {
                        ExactCollateralTransferOutcome::Applied(block) => Some(block),
                        ExactCollateralTransferOutcome::ProvenNoEffect(error) => {
                            mutate_state(|s| {
                                let _ = crate::state::set_sp_liquidation_v2_status(
                                    s,
                                    pool,
                                    request.request_id,
                                    Status::StablePullRefundPending {
                                        stable_pull_receipt: stable_pull_receipt.clone(),
                                        tuple: tuple.clone(),
                                        candidate_block_index: None,
                                        last_error: Some(format!(
                                            "refund proven no-effect: {error:?}"
                                        )),
                                    },
                                );
                            });
                            return Err(ProtocolError::GenericError(
                                "SP liquidation refund was rejected without effect; exact tuple remains held for reconciliation".into(),
                            ));
                        }
                        ExactCollateralTransferOutcome::CallRejected { code, message } => {
                            mutate_state(|s| {
                                let _ = crate::state::set_sp_liquidation_v2_status(
                                    s,
                                    pool,
                                    request.request_id,
                                    Status::StablePullRefundPending {
                                        stable_pull_receipt: stable_pull_receipt.clone(),
                                        tuple: tuple.clone(),
                                        candidate_block_index: None,
                                        last_error: Some(format!(
                                            "refund call rejected {code}: {message}"
                                        )),
                                    },
                                );
                            });
                            return Err(ProtocolError::GenericError(
                                "SP liquidation refund outcome is ambiguous; exact tuple remains pending".into(),
                            ));
                        }
                        ExactCollateralTransferOutcome::InvalidBlockIndex => None,
                        ExactCollateralTransferOutcome::LedgerError(error) => {
                            return Err(ProtocolError::GenericError(format!(
                                "SP liquidation refund remains pending: {error:?}"
                            )))
                        }
                    },
                };
                let Some(block) = block else {
                    return Err(ProtocolError::GenericError(
                        "SP liquidation refund reply did not contain a usable receipt index".into(),
                    ));
                };
                mutate_state(|s| {
                    let _ = crate::state::set_sp_liquidation_v2_status(
                        s,
                        pool,
                        request.request_id,
                        Status::StablePullRefundPending {
                            stable_pull_receipt: stable_pull_receipt.clone(),
                            tuple: tuple.clone(),
                            candidate_block_index: Some(block),
                            last_error: None,
                        },
                    );
                });
                if let Err(error) =
                    crate::icrc3_proof::verify_sp_liquidation_refund_block(&tuple, block).await
                {
                    return Err(ProtocolError::GenericError(format!(
                        "SP refund receipt is not yet verifiable; exact refund remains pending: {error}"
                    )));
                }
                mutate_state(|s| {
                    let _ = crate::state::set_sp_liquidation_v2_status(
                        s,
                        pool,
                        request.request_id,
                        Status::StablePullRefunded {
                            stable_pull_receipt,
                            refund_receipt: crate::SpLiquidationStableRefundReceipt {
                                block_index: block,
                                tuple,
                            },
                            reason: "pre-pull rejection fee refund confirmed".into(),
                        },
                    );
                });
                return Err(ProtocolError::GenericError(
                    "SP liquidation was safely rejected and its approval fee was refunded".into(),
                ));
            }
            Status::StablePullPending { .. }
            | Status::CollateralPayoutPending { .. }
            | Status::CollateralPayoutSupersessionPending { .. } => {}
            Status::Unseen => unreachable!("journal rows are never stored as Unseen"),
        }
    } else {
        if request.request_id == 0 || request.amount == 0 {
            return Err(ProtocolError::GenericError(
                "request ID and liquidation amount must be nonzero".into(),
            ));
        }
        // Approval proof is an input to admission, not an assertion by the SP.
        if let Err(error) = crate::icrc3_proof::verify_icrc3_approval_block(&request.approval).await
        {
            return Err(ProtocolError::GenericError(format!(
                "SP approval receipt does not prove its exact ledger tuple: {error}"
            )));
        }
        if !read_state(|s| s.stability_pool_canister == Some(pool)) {
            return Err(ProtocolError::GenericError(
                "Stability Pool registration changed during approval verification".into(),
            ));
        }
        if request.approval.tuple.owner.owner != pool
            || request.approval.tuple.spender.owner != backend
            || request.approval.tuple.owner.subaccount.is_some()
            || request.approval.tuple.spender.subaccount.is_some()
        {
            return Err(ProtocolError::GenericError(
                "approval receipt does not bind the default SP and backend accounts".into(),
            ));
        }

        // An approval may expire while its exact block is being proved. The
        // approval still has an economic effect (its ledger fee), so journal
        // the fee-only mint refund instead of returning without a durable
        // recovery row.
        if crate::state::sp_liquidation_v2_approval_expired(
            ic_cdk::api::time(),
            request.approval.tuple.expires_at_ns,
        ) {
            let ledger = request.approval.tuple.ledger;
            let refund_tuple =
                sp_liquidation_v2_approval_fee_mint_refund(&request, ledger, pool, backend);
            mutate_state(|s| {
                crate::state::admit_sp_liquidation_v2_refund(
                    s,
                    pool,
                    request.clone(),
                    refund_tuple,
                    "proven SP approval expired before backend admission".into(),
                )
            })
            .map_err(ProtocolError::GenericError)?;
            schedule_stability_pool_liquidation_v2_resume();
            return Err(ProtocolError::TemporarilyUnavailable(
                "SP approval expired before admission; exact approval-fee refund is pending".into(),
            ));
        }

        // CK stable principal inversion is deliberately not guessed: this
        // narrow V2 executor currently accepts only the IcUSD route. Refund
        // the already-proven approval fee before surfacing a terminal reject.
        if request.token != crate::SpLiquidationToken::IcUsd {
            let refund_tuple = crate::SpLiquidationStableRefundTuple {
                op_nonce: mutate_state(|s| s.next_op_nonce()),
                ledger: request.approval.tuple.ledger,
                source: Account {
                    owner: backend,
                    subaccount: None,
                },
                destination: Account {
                    owner: pool,
                    subaccount: None,
                },
                principal_refund_raw: 0,
                approval_fee_refund_raw: request.approval.tuple.fee_raw,
                pull_fee_refund_raw: 0,
                amount_raw: request.approval.tuple.fee_raw,
                fee_raw: 0,
                memo: b"RUMI-SP-LIQ-V2-REFUND".to_vec(),
                created_at_time_ns: ic_cdk::api::time(),
            };
            mutate_state(|s| {
                crate::state::admit_sp_liquidation_v2_refund(
                    s,
                    pool,
                    request.clone(),
                    refund_tuple,
                    "unsupported liquidation token; approval fee refund required".into(),
                )
            })
            .map_err(ProtocolError::GenericError)?;
            return Err(ProtocolError::GenericError(
                "CK stable V2 route is not enabled; exact approval-fee refund is pending".into(),
            ));
        }

        let (ledger, icusd_ledger, config_ok) = read_state(|s| {
            (
                s.icusd_ledger_principal,
                s.icusd_ledger_principal,
                !s.frozen && !s.liquidation_frozen && !s.sp_writedown_disabled,
            )
        });
        if request.approval.tuple.ledger != icusd_ledger || !config_ok {
            let refund_tuple = crate::SpLiquidationStableRefundTuple {
                op_nonce: mutate_state(|s| s.next_op_nonce()),
                ledger: request.approval.tuple.ledger,
                source: Account {
                    owner: backend,
                    subaccount: None,
                },
                destination: Account {
                    owner: pool,
                    subaccount: None,
                },
                principal_refund_raw: 0,
                approval_fee_refund_raw: request.approval.tuple.fee_raw,
                pull_fee_refund_raw: 0,
                amount_raw: request.approval.tuple.fee_raw,
                fee_raw: 0,
                memo: b"RUMI-SP-LIQ-V2-REFUND".to_vec(),
                created_at_time_ns: ic_cdk::api::time(),
            };
            mutate_state(|s| {
                crate::state::admit_sp_liquidation_v2_refund(
                    s,
                    pool,
                    request.clone(),
                    refund_tuple,
                    "IcUSD ledger mismatch or liquidation disabled".into(),
                )
            })
            .map_err(ProtocolError::GenericError)?;
            return Err(ProtocolError::GenericError(
                "liquidation preflight rejected; exact approval-fee refund is pending".into(),
            ));
        }

        match sp_legacy_liquidation_in_flight(pool, request.vault_id).await {
            Ok(false) => {}
            result => {
                let reason = match result {
                    Ok(true) => "legacy SP liquidation marker is still present".to_string(),
                    Ok(false) => "legacy SP liquidation marker unexpectedly changed".to_string(),
                    Err(error) => error,
                };
                let refund_tuple =
                    sp_liquidation_v2_approval_fee_mint_refund(&request, ledger, pool, backend);
                mutate_state(|s| {
                    crate::state::admit_sp_liquidation_v2_refund(
                        s,
                        pool,
                        request.clone(),
                        refund_tuple,
                        reason.clone(),
                    )
                })
                .map_err(ProtocolError::GenericError)?;
                schedule_stability_pool_liquidation_v2_resume();
                return Err(ProtocolError::TemporarilyUnavailable(format!(
                    "legacy SP liquidation fence is active or unavailable; approval-fee mint refund is pending: {reason}"
                )));
            }
        }

        // Source config is not evidence that this ledger still burns into the
        // backend account. Confirm the live minting authority before pinning
        // the account or admitting any pull. A failed query is recoverable and
        // still requires the SP's proven approval fee to be reimbursed.
        if let Err(error) = crate::sp_burn_refund::verify_mint_authority(ledger).await {
            let refund_tuple =
                sp_liquidation_v2_approval_fee_mint_refund(&request, ledger, pool, backend);
            mutate_state(|s| {
                crate::state::admit_sp_liquidation_v2_refund(
                    s,
                    pool,
                    request.clone(),
                    refund_tuple,
                    format!("icUSD minter authority could not be verified: {error:?}"),
                )
            })
            .map_err(ProtocolError::GenericError)?;
            schedule_stability_pool_liquidation_v2_resume();
            return Err(ProtocolError::TemporarilyUnavailable(
                "icUSD minter authority could not be verified; approval-fee mint refund is pending"
                    .into(),
            ));
        }

        // An ICRC-2 pull to the verified minting account is a burn and is
        // fee-free under ICRC-1. The prior approval fee is independently
        // proven and reimbursed through the mint refund outbox if needed.
        let current_fee = 0u64;
        let plan = read_state(
            |s| -> Result<crate::state::SpLiquidationV2PinnedPlan, String> {
                if s.frozen || s.liquidation_frozen || s.sp_writedown_disabled {
                    return Err("Stability Pool liquidations are disabled".into());
                }
                let vault = s
                    .vault_id_to_vaults
                    .get(&request.vault_id)
                    .ok_or_else(|| format!("Vault #{} not found", request.vault_id))?;
                if vault_is_native_xrp(request.vault_id) {
                    return Err(
                        "native-XRP collateral requires the dedicated claim settlement route"
                            .into(),
                    );
                }
                if !s
                    .get_collateral_status(&vault.collateral_type)
                    .is_none_or(|status| status.allows_liquidation())
                {
                    return Err("Liquidation is not allowed for this collateral type".into());
                }
                let price = s
                    .get_collateral_price_decimal(&vault.collateral_type)
                    .ok_or_else(|| "No price available for collateral".to_string())?;
                let collateral_config = s
                    .get_collateral_config(&vault.collateral_type)
                    .ok_or_else(|| "collateral has no configured ICRC ledger route".to_string())?;
                if collateral_config.ledger_canister_id != vault.collateral_type {
                    return Err(
                        "collateral ICRC ledger does not match the vault collateral type".into(),
                    );
                }
                let collateral_ledger = collateral_config.ledger_canister_id;
                let collateral_ledger_fee_raw = collateral_config.ledger_fee;
                let decimals = collateral_config.decimals;
                let price_usd = UsdIcp::from(price);
                if compute_collateral_ratio(vault, price_usd, s)
                    >= s.get_min_liquidation_ratio_for(&vault.collateral_type)
                {
                    return Err("Vault is not liquidatable at the current collateral ratio".into());
                }
                let debt = s.effective_liquidation_amount(
                    vault,
                    price_usd,
                    Some(ICUSD::new(request.amount)),
                );
                if debt.0 == 0 || debt.0 > request.amount {
                    return Err("liquidation amount is zero or exceeds the SP principal cap".into());
                }
                if debt < s.min_icusd_amount && debt != vault.borrowed_icusd_amount {
                    return Err("liquidation amount is below the minimum debt".into());
                }
                let collateral_raw =
                    crate::numeric::try_icusd_to_collateral_amount(debt, price, decimals)
                        .ok_or_else(|| {
                            "liquidation collateral conversion is out of range".to_string()
                        })?;
                let seized = (ICP::from(collateral_raw)
                    * s.get_liquidation_bonus_for(&vault.collateral_type))
                .min(ICP::from(vault.collateral_amount))
                .to_u64();
                let protocol_cut =
                    (rust_decimal::Decimal::from(seized.saturating_sub(collateral_raw))
                        * s.get_liquidation_protocol_share().0)
                        .to_u64()
                        .unwrap_or(0);
                let collateral_to_liquidator = seized.saturating_sub(protocol_cut);
                if collateral_to_liquidator == 0 {
                    return Err("liquidation would produce no collateral payout".into());
                }
                // The ledger debits this fee from the transfer amount. Reject a
                // dust payout before admitting the stable pull so that commit
                // cannot strand a burned principal behind an impossible payout.
                if !crate::state::sp_liquidation_v2_payout_covers_fee(
                    collateral_to_liquidator,
                    collateral_ledger_fee_raw,
                ) {
                    return Err(
                        "liquidation collateral payout does not cover the pinned ledger fee".into(),
                    );
                }
                let interest = crate::numeric::proportional_interest_share(
                    debt.0,
                    vault.accrued_interest.0,
                    vault.borrowed_icusd_amount.0,
                );
                Ok(crate::state::SpLiquidationV2PinnedPlan {
                    vault: vault.clone(),
                    collateral_price: price,
                    collateral_decimals: decimals,
                    collateral_ledger,
                    collateral_ledger_fee_raw,
                    mode: s.mode,
                    liquidation_ratio: s.get_min_liquidation_ratio_for(&vault.collateral_type),
                    liquidation_bonus: s.get_liquidation_bonus_for(&vault.collateral_type),
                    protocol_share: s.get_liquidation_protocol_share(),
                    collateral_status: s.get_collateral_status(&vault.collateral_type),
                    stable_token_ledger: ledger,
                    stable_token_minting_account: Some(Account {
                        owner: backend,
                        subaccount: None,
                    }),
                    stable_token_enabled: true,
                    stable_repay_fee: crate::numeric::Ratio::new(rust_decimal::Decimal::ZERO),
                    debt_liquidated_e8s: debt.0,
                    collateral_to_liquidator_raw: collateral_to_liquidator,
                    collateral_to_seize_raw: seized,
                    protocol_cut_raw: protocol_cut,
                    interest_share_e8s: interest,
                    pinned_at_ns: ic_cdk::api::time(),
                })
            },
        );
        let plan = match plan {
            Ok(plan) => plan,
            Err(reason) => {
                let refund_tuple = crate::SpLiquidationStableRefundTuple {
                    op_nonce: mutate_state(|s| s.next_op_nonce()),
                    ledger,
                    source: Account {
                        owner: backend,
                        subaccount: None,
                    },
                    destination: Account {
                        owner: pool,
                        subaccount: None,
                    },
                    principal_refund_raw: 0,
                    approval_fee_refund_raw: request.approval.tuple.fee_raw,
                    pull_fee_refund_raw: 0,
                    amount_raw: request.approval.tuple.fee_raw,
                    fee_raw: 0,
                    memo: b"RUMI-SP-LIQ-V2-REFUND".to_vec(),
                    created_at_time_ns: ic_cdk::api::time(),
                };
                mutate_state(|s| {
                    crate::state::admit_sp_liquidation_v2_refund(
                        s,
                        pool,
                        request.clone(),
                        refund_tuple,
                        reason.clone(),
                    )
                })
                .map_err(ProtocolError::GenericError)?;
                return Err(ProtocolError::GenericError(format!(
                    "liquidation rejected; exact approval-fee refund is pending: {reason}"
                )));
            }
        };
        let required_allowance = request
            .amount
            .checked_add(current_fee)
            .ok_or_else(|| ProtocolError::GenericError("allowance cap overflow".into()))?;
        if request.approval.tuple.allowance_raw < required_allowance {
            let refund_tuple = crate::SpLiquidationStableRefundTuple {
                op_nonce: mutate_state(|s| s.next_op_nonce()),
                ledger,
                source: Account {
                    owner: backend,
                    subaccount: None,
                },
                destination: Account {
                    owner: pool,
                    subaccount: None,
                },
                principal_refund_raw: 0,
                approval_fee_refund_raw: request.approval.tuple.fee_raw,
                pull_fee_refund_raw: 0,
                amount_raw: request.approval.tuple.fee_raw,
                fee_raw: 0,
                memo: b"RUMI-SP-LIQ-V2-REFUND".to_vec(),
                created_at_time_ns: ic_cdk::api::time(),
            };
            mutate_state(|s| {
                crate::state::admit_sp_liquidation_v2_refund(
                    s,
                    pool,
                    request.clone(),
                    refund_tuple,
                    "approval does not cover request cap plus ledger fee".into(),
                )
            })
            .map_err(ProtocolError::GenericError)?;
            return Err(ProtocolError::GenericError(
                "approval allowance does not cover the requested cap and transfer fee; refund pending".into(),
            ));
        }

        // The legacy fence and mint-authority checks above are inter-canister
        // awaits. An approval that was live at the initial check can expire
        // while either call is in flight. Recheck at the final synchronous
        // boundary before admitting any pull; the approval block already
        // proves its fee was charged, so preserve that obligation in the
        // exact mint-refund journal rather than returning a bare rejection.
        if crate::state::sp_liquidation_v2_approval_expired(
            ic_cdk::api::time(),
            request.approval.tuple.expires_at_ns,
        ) {
            let refund_tuple =
                sp_liquidation_v2_approval_fee_mint_refund(&request, ledger, pool, backend);
            mutate_state(|s| {
                crate::state::admit_sp_liquidation_v2_refund(
                    s,
                    pool,
                    request.clone(),
                    refund_tuple,
                    "proven SP approval expired during backend admission checks".into(),
                )
            })
            .map_err(ProtocolError::GenericError)?;
            schedule_stability_pool_liquidation_v2_resume();
            return Err(ProtocolError::TemporarilyUnavailable(
                "SP approval expired during admission checks; exact approval-fee refund is pending"
                    .into(),
            ));
        }

        let nonce = mutate_state(|s| s.next_op_nonce());
        let pull_tuple = crate::SpLiquidationStablePullTuple {
            op_nonce: nonce,
            ledger,
            from: Account {
                owner: pool,
                subaccount: None,
            },
            spender: Account {
                owner: backend,
                subaccount: None,
            },
            to: Account {
                owner: backend,
                subaccount: None,
            },
            amount_raw: plan.debt_liquidated_e8s,
            fee_raw: current_fee,
            memo: [
                b"RUMI-SP-LIQ-V2:".as_slice(),
                &request.request_id.to_be_bytes(),
            ]
            .concat(),
            created_at_time_ns: ic_cdk::api::time(),
        };
        mutate_state(|s| {
            crate::state::admit_sp_liquidation_v2(
                s,
                pool,
                backend,
                ic_cdk::api::time(),
                request.clone(),
                plan,
                pull_tuple,
            )
        })
        .map_err(ProtocolError::GenericError)?;
        schedule_stability_pool_liquidation_v2_resume();
    }

    let row = read_state(|s| {
        s.sp_liquidation_v2_journals
            .get(&(pool, request.request_id))
            .cloned()
    })
    .ok_or_else(|| ProtocolError::GenericError("SP liquidation journal disappeared".into()))?;
    match row.status {
        Status::StablePullPending {
            tuple,
            candidate_block_index,
            ..
        } => {
            let had_prior_ambiguous_attempt = row.had_ambiguous_stable_pull_attempt;
            if candidate_block_index.is_none() {
                let fence = sp_legacy_liquidation_in_flight(pool, request.vault_id).await;
                if !matches!(fence, Ok(false)) {
                    let reason = match fence {
                        Ok(true) => "legacy SP liquidation marker is still present".to_string(),
                        Ok(false) => {
                            "legacy SP liquidation marker unexpectedly changed".to_string()
                        }
                        Err(error) => error,
                    };
                    let refund_tuple = sp_liquidation_v2_approval_fee_mint_refund(
                        &request,
                        tuple.ledger,
                        pool,
                        backend,
                    );
                    let disposition = mutate_state(|s| {
                        crate::state::record_sp_liquidation_v2_pre_pull_failure(
                            s,
                            pool,
                            request.request_id,
                            refund_tuple,
                            reason.clone(),
                        )
                    })
                    .map_err(ProtocolError::GenericError)?;
                    schedule_stability_pool_liquidation_v2_resume();
                    return Err(ProtocolError::TemporarilyUnavailable(match disposition {
                        crate::state::SpLiquidationPrePullFailureDisposition::ApprovalFeeRefundPending => format!(
                            "legacy SP liquidation fence is active or unavailable; approval-fee mint refund is pending: {reason}"
                        ),
                        crate::state::SpLiquidationPrePullFailureDisposition::ExactPullHeld => format!(
                            "legacy SP liquidation fence is active or unavailable after an ambiguous stable pull; exact pull remains held: {reason}"
                        ),
                    }));
                }
                // This is the final inter-canister await before dispatch.
                // Reconfirm the live minter and exact durable row, then make
                // no further await or state mutation before transfer_from.
                if crate::sp_burn_refund::verify_mint_authority(tuple.ledger)
                    .await
                    .is_err()
                {
                    let refund_tuple = sp_liquidation_v2_approval_fee_mint_refund(
                        &request,
                        tuple.ledger,
                        pool,
                        backend,
                    );
                    let disposition = mutate_state(|s| {
                        crate::state::record_sp_liquidation_v2_pre_pull_failure(
                            s,
                            pool,
                            request.request_id,
                            refund_tuple,
                            "icUSD minting authority changed before pull dispatch".into(),
                        )
                    })
                    .map_err(ProtocolError::GenericError)?;
                    schedule_stability_pool_liquidation_v2_resume();
                    return Err(ProtocolError::TemporarilyUnavailable(match disposition {
                        crate::state::SpLiquidationPrePullFailureDisposition::ApprovalFeeRefundPending =>
                            "icUSD minting authority changed before pull; approval fee refund is pending".into(),
                        crate::state::SpLiquidationPrePullFailureDisposition::ExactPullHeld =>
                            "icUSD minting authority could not be reverified after an ambiguous stable pull; exact pull remains held".into(),
                    }));
                }
                // A timer-driven retry can reach this row long after the
                // proven approval's expiry. Recheck after the final await
                // and before dispatching transfer_from. A never-dispatched
                // row can safely refund the approval fee; if any earlier
                // pull dispatch was ambiguous, the state helper preserves the
                // exact pull liability instead.
                if crate::state::sp_liquidation_v2_approval_expired(
                    ic_cdk::api::time(),
                    request.approval.tuple.expires_at_ns,
                ) {
                    let refund_tuple = sp_liquidation_v2_approval_fee_mint_refund(
                        &request,
                        tuple.ledger,
                        pool,
                        backend,
                    );
                    let disposition = mutate_state(|s| {
                        crate::state::record_sp_liquidation_v2_pre_pull_failure(
                            s,
                            pool,
                            request.request_id,
                            refund_tuple,
                            "proven SP approval expired before stable pull dispatch".into(),
                        )
                    })
                    .map_err(ProtocolError::GenericError)?;
                    schedule_stability_pool_liquidation_v2_resume();
                    return Err(ProtocolError::TemporarilyUnavailable(match disposition {
                        crate::state::SpLiquidationPrePullFailureDisposition::ApprovalFeeRefundPending =>
                            "SP approval expired before stable pull; exact approval-fee refund is pending".into(),
                        crate::state::SpLiquidationPrePullFailureDisposition::ExactPullHeld =>
                            "SP approval expired after an ambiguous stable pull; exact pull remains held for receipt reconciliation".into(),
                    }));
                }
                let row_is_current = read_state(|s| {
                    s.sp_liquidation_v2_journals
                        .get(&(pool, request.request_id))
                        .is_some_and(|current| {
                            current.request == request
                                && matches!(
                                    &current.status,
                                    Status::StablePullPending {
                                        tuple: current_tuple,
                                        candidate_block_index: None,
                                        ..
                                    } if current_tuple == &tuple
                                )
                                && crate::state::sp_liquidation_v2_plan_is_current(s, current)
                        })
                });
                if !row_is_current {
                    let refund_tuple = sp_liquidation_v2_approval_fee_mint_refund(
                        &request,
                        tuple.ledger,
                        pool,
                        backend,
                    );
                    let disposition = mutate_state(|s| {
                        crate::state::record_sp_liquidation_v2_pre_pull_failure(
                            s,
                            pool,
                            request.request_id,
                            refund_tuple,
                            "pinned pull plan changed immediately before dispatch".into(),
                        )
                    })
                    .map_err(ProtocolError::GenericError)?;
                    schedule_stability_pool_liquidation_v2_resume();
                    return Err(ProtocolError::TemporarilyUnavailable(match disposition {
                        crate::state::SpLiquidationPrePullFailureDisposition::ApprovalFeeRefundPending =>
                            "pinned plan changed before pull; approval fee refund is pending".into(),
                        crate::state::SpLiquidationPrePullFailureDisposition::ExactPullHeld =>
                            "pinned plan or journal changed after an ambiguous stable pull; exact pull remains held for receipt reconciliation".into(),
                    }));
                }
            }
            let block = match candidate_block_index {
                Some(block) => block,
                None => {
                    // Persist possible-effect provenance before dispatch. A
                    // callback trap must never make a later typed retry error
                    // look like proof that the original attempt did nothing.
                    mutate_state(|s| {
                        crate::state::set_sp_liquidation_v2_status_and_pull_ambiguity(
                            s,
                            pool,
                            request.request_id,
                            Status::StablePullPending {
                                tuple: tuple.clone(),
                                candidate_block_index: None,
                                last_error: Some("stable pull dispatch may have occurred".into()),
                            },
                            true,
                        )
                    })
                    .map_err(ProtocolError::GenericError)?;
                    match management::transfer_from_with_exact_tuple_outcome(&tuple).await {
                        ExactTransferFromOutcome::Applied(block) => block,
                        ExactTransferFromOutcome::ProvenNoEffect(error) => {
                            if had_prior_ambiguous_attempt {
                                mutate_state(|s| crate::state::set_sp_liquidation_v2_status_and_pull_ambiguity(
                                s,
                                pool,
                                request.request_id,
                                Status::StablePullPending {
                                    tuple: tuple.clone(),
                                    candidate_block_index: None,
                                    last_error: Some(format!("typed no-effect on retry after earlier ambiguous dispatch: {error:?}")),
                                },
                                true,
                            )).map_err(ProtocolError::GenericError)?;
                                return Err(ProtocolError::TemporarilyUnavailable(
                                "stable pull retry returned a typed no-effect error after an earlier ambiguous attempt; exact receipt reconciliation is required".into(),
                            ));
                            }
                            let refund_tuple = sp_liquidation_v2_approval_fee_mint_refund(
                                &request,
                                tuple.ledger,
                                pool,
                                backend,
                            );
                            mutate_state(|s| {
                                crate::state::set_sp_liquidation_v2_status_and_pull_ambiguity(
                                    s,
                                    pool,
                                    request.request_id,
                                    Status::StablePullRefundPending {
                                        stable_pull_receipt: None,
                                        tuple: refund_tuple,
                                        candidate_block_index: None,
                                        last_error: Some(format!(
                                            "stable pull proven no-effect: {error:?}"
                                        )),
                                    },
                                    false,
                                )
                            })
                            .map_err(ProtocolError::GenericError)?;
                            schedule_stability_pool_liquidation_v2_resume();
                            return Err(ProtocolError::GenericError(
                            "ICRC2 pull had no effect; exact approval-fee mint refund is pending".into(),
                        ));
                        }
                        ExactTransferFromOutcome::CallRejected { code, message } => {
                            mutate_state(|s| crate::state::set_sp_liquidation_v2_status_and_pull_ambiguity(
                            s, pool, request.request_id,
                            Status::StablePullPending { tuple: tuple.clone(), candidate_block_index: None,
                                last_error: Some(format!("ambiguous transfer_from call rejection {code}: {message}")) },
                            true,
                        )).map_err(ProtocolError::GenericError)?;
                            return Err(ProtocolError::GenericError(
                                "ICRC2 pull outcome is ambiguous; retry the exact same request"
                                    .into(),
                            ));
                        }
                        ExactTransferFromOutcome::AmbiguousLedgerError(error) => {
                            mutate_state(|s| {
                                crate::state::set_sp_liquidation_v2_status_and_pull_ambiguity(
                                    s,
                                    pool,
                                    request.request_id,
                                    Status::StablePullPending {
                                        tuple: tuple.clone(),
                                        candidate_block_index: None,
                                        last_error: Some(format!(
                                            "ambiguous typed transfer_from error: {error:?}"
                                        )),
                                    },
                                    true,
                                )
                            })
                            .map_err(ProtocolError::GenericError)?;
                            return Err(ProtocolError::TemporarilyUnavailable(
                            "ICRC2 returned an ambiguous generic/temporary error; exact pull remains held".into(),
                        ));
                        }
                        ExactTransferFromOutcome::InvalidBlockIndex => {
                            mutate_state(|s| {
                                crate::state::set_sp_liquidation_v2_status_and_pull_ambiguity(
                                    s,
                                    pool,
                                    request.request_id,
                                    Status::StablePullPending {
                                        tuple: tuple.clone(),
                                        candidate_block_index: None,
                                        last_error: Some(
                                            "stable pull committed without a usable block index"
                                                .into(),
                                        ),
                                    },
                                    true,
                                )
                            })
                            .map_err(ProtocolError::GenericError)?;
                            return Err(ProtocolError::GenericError(
                            "ICRC2 pull committed without a usable block index; reconcile the exact tuple".into(),
                        ));
                        }
                    }
                }
            };
            mutate_state(|s| {
                crate::state::set_sp_liquidation_v2_status_and_pull_ambiguity(
                    s,
                    pool,
                    request.request_id,
                    Status::StablePullPending {
                        tuple: tuple.clone(),
                        candidate_block_index: Some(block),
                        last_error: None,
                    },
                    true,
                )
            })
            .map_err(ProtocolError::GenericError)?;
            if let Err(error) =
                crate::icrc3_proof::verify_sp_liquidation_icusd_burn_block(&tuple, block).await
            {
                return Err(ProtocolError::GenericError(format!(
                    "ICRC2 pull receipt is not yet verifiable; exact request remains pending: {error}"
                )));
            }
            let pull_receipt = crate::SpLiquidationStablePullReceipt {
                block_index: block,
                tuple: tuple.clone(),
            };
            let latest = read_state(|s| {
                s.sp_liquidation_v2_journals
                    .get(&(pool, request.request_id))
                    .cloned()
            })
            .ok_or_else(|| {
                ProtocolError::GenericError("SP liquidation journal disappeared".into())
            })?;
            let plan = latest.plan.clone().ok_or_else(|| {
                ProtocolError::GenericError("SP liquidation plan is missing".into())
            })?;
            let still_current =
                read_state(|s| crate::state::sp_liquidation_v2_plan_is_current(s, &latest));
            if !still_current {
                let total_refund = tuple
                    .amount_raw
                    .checked_add(tuple.fee_raw)
                    .and_then(|value| value.checked_add(request.approval.tuple.fee_raw))
                    .ok_or_else(|| ProtocolError::GenericError("refund amount overflow".into()))?;
                let refund_tuple = crate::SpLiquidationStableRefundTuple {
                    op_nonce: mutate_state(|s| s.next_op_nonce()),
                    ledger: tuple.ledger,
                    source: Account {
                        owner: backend,
                        subaccount: None,
                    },
                    destination: Account {
                        owner: pool,
                        subaccount: None,
                    },
                    principal_refund_raw: tuple.amount_raw,
                    approval_fee_refund_raw: request.approval.tuple.fee_raw,
                    pull_fee_refund_raw: tuple.fee_raw,
                    amount_raw: total_refund,
                    fee_raw: 0,
                    memo: [
                        b"RUMI-SP-LIQ-V2-REFUND:".as_slice(),
                        &request.request_id.to_be_bytes(),
                    ]
                    .concat(),
                    created_at_time_ns: ic_cdk::api::time(),
                };
                mutate_state(|s| {
                    crate::state::set_sp_liquidation_v2_status(
                        s,
                        pool,
                        request.request_id,
                        Status::StablePullRefundPending {
                            stable_pull_receipt: Some(pull_receipt.clone()),
                            tuple: refund_tuple,
                            candidate_block_index: None,
                            last_error: Some(
                                "pinned liquidation plan changed after the stable pull".into(),
                            ),
                        },
                    )
                })
                .map_err(ProtocolError::GenericError)?;
                return Err(ProtocolError::GenericError(
                    "pinned plan changed after pull; exact full refund is pending".into(),
                ));
            }
            let time = ic_cdk::api::time();
            let result = mutate_state(
                |s| -> Result<(SuccessWithFee, crate::SpLiquidationPayoutTuple), String> {
                    if !crate::state::sp_liquidation_v2_plan_is_current(s, &latest) {
                        return Err("pinned plan changed immediately before commit".into());
                    }
                    let current_vault = s
                        .vault_id_to_vaults
                        .get(&request.vault_id)
                        .ok_or_else(|| "vault disappeared before commit".to_string())?;
                    if current_vault != &plan.vault {
                        return Err("vault version changed before commit".into());
                    }
                    let collateral_type = plan.vault.collateral_type;
                    let config = s
                        .get_collateral_config(&collateral_type)
                        .cloned()
                        .ok_or_else(|| {
                            "collateral has no local ICRC ledger configuration".to_string()
                        })?;
                    if config.ledger_canister_id != collateral_type
                        || config.is_native_xrp()
                        || config.ledger_canister_id != plan.collateral_ledger
                        || config.ledger_fee != plan.collateral_ledger_fee_raw
                    {
                        return Err(
                            "pinned ICRC collateral ledger or fee changed before payout commit"
                                .into(),
                        );
                    }
                    let payout_gross = plan.collateral_to_liquidator_raw;
                    if !crate::state::sp_liquidation_v2_payout_covers_fee(
                        payout_gross,
                        plan.collateral_ledger_fee_raw,
                    ) {
                        return Err("collateral payout is below pinned ledger fee".into());
                    }
                    if !s
                        .sp_liquidation_v2_journals
                        .contains_key(&(pool, request.request_id))
                    {
                        return Err("SP liquidation journal disappeared before commit".into());
                    }
                    // All checks that can reject this liquidation happen before
                    // the first economic state mutation. The pinned payout tuple
                    // and all protocol obligations are committed atomically below.
                    let nonce = s.next_op_nonce();
                    let payout = crate::SpLiquidationPayoutTuple {
                        op_nonce: nonce,
                        ledger: plan.collateral_ledger,
                        source: Account {
                            owner: backend,
                            subaccount: None,
                        },
                        destination: Account {
                            owner: pool,
                            subaccount: None,
                        },
                        gross_amount_raw: payout_gross,
                        net_amount_raw: payout_gross - plan.collateral_ledger_fee_raw,
                        fee_raw: plan.collateral_ledger_fee_raw,
                        memo: nonce.to_be_bytes().to_vec(),
                        created_at_time_ns: management::nonce_to_created_at_time(nonce),
                        collateral_type,
                    };
                    let liquidator_value = crate::numeric::collateral_usd_value(
                        payout_gross,
                        plan.collateral_price,
                        plan.collateral_decimals,
                    );
                    let fee = if liquidator_value > ICUSD::new(plan.debt_liquidated_e8s) {
                        (liquidator_value - ICUSD::new(plan.debt_liquidated_e8s)).to_u64()
                    } else {
                        0
                    };
                    let result = SuccessWithFee {
                        block_index: pull_receipt.block_index,
                        fee_amount_paid: fee,
                        collateral_amount_received: Some(payout_gross),
                        debt_liquidated_e8s: Some(plan.debt_liquidated_e8s),
                        stable_pulled_e6s: None,
                        xrp_claim_id: None,
                    };
                    let current_vault = s
                        .vault_id_to_vaults
                        .get_mut(&request.vault_id)
                        .expect("vault was validated before liquidation commit");
                    current_vault.borrowed_icusd_amount = current_vault
                        .borrowed_icusd_amount
                        .saturating_sub(ICUSD::new(plan.debt_liquidated_e8s));
                    current_vault.collateral_amount = current_vault
                        .collateral_amount
                        .saturating_sub(plan.collateral_to_seize_raw);
                    current_vault.accrued_interest = current_vault
                        .accrued_interest
                        .saturating_sub(ICUSD::new(plan.interest_share_e8s));
                    let collateral_price_usd = UsdIcp::from(plan.collateral_price);
                    let shortfall = crate::numeric::collateral_usd_value(
                        plan.collateral_to_seize_raw,
                        plan.collateral_price,
                        plan.collateral_decimals,
                    );
                    let deficit = if shortfall < plan.debt_liquidated_e8s {
                        plan.debt_liquidated_e8s - shortfall.to_u64()
                    } else {
                        0
                    };
                    crate::event::record_liquidation_for_breaker(s, plan.debt_liquidated_e8s);
                    if deficit > 0 {
                        crate::event::record_deficit_accrued(
                            s,
                            crate::event::DeficitSource::Liquidation {
                                vault_id: request.vault_id,
                            },
                            ICUSD::new(deficit),
                            time,
                        );
                        let _ = s.check_deficit_readonly_latch();
                    }
                    s.restore_pending_interest_for_pool(collateral_type, plan.interest_share_e8s);
                    crate::treasury::queue_liquidation_fee_obligation_in_state(
                        s,
                        collateral_type,
                        plan.protocol_cut_raw,
                    );
                    let event = crate::event::Event::PartialLiquidateVault {
                        vault_id: request.vault_id,
                        liquidator_payment: ICUSD::new(plan.debt_liquidated_e8s),
                        icp_to_liquidator: ICP::from(plan.collateral_to_liquidator_raw),
                        liquidator: Some(pool),
                        icp_rate: Some(collateral_price_usd),
                        protocol_fee_collateral: (plan.protocol_cut_raw > 0)
                            .then_some(plan.protocol_cut_raw),
                        timestamp: Some(time),
                        three_usd_reserves_e8s: None,
                    };
                    crate::storage::record_event(&event);
                    if s.cleanup_if_drained(request.vault_id) {
                        log!(
                            INFO,
                            "[SP-LIQ-V2] Vault #{} fully liquidated",
                            request.vault_id
                        );
                    }
                    let row = s
                        .sp_liquidation_v2_journals
                        .get_mut(&(pool, request.request_id))
                        .expect("journal was validated before liquidation commit");
                    row.status = Status::CollateralPayoutPending {
                        stable_pull_receipt: pull_receipt.clone(),
                        result: result.clone(),
                        tuple: payout.clone(),
                        candidate_block_index: None,
                        last_error: None,
                    };
                    crate::storage::mark_sp_liquidation_v2_used()
                        .expect("persist V2 payout outbox");
                    crate::storage::save_state_to_stable(s);
                    Ok((result, payout))
                },
            )
            .map_err(ProtocolError::GenericError)?;
            let _ = result;
            let retry_request = request.clone();
            ic_cdk_timers::set_timer(std::time::Duration::ZERO, move || {
                ic_cdk::spawn(async move {
                    let _ = stability_pool_liquidate_v2_inner(retry_request, pool, false).await;
                });
            });
            Err(ProtocolError::GenericError(
                "IcUSD pull verified and liquidation committed; exact collateral payout is pending"
                    .into(),
            ))
        }
        Status::CollateralPayoutPending {
            stable_pull_receipt,
            result,
            tuple,
            candidate_block_index,
            ..
        } => {
            let block = match candidate_block_index {
                Some(block) => block,
                None => {
                    let Some(_dispatch_guard) =
                        SpV2PayoutDispatchGuard::try_new(pool, request.request_id)
                    else {
                        return Err(ProtocolError::TemporarilyUnavailable(
                            "another payout dispatch is already active for this request".into(),
                        ));
                    };
                    let payout_row_before_balance = read_state(|s| {
                        s.sp_liquidation_v2_journals
                            .get(&(pool, request.request_id))
                            .cloned()
                    })
                    .ok_or_else(|| {
                        ProtocolError::TemporarilyUnavailable(
                            "payout journal disappeared before dispatch".into(),
                        )
                    })?;
                    let needs_balance_recheck =
                        payout_row_before_balance
                            .accepted_payout_supersession
                            .as_ref()
                            .is_some_and(|accepted| {
                                accepted.replacement == tuple
                                    && matches!(
                                &accepted.evidence,
                                crate::SpLiquidationPayoutNoEffectEvidence::InsufficientFunds {
                                    ..
                                }
                            )
                            });
                    if needs_balance_recheck {
                        let balance = management::get_icrc1_reserve_balance(
                        tuple.ledger,
                        tuple.source.clone(),
                    )
                    .await
                    .map_err(|error| {
                        ProtocolError::TemporarilyUnavailable(format!(
                            "InsufficientFunds payout successor remains held until source balance is verified: {error}"
                        ))
                    })?;
                        if balance < tuple.gross_amount_raw {
                            return Err(ProtocolError::TemporarilyUnavailable(format!(
                            "InsufficientFunds payout successor remains held: source balance {balance} is below gross entitlement {}",
                            tuple.gross_amount_raw
                        )));
                        }
                        if !read_state(|s| {
                            s.stability_pool_canister == Some(pool)
                                && s.sp_liquidation_v2_journals
                                    .get(&(pool, request.request_id))
                                    == Some(&payout_row_before_balance)
                        }) {
                            return Err(ProtocolError::TemporarilyUnavailable(
                            "InsufficientFunds payout successor changed during balance verification".into(),
                        ));
                        }
                    }
                    let may_authorize_successor = mutate_state(|s| {
                        crate::state::begin_sp_liquidation_v2_payout_dispatch(
                            s,
                            pool,
                            request.request_id,
                        )
                    })
                    .map_err(ProtocolError::GenericError)?;
                    match management::transfer_sp_liquidation_payout(&tuple).await {
                        ExactCollateralTransferOutcome::Applied(block) => block,
                        ExactCollateralTransferOutcome::ProvenNoEffect(error)
                            if may_authorize_successor =>
                        {
                            let evidence = match &error {
                            icrc_ledger_types::icrc1::transfer::TransferError::BadFee {
                                expected_fee,
                            } => expected_fee.0.to_u64().map(|expected_fee_raw| {
                                crate::SpLiquidationPayoutNoEffectEvidence::BadFee {
                                    expected_fee_raw,
                                }
                            }),
                            icrc_ledger_types::icrc1::transfer::TransferError::InsufficientFunds {
                                balance,
                            } => balance.0.to_u64().map(|reported_balance_raw| {
                                crate::SpLiquidationPayoutNoEffectEvidence::InsufficientFunds {
                                    reported_balance_raw,
                                }
                            }),
                            _ => None,
                        };
                            let Some(evidence) = evidence else {
                                mutate_state(|s| {
                                crate::state::record_sp_liquidation_v2_payout_dispatch_failure(
                                    s,
                                    pool,
                                    request.request_id,
                                    &tuple,
                                    format!("typed no-effect has no supported successor policy: {error:?}"),
                                    false,
                                )
                            })
                            .map_err(ProtocolError::GenericError)?;
                                return Err(ProtocolError::TemporarilyUnavailable(
                                "typed payout no-effect has no authorized successor policy; exact entitlement remains held".into(),
                            ));
                            };
                            let replacement =
                                match sp_liquidation_v2_payout_successor(&tuple, &evidence) {
                                    Ok(replacement) => replacement,
                                    Err(reason) => {
                                        mutate_state(|s| {
                                    crate::state::record_sp_liquidation_v2_payout_dispatch_failure(
                                        s,
                                        pool,
                                        request.request_id,
                                        &tuple,
                                        reason.clone(),
                                        false,
                                    )
                                })
                                .map_err(ProtocolError::GenericError)?;
                                        return Err(ProtocolError::TemporarilyUnavailable(
                                            format!(
                                    "payout successor could not be represented safely: {reason}"
                                ),
                                        ));
                                    }
                                };
                            mutate_state(|s| {
                                crate::state::record_sp_liquidation_v2_payout_supersession(
                                    s,
                                    pool,
                                    request.request_id,
                                    tuple.clone(),
                                    replacement,
                                    evidence,
                                    1,
                                )
                            })
                            .map_err(ProtocolError::GenericError)?;
                            return Err(ProtocolError::TemporarilyUnavailable(
                            "first typed payout no-effect is recorded; exact successor awaits Stability Pool adoption".into(),
                        ));
                        }
                        ExactCollateralTransferOutcome::ProvenNoEffect(error) => {
                            mutate_state(|s| {
                                crate::state::record_sp_liquidation_v2_payout_dispatch_failure(
                                    s,
                                    pool,
                                    request.request_id,
                                    &tuple,
                                    format!("typed payout no-effect: {error:?}"),
                                    false,
                                )
                            })
                            .map_err(ProtocolError::GenericError)?;
                            return Err(ProtocolError::TemporarilyUnavailable(
                            "payout had a typed no-effect result but no authorized successor; exact entitlement remains held".into(),
                        ));
                        }
                        ExactCollateralTransferOutcome::LedgerError(error) => {
                            mutate_state(|s| {
                                crate::state::record_sp_liquidation_v2_payout_dispatch_failure(
                                    s,
                                    pool,
                                    request.request_id,
                                    &tuple,
                                    format!("payout ledger outcome is ambiguous: {error:?}"),
                                    true,
                                )
                            })
                            .map_err(ProtocolError::GenericError)?;
                            return Err(ProtocolError::TemporarilyUnavailable(
                                "payout ledger outcome is ambiguous; exact tuple remains held"
                                    .into(),
                            ));
                        }
                        ExactCollateralTransferOutcome::CallRejected { code, message } => {
                            mutate_state(|s| {
                                crate::state::record_sp_liquidation_v2_payout_dispatch_failure(
                                    s,
                                    pool,
                                    request.request_id,
                                    &tuple,
                                    format!("ambiguous payout call rejection {code}: {message}"),
                                    true,
                                )
                            })
                            .map_err(ProtocolError::GenericError)?;
                            return Err(ProtocolError::TemporarilyUnavailable(
                            "collateral payout outcome is ambiguous; exact payout tuple remains pending".into(),
                        ));
                        }
                        ExactCollateralTransferOutcome::InvalidBlockIndex => {
                            mutate_state(|s| {
                                crate::state::record_sp_liquidation_v2_payout_dispatch_failure(
                                    s,
                                    pool,
                                    request.request_id,
                                    &tuple,
                                    "payout committed without a usable block index".into(),
                                    true,
                                )
                            })
                            .map_err(ProtocolError::GenericError)?;
                            return Err(ProtocolError::TemporarilyUnavailable(
                            "collateral payout committed without an index; attach an exact receipt candidate".into(),
                        ));
                        }
                    }
                }
            };
            mutate_state(|s| {
                crate::state::set_sp_liquidation_v2_status(
                    s,
                    pool,
                    request.request_id,
                    Status::CollateralPayoutPending {
                        stable_pull_receipt: stable_pull_receipt.clone(),
                        result: result.clone(),
                        tuple: tuple.clone(),
                        candidate_block_index: Some(block),
                        last_error: None,
                    },
                )
            })
            .map_err(ProtocolError::GenericError)?;
            crate::icrc3_proof::verify_sp_liquidation_payout_block(&tuple, block)
                .await
                .map_err(|error| {
                    ProtocolError::GenericError(format!(
                        "collateral payout receipt remains unverified: {error}"
                    ))
                })?;
            let payout_receipt = crate::SpLiquidationPayoutReceipt {
                block_index: block,
                tuple,
            };
            mutate_state(|s| {
                crate::state::set_sp_liquidation_v2_status(
                    s,
                    pool,
                    request.request_id,
                    Status::Complete {
                        stable_pull_receipt,
                        result: result.clone(),
                        payout_receipt,
                    },
                )
            })
            .map_err(ProtocolError::GenericError)?;
            Ok(result)
        }
        Status::CollateralPayoutSupersessionPending { .. } => {
            Err(ProtocolError::TemporarilyUnavailable(
                "fee-adjusted payout successor is held until the Stability Pool durably adopts it"
                    .into(),
            ))
        }
        _ => Err(ProtocolError::GenericError(
            "SP liquidation state changed; query its durable status".into(),
        )),
    }
}

pub async fn liquidate_vault_partial(
    vault_id: u64,
    icusd_amount: u64,
) -> Result<SuccessWithFee, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let guard_principal =
        GuardPrincipal::new(caller, &format!("liquidate_vault_partial_{}", vault_id))?;
    reject_if_bot_processing(vault_id)?; // LIQ-101: don't double-seize a bot-claimed vault
                                         // BK-001/002: per-vault lock so two different callers can't race this vault
                                         // and both be paid the full pre-state collateral from the shared pool.
    let _vault_liq_guard = VaultLiquidationGuard::new(vault_id)?;
    if let Err(e) = reject_active_xrp_sp_absorb_preflight(vault_id, ic_cdk::api::time()) {
        guard_principal.fail();
        return Err(e);
    }

    // Wave-8b LIQ-002 band gate deactivated 2026-05-18. The per-vault CR
    // check below remains the authoritative liquidatability test. See
    // `tests/audit_pocs_liq_002_sorted_troves_index.rs` ("Layer 2.5 —
    // band gate DEACTIVATION fence") for background. The helper
    // `state::is_within_liquidation_band` is preserved as dead code for
    // future MEV-resistance re-introduction (per-collateral index +
    // liquidatable-filtered floor).

    let liquidation_amount: ICUSD = icusd_amount.into();

    // LIQ-0XX: a requested amount of exactly zero is always rejected,
    // regardless of the dust rule below (which can still close a vault
    // whose FULL debt is small, but never accepts a caller-requested 0).
    if liquidation_amount == ICUSD::new(0) {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "Cannot liquidate zero amount".to_string(),
        ));
    }

    // Step 1: Validate vault is liquidatable and get partial liquidation amounts
    let (
        vault,
        collateral_price,
        config_decimals,
        collateral_price_usd,
        _mode,
        max_liquidatable_debt,
        collateral_to_liquidator,
        total_to_seize,
        protocol_cut,
    ) = match read_state(|s| {
        match s.vault_id_to_vaults.get(&vault_id) {
            Some(vault) => {
                // Check collateral status allows liquidation
                if let Some(status) = s.get_collateral_status(&vault.collateral_type) {
                    if !status.allows_liquidation() {
                        return Err(
                            "Liquidation is not allowed for this collateral type.".to_string()
                        );
                    }
                }

                let price = s
                    .get_collateral_price_decimal(&vault.collateral_type)
                    .ok_or_else(|| {
                        "No price available for collateral. Price feed may be down.".to_string()
                    })?;
                let decimals = s
                    .get_collateral_config(&vault.collateral_type)
                    .map(|c| c.decimals)
                    .unwrap_or(8);
                let collateral_price_usd = UsdIcp::from(price);
                let ratio = compute_collateral_ratio(vault, collateral_price_usd, s);
                let min_liq_ratio = s.get_min_liquidation_ratio_for(&vault.collateral_type);

                if ratio >= min_liq_ratio {
                    Err(format!(
                        "Vault #{} is not liquidatable. Current ratio: {}, minimum: {}",
                        vault_id,
                        ratio.to_f64(),
                        min_liq_ratio.to_f64()
                    ))
                } else {
                    // LIQ-0XX: single shared decision point for the amount —
                    // dust-vault full-close, cap (recovery/partial), requested
                    // amount, min floor, and the LIQ-003 residual round-up.
                    let actual_liquidation_amount = s.effective_liquidation_amount(
                        vault,
                        collateral_price_usd,
                        Some(liquidation_amount),
                    );

                    if actual_liquidation_amount == ICUSD::new(0) {
                        return Err("Cannot liquidate zero amount".to_string());
                    }

                    // Calculate collateral to transfer (debt + liquidation bonus)
                    let liq_bonus = s.get_liquidation_bonus_for(&vault.collateral_type);
                    let protocol_share = s.get_liquidation_protocol_share();
                    let collateral_raw = crate::numeric::try_icusd_to_collateral_amount(
                        actual_liquidation_amount,
                        price,
                        decimals,
                    )
                    .ok_or_else(|| {
                        "Required liquidation collateral exceeds the supported raw-token range."
                            .to_string()
                    })?;
                    let collateral_with_bonus = ICP::from(collateral_raw) * liq_bonus;
                    let total_to_seize =
                        collateral_with_bonus.min(ICP::from(vault.collateral_amount));

                    // Split: protocol gets a share of the bonus portion (liquidator's profit)
                    let bonus_portion = total_to_seize.to_u64().saturating_sub(collateral_raw);
                    let protocol_cut = (rust_decimal::Decimal::from(bonus_portion)
                        * protocol_share.0)
                        .to_u64()
                        .unwrap_or(0);
                    let collateral_to_liquidator =
                        ICP::from(total_to_seize.to_u64() - protocol_cut);

                    Ok((
                        vault.clone(),
                        price,
                        decimals,
                        collateral_price_usd,
                        s.mode,
                        actual_liquidation_amount,
                        collateral_to_liquidator,
                        total_to_seize,
                        protocol_cut,
                    ))
                }
            }
            None => Err(format!("Vault #{} not found", vault_id)),
        }
    }) {
        Ok(result) => result,
        Err(msg) => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(msg));
        }
    };

    // LIQ-0XX: min_icusd_amount applies to the FINAL (post-cap, post-dust-
    // round-up) amount, and is skipped when that amount closes the vault
    // fully — a dust vault must be closable even if the liquidator's
    // requested amount, or the computed cap, is below the floor. `vault`
    // here is the pre-liquidation snapshot, so `vault.borrowed_icusd_amount`
    // is the full debt being compared against.
    if max_liquidatable_debt < read_state(|s| s.min_icusd_amount)
        && max_liquidatable_debt != vault.borrowed_icusd_amount
    {
        guard_principal.fail();
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: read_state(|s| s.min_icusd_amount).to_u64(),
        });
    }

    if collateral_to_liquidator == ICP::new(0) || total_to_seize == ICP::new(0) {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "Liquidation would produce no collateral payout".to_string(),
        ));
    }

    log!(INFO,
        "[liquidate_vault_partial] Vault #{}: liquidating {} icUSD (max: {}), getting {} ICP collateral (protocol fee: {} ICP)",
        vault_id,
        max_liquidatable_debt.to_u64(),
        vault.borrowed_icusd_amount.to_u64(),
        collateral_to_liquidator.to_u64(),
        protocol_cut
    );

    // Step 2: Take icUSD from liquidator
    reject_pending_collateral_withdrawal(vault_id)?;
    let icusd_block_index = match transfer_icusd_from(max_liquidatable_debt, caller).await {
        Ok(block_index) => {
            log!(
                INFO,
                "[liquidate_vault_partial] Received {} icUSD from liquidator",
                max_liquidatable_debt.to_u64()
            );
            block_index
        }
        Err(transfer_from_error) => {
            guard_principal.fail();
            return Err(ProtocolError::TransferFromError(
                transfer_from_error,
                max_liquidatable_debt.to_u64(),
            ));
        }
    };

    // Step 3: Update protocol state (partial liquidation)
    let mut immediate_payouts = Vec::new();
    let (interest_share, xrp_claim_id) = mutate_state(|s| {
        // Compute proportional interest share before reducing debt
        let interest_share = if let Some(vault) = s.vault_id_to_vaults.get(&vault_id) {
            if vault.accrued_interest.0 > 0 && vault.borrowed_icusd_amount.0 > 0 {
                ICUSD::new(crate::numeric::proportional_interest_share(
                    max_liquidatable_debt.0,
                    vault.accrued_interest.0,
                    vault.borrowed_icusd_amount.0,
                ))
            } else {
                ICUSD::new(0)
            }
        } else {
            ICUSD::new(0)
        };

        // Reduce vault debt and collateral directly
        // Vault loses total_to_seize (liquidator + protocol cut)
        //
        // AR-B-001/BK-001 (audit 2026-06-09): the applied amounts captured
        // here also drive the PAYOUT below. Pre-fix, the payout used the
        // stale pre-await `collateral_to_liquidator`, so any concurrent
        // reduction of this vault paid the liquidator collateral the vault
        // no longer had, draining the shared pool. The per-vault op lock
        // makes such a reduction unreachable; the re-capped payout keeps any
        // residual drift solvency-safe.
        let mut debt_applied = max_liquidatable_debt;
        let mut collateral_applied = total_to_seize.to_u64();
        if let Some(vault) = s.vault_id_to_vaults.get_mut(&vault_id) {
            // ASYNC-001: cap each reduction to the CURRENT vault state and
            // saturating_sub. A concurrent partial liquidation may have reduced
            // this vault between our pre-await read and now; without the cap the
            // ICUSD Token::sub would underflow-PANIC and the raw u64 collateral
            // sub would WRAP, both after the liquidator's icUSD was already pulled.
            debt_applied = max_liquidatable_debt.min(vault.borrowed_icusd_amount);
            collateral_applied = total_to_seize.to_u64().min(vault.collateral_amount);
            let interest_applied = interest_share.min(vault.accrued_interest);
            vault.borrowed_icusd_amount = vault.borrowed_icusd_amount.saturating_sub(debt_applied);
            vault.collateral_amount = vault.collateral_amount.saturating_sub(collateral_applied);
            vault.accrued_interest = vault.accrued_interest.saturating_sub(interest_applied);
        }
        let payout_to_liquidator = ICP::from(collateral_applied.saturating_sub(protocol_cut));

        // Wave-10 LIQ-008: append the gross debt cleared to the rolling-
        // window log. Records all liquidations (healthy and underwater) so
        // the circuit breaker can pause auto-publishing during cascades.
        crate::event::record_liquidation_for_breaker(s, max_liquidatable_debt.to_u64());

        // Wave-8e LIQ-005: per-call deficit accrual against the APPLIED
        // amounts. Predicate: seized USD < debt cleared.
        let seized_usd = crate::numeric::collateral_usd_value(
            collateral_applied,
            collateral_price,
            config_decimals,
        );
        let shortfall = if seized_usd < debt_applied {
            debt_applied - seized_usd
        } else {
            ICUSD::new(0)
        };
        if shortfall.0 > 0 {
            crate::event::record_deficit_accrued(
                s,
                crate::event::DeficitSource::Liquidation { vault_id },
                shortfall,
                ic_cdk::api::time(),
            );
            if s.check_deficit_readonly_latch() {
                log!(INFO,
                    "[LIQ-005] deficit threshold {} crossed by partial vault #{} shortfall {}; auto-latched ReadOnly",
                    s.deficit_readonly_threshold_e8s, vault_id, shortfall.to_u64()
                );
            }
        }

        // Record the partial liquidation event (applied payout, so replay's
        // per-event deduction mirrors live state exactly)
        let event = crate::event::Event::PartialLiquidateVault {
            vault_id,
            liquidator_payment: max_liquidatable_debt,
            icp_to_liquidator: payout_to_liquidator,
            liquidator: Some(caller),
            icp_rate: Some(collateral_price_usd),
            protocol_fee_collateral: if protocol_cut > 0 {
                Some(protocol_cut.min(collateral_applied))
            } else {
                None
            },
            timestamp: Some(ic_cdk::api::time()),
            three_usd_reserves_e8s: None,
        };
        crate::storage::record_event(&event);

        // Liquidator-reward payout: PendingMarginTransfer for ICRC, XrpClaim for
        // native-XRP. Capture vault.owner (custody key) BEFORE cleanup_if_drained.
        let nonce = s.next_op_nonce();
        let xrp_claim_id = queue_collateral_payout(
            s,
            vault_id,
            vault.owner,
            caller,
            payout_to_liquidator,
            vault.collateral_type,
            nonce,
            ic_cdk::api::time(),
            &mut immediate_payouts,
        );

        // Shared drain rule (see state::cleanup_if_drained): remove the vault
        // if this liquidation emptied it, else re-key its CR index entry.
        if s.cleanup_if_drained(vault_id) {
            log!(
                INFO,
                "[liquidate_vault_partial] Vault #{} fully liquidated — removed",
                vault_id
            );
        }

        log!(
            INFO,
            "[liquidate_vault_partial] Partial liquidation completed, {} pending transfers created",
            1
        );
        (interest_share, xrp_claim_id)
    });

    // Route interest share via N-way split
    // IC-B-002 (audit 2026-06-09): re-queue any unminted interest share so the
    // next flush retries it instead of silently dropping treasury revenue.
    let unminted_interest =
        crate::treasury::distribute_interest(interest_share, vault.collateral_type).await;
    if unminted_interest.to_u64() > 0 {
        mutate_state(|s| {
            s.restore_pending_interest_for_pool(vault.collateral_type, unminted_interest.to_u64())
        });
    }

    // Send protocol's liquidation fee cut to treasury (fire-and-forget)
    if protocol_cut > 0 {
        if vault.collateral_type == crate::state::xrp_collateral_principal() {
            // P5: native-XRP protocol fee -> a developer-settleable XrpClaim (the
            // ICRC treasury transfer cannot target the synthetic XRP ledger). Keyed
            // by collateral_type (not a vault lookup, since the vault may already be
            // drained/removed by cleanup_if_drained above).
            let dev = read_state(|s| s.developer_principal);
            let now_ns = ic_cdk::api::time();
            mutate_state(|s| {
                record_xrp_claim(
                    s,
                    dev,
                    vault.owner,
                    vault.vault_id,
                    protocol_cut.to_u64().unwrap_or(0),
                    now_ns,
                );
            });
        } else {
            let asset_type = crate::treasury::collateral_to_asset_type(&vault.collateral_type);
            crate::treasury::send_liquidation_fee_to_treasury(
                protocol_cut,
                vault.collateral_type,
                asset_type,
            )
            .await;
        }
    }

    // Step 4: Process transfer (same as complete liquidation)
    match try_process_pending_transfers_immediate(&immediate_payouts).await {
        Ok(processed_count) => {
            log!(
                INFO,
                "[liquidate_vault_partial] Successfully processed {} transfers immediately",
                processed_count
            );
        }
        Err(e) => {
            log!(INFO, "[liquidate_vault_partial] Immediate processing failed: {}. Transfers will be retried via timer", e);
            schedule_transfer_retry(vault_id, immediate_payouts.clone(), 0);
        }
    }

    ic_cdk_timers::set_timer(std::time::Duration::from_secs(2), move || {
        ic_cdk::spawn(async move {
            log!(
                INFO,
                "[liquidate_vault_partial] Backup timer processing transfers for vault #{}",
                vault_id
            );
            let _ = crate::process_pending_transfer().await;
        })
    });

    guard_principal.complete();

    // Calculate fee (liquidator bonus)
    let liquidator_value_received = crate::numeric::collateral_usd_value(
        collateral_to_liquidator.to_u64(),
        collateral_price,
        config_decimals,
    );
    let fee_amount = if liquidator_value_received > max_liquidatable_debt {
        liquidator_value_received - max_liquidatable_debt
    } else {
        ICUSD::new(0)
    };

    log!(INFO, "[liquidate_vault_partial] Partial liquidation completed. Block index: {}, Fee: {}, Collateral: {}",
         icusd_block_index, fee_amount.to_u64(), collateral_to_liquidator.to_u64());

    Ok(SuccessWithFee {
        block_index: icusd_block_index,
        fee_amount_paid: fee_amount.to_u64(),
        collateral_amount_received: Some(collateral_to_liquidator.to_u64()),
        debt_liquidated_e8s: Some(max_liquidatable_debt.to_u64()), // SP-101
        stable_pulled_e6s: None, // SP-110 (icUSD path: no stable surcharge)
        xrp_claim_id,
    })
}

pub async fn resume_stability_pool_liquidations_v2() {
    const MAX_ROWS_PER_PASS: usize = 8;
    let pending = mutate_state(|s| {
        let retryable = |row: &&crate::state::SpLiquidationV2Journal| {
            !matches!(
                row.status,
                crate::SpLiquidationV2Status::Complete { .. }
                    | crate::SpLiquidationV2Status::StablePullRefunded { .. }
                    | crate::SpLiquidationV2Status::Rejected { .. }
                    | crate::SpLiquidationV2Status::Acknowledged
            )
        };
        let mut rows = s
            .sp_liquidation_v2_journals
            .iter()
            .filter(|(_, row)| retryable(row))
            .collect::<Vec<_>>();
        let cursor = s.sp_liquidation_v2_resume_cursor;
        let start = cursor
            .and_then(|key| rows.iter().position(|(row_key, _)| **row_key > key))
            .unwrap_or(0);
        rows.rotate_left(start);
        let pending = rows
            .iter()
            .take(MAX_ROWS_PER_PASS)
            .map(|((pool, _), row)| (*pool, row.request.clone()))
            .collect::<Vec<_>>();
        if let Some((pool, request)) = pending.last() {
            s.sp_liquidation_v2_resume_cursor = Some((*pool, request.request_id));
            crate::storage::mark_sp_liquidation_v2_used()
                .expect("persist V2 recovery cursor before awaits");
            crate::storage::save_state_to_stable(s);
        }
        pending
    });
    for (pool, request) in pending {
        let _ = stability_pool_liquidate_v2_inner(request, pool, false).await;
    }
}

pub fn has_retryable_stability_pool_liquidation_v2() -> bool {
    read_state(|s| {
        s.sp_liquidation_v2_journals.values().any(|row| {
            !matches!(
                row.status,
                crate::SpLiquidationV2Status::Complete { .. }
                    | crate::SpLiquidationV2Status::StablePullRefunded { .. }
                    | crate::SpLiquidationV2Status::Rejected { .. }
                    | crate::SpLiquidationV2Status::Acknowledged
            )
        })
    })
}

pub fn schedule_stability_pool_liquidation_v2_resume() {
    ic_cdk_timers::set_timer(std::time::Duration::from_secs(2), || {
        ic_cdk::spawn(async {
            resume_stability_pool_liquidations_v2().await;
        });
    });
}

/// Liquidate a vault using ckUSDT or ckUSDC (1:1 with icUSD, plus configurable fee)
pub async fn liquidate_vault_partial_with_stable(
    vault_id: u64,
    stable_amount: u64,
    token_type: StableTokenType,
) -> Result<SuccessWithFee, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let guard_principal =
        GuardPrincipal::new(caller, &format!("liquidate_vault_stable_{}", vault_id))?;
    reject_if_bot_processing(vault_id)?; // LIQ-101: don't double-seize a bot-claimed vault
    let _vault_liq_guard = VaultLiquidationGuard::new(vault_id)?; // BK-001/002 per-vault lock
    if let Err(e) = reject_active_xrp_sp_absorb_preflight(vault_id, ic_cdk::api::time()) {
        guard_principal.fail();
        return Err(e);
    }

    // Wave-8b LIQ-002 band gate deactivated 2026-05-18 (see
    // `liquidate_vault_partial` above for rationale).

    // Check if the selected stable token is enabled
    let is_enabled = read_state(|s| match token_type {
        StableTokenType::CKUSDT => s.ckusdt_enabled,
        StableTokenType::CKUSDC => s.ckusdc_enabled,
    });
    if !is_enabled {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(format!(
            "{:?} liquidations are currently disabled",
            token_type
        )));
    }

    // Depeg protection: fetch fresh stablecoin price and reject if outside $0.95–$1.05
    if let Err(e) = crate::xrc::ensure_stable_not_depegged(&token_type).await {
        guard_principal.fail();
        return Err(e);
    }

    // Truncate to nearest 100 e8s for clean 8→6 decimal conversion
    let raw_amount_e8s = stable_amount - (stable_amount % 100);
    let liquidation_amount: ICUSD = raw_amount_e8s.into();

    // LIQ-0XX: a requested amount of exactly zero is always rejected,
    // regardless of the dust rule below (which can still close a vault
    // whose FULL debt is small, but never accepts a caller-requested 0).
    if liquidation_amount == ICUSD::new(0) {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "Cannot liquidate zero amount".to_string(),
        ));
    }

    // Step 1: Validate vault is liquidatable and get partial liquidation amounts
    let (
        vault,
        collateral_price,
        config_decimals,
        collateral_price_usd,
        _mode,
        max_liquidatable_debt,
        collateral_to_liquidator,
        total_to_seize,
        protocol_cut,
    ) = match read_state(|s| {
        match s.vault_id_to_vaults.get(&vault_id) {
            Some(vault) => {
                // Check collateral status allows liquidation
                if let Some(status) = s.get_collateral_status(&vault.collateral_type) {
                    if !status.allows_liquidation() {
                        return Err(
                            "Liquidation is not allowed for this collateral type.".to_string()
                        );
                    }
                }

                let price = s
                    .get_collateral_price_decimal(&vault.collateral_type)
                    .ok_or_else(|| {
                        "No price available for collateral. Price feed may be down.".to_string()
                    })?;
                let decimals = s
                    .get_collateral_config(&vault.collateral_type)
                    .map(|c| c.decimals)
                    .unwrap_or(8);
                let collateral_price_usd = UsdIcp::from(price);
                let ratio = compute_collateral_ratio(vault, collateral_price_usd, s);
                let min_liq_ratio = s.get_min_liquidation_ratio_for(&vault.collateral_type);

                if ratio >= min_liq_ratio {
                    Err(format!(
                        "Vault #{} is not liquidatable. Current ratio: {}, minimum: {}",
                        vault_id,
                        ratio.to_f64(),
                        min_liq_ratio.to_f64()
                    ))
                } else {
                    // LIQ-0XX: single shared decision point for the amount —
                    // dust-vault full-close, cap (recovery/partial), requested
                    // amount, min floor, and the LIQ-003 residual round-up.
                    let actual_liquidation_amount = s.effective_liquidation_amount(
                        vault,
                        collateral_price_usd,
                        Some(liquidation_amount),
                    );

                    if actual_liquidation_amount == ICUSD::new(0) {
                        return Err("Cannot liquidate zero amount".to_string());
                    }

                    let liq_bonus = s.get_liquidation_bonus_for(&vault.collateral_type);
                    let protocol_share = s.get_liquidation_protocol_share();
                    let collateral_raw = crate::numeric::try_icusd_to_collateral_amount(
                        actual_liquidation_amount,
                        price,
                        decimals,
                    )
                    .ok_or_else(|| {
                        "Cannot safely size liquidation collateral: conversion is unrepresentable"
                            .to_string()
                    })?;
                    let collateral_with_bonus = ICP::from(collateral_raw) * liq_bonus;
                    let total_to_seize =
                        collateral_with_bonus.min(ICP::from(vault.collateral_amount));

                    // Split: protocol gets a share of the bonus portion
                    let bonus_portion = total_to_seize.to_u64().saturating_sub(collateral_raw);
                    let protocol_cut = (rust_decimal::Decimal::from(bonus_portion)
                        * protocol_share.0)
                        .to_u64()
                        .unwrap_or(0);
                    let collateral_to_liquidator =
                        ICP::from(total_to_seize.to_u64() - protocol_cut);

                    Ok((
                        vault.clone(),
                        price,
                        decimals,
                        collateral_price_usd,
                        s.mode,
                        actual_liquidation_amount,
                        collateral_to_liquidator,
                        total_to_seize,
                        protocol_cut,
                    ))
                }
            }
            None => Err(format!("Vault #{} not found", vault_id)),
        }
    }) {
        Ok(result) => result,
        Err(msg) => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(msg));
        }
    };

    // LIQ-0XX: min_icusd_amount applies to the FINAL (post-cap, post-dust-
    // round-up) amount, and is skipped when that amount closes the vault
    // fully. `vault` here is the pre-liquidation snapshot.
    if max_liquidatable_debt < read_state(|s| s.min_icusd_amount)
        && max_liquidatable_debt != vault.borrowed_icusd_amount
    {
        guard_principal.fail();
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: read_state(|s| s.min_icusd_amount).to_u64(),
        });
    }

    if collateral_to_liquidator == ICP::new(0) || total_to_seize == ICP::new(0) {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "Liquidation would produce no collateral payout".to_string(),
        ));
    }

    log!(INFO,
        "[liquidate_vault_stable] Vault #{}: liquidating {} {:?} (max: {}), getting {} ICP collateral (protocol fee: {} ICP)",
        vault_id,
        max_liquidatable_debt.to_u64(),
        token_type,
        vault.borrowed_icusd_amount.to_u64(),
        collateral_to_liquidator.to_u64(),
        protocol_cut
    );

    // Step 2: Pull enough stable principal to cover the finalized liquidation
    // debt, which may be rounded up to the full debt to prevent a dust remainder.
    let fee_rate = read_state(|s| s.ckstable_repay_fee);
    let (_base_stable_e6s, fee_e6s, total_pull_e6s) =
        match stable_repay_pull_e6s(max_liquidatable_debt, fee_rate) {
            Ok(pull) => pull,
            Err(error) => {
                guard_principal.fail();
                return Err(error);
            }
        };

    reject_pending_collateral_withdrawal(vault_id)?;
    let stable_block_index =
        match transfer_stable_from(token_type.clone(), total_pull_e6s, caller).await {
            Ok(block_index) => {
                log!(
                    INFO,
                    "[liquidate_vault_stable] Received {} e6s {:?} from liquidator (fee: {} e6s)",
                    total_pull_e6s,
                    token_type,
                    fee_e6s
                );
                block_index
            }
            Err(transfer_from_error) => {
                guard_principal.fail();
                return Err(ProtocolError::TransferFromError(
                    transfer_from_error,
                    total_pull_e6s,
                ));
            }
        };

    // Step 3: Update protocol state (partial liquidation)
    let mut immediate_payouts = Vec::new();
    let (interest_share, xrp_claim_id) = mutate_state(|s| {
        // Compute proportional interest share before reducing debt
        let interest_share = if let Some(vault) = s.vault_id_to_vaults.get(&vault_id) {
            if vault.accrued_interest.0 > 0 && vault.borrowed_icusd_amount.0 > 0 {
                ICUSD::new(crate::numeric::proportional_interest_share(
                    max_liquidatable_debt.0,
                    vault.accrued_interest.0,
                    vault.borrowed_icusd_amount.0,
                ))
            } else {
                ICUSD::new(0)
            }
        } else {
            ICUSD::new(0)
        };

        // Reduce vault debt and collateral directly
        // Vault loses total_to_seize (liquidator + protocol cut)
        // AR-B-001/BK-001 (audit 2026-06-09): capture applied amounts and
        // re-cap the payout, mirroring `liquidate_vault_partial`.
        let mut debt_applied = max_liquidatable_debt;
        let mut collateral_applied = total_to_seize.to_u64();
        if let Some(vault) = s.vault_id_to_vaults.get_mut(&vault_id) {
            // ASYNC-001: cap each reduction to the CURRENT vault state and
            // saturating_sub. A concurrent partial liquidation may have reduced
            // this vault between our pre-await read and now; without the cap the
            // ICUSD Token::sub would underflow-PANIC and the raw u64 collateral
            // sub would WRAP, both after the liquidator's icUSD was already pulled.
            debt_applied = max_liquidatable_debt.min(vault.borrowed_icusd_amount);
            collateral_applied = total_to_seize.to_u64().min(vault.collateral_amount);
            let interest_applied = interest_share.min(vault.accrued_interest);
            vault.borrowed_icusd_amount = vault.borrowed_icusd_amount.saturating_sub(debt_applied);
            vault.collateral_amount = vault.collateral_amount.saturating_sub(collateral_applied);
            vault.accrued_interest = vault.accrued_interest.saturating_sub(interest_applied);
        }
        let payout_to_liquidator = ICP::from(collateral_applied.saturating_sub(protocol_cut));

        // Wave-10 LIQ-008: append the gross debt cleared to the rolling-
        // window log for the mass-liquidation circuit breaker.
        crate::event::record_liquidation_for_breaker(s, max_liquidatable_debt.to_u64());

        // Wave-8e LIQ-005: per-call deficit accrual against the APPLIED
        // amounts. The stablecoin path pulls ckUSDT/ckUSDC from the
        // liquidator (1:1 with icUSD plus a surcharge). Predicate measured
        // in icUSD-equivalent collateral USD value.
        let seized_usd = crate::numeric::collateral_usd_value(
            collateral_applied,
            collateral_price,
            config_decimals,
        );
        let shortfall = if seized_usd < debt_applied {
            debt_applied - seized_usd
        } else {
            ICUSD::new(0)
        };
        if shortfall.0 > 0 {
            crate::event::record_deficit_accrued(
                s,
                crate::event::DeficitSource::Liquidation { vault_id },
                shortfall,
                ic_cdk::api::time(),
            );
            if s.check_deficit_readonly_latch() {
                log!(INFO,
                    "[LIQ-005] deficit threshold {} crossed by stable-partial vault #{} shortfall {}; auto-latched ReadOnly",
                    s.deficit_readonly_threshold_e8s, vault_id, shortfall.to_u64()
                );
            }
        }

        // Record the partial liquidation event (applied payout, replay-exact)
        let event = crate::event::Event::PartialLiquidateVault {
            vault_id,
            liquidator_payment: max_liquidatable_debt,
            icp_to_liquidator: payout_to_liquidator,
            liquidator: Some(caller),
            icp_rate: Some(collateral_price_usd),
            protocol_fee_collateral: if protocol_cut > 0 {
                Some(protocol_cut.min(collateral_applied))
            } else {
                None
            },
            timestamp: Some(ic_cdk::api::time()),
            three_usd_reserves_e8s: None,
        };
        crate::storage::record_event(&event);

        // Create pending transfer for liquidator reward
        let nonce = s.next_op_nonce();
        let xrp_claim_id = queue_collateral_payout(
            s,
            vault_id,
            vault.owner,
            caller,
            payout_to_liquidator,
            vault.collateral_type,
            nonce,
            ic_cdk::api::time(),
            &mut immediate_payouts,
        );

        // Shared drain rule (see state::cleanup_if_drained): remove the vault
        // if this liquidation emptied it, else re-key its CR index entry.
        if s.cleanup_if_drained(vault_id) {
            log!(
                INFO,
                "[liquidate_vault_stable] Vault #{} fully liquidated — removed",
                vault_id
            );
        }

        log!(
            INFO,
            "[liquidate_vault_stable] Partial liquidation completed, pending transfer created"
        );
        (interest_share, xrp_claim_id)
    });

    // The liquidation state is committed. Pin the fee before a later await.
    let fee_payment_id = (fee_e6s > 0).then(|| {
        crate::treasury::queue_stablecoin_surcharge_obligation(
            fee_e6s,
            token_type.clone(),
            crate::state::TreasuryPaymentKind::LiquidationStablecoinSurcharge,
        )
    });

    // Route interest via N-way split (stablecoin-denominated)
    if interest_share.to_u64() > 0 {
        crate::treasury::distribute_stablecoin_interest(
            interest_share.to_u64(),
            vault.collateral_type,
            token_type.clone(),
        )
        .await;
    }

    if let Some(operation_id) = fee_payment_id {
        crate::treasury::dispatch_pending_treasury_payment(operation_id).await;
    }

    // Send protocol's liquidation fee cut to treasury (fire-and-forget)
    if protocol_cut > 0 {
        if vault.collateral_type == crate::state::xrp_collateral_principal() {
            // P5: native-XRP protocol fee -> a developer-settleable XrpClaim (the
            // ICRC treasury transfer cannot target the synthetic XRP ledger). Keyed
            // by collateral_type (not a vault lookup, since the vault may already be
            // drained/removed by cleanup_if_drained above).
            let dev = read_state(|s| s.developer_principal);
            let now_ns = ic_cdk::api::time();
            mutate_state(|s| {
                record_xrp_claim(
                    s,
                    dev,
                    vault.owner,
                    vault.vault_id,
                    protocol_cut.to_u64().unwrap_or(0),
                    now_ns,
                );
            });
        } else {
            let asset_type = crate::treasury::collateral_to_asset_type(&vault.collateral_type);
            crate::treasury::send_liquidation_fee_to_treasury(
                protocol_cut,
                vault.collateral_type,
                asset_type,
            )
            .await;
        }
    }

    // Step 4: Process transfer
    match try_process_pending_transfers_immediate(&immediate_payouts).await {
        Ok(processed_count) => {
            log!(
                INFO,
                "[liquidate_vault_stable] Successfully processed {} transfers immediately",
                processed_count
            );
        }
        Err(e) => {
            log!(INFO, "[liquidate_vault_stable] Immediate processing failed: {}. Transfers will be retried via timer", e);
            schedule_transfer_retry(vault_id, immediate_payouts.clone(), 0);
        }
    }

    ic_cdk_timers::set_timer(std::time::Duration::from_secs(2), move || {
        ic_cdk::spawn(async move {
            log!(
                INFO,
                "[liquidate_vault_stable] Backup timer processing transfers for vault #{}",
                vault_id
            );
            let _ = crate::process_pending_transfer().await;
        })
    });

    guard_principal.complete();

    // Calculate fee (liquidator bonus)
    let liquidator_value_received = crate::numeric::collateral_usd_value(
        collateral_to_liquidator.to_u64(),
        collateral_price,
        config_decimals,
    );
    let fee_amount = if liquidator_value_received > max_liquidatable_debt {
        liquidator_value_received - max_liquidatable_debt
    } else {
        ICUSD::new(0)
    };

    log!(
        INFO,
        "[liquidate_vault_stable] Liquidation completed. Block index: {}, Fee: {}, Collateral: {}",
        stable_block_index,
        fee_amount.to_u64(),
        collateral_to_liquidator.to_u64()
    );

    Ok(SuccessWithFee {
        block_index: stable_block_index,
        fee_amount_paid: fee_amount.to_u64(),
        collateral_amount_received: Some(collateral_to_liquidator.to_u64()),
        debt_liquidated_e8s: Some(max_liquidatable_debt.to_u64()), // SP-101
        stable_pulled_e6s: Some(total_pull_e6s), // SP-110: base + repay-fee surcharge
        xrp_claim_id,
    })
}

fn already_burned_liquidation_seizure(
    debt: ICUSD,
    price: Decimal,
    decimals: u8,
    liq_bonus: Ratio,
    protocol_share: Ratio,
    vault_collateral: u64,
) -> (ICP, u64) {
    match crate::numeric::try_icusd_to_collateral_amount(debt, price, decimals) {
        Some(collateral_raw) => {
            let total_to_seize =
                (ICP::from(collateral_raw) * liq_bonus).min(ICP::from(vault_collateral));
            let bonus_portion = total_to_seize.to_u64().saturating_sub(collateral_raw);
            let protocol_cut = (rust_decimal::Decimal::from(bonus_portion) * protocol_share.0)
                .to_u64()
                .unwrap_or(0);
            (total_to_seize, protocol_cut)
        }
        // icUSD has already been burned by the Stability Pool. An
        // unrepresentable theoretical seize therefore consumes all
        // collateral rather than rejecting the write-down or converting the
        // overflow into a zero-collateral seizure.
        None => (ICP::from(vault_collateral), 0),
    }
}

#[cfg(test)]
mod cl16_already_burned_seizure_tests {
    use super::already_burned_liquidation_seizure;
    use crate::numeric::{Ratio, ICP, ICUSD};
    use rust_decimal_macros::dec;

    #[test]
    fn unrepresentable_already_burned_seizure_clamps_to_all_vault_collateral() {
        let available = 42_000_000u64;
        let (seized, protocol_cut) = already_burned_liquidation_seizure(
            ICUSD::new(100_000_000), // 1 icUSD
            dec!(0.05),
            18,
            Ratio::from(dec!(1.15)),
            Ratio::from(dec!(0.10)),
            available,
        );
        assert_eq!(seized, ICP::new(available));
        assert_eq!(protocol_cut, 0, "overflow clamp carries no bonus fee");
    }
}

/// Liquidate a vault when the debt has already been covered externally.
///
/// Two modes:
/// - `three_usd_received_e8s: None` — legacy burn path: icUSD was destroyed via 3pool.
/// - `three_usd_received_e8s: Some(amount)` — reserves path: 3USD was transferred to
///   the backend's protocol reserves subaccount. No icUSD was burned; the 3USD in
///   reserves serves as backing for the written-off debt.
///
/// Called by the stability pool canister.
///
/// Wave-8d LIQ-004 Phase 2: `proof` is required. Every writedown must
/// reference an on-chain ICRC-3 block (a real icUSD burn for the legacy
/// path, or a real 3USD transfer to the protocol's reserves subaccount for
/// the reserves path). The Wave-8c migration window where `None` was
/// accepted with a per-call WARN log has been retired.
pub async fn liquidate_vault_debt_already_burned(
    vault_id: u64,
    icusd_burned_e8s: u64,
    caller: Principal,
    three_usd_received_e8s: Option<u64>,
    proof: crate::icrc3_proof::SpWritedownProof,
) -> Result<StabilityPoolLiquidationResult, ProtocolError> {
    liquidate_vault_debt_already_burned_inner(
        vault_id,
        icusd_burned_e8s,
        caller,
        three_usd_received_e8s,
        proof,
        None,
    )
    .await
}

/// V2-only entry point that carries the exact durable ingress identity into
/// the write-down verifier.
pub async fn liquidate_vault_debt_already_burned_v2(
    vault_id: u64,
    icusd_burned_e8s: u64,
    caller: Principal,
    three_usd_received_e8s: Option<u64>,
    proof: crate::icrc3_proof::SpWritedownProof,
    ingress_key: crate::state::ThreeUsdReserveIngressKey,
) -> Result<StabilityPoolLiquidationResult, ProtocolError> {
    liquidate_vault_debt_already_burned_inner(
        vault_id,
        icusd_burned_e8s,
        caller,
        three_usd_received_e8s,
        proof,
        Some(ingress_key),
    )
    .await
}

async fn liquidate_vault_debt_already_burned_inner(
    vault_id: u64,
    icusd_burned_e8s: u64,
    caller: Principal,
    three_usd_received_e8s: Option<u64>,
    proof: crate::icrc3_proof::SpWritedownProof,
    ingress_key: Option<crate::state::ThreeUsdReserveIngressKey>,
) -> Result<StabilityPoolLiquidationResult, ProtocolError> {
    // V2 default-account reserve transfers are idempotent across a lost
    // inter-canister reply. The proof-keyed result is committed atomically
    // with the accounting mutation below, so return it before kill-switch or
    // consumed-proof checks only for the exact original request. A consumed
    // proof with no row remains fail-closed at the ordinary replay check.
    if proof.ledger_kind == crate::icrc3_proof::SpProofLedger::ThreePoolTransferDefault {
        let replay = replay_committed_three_usd_reserve_absorb_result(
            caller,
            vault_id,
            icusd_burned_e8s,
            three_usd_received_e8s,
            &proof,
        )?;
        if let Some(result) = replay {
            return Ok(result);
        }
    }

    // Wave-8b LIQ-002 band gate is deactivated globally as of 2026-05-18.
    // This path was never gated to begin with: it is the stability-pool-
    // triggered writedown, with the caller gated on
    // `caller == stability_pool_canister` by the entry point in `main.rs`
    // (`stability_pool_liquidate_*`), and the SP has already committed
    // icUSD via the 3pool burn. The CR index is still kept fresh by the
    // post-mutation `reindex_vault_cr` call at the end of this function so
    // any future re-introduction of the band check sees a current CR.

    // Wave-8c LIQ-004 (kill switch): admin-toggleable disable of this path.
    // Independent of `frozen` and `liquidation_frozen`. Use during a
    // confirmed SP compromise / drift event.
    if read_state(|s| s.sp_writedown_disabled) {
        return Err(ProtocolError::TemporarilyUnavailable(
            "SP writedown path is disabled by admin".to_string(),
        ));
    }

    // Defense-in-depth: native-XRP collateral can NEVER be liquidated via the SP
    // write-down path. This path proof-verifies + settles against the icUSD/3pool
    // ledger, but the seized collateral here is XRP held on XRPL. The SP cannot
    // settle that, so a write-down would strand the seized XRP (it never becomes
    // an `XrpClaim`) and burn SP depositors. Native-XRP is liquidated only via the
    // manual paths (`liquidate_vault` / `liquidate_vault_partial` /
    // `partial_liquidate_vault` / `liquidate_vault_partial_with_stable`), which
    // route collateral into an `XrpClaim`. The two `main.rs` entry points are the
    // first line of defense; this in-function reject is the backstop so any future
    // third caller (or a refactor that drops the caller-side check) cannot reach
    // the write-down. Placed before `GuardPrincipal::new` so it returns without
    // touching any guard/state (and before any `ic_cdk::api::time()` call).
    if vault_is_native_xrp(vault_id) {
        return Err(ProtocolError::GenericError(
            "Native-XRP collateral cannot be liquidated via the SP write-down path".to_string(),
        ));
    }

    let guard_principal =
        GuardPrincipal::new(caller, &format!("liquidate_vault_debt_burned_{}", vault_id))?;
    reject_if_bot_processing(vault_id)?; // LIQ-101: don't double-seize a bot-claimed vault (SP path)
    let _vault_liq_guard = VaultLiquidationGuard::new(vault_id)?; // BK-001/002 per-vault lock

    let liquidation_amount: ICUSD = icusd_burned_e8s.into();

    // LIQ-0XX: the `min_icusd_amount` check moves below, to run on the FINAL
    // amount (`liquidation_amount.min(vault.borrowed_icusd_amount)`) and skip
    // when that amount closes the vault fully. This path honors icUSD the SP
    // has ALREADY burned atomically in the 3pool — rejecting a write-down
    // for a dust vault (full debt below the floor) here would strand that
    // burn with no vault relief, which is worse than the small-position
    // stuck-vault bug this fix targets.

    // Wave-8d LIQ-004 Phase 2 (replay defense + ICRC-3 verification). Verify
    // the proof BEFORE touching any state. If the proof's
    // (ledger_kind, block_index) was already consumed, refuse — pre-
    // mutation — so the caller gets a clean error instead of a stale partial
    // success. Then validate the on-chain block matches expected accounts,
    // amount, and (for IcusdBurn) memo.
    //
    // The Wave-8c migration WARN-on-None branch has been retired in Phase 2.
    if read_state(|s| {
        s.consumed_writedown_proofs
            .contains(&(proof.ledger_kind, proof.block_index))
    }) {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(format!(
            "SP writedown proof replay rejected: ({:?}, block {}) already consumed",
            proof.ledger_kind, proof.block_index
        )));
    }

    let (ledger_principal, reserves_account) = read_state(|s| {
        let ledger = match proof.ledger_kind {
            crate::icrc3_proof::SpProofLedger::IcusdBurn => s.icusd_ledger_principal,
            crate::icrc3_proof::SpProofLedger::ThreePoolTransfer
            | crate::icrc3_proof::SpProofLedger::ThreePoolTransferDefault => {
                s.three_pool_canister.unwrap_or(Principal::anonymous())
            }
        };
        let reserves = icrc_ledger_types::icrc1::account::Account {
            owner: ic_cdk::id(),
            subaccount: match proof.ledger_kind {
                crate::icrc3_proof::SpProofLedger::ThreePoolTransferDefault => None,
                crate::icrc3_proof::SpProofLedger::IcusdBurn
                | crate::icrc3_proof::SpProofLedger::ThreePoolTransfer => {
                    Some(crate::management::protocol_3usd_reserves_subaccount())
                }
            },
        };
        (ledger, reserves)
    });

    if ledger_principal == Principal::anonymous() {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "SP writedown proof references a ledger that is not configured".to_string(),
        ));
    }

    let expected_amount_e8s = match proof.ledger_kind {
        crate::icrc3_proof::SpProofLedger::IcusdBurn => icusd_burned_e8s,
        crate::icrc3_proof::SpProofLedger::ThreePoolTransfer
        | crate::icrc3_proof::SpProofLedger::ThreePoolTransferDefault => {
            three_usd_received_e8s.unwrap_or(0)
        }
    };

    let expectations = crate::icrc3_proof::ProofExpectations {
        ledger_kind: proof.ledger_kind,
        expected_amount_e8s,
        sp_principal: caller,
        reserves_account,
        vault_id_memo: vault_id,
    };

    let proof_validation = if proof.ledger_kind
        == crate::icrc3_proof::SpProofLedger::ThreePoolTransferDefault
    {
        let Some(key) = ingress_key.as_ref() else {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(
                "V2 reserve proof requires its exact persisted ingress identity".into(),
            ));
        };
        if key.stability_pool != caller || key.vault_id != vault_id {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(
                "V2 reserve proof ingress identity does not match the caller and vault".into(),
            ));
        }
        let Some(journal) = crate::management::three_usd_reserve_ingress_journal(key) else {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(
                "V2 reserve proof has no persisted ingress journal".into(),
            ));
        };
        let (tuple, journal_block_index) = match journal.phase {
            crate::state::ThreeUsdReserveIngressPhase::TransferConfirmed { tuple, block_index }
            | crate::state::ThreeUsdReserveIngressPhase::Absorbed {
                tuple, block_index, ..
            }
            | crate::state::ThreeUsdReserveIngressPhase::FailedAfterTransfer {
                tuple,
                block_index,
                ..
            } => (tuple, block_index),
            crate::state::ThreeUsdReserveIngressPhase::AdmissionPending
            | crate::state::ThreeUsdReserveIngressPhase::PreTransferRejected { .. }
            | crate::state::ThreeUsdReserveIngressPhase::SubmittedOrUnknown { .. } => {
                guard_principal.fail();
                return Err(ProtocolError::GenericError(
                    "V2 reserve proof journal has no confirmed transfer block".into(),
                ));
            }
        };
        if journal.request.ledger != ledger_principal
            || journal.request.icusd_debt_covered_e8s != icusd_burned_e8s
            || Some(journal.request.three_usd_amount_e8s) != three_usd_received_e8s
        {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(
                "V2 reserve proof request does not match its persisted ingress journal".into(),
            ));
        }
        crate::icrc3_proof::verify_three_usd_reserve_ingress_block(
            ledger_principal,
            proof.block_index,
            journal_block_index,
            &tuple,
        )
        .await
    } else {
        crate::icrc3_proof::fetch_and_validate_block(
            ledger_principal,
            proof.block_index,
            &expectations,
        )
        .await
        .map(|_| ())
    };
    if let Err(err) = proof_validation {
        guard_principal.fail();
        log!(
            INFO,
            "[liquidate_vault_debt_burned] [LIQ-004] proof verification FAILED for vault #{} \
             ({:?} block {}): {}",
            vault_id,
            proof.ledger_kind,
            proof.block_index,
            err
        );
        return Err(ProtocolError::GenericError(format!(
            "SP writedown proof verification failed: {}",
            err
        )));
    }

    if vault_id != proof.vault_id_memo {
        // For IcusdBurn `validate_block` already enforces this against the
        // memo. For ThreePoolTransfer there is no memo on the block (3pool
        // ledger doesn't persist memos into ICRC-3); this assertion is the
        // single binding point against the call's vault_id, so any internal
        // misconstruction surfaces with a tight error before mutation.
        guard_principal.fail();
        return Err(ProtocolError::GenericError(format!(
            "SP writedown proof vault_id_memo {} does not match call vault_id {}",
            proof.vault_id_memo, vault_id
        )));
    }

    // The reserves branch pulls 3USD after its own preflight canister call.
    // Read the configured pool's current value here, after that await, so the
    // backend itself binds the amount of 3USD to the debt it is about to retire.
    let three_usd_virtual_price = if proof.ledger_kind
        == crate::icrc3_proof::SpProofLedger::ThreePoolTransfer
        || proof.ledger_kind == crate::icrc3_proof::SpProofLedger::ThreePoolTransferDefault
    {
        let Some(three_usd_e8s) = three_usd_received_e8s.filter(|amount| *amount > 0) else {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(
                "Reserves liquidation requires a non-zero 3USD amount.".to_string(),
            ));
        };
        let Some(pool) = read_state(|s| s.three_pool_canister) else {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(
                "The configured 3pool is unavailable for reserves valuation.".to_string(),
            ));
        };
        let virtual_price = match fetch_three_pool_virtual_price(pool).await {
            Ok(value) if value > 0 => value,
            Ok(_) => {
                guard_principal.fail();
                return Err(ProtocolError::GenericError(
                    "The configured 3pool returned a zero virtual price.".to_string(),
                ));
            }
            Err(error) => {
                guard_principal.fail();
                return Err(error);
            }
        };
        Some((three_usd_e8s, virtual_price))
    } else {
        None
    };

    // Step 1: Validate vault is liquidatable and compute collateral to release
    let (
        vault,
        collateral_price,
        config_decimals,
        collateral_price_usd,
        max_liquidatable_debt,
        collateral_to_liquidator,
        total_to_seize,
        protocol_cut,
        liquidation_bonus,
        protocol_share,
        minimum_liquidation_ratio,
    ) = match read_state(|s| {
        match s.vault_id_to_vaults.get(&vault_id) {
            Some(vault) => {
                if let Some(status) = s.get_collateral_status(&vault.collateral_type) {
                    if !status.allows_liquidation() {
                        return Err(
                            "Liquidation is not allowed for this collateral type.".to_string()
                        );
                    }
                }

                let price = s
                    .get_collateral_price_decimal(&vault.collateral_type)
                    .ok_or_else(|| {
                        "No price available for collateral. Price feed may be down.".to_string()
                    })?;
                let decimals = s
                    .get_collateral_config(&vault.collateral_type)
                    .map(|c| c.decimals)
                    .unwrap_or(8);
                let collateral_price_usd = UsdIcp::from(price);

                // The legacy burn path must honor already-burned icUSD even if
                // the vault recovered. The reserves path is checked again below
                // after its 3USD transfer await and can safely reject because
                // the entry point refunds that transfer on error.
                {
                    let actual_liquidation_amount =
                        liquidation_amount.min(vault.borrowed_icusd_amount);

                    if actual_liquidation_amount == ICUSD::new(0) {
                        return Err("Cannot liquidate zero amount".to_string());
                    }

                    let liq_bonus = s.get_liquidation_bonus_for(&vault.collateral_type);
                    let protocol_share = s.get_liquidation_protocol_share();
                    let minimum_liquidation_ratio =
                        s.get_min_liquidation_ratio_for(&vault.collateral_type);
                    let (total_to_seize, protocol_cut) = already_burned_liquidation_seizure(
                        actual_liquidation_amount,
                        price,
                        decimals,
                        liq_bonus,
                        protocol_share,
                        vault.collateral_amount,
                    );
                    let collateral_to_liquidator =
                        ICP::from(total_to_seize.to_u64() - protocol_cut);

                    Ok((
                        vault.clone(),
                        price,
                        decimals,
                        collateral_price_usd,
                        actual_liquidation_amount,
                        collateral_to_liquidator,
                        total_to_seize,
                        protocol_cut,
                        liq_bonus,
                        protocol_share,
                        minimum_liquidation_ratio,
                    ))
                }
            }
            None => Err(format!("Vault #{} not found", vault_id)),
        }
    }) {
        Ok(result) => result,
        Err(msg) => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(msg));
        }
    };

    // LIQ-0XX: min_icusd_amount applies to the FINAL amount and is skipped
    // when that amount closes the vault fully (see comment above — the icUSD
    // was already burned, so rejecting a genuine dust vault here would
    // strand it rather than protect anything).
    if max_liquidatable_debt < read_state(|s| s.min_icusd_amount)
        && max_liquidatable_debt != vault.borrowed_icusd_amount
    {
        guard_principal.fail();
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: read_state(|s| s.min_icusd_amount).to_u64(),
        });
    }

    log!(INFO,
        "[liquidate_vault_debt_burned] Vault #{}: writing down {} icUSD (burned via 3pool), releasing {} collateral (protocol fee: {})",
        vault_id, max_liquidatable_debt.to_u64(), collateral_to_liquidator.to_u64(), protocol_cut
    );

    // The legacy burn path logs healthy-vault write-downs but cannot reject an
    // already-burned amount. The reserves path has a refund route, so it rejects
    // healthy vaults after the transfer await below.
    let pre_call_cr = read_state(|s| compute_collateral_ratio(&vault, collateral_price_usd, s));
    let min_liq = read_state(|s| s.get_min_liquidation_ratio_for(&vault.collateral_type));
    if pre_call_cr >= min_liq {
        if matches!(
            proof.ledger_kind,
            crate::icrc3_proof::SpProofLedger::ThreePoolTransfer
                | crate::icrc3_proof::SpProofLedger::ThreePoolTransferDefault
        ) {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(
                "Reserves liquidation rejected because the vault is no longer liquidatable."
                    .to_string(),
            ));
        }
        log!(INFO,
            "[liquidate_vault_debt_burned] [LIQ-004] WARN: SP writedown applied to vault #{} \
             whose pre-call CR ({}) is above min_liq_ratio ({}). Caller={} proof={:?}. Investigate.",
            vault_id, pre_call_cr.to_f64(), min_liq.to_f64(), caller, proof
        );
    }

    if let Some((three_usd_e8s, virtual_price)) = three_usd_virtual_price {
        if proof.ledger_kind == crate::icrc3_proof::SpProofLedger::ThreePoolTransferDefault
            && !three_usd_live_value_matches_debt_quote(
                three_usd_e8s,
                icusd_burned_e8s,
                virtual_price,
            )
        {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(
                "3USD live virtual price no longer matches the V2 debt quote.".to_string(),
            ));
        }
        if !three_usd_covers_debt(three_usd_e8s, max_liquidatable_debt.to_u64(), virtual_price) {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(
                "3USD reserves value is below the debt amount being retired.".to_string(),
            ));
        }
    }

    // Step 2: SKIPPED — icUSD was already burned atomically in the 3pool.
    // The icUSD supply has already been reduced by `icusd_burned_e8s`.

    // Step 3: Update protocol state (partial liquidation)
    let mut immediate_payouts = Vec::new();
    let (interest_share, committed_result) = match mutate_state(
        |s| -> Result<(ICUSD, StabilityPoolLiquidationResult), ProtocolError> {
            let settlement_time = ic_cdk::api::time();
            let proof_key = (proof.ledger_kind, proof.block_index);
            // Pin every outbound field for V2's Stability Pool collateral receipt
            // before consuming the proof or mutating debt. This check is repeated
            // in the same commit message that installs the payout row and parent
            // link, so the ledger route cannot rotate across the transfer await.
            let v2_payout_binding = if proof.ledger_kind
                == crate::icrc3_proof::SpProofLedger::ThreePoolTransferDefault
            {
                let Some(key) = ingress_key.as_ref() else {
                    return Err(ProtocolError::GenericError(
                        "V2 reserve payout has no explicit ingress parent".into(),
                    ));
                };
                if key.stability_pool != caller
                    || key.vault_id != vault_id
                    || s.stability_pool_canister != Some(caller)
                {
                    return Err(ProtocolError::GenericError(
                        "V2 reserve payout parent is not the registered Stability Pool operation"
                            .into(),
                    ));
                }
                if s.pending_margin_transfers
                    .values()
                    .any(|row| row.vault_id == vault_id && row.owner == caller)
                {
                    return Err(ProtocolError::GenericError(
                        "V2 reserve payout cannot replace an existing margin obligation".into(),
                    ));
                }
                let Some(parent) = s.three_usd_reserve_ingress_journals.get(key) else {
                    return Err(ProtocolError::GenericError(
                        "V2 reserve payout parent journal disappeared before debt commit".into(),
                    ));
                };
                if parent.payout.is_some()
                    || !matches!(&parent.phase,
                    crate::state::ThreeUsdReserveIngressPhase::TransferConfirmed { block_index, .. }
                        if *block_index == proof.block_index)
                    || parent.request.ledger != ledger_principal
                    || parent.request.icusd_debt_covered_e8s != icusd_burned_e8s
                    || Some(parent.request.three_usd_amount_e8s) != three_usd_received_e8s
                {
                    return Err(ProtocolError::GenericError(
                        "V2 reserve payout parent does not match its confirmed proof and request"
                            .into(),
                    ));
                }
                let live = s.vault_id_to_vaults.get(&vault_id).ok_or_else(|| {
                    ProtocolError::GenericError(
                        "V2 reserve payout vault disappeared before commit".into(),
                    )
                })?;
                let Some(config) = s.get_collateral_config(&live.collateral_type) else {
                    return Err(ProtocolError::GenericError(
                        "V2 reserve payout collateral has no pinned ledger configuration".into(),
                    ));
                };
                if config.is_native_xrp() || config.ledger_canister_id == Principal::anonymous() {
                    return Err(ProtocolError::GenericError(
                        "V2 reserve payout requires a configured non-XRP collateral ledger".into(),
                    ));
                }
                let live_gross = total_to_seize
                    .to_u64()
                    .min(live.collateral_amount)
                    .saturating_sub(protocol_cut.min(live.collateral_amount));
                let live_net = live_gross
                    .checked_sub(config.ledger_fee)
                    .filter(|amount| *amount > 0)
                    .ok_or_else(|| {
                        ProtocolError::GenericError(
                            "V2 reserve payout cannot cover the pinned collateral ledger fee"
                                .into(),
                        )
                    })?;
                Some((
                    key.clone(),
                    config.ledger_canister_id,
                    config.ledger_fee,
                    s.payout_proof_kind_for_ledger(config.ledger_canister_id),
                    live.collateral_type,
                    live_gross,
                    live_net,
                ))
            } else {
                None
            };
            if proof.ledger_kind == crate::icrc3_proof::SpProofLedger::ThreePoolTransferDefault {
                if v2_reserve_commit_is_paused(s) {
                    return Err(ProtocolError::TemporarilyUnavailable(
                    "V2 reserve liquidation was paused before accounting commit; exact refund reconciliation is required".into(),
                ));
                }
                let live_vault = s.vault_id_to_vaults.get(&vault_id).ok_or_else(|| {
                    ProtocolError::GenericError("V2 reserve vault disappeared before commit".into())
                })?;
                let live_price = s.get_collateral_price_decimal(&live_vault.collateral_type);
                let live_decimals = s
                    .get_collateral_config(&live_vault.collateral_type)
                    .map(|config| config.decimals)
                    .unwrap_or(8);
                let live_status_allows = s
                    .get_collateral_status(&live_vault.collateral_type)
                    .is_none_or(|status| status.allows_liquidation());
                let live_ratio = compute_collateral_ratio(
                    live_vault,
                    s.last_icp_rate.unwrap_or_else(|| UsdIcp::from(dec!(0.0))),
                    s,
                );
                if s.three_pool_canister != Some(ledger_principal)
                    || live_vault.borrowed_icusd_amount != vault.borrowed_icusd_amount
                    || live_vault.collateral_amount != vault.collateral_amount
                    || live_vault.accrued_interest != vault.accrued_interest
                    || live_price != Some(collateral_price)
                    || live_decimals != config_decimals
                    || !live_status_allows
                    || s.get_liquidation_bonus_for(&live_vault.collateral_type) != liquidation_bonus
                    || s.get_liquidation_protocol_share() != protocol_share
                    || s.get_min_liquidation_ratio_for(&live_vault.collateral_type)
                        != minimum_liquidation_ratio
                    || live_ratio >= minimum_liquidation_ratio
                {
                    return Err(ProtocolError::GenericError(
                    "V2 reserve liquidation inputs changed before atomic commit; transfer retained for exact refund reconciliation".into(),
                ));
                }
            }
            // A pulled 3USD ingress must reach the typed failure/refund path if
            // reserve accounting cannot represent its realized credit. Check
            // before consuming the proof or changing debt: returning Err after a
            // state mutation would not roll those mutations back.
            let prospective_debt_applied = s
                .vault_id_to_vaults
                .get(&vault_id)
                .map(|live| max_liquidatable_debt.min(live.borrowed_icusd_amount))
                .unwrap_or(max_liquidatable_debt);
            let prospective_event_debt = if proof.ledger_kind
                == crate::icrc3_proof::SpProofLedger::ThreePoolTransferDefault
            {
                prospective_debt_applied
            } else {
                max_liquidatable_debt
            };
            let reserve_realized_prechecked = preflight_three_usd_reserve_credit(
                s.protocol_3usd_reserves,
                three_usd_received_e8s,
                icusd_burned_e8s,
                prospective_event_debt.to_u64(),
            )?;
            if !s.consumed_writedown_proofs.insert(proof_key) {
                return Err(ProtocolError::GenericError(format!(
                "SP writedown proof replay rejected: ({:?}, block {}) was consumed while verifying",
                proof.ledger_kind, proof.block_index
            )));
            }
            // Wave-8c LIQ-004: record the proof as consumed atomically with the
            // writedown so a partial failure cannot leave the proof unconsumed
            // (replay risk) or consumed without an effect (orphan risk).
            let interest_share = if let Some(vault) = s.vault_id_to_vaults.get(&vault_id) {
                if vault.accrued_interest.0 > 0 && vault.borrowed_icusd_amount.0 > 0 {
                    ICUSD::new(crate::numeric::proportional_interest_share(
                        max_liquidatable_debt.0,
                        vault.accrued_interest.0,
                        vault.borrowed_icusd_amount.0,
                    ))
                } else {
                    ICUSD::new(0)
                }
            } else {
                ICUSD::new(0)
            };

            // AR-B-001/BK-001 (audit 2026-06-09): capture applied amounts and
            // re-cap the payout, mirroring `liquidate_vault_partial`.
            let mut debt_applied = max_liquidatable_debt;
            let mut collateral_applied = total_to_seize.to_u64();
            if let Some(vault) = s.vault_id_to_vaults.get_mut(&vault_id) {
                // ASYNC-001: cap each reduction to the CURRENT vault state and
                // saturating_sub. A concurrent partial liquidation may have reduced
                // this vault between our pre-await read and now; without the cap the
                // ICUSD Token::sub would underflow-PANIC and the raw u64 collateral
                // sub would WRAP, both after the liquidator's icUSD was already pulled.
                debt_applied = max_liquidatable_debt.min(vault.borrowed_icusd_amount);
                collateral_applied = total_to_seize.to_u64().min(vault.collateral_amount);
                let interest_applied = interest_share.min(vault.accrued_interest);
                vault.borrowed_icusd_amount =
                    vault.borrowed_icusd_amount.saturating_sub(debt_applied);
                vault.collateral_amount =
                    vault.collateral_amount.saturating_sub(collateral_applied);
                vault.accrued_interest = vault.accrued_interest.saturating_sub(interest_applied);
            }
            let payout_to_liquidator = ICP::from(collateral_applied.saturating_sub(protocol_cut));

            // Wave-10 LIQ-008: append the gross debt cleared to the rolling-
            // window log. SP writedowns count toward the breaker — a flood of
            // SP-absorbed liquidations is still a stress signal worth pausing
            // bot/SP auto-publishing on.
            let event_debt = if proof.ledger_kind
                == crate::icrc3_proof::SpProofLedger::ThreePoolTransferDefault
            {
                debt_applied
            } else {
                max_liquidatable_debt
            };
            crate::event::record_liquidation_for_breaker(s, event_debt.to_u64());

            // Wave-8e LIQ-005: per-call deficit accrual on the SP writedown
            // path, against the APPLIED amounts. Even though icUSD was burned
            // externally (legacy 3pool burn) or 3USD reserves were credited
            // (reserves path), the protocol's solvency invariant is still:
            // seized collateral USD value vs. debt cleared. If the SP absorbed
            // an underwater vault, the protocol records the shortfall here so
            // future fee revenue burns it down — this is what the audit
            // (LIQ-005) prescribes instead of socializing onto SP depositors.
            let seized_usd = crate::numeric::collateral_usd_value(
                collateral_applied,
                collateral_price,
                config_decimals,
            );
            let shortfall = if seized_usd < debt_applied {
                debt_applied - seized_usd
            } else {
                ICUSD::new(0)
            };
            if shortfall.0 > 0 {
                crate::event::record_deficit_accrued(
                    s,
                    crate::event::DeficitSource::Liquidation { vault_id },
                    shortfall,
                    settlement_time,
                );
                if s.check_deficit_readonly_latch() {
                    log!(INFO,
                    "[LIQ-005] deficit threshold {} crossed by SP writedown vault #{} shortfall {}; auto-latched ReadOnly",
                    s.deficit_readonly_threshold_e8s, vault_id, shortfall.to_u64()
                );
                }
            }

            // Commit the applied fee obligation with the proof consumption and
            // accounting event; native-XRP developer claims are pinned by exact id.
            // AR-B-001/BK-001 (audit 2026-06-09): applied payout, replay-exact.
            let reserve_realized_e8s = reserve_realized_prechecked;
            let event = crate::event::Event::PartialLiquidateVault {
                vault_id,
                liquidator_payment: event_debt,
                icp_to_liquidator: payout_to_liquidator,
                liquidator: Some(caller),
                icp_rate: Some(collateral_price_usd),
                protocol_fee_collateral: if protocol_cut > 0 {
                    Some(protocol_cut.min(collateral_applied))
                } else {
                    None
                },
                timestamp: Some(settlement_time),
                three_usd_reserves_e8s: reserve_realized_e8s,
            };
            crate::storage::record_event(&event);

            // Track 3USD reserves at runtime (also persisted via event replay)
            if let Some(three_usd_e8s) = reserve_realized_e8s {
                s.protocol_3usd_reserves = s
                    .protocol_3usd_reserves
                    .checked_add(three_usd_e8s)
                    .expect("3USD reserve capacity was checked before debt commit");
            }

            let mut nonce = s.next_op_nonce();
            while s.three_usd_reserve_payout_parents.contains_key(&nonce) {
                nonce = s.next_op_nonce();
            }
            queue_collateral_payout(
                s,
                vault_id,
                vault.owner,
                caller,
                payout_to_liquidator,
                vault.collateral_type,
                nonce,
                ic_cdk::api::time(),
                &mut immediate_payouts,
            );
            if let Some((key, ledger, fee, proof_kind, collateral_type, gross, net)) =
                v2_payout_binding
            {
                let row = s.pending_margin_transfers.get(&nonce).expect(
                    "validated non-XRP V2 payout configuration must enqueue a margin transfer",
                );
                assert!(
                    row.op_nonce == nonce
                        && row.owner == key.stability_pool
                        && row.margin.to_u64() == gross
                        && row.collateral_type == collateral_type,
                    "V2 reserve collateral payout differs from its commit-time pins"
                );
                let memo = crate::management::nonce_to_memo(nonce);
                let payout_tuple = crate::state::ThreeUsdReserveIngressPayoutTuple {
                    op_nonce: nonce,
                    ledger,
                    proof_kind,
                    source: icrc_ledger_types::icrc1::account::Account {
                        owner: ic_cdk::id(),
                        subaccount: None,
                    },
                    destination: icrc_ledger_types::icrc1::account::Account {
                        owner: key.stability_pool,
                        subaccount: None,
                    },
                    gross_amount_e8s: gross,
                    net_amount_e8s: net,
                    fee_e8s: fee,
                    memo: memo
                        .0
                        .as_slice()
                        .try_into()
                        .expect("nonce memo is exactly 16 bytes"),
                    created_at_time_ns: crate::management::nonce_to_created_at_time(nonce),
                    collateral_type,
                };
                s.three_usd_reserve_payout_parents
                    .insert(nonce, key.clone());
                let parent = s
                    .three_usd_reserve_ingress_journals
                    .get_mut(&key)
                    .expect("validated V2 reserve parent must survive the atomic debt commit");
                parent.payout = Some(crate::state::ThreeUsdReserveIngressPayout {
                    tuple: payout_tuple,
                    receipt: None,
                });
            }

            // Shared drain rule (see state::cleanup_if_drained): remove the vault
            // if this liquidation emptied it, else re-key its CR index entry.
            // The band gate that originally consumed the CR index was deactivated
            // 2026-05-18, but `check_vaults`' at-risk-band sharding (Wave-9c
            // DOS-005) still relies on accurate CR keys.
            if s.cleanup_if_drained(vault_id) {
                log!(
                    INFO,
                    "[liquidate_vault_debt_burned] Vault #{} fully liquidated — removed",
                    vault_id
                );
            }

            let result_debt = event_debt.to_u64();
            let result_collateral = if proof.ledger_kind
                == crate::icrc3_proof::SpProofLedger::ThreePoolTransferDefault
            {
                payout_to_liquidator.to_u64()
            } else {
                collateral_to_liquidator.to_u64()
            };
            let liquidator_value_received = crate::numeric::collateral_usd_value(
                result_collateral,
                collateral_price,
                config_decimals,
            );
            let fee_amount = if liquidator_value_received > event_debt {
                liquidator_value_received - event_debt
            } else {
                ICUSD::new(0)
            };
            let result = StabilityPoolLiquidationResult {
                success: true,
                vault_id,
                liquidated_debt: result_debt,
                collateral_received: result_collateral,
                collateral_type: vault.collateral_type.to_string(),
                block_index: 0,
                fee: fee_amount.to_u64(),
                collateral_price_e8s: collateral_price_usd.to_e8s(),
            };
            if proof.ledger_kind == crate::icrc3_proof::SpProofLedger::ThreePoolTransferDefault {
                let stored = crate::state::StoredThreeUsdReserveAbsorbResult {
                    caller,
                    vault_id,
                    icusd_debt_covered_e8s: icusd_burned_e8s,
                    three_usd_amount_e8s: three_usd_received_e8s.unwrap_or(0),
                    ledger: ledger_principal,
                    proof: proof.clone(),
                    result: result.clone(),
                };
                s.sp_three_usd_reserve_absorb_results_by_proof
                    .insert(proof_key, stored);
            }
            Ok((interest_share, result))
        },
    ) {
        Ok(result) => result,
        Err(err) => {
            guard_principal.fail();
            return Err(err);
        }
    };

    // Route interest share via N-way split
    // IC-B-002 (audit 2026-06-09): re-queue any unminted interest share so the
    // next flush retries it instead of silently dropping treasury revenue.
    let unminted_interest =
        crate::treasury::distribute_interest(interest_share, vault.collateral_type).await;
    if unminted_interest.to_u64() > 0 {
        mutate_state(|s| {
            s.restore_pending_interest_for_pool(vault.collateral_type, unminted_interest.to_u64())
        });
    }

    // Fee obligations and native-XRP developer claims were persisted atomically
    // with the liquidation accounting event above; outbox delivery is retried
    // independently and cannot change the committed amounts.

    // Step 4: Process collateral transfer to stability pool
    match try_process_pending_transfers_immediate(&immediate_payouts).await {
        Ok(processed_count) => {
            log!(
                INFO,
                "[liquidate_vault_debt_burned] Processed {} transfers immediately",
                processed_count
            );
        }
        Err(e) => {
            log!(
                INFO,
                "[liquidate_vault_debt_burned] Immediate processing failed: {}. Retrying via timer",
                e
            );
            schedule_transfer_retry(vault_id, immediate_payouts.clone(), 0);
        }
    }

    ic_cdk_timers::set_timer(std::time::Duration::from_secs(2), move || {
        ic_cdk::spawn(async move {
            log!(
                INFO,
                "[liquidate_vault_debt_burned] Backup timer for vault #{}",
                vault_id
            );
            let _ = crate::process_pending_transfer().await;
        })
    });

    guard_principal.complete();

    log!(
        INFO,
        "[liquidate_vault_debt_burned] Completed. Fee: {}, Collateral: {}",
        committed_result.fee,
        committed_result.collateral_received
    );

    Ok(committed_result)
}

pub async fn liquidate_vault(vault_id: u64) -> Result<SuccessWithFee, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let guard_principal = GuardPrincipal::new(caller, &format!("liquidate_vault_{}", vault_id))?;
    reject_if_bot_processing(vault_id)?; // LIQ-101: don't double-seize a bot-claimed vault
    let _vault_liq_guard = VaultLiquidationGuard::new(vault_id)?; // BK-001/002 per-vault lock
    if let Err(e) = reject_active_xrp_sp_absorb_preflight(vault_id, ic_cdk::api::time()) {
        guard_principal.fail();
        return Err(e);
    }

    // Wave-8b LIQ-002 band gate deactivated 2026-05-18 (see
    // `liquidate_vault_partial` above for rationale).

    // Step 1: Validate vault is liquidatable
    let (vault, collateral_price, config_decimals, collateral_price_usd, mode) =
        match read_state(|s| {
            match s.vault_id_to_vaults.get(&vault_id) {
                Some(vault) => {
                    // Check collateral status allows liquidation
                    if let Some(status) = s.get_collateral_status(&vault.collateral_type) {
                        if !status.allows_liquidation() {
                            return Err(
                                "Liquidation is not allowed for this collateral type.".to_string()
                            );
                        }
                    }

                    let price = s
                        .get_collateral_price_decimal(&vault.collateral_type)
                        .ok_or_else(|| {
                            "No price available for collateral. Price feed may be down.".to_string()
                        })?;
                    let decimals = s
                        .get_collateral_config(&vault.collateral_type)
                        .map(|c| c.decimals)
                        .unwrap_or(8);
                    let collateral_price_usd = UsdIcp::from(price);
                    let ratio = compute_collateral_ratio(vault, collateral_price_usd, s);
                    let min_liq_ratio = s.get_min_liquidation_ratio_for(&vault.collateral_type);

                    if ratio >= min_liq_ratio {
                        Err(format!(
                            "Vault #{} is not liquidatable. Current ratio: {}, minimum: {}",
                            vault_id,
                            ratio.to_f64(),
                            min_liq_ratio.to_f64()
                        ))
                    } else {
                        Ok((vault.clone(), price, decimals, collateral_price_usd, s.mode))
                    }
                }
                None => Err(format!("Vault #{} not found", vault_id)),
            }
        }) {
            Ok(result) => result,
            Err(msg) => {
                guard_principal.fail();
                return Err(ProtocolError::GenericError(msg));
            }
        };

    // Step 2: Calculate liquidation amounts.
    // LIQ-0XX: `debt_amount` is decided by `effective_liquidation_amount`,
    // the single shared cap/dust decision point — applied in ALL modes, not
    // just Recovery (this is the approved backend enforcement of the
    // partial-liquidation cap; see the helper's doc comment). Previously
    // this branched only on `compute_recovery_repay_cap` (Recovery mode) and
    // otherwise ALWAYS took the full debt uncapped in GeneralAvailability —
    // that asymmetry is what let a small vault's cap fall below
    // `min_icusd_amount` with no partial-liquidation endpoint able to accept
    // it. `is_partial_liquidation` (was `is_recovery_partial`) now means
    // "the helper returned less than the full debt", covering both the
    // recovery-target-CR case and the (new) GA-mode partial-cap case.
    let vault_collateral = ICP::from(vault.collateral_amount);
    let (
        debt_amount,
        collateral_to_liquidator,
        total_to_seize,
        protocol_cut,
        excess_collateral,
        is_partial_liquidation,
        collateral_conversion_succeeded,
    ) = read_state(|s| {
        let liq_bonus = s.get_liquidation_bonus_for(&vault.collateral_type);
        let protocol_share = s.get_liquidation_protocol_share();
        let debt = s.effective_liquidation_amount(&vault, collateral_price_usd, None);
        let is_partial = debt < vault.borrowed_icusd_amount;
        let collateral_raw =
            crate::numeric::try_icusd_to_collateral_amount(debt, collateral_price, config_decimals);
        let Some(collateral_raw) = collateral_raw else {
            return (
                debt,
                ICP::new(0),
                ICP::new(0),
                0,
                ICP::new(0),
                is_partial,
                false,
            );
        };
        let total_to_seize = (ICP::from(collateral_raw) * liq_bonus).min(vault_collateral);
        // Split: protocol gets a share of the bonus portion (liquidator's profit)
        let bonus_portion = total_to_seize.to_u64().saturating_sub(collateral_raw);
        let protocol_cut = (rust_decimal::Decimal::from(bonus_portion) * protocol_share.0)
            .to_u64()
            .unwrap_or(0);
        let collateral_to_liquidator = ICP::from(total_to_seize.to_u64() - protocol_cut);
        // Excess collateral only returns to the owner on a full liquidation —
        // a partial liquidation leaves the vault open with its remaining
        // collateral backing its remaining debt.
        let excess = if is_partial {
            ICP::new(0)
        } else {
            vault_collateral.saturating_sub(total_to_seize)
        };
        (
            debt,
            collateral_to_liquidator,
            total_to_seize,
            protocol_cut,
            excess,
            is_partial,
            true,
        )
    });

    if !collateral_conversion_succeeded {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "Cannot safely size liquidation collateral: conversion is unrepresentable".to_string(),
        ));
    }
    if debt_amount > ICUSD::new(0)
        && (collateral_to_liquidator == ICP::new(0) || total_to_seize == ICP::new(0))
    {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "Liquidation would produce no collateral payout".to_string(),
        ));
    }

    log!(INFO,
        "[liquidate_vault] Vault #{}: debt_to_repay={} icUSD, liquidator gets {} ICP (protocol fee: {} ICP), excess={} ICP, partial={}",
        vault_id,
        debt_amount.to_u64(),
        collateral_to_liquidator.to_u64(),
        protocol_cut,
        excess_collateral.to_u64(),
        is_partial_liquidation
    );

    // Step 3: Take icUSD from liquidator (this must succeed for liquidation to proceed)
    reject_pending_collateral_withdrawal(vault_id)?;
    let icusd_block_index = match transfer_icusd_from(debt_amount, caller).await {
        Ok(block_index) => {
            log!(
                INFO,
                "[liquidate_vault] Received {} icUSD from liquidator",
                debt_amount.to_u64()
            );
            block_index
        }
        Err(transfer_from_error) => {
            guard_principal.fail();
            return Err(ProtocolError::TransferFromError(
                transfer_from_error,
                debt_amount.to_u64(),
            ));
        }
    };

    // Step 4: Update protocol state ATOMICALLY (this is the critical section).
    // ASYNC-002: a concurrent full liquidation may have removed this vault between
    // our pre-await read and the icUSD pull above. Detect it BEFORE any
    // irreversible state work and refund the liquidator (None branch below)
    // instead of trapping inside s.liquidate_vault()'s vault lookup.
    let mut immediate_payouts = Vec::new();
    let (interest_share, xrp_claim_id) = match mutate_state(|s| {
        if !s.vault_id_to_vaults.contains_key(&vault_id) {
            return None;
        }
        // AR-B-001/BK-001 (audit 2026-06-09): re-cap every collateral payout
        // to the vault's LIVE collateral at commit time. The split below was
        // computed from the pre-await snapshot; the per-vault op lock makes a
        // concurrent reduction unreachable, and this clamp keeps any residual
        // drift solvency-safe (protocol cut first, then liquidator, then the
        // owner's excess — never more than the vault actually holds).
        let live_collateral = s
            .vault_id_to_vaults
            .get(&vault_id)
            .map(|v| v.collateral_amount)
            .unwrap_or(0);
        let cut_applied = protocol_cut.min(live_collateral);
        let liquidator_pay = ICP::from(
            collateral_to_liquidator
                .to_u64()
                .min(live_collateral.saturating_sub(cut_applied)),
        );
        let excess_pay = ICP::from(
            excess_collateral.to_u64().min(
                live_collateral
                    .saturating_sub(cut_applied)
                    .saturating_sub(liquidator_pay.to_u64()),
            ),
        );

        // Execute the liquidation in state first (this must happen).
        // LIQ-0XX (review finding 1): pass the PINNED `debt_amount` decided
        // pre-await (Step 2 above, before `transfer_icusd_from(...).await`)
        // so the amount applied here always equals the amount pulled from
        // the liquidator, even if an admin setter (e.g.
        // `set_dust_liquidation_threshold`) landed during the await.
        // liquidate_vault returns the interest share of the debt reduction.
        let interest_share = s.liquidate_vault_with_pinned_seize(
            vault_id,
            mode,
            collateral_price_usd,
            Some(debt_amount),
            total_to_seize.to_u64(),
        );

        // Wave-10 LIQ-008: append the gross debt cleared to the rolling-
        // window log for the mass-liquidation circuit breaker.
        crate::event::record_liquidation_for_breaker(s, debt_amount.to_u64());

        // Wave-8e LIQ-005: if seized USD < debt cleared, the protocol
        // absorbed bad debt. Track the shortfall in `protocol_deficit_icusd`
        // and check the ReadOnly latch. The liquidator's icUSD payment was
        // already burned via `transfer_icusd_from` (the protocol IS the
        // icUSD minting account), so the supply side is consistent — the
        // liquidator effectively paid `debt_amount` icUSD for collateral
        // worth less, and the protocol now records the outstanding loss.
        let seized_usd = crate::numeric::collateral_usd_value(
            total_to_seize.to_u64().min(live_collateral),
            collateral_price,
            config_decimals,
        );
        let shortfall = if seized_usd < debt_amount {
            debt_amount - seized_usd
        } else {
            ICUSD::new(0)
        };
        if shortfall.0 > 0 {
            crate::event::record_deficit_accrued(
                s,
                crate::event::DeficitSource::Liquidation { vault_id },
                shortfall,
                ic_cdk::api::time(),
            );
            if s.check_deficit_readonly_latch() {
                log!(INFO,
                    "[LIQ-005] deficit threshold {} crossed by vault #{} shortfall {}; auto-latched ReadOnly",
                    s.deficit_readonly_threshold_e8s, vault_id, shortfall.to_u64()
                );
            }
        }

        // Record the liquidation event. LIQ-0XX (review finding 2): record
        // the realized `debt_amount` so replay applies exactly this amount
        // instead of recomputing (and potentially diverging) on upgrade.
        let event = crate::event::Event::LiquidateVault {
            vault_id,
            mode,
            icp_rate: collateral_price_usd,
            liquidator: Some(caller),
            timestamp: Some(ic_cdk::api::time()),
            repay_amount: Some(debt_amount),
            collateral_seized_raw: Some(total_to_seize.to_u64()),
        };
        crate::storage::record_event(&event);

        // Create pending transfer for liquidator reward (minus protocol cut)
        let liquidator_nonce = s.next_op_nonce();
        let xrp_claim_id = queue_collateral_payout(
            s,
            vault_id,
            vault.owner,
            caller,
            liquidator_pay,
            vault.collateral_type,
            liquidator_nonce,
            ic_cdk::api::time(),
            &mut immediate_payouts,
        );

        // Create pending transfer for excess collateral to vault owner (if any)
        // (only for full liquidations, not partial)
        if !is_partial_liquidation && excess_pay > ICP::new(0) {
            log!(
                INFO,
                "[liquidate_vault] Scheduling excess collateral return to vault owner"
            );
            // Native-XRP excess returns to the owner as an XrpClaim; ICRC excess
            // goes through the pending-excess transfer machinery.
            if s.get_collateral_config(&vault.collateral_type)
                .map(|c| c.is_native_xrp())
                .unwrap_or(false)
            {
                record_xrp_claim(
                    s,
                    vault.owner,
                    vault.owner,
                    vault_id,
                    excess_pay.to_u64(),
                    ic_cdk::api::time(),
                );
            } else {
                let excess_nonce = s.next_op_nonce();
                let (ledger, fee) = s
                    .get_collateral_config(&vault.collateral_type)
                    .map(|config| (config.ledger_canister_id, config.ledger_fee))
                    .unwrap_or((s.icp_ledger_principal, s.icp_ledger_fee.to_u64()));
                let transfer_amount_raw = excess_pay.to_u64().saturating_sub(fee);
                let transfer = PendingMarginTransfer {
                    vault_id,
                    owner: vault.owner,
                    margin: excess_pay,
                    collateral_type: vault.collateral_type,
                    retry_count: 0,
                    op_nonce: excess_nonce,
                    ledger: Some(ledger),
                    transfer_amount_raw: Some(transfer_amount_raw),
                    redemption_transfer: None,
                    held_for_manual_retry: transfer_amount_raw == 0,
                    reconciliation_required: transfer_amount_raw == 0,
                    min_net_collateral_raw: None,
                };
                crate::event::record_pending_payout_queued(
                    s,
                    excess_nonce,
                    crate::event::PendingPayoutKind::Excess,
                    transfer,
                );
                immediate_payouts.push((crate::event::PendingPayoutKind::Excess, excess_nonce));
            }
        }

        log!(
            INFO,
            "[liquidate_vault] Protocol state updated, {} pending transfers created",
            if !is_partial_liquidation && excess_pay > ICP::new(0) {
                2
            } else {
                1
            }
        );
        Some((interest_share, xrp_claim_id))
    }) {
        Some(result) => result,
        None => {
            // ASYNC-002: the vault was liquidated by a concurrent op while our
            // icUSD pull was in flight. Refund the liquidator (mirrors the
            // redeem_reserves durable-refund saga) and return a clean error
            // instead of trapping with the liquidator's icUSD stuck.
            guard_principal.fail();
            log!(INFO,
                "[liquidate_vault] Vault #{} already liquidated by a concurrent op; refunding {} icUSD to {}",
                vault_id, debt_amount.to_u64(), caller);
            let refund_nonce = mutate_state(|s| s.next_op_nonce());
            match management::transfer_icusd_with_nonce(debt_amount, caller, refund_nonce).await {
                Ok(refund_block) => {
                    log!(
                        INFO,
                        "[liquidate_vault] Refunded {} icUSD to {} (block {})",
                        debt_amount.to_u64(),
                        caller,
                        refund_block
                    );
                }
                Err(refund_err) => {
                    log!(INFO,
                        "[liquidate_vault] Vault gone AND inline icUSD refund failed for {}: {:?}. \
                         Enqueueing durable refund (block {}).",
                        caller, refund_err, icusd_block_index);
                    mutate_state(|s| {
                        s.pending_refunds.insert(
                            icusd_block_index,
                            crate::state::PendingRefund {
                                user: caller,
                                amount_e8s: debt_amount.to_u64(),
                                retry_count: 0,
                                op_nonce: refund_nonce,
                            },
                        );
                    });
                    ic_cdk_timers::set_timer(std::time::Duration::from_secs(2), || {
                        ic_cdk::spawn(crate::process_pending_transfer())
                    });
                }
            }
            return Err(ProtocolError::GenericError(format!(
                "Vault #{} was already liquidated by a concurrent operation; your {} icUSD has been refunded",
                vault_id, debt_amount.to_u64()
            )));
        }
    };

    // Route interest share via N-way split
    // IC-B-002 (audit 2026-06-09): re-queue any unminted interest share so the
    // next flush retries it instead of silently dropping treasury revenue.
    let unminted_interest =
        crate::treasury::distribute_interest(interest_share, vault.collateral_type).await;
    if unminted_interest.to_u64() > 0 {
        mutate_state(|s| {
            s.restore_pending_interest_for_pool(vault.collateral_type, unminted_interest.to_u64())
        });
    }

    // Send protocol's liquidation fee cut to treasury (fire-and-forget)
    if protocol_cut > 0 {
        if vault.collateral_type == crate::state::xrp_collateral_principal() {
            // P5: native-XRP protocol fee -> a developer-settleable XrpClaim (the
            // ICRC treasury transfer cannot target the synthetic XRP ledger). Keyed
            // by collateral_type (not a vault lookup, since the vault may already be
            // drained/removed by cleanup_if_drained above).
            let dev = read_state(|s| s.developer_principal);
            let now_ns = ic_cdk::api::time();
            mutate_state(|s| {
                record_xrp_claim(
                    s,
                    dev,
                    vault.owner,
                    vault.vault_id,
                    protocol_cut.to_u64().unwrap_or(0),
                    now_ns,
                );
            });
        } else {
            let asset_type = crate::treasury::collateral_to_asset_type(&vault.collateral_type);
            crate::treasury::send_liquidation_fee_to_treasury(
                protocol_cut,
                vault.collateral_type,
                asset_type,
            )
            .await;
        }
    }

    // Step 5: Attempt immediate transfer processing (best effort)
    log!(
        INFO,
        "[liquidate_vault] Attempting immediate transfer processing..."
    );

    // Try to process transfers immediately
    match try_process_pending_transfers_immediate(&immediate_payouts).await {
        Ok(processed_count) => {
            log!(
                INFO,
                "[liquidate_vault] Successfully processed {} transfers immediately",
                processed_count
            );
        }
        Err(e) => {
            log!(INFO, "[liquidate_vault] Immediate processing failed: {}. Transfers will be retried via timer", e);

            // Schedule retry with exponential backoff
            schedule_transfer_retry(vault_id, immediate_payouts.clone(), 0);
        }
    }

    // Step 6: Always schedule a backup timer (in case immediate processing failed)
    ic_cdk_timers::set_timer(std::time::Duration::from_secs(2), move || {
        ic_cdk::spawn(async move {
            log!(
                INFO,
                "[liquidate_vault] Backup timer processing transfers for vault #{}",
                vault_id
            );
            let _ = crate::process_pending_transfer().await;
        })
    });

    // Step 7: Liquidation is successful (protocol state is consistent)
    guard_principal.complete();

    // Calculate fee
    let liquidator_value_received = crate::numeric::collateral_usd_value(
        collateral_to_liquidator.to_u64(),
        collateral_price,
        config_decimals,
    );
    let fee_amount = if liquidator_value_received > debt_amount {
        liquidator_value_received - debt_amount
    } else {
        ICUSD::new(0)
    };

    log!(INFO, "[liquidate_vault] Liquidation completed successfully. Block index: {}, Fee: {}, Collateral: {}",
         icusd_block_index, fee_amount.to_u64(), collateral_to_liquidator.to_u64());

    Ok(SuccessWithFee {
        block_index: icusd_block_index,
        fee_amount_paid: fee_amount.to_u64(),
        collateral_amount_received: Some(collateral_to_liquidator.to_u64()),
        // Review finding 4: expose the authoritative realized amount so the
        // frontend success message doesn't have to trust its own local
        // prediction. `debt_amount` is the SAME pinned amount applied to the
        // vault (see the `Some(debt_amount)` passed to `s.liquidate_vault`
        // above), so this is exact, not an estimate.
        debt_liquidated_e8s: Some(debt_amount.to_u64()),
        stable_pulled_e6s: None, // SP-110 (icUSD path: no stable surcharge)
        xrp_claim_id,
    })
}

// Helper function to attempt immediate transfer processing
async fn try_process_pending_transfers_immediate(
    operation_ids: &[PendingPayoutRef],
) -> Result<u32, String> {
    let mut processed_count = 0;

    // Process only operation ids created by this liquidation. The background
    // timer handles other queued obligations without an unbounded vault scan.
    let transfers_to_process = read_state(|s| {
        operation_ids
            .iter()
            .filter_map(|(kind, operation_id)| {
                let transfer = match kind {
                    crate::event::PendingPayoutKind::Margin => {
                        s.pending_margin_transfers.get(operation_id)
                    }
                    crate::event::PendingPayoutKind::Excess => {
                        s.pending_excess_transfers.get(operation_id)
                    }
                }?;
                Some((*kind, *operation_id, *transfer))
            })
            .collect::<Vec<_>>()
    });

    // Process each transfer
    for (kind, operation_id, transfer) in transfers_to_process {
        // A V2 3USD absorb has a persisted exact payout tuple linked to the
        // committed debt write-down. Never reprice it through the legacy
        // margin path, and never remove its queue row on a transfer reply
        // alone; retain the obligation until the exact receipt is verified.
        let v2_payout = read_state(|state| {
            state
                .three_usd_reserve_payout_parents
                .get(&operation_id)
                .map(|key| {
                    state
                        .three_usd_reserve_ingress_journals
                        .get(key)
                        .and_then(|journal| journal.payout.as_ref())
                        .map(|payout| payout.tuple.clone())
                })
        });
        if let Some(v2_payout) = v2_payout {
            let Some(tuple) = v2_payout else {
                log!(INFO, "[immediate_transfer] Holding V2 reserve payout {} because its pinned tuple is missing", operation_id);
                continue;
            };
            if kind != crate::event::PendingPayoutKind::Margin
                || tuple.op_nonce != operation_id
                || tuple.ledger == Principal::anonymous()
                || tuple.source.owner != ic_cdk::id()
                || tuple.source.subaccount.is_some()
                || tuple.destination.owner != transfer.owner
                || tuple.destination.subaccount.is_some()
                || tuple.gross_amount_e8s != transfer.margin.to_u64()
                || tuple.collateral_type != transfer.collateral_type
            {
                log!(INFO, "[immediate_transfer] Holding V2 reserve payout {} because its queue tuple does not match", transfer.op_nonce);
                continue;
            }
            match management::transfer_idempotent_exact(
                tuple.ledger,
                None,
                tuple.destination,
                u128::from(tuple.net_amount_e8s),
                tuple.fee_e8s,
                icrc_ledger_types::icrc1::transfer::Memo::from(tuple.memo.to_vec()),
                tuple.created_at_time_ns,
            ).await {
                Ok(block_index) => match verify_and_record_three_usd_reserve_payout(
                    transfer.op_nonce, block_index,
                ).await {
                    Ok(Some(true)) => processed_count += 1,
                    Ok(Some(false)) | Ok(None) => log!(INFO,
                        "[immediate_transfer] V2 reserve payout {} remains pending until its exact receipt is committed", transfer.op_nonce),
                    Err(error) => log!(INFO,
                        "[immediate_transfer] V2 reserve payout {} remains pending because receipt verification failed: {}", transfer.op_nonce, error),
                },
                Err(error) => log!(INFO,
                    "[immediate_transfer] V2 reserve payout {} remains pending after exact transfer attempt: {}", transfer.op_nonce, error),
            }
            continue;
        }
        if transfer.held_for_manual_retry || transfer.reconciliation_required {
            continue;
        }
        // Note: native-XRP collateral never reaches this ICRC processor — it is
        // converted to an XrpClaim at the moment of liquidation/withdrawal (see
        // `queue_collateral_payout`), so a NativeXrp `collateral_type` cannot appear
        // in pending_margin_transfers / pending_excess_transfers.
        let (ledger_canister_id, transfer_amount) =
            match (transfer.ledger, transfer.transfer_amount_raw) {
                (Some(ledger), Some(amount)) if transfer.op_nonce != 0 && amount > 0 => {
                    (ledger, amount)
                }
                _ => {
                    mutate_state(|s| {
                        let map = match kind {
                            crate::event::PendingPayoutKind::Margin => {
                                &mut s.pending_margin_transfers
                            }
                            _ => &mut s.pending_excess_transfers,
                        };
                        if let Some(p) = map.get_mut(&operation_id) {
                            crate::event::record_pending_payout_held(operation_id, kind, p, true);
                        }
                    });
                    continue;
                }
            };

        log!(
            INFO,
            "[immediate_transfer] Processing {:?} transfer {} of {} collateral to {}",
            kind,
            transfer.vault_id,
            transfer_amount,
            transfer.owner
        );

        match management::transfer_collateral_with_nonce(
            transfer_amount,
            transfer.owner,
            ledger_canister_id,
            transfer.op_nonce,
        )
        .await
        {
            Ok(block_index) => {
                log!(
                    INFO,
                    "[immediate_transfer] Transfer {} owner {} successful, block: {}",
                    transfer.vault_id,
                    transfer.owner,
                    block_index
                );

                mutate_state(|s| {
                    crate::event::record_pending_payout_settled(
                        s,
                        operation_id,
                        kind,
                        transfer.vault_id,
                        block_index,
                    )
                });

                processed_count += 1;
            }
            Err(error) => {
                log!(
                    INFO,
                    "[immediate_transfer] Transfer {} owner {} failed: {}. Will retry later",
                    transfer.vault_id,
                    transfer.owner,
                    error
                );
                if matches!(
                    &error,
                    icrc_ledger_types::icrc1::transfer::TransferError::TooOld
                        | icrc_ledger_types::icrc1::transfer::TransferError::BadFee { .. }
                ) {
                    mutate_state(|s| {
                        let map = match kind {
                            crate::event::PendingPayoutKind::Margin => {
                                &mut s.pending_margin_transfers
                            }
                            _ => &mut s.pending_excess_transfers,
                        };
                        if let Some(p) = map.get_mut(&operation_id) {
                            crate::event::record_pending_payout_held(operation_id, kind, p, true);
                        }
                    });
                }
                // Leave retryable failures pending.
                return Err(format!("Transfer {} failed: {}", transfer.vault_id, error));
            }
        }
    }

    Ok(processed_count)
}

// Helper function to schedule transfer retries with exponential backoff
fn schedule_transfer_retry(vault_id: u64, operation_ids: Vec<PendingPayoutRef>, retry_count: u32) {
    let max_retries = 5;
    if retry_count >= max_retries {
        log!(
            INFO,
            "[retry_scheduler] Max retries reached for vault #{}",
            vault_id
        );
        return;
    }

    // Exponential backoff: 1s, 2s, 4s, 8s, 16s
    let delay_seconds = 1u64 << retry_count;

    log!(
        INFO,
        "[retry_scheduler] Scheduling retry #{} for vault #{} in {}s",
        retry_count + 1,
        vault_id,
        delay_seconds
    );

    ic_cdk_timers::set_timer(std::time::Duration::from_secs(delay_seconds), move || {
        ic_cdk::spawn(async move {
            log!(
                INFO,
                "[retry_scheduler] Retry #{} executing for vault #{}",
                retry_count + 1,
                vault_id
            );

            match try_process_pending_transfers_immediate(&operation_ids).await {
                Ok(processed) => {
                    log!(
                        INFO,
                        "[retry_scheduler] Retry #{} successful, processed {} transfers",
                        retry_count + 1,
                        processed
                    );
                }
                Err(_) => {
                    log!(
                        INFO,
                        "[retry_scheduler] Retry #{} failed, scheduling next retry",
                        retry_count + 1
                    );
                    schedule_transfer_retry(vault_id, operation_ids, retry_count + 1);
                }
            }
        })
    });
}

pub async fn partial_repay_to_vault(arg: VaultArg) -> Result<u64, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let guard_principal =
        GuardPrincipal::new(caller, &format!("partial_repay_vault_{}", arg.vault_id))?;
    // AR-B-003: per-vault op lock; see guard.rs::VaultLiquidationGuard. This
    // endpoint pulls icUSD across an await then commits via repay_to_vault, so
    // it needs the same serialization vs liquidation/redemption as its siblings.
    let _vault_op_guard = match VaultLiquidationGuard::new(arg.vault_id) {
        Ok(g) => g,
        Err(e) => {
            guard_principal.fail();
            return Err(e);
        }
    };
    let amount: ICUSD = arg.amount.into();

    // Accrue interest before repayment so the correct debt balance is used.
    let now = ic_cdk::api::time();
    if let Err(e) = reject_active_xrp_sp_absorb_preflight(arg.vault_id, now) {
        guard_principal.fail();
        return Err(e);
    }
    mutate_state(|s| s.accrue_single_vault(arg.vault_id, now));

    let vault = match read_state(|s| s.vault_id_to_vaults.get(&arg.vault_id).cloned()) {
        Some(v) => v,
        None => {
            guard_principal.fail();
            return Err(ProtocolError::GenericError(format!(
                "Vault #{} not found",
                arg.vault_id
            )));
        }
    };

    // Check collateral status allows repayment
    let collateral_status = read_state(|s| s.get_collateral_status(&vault.collateral_type));
    if let Some(status) = collateral_status {
        if !status.allows_repay() {
            guard_principal.fail();
            return Err(ProtocolError::TemporarilyUnavailable(format!(
                "Collateral is {:?}, repayment not allowed",
                status
            )));
        }
    }

    if caller != vault.owner {
        guard_principal.fail();
        return Err(ProtocolError::CallerNotOwner);
    }

    if amount < read_state(|s| s.min_icusd_amount) {
        guard_principal.fail();
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: read_state(|s| s.min_icusd_amount).to_u64(),
        });
    }

    // Cap repay amount to actual debt. Interest accrued between when the
    // frontend read the balance and now can push debt slightly above what
    // the user entered. If the requested amount exceeds or nearly matches
    // the current debt (within 1% or 0.01 icUSD — whichever is larger),
    // treat it as a full repayment to avoid leaving un-repayable dust.
    let debt = vault.borrowed_icusd_amount;
    let dust_threshold = std::cmp::max(debt.0 / 100, 1_000_000); // 1% or 0.01 icUSD
    let amount = if amount > debt {
        debt
    } else if debt.0.saturating_sub(amount.0) <= dust_threshold {
        debt
    } else {
        amount
    };

    if let Err(e) = check_min_vault_debt_after_repay(&vault, amount) {
        guard_principal.fail();
        return Err(e);
    }

    match transfer_icusd_from(amount, caller).await {
        Ok(block_index) => {
            let interest_share =
                mutate_state(|s| record_repayed_to_vault(s, arg.vault_id, amount, block_index));
            // IC-B-002 (audit 2026-06-09): re-queue any unminted interest share so the
            // next flush retries it instead of silently dropping treasury revenue.
            let unminted_interest =
                crate::treasury::distribute_interest(interest_share, vault.collateral_type).await;
            if unminted_interest.to_u64() > 0 {
                mutate_state(|s| {
                    s.restore_pending_interest_for_pool(
                        vault.collateral_type,
                        unminted_interest.to_u64(),
                    )
                });
            }
            guard_principal.complete(); // Mark as completed
            Ok(block_index)
        }
        Err(transfer_from_error) => {
            guard_principal.fail(); // Mark as failed
            Err(ProtocolError::TransferFromError(
                transfer_from_error,
                amount.to_u64(),
            ))
        }
    }
}

const MANUAL_LIQUIDATION_V2_REQUEST_TTL_NS: u64 = 24 * 60 * 60 * 1_000_000_000;
// Keep new manual V2 rows closed in ordinary builds. This test-only switch
// permits source-matched PocketIC coverage without changing production policy.
#[cfg(feature = "test-manual-liquidation-v2-admission")]
const MANUAL_LIQUIDATION_V2_ADMISSION_ENABLED: bool = true;
#[cfg(not(feature = "test-manual-liquidation-v2-admission"))]
const MANUAL_LIQUIDATION_V2_ADMISSION_ENABLED: bool = false;

fn manual_liquidation_v2_matches(
    row: &crate::state::ManualLiquidationV2Journal,
    caller: Principal,
    request_id: u128,
    vault_id: u64,
    route: &crate::ManualLiquidationRoute,
    requested_amount_e8s: u64,
) -> bool {
    row.owner == caller
        && row.request_id == request_id
        && row.vault_id == vault_id
        && row.route == *route
        && row.requested_amount_e8s == requested_amount_e8s
}

pub async fn liquidate_vault_v2(
    request_id: u128,
    vault_id: u64,
) -> Result<crate::ManualLiquidationV2StatusView, ProtocolError> {
    manual_liquidation_v2(
        request_id,
        vault_id,
        crate::ManualLiquidationRoute::FullIcusd,
        0,
    )
    .await
}

pub async fn liquidate_vault_partial_v2(
    request_id: u128,
    arg: VaultArg,
) -> Result<crate::ManualLiquidationV2StatusView, ProtocolError> {
    manual_liquidation_v2(
        request_id,
        arg.vault_id,
        crate::ManualLiquidationRoute::PartialIcusd,
        arg.amount,
    )
    .await
}

pub async fn liquidate_vault_partial_with_stable_v2(
    request_id: u128,
    arg: VaultArgWithToken,
) -> Result<crate::ManualLiquidationV2StatusView, ProtocolError> {
    manual_liquidation_v2(
        request_id,
        arg.vault_id,
        crate::ManualLiquidationRoute::PartialStable {
            token_type: arg.token_type,
        },
        arg.amount,
    )
    .await
}

async fn settle_manual_liquidation_v2(
    mut row: crate::state::ManualLiquidationV2Journal,
) -> Result<crate::ManualLiquidationV2StatusView, ProtocolError> {
    let _vault_guard = VaultLiquidationGuard::new(row.vault_id)?;
    let owner = row.owner;
    let request_id = row.request_id;
    if matches!(
        row.phase,
        crate::ManualLiquidationV2Phase::CommittedPayoutQueued
            | crate::ManualLiquidationV2Phase::Refunded
            | crate::ManualLiquidationV2Phase::Rejected
    ) {
        return Ok(row.status_view());
    }
    if row.phase == crate::ManualLiquidationV2Phase::RefundPending {
        return resume_manual_liquidation_v2_refund(row).await;
    }

    if row.candidate_block_index.is_none() {
        // Once dispatch was persisted, the future may have crossed the ledger
        // await even if its reply was lost to a trap or upgrade. Never issue a
        // second pull based on a wall-clock window: require an exact candidate
        // receipt before any financial commit or compensation.
        if row.dispatch_attempts > 0 {
            row.phase = crate::ManualLiquidationV2Phase::HeldPull;
            row.had_ambiguous_attempt = true;
            row.last_error = Some(
                "prior pull dispatch may have succeeded; attach its exact receipt; tuple will not be reissued".into(),
            );
            mutate_state(|s| crate::state::save_manual_liquidation_v2(s, row.clone()))
                .map_err(ProtocolError::GenericError)?;
            return Ok(row.status_view());
        }
        if ic_cdk::api::time().saturating_sub(row.created_at_ns)
            >= MANUAL_LIQUIDATION_V2_REQUEST_TTL_NS
        {
            row.phase = crate::ManualLiquidationV2Phase::Rejected;
            row.last_error =
                Some("manual liquidation request expired before any ledger dispatch".into());
            mutate_state(|s| {
                crate::state::save_manual_liquidation_v2(s, row.clone())?;
                crate::state::finish_manual_liquidation_v2(s, owner, request_id)
            })
            .map_err(ProtocolError::GenericError)?;
            return Ok(row.status_view());
        }
        row.dispatch_attempts = row.dispatch_attempts.saturating_add(1);
        let attempt = row.dispatch_attempts;
        let possible_effect = row.had_ambiguous_attempt || attempt > 1;
        row.last_error = None;
        mutate_state(|s| crate::state::save_manual_liquidation_v2(s, row.clone()))
            .map_err(ProtocolError::GenericError)?;
        match management::transfer_from_with_exact_tuple_outcome(&row.tuple).await {
            management::ExactTransferFromOutcome::Applied(block_index) => {
                row.candidate_block_index = Some(block_index);
                mutate_state(|s| crate::state::save_manual_liquidation_v2(s, row.clone()))
                    .map_err(ProtocolError::GenericError)?;
            }
            management::ExactTransferFromOutcome::ProvenNoEffect(error)
                if !possible_effect && attempt == 1 =>
            {
                row.phase = crate::ManualLiquidationV2Phase::Rejected;
                row.last_error = Some(format!("manual liquidation pull had no effect: {error:?}"));
                mutate_state(|s| {
                    crate::state::save_manual_liquidation_v2(s, row.clone())?;
                    crate::state::finish_manual_liquidation_v2(s, owner, request_id)
                })
                .map_err(ProtocolError::GenericError)?;
                return Ok(row.status_view());
            }
            outcome => {
                row.phase = crate::ManualLiquidationV2Phase::HeldPull;
                row.had_ambiguous_attempt = true;
                row.last_error = Some(match outcome {
                    management::ExactTransferFromOutcome::ProvenNoEffect(error) => {
                        format!("manual liquidation no-effect after a prior possible dispatch: {error:?}")
                    }
                    management::ExactTransferFromOutcome::AmbiguousLedgerError(error) => {
                        format!("manual liquidation ledger outcome is ambiguous: {error:?}")
                    }
                    management::ExactTransferFromOutcome::CallRejected { code, message } => {
                        format!("manual liquidation call rejected ({code}): {message}")
                    }
                    management::ExactTransferFromOutcome::InvalidBlockIndex => {
                        "manual liquidation returned an invalid block index".into()
                    }
                    management::ExactTransferFromOutcome::Applied(_) => unreachable!(),
                });
                mutate_state(|s| crate::state::save_manual_liquidation_v2(s, row.clone()))
                    .map_err(ProtocolError::GenericError)?;
                return Ok(row.status_view());
            }
        }
    }

    let block_index = row.candidate_block_index.ok_or_else(|| {
        ProtocolError::TemporarilyUnavailable("manual liquidation has no receipt candidate".into())
    })?;
    if let Err(error) = verify_manual_liquidation_pull_receipt(&row, block_index).await {
        row.phase = crate::ManualLiquidationV2Phase::HeldPull;
        row.had_ambiguous_attempt = true;
        row.last_error = Some(format!("manual liquidation receipt proof failed: {error}"));
        mutate_state(|s| crate::state::save_manual_liquidation_v2(s, row.clone()))
            .map_err(ProtocolError::GenericError)?;
        return Ok(row.status_view());
    }

    let plan = row.plan.clone();
    if let Err(message) =
        read_state(|s| manual_liquidation_v2_commit_preflight(s, &row, block_index))
    {
        let prior_commit_started = read_state(|s| {
            s.manual_liquidation_v2_active
                .get(&row.owner)
                .is_some_and(|current| {
                    current.request_id == row.request_id && current.commit_started
                })
        });
        if prior_commit_started {
            row = read_state(|s| s.manual_liquidation_v2_active.get(&row.owner).cloned())
                .unwrap_or(row);
            row.phase = crate::ManualLiquidationV2Phase::HeldPull;
            row.had_ambiguous_attempt = true;
            row.last_error = Some(format!("receipt-backed commit already started; automatic refund is prohibited until outbox/accounting reconciliation: {message}"));
            mutate_state(|s| crate::state::save_manual_liquidation_v2(s, row.clone()))
                .map_err(ProtocolError::GenericError)?;
            return Ok(row.status_view());
        }
        if matches!(
            &row.route,
            crate::ManualLiquidationRoute::FullIcusd
                | crate::ManualLiquidationRoute::PartialIcusd
                | crate::ManualLiquidationRoute::PartialStable { .. }
        ) {
            return begin_manual_liquidation_v2_refund(row, message).await;
        }
        return Err(ProtocolError::GenericError(
            "unsupported manual liquidation compensation route".into(),
        ));
    }
    let mut immediate_payouts = Vec::new();
    let commit = mutate_state(|s| -> Result<(), String> {
        manual_liquidation_v2_commit_preflight(s, &row, block_index)?;
        let mut committing = s
            .manual_liquidation_v2_active
            .get(&row.owner)
            .filter(|saved| saved.request_id == row.request_id)
            .cloned()
            .ok_or_else(|| "manual liquidation row disappeared before commit marker".to_string())?;
        committing.commit_started = true;
        crate::state::save_manual_liquidation_v2(s, committing)?;
        let live = s
            .vault_id_to_vaults
            .get(&row.vault_id)
            .cloned()
            .ok_or_else(|| {
                "vault disappeared after payment; manual liquidation remains held".to_string()
            })?;
        let new_debt_e8s = live
            .borrowed_icusd_amount
            .to_u64()
            .checked_sub(plan.debt_liquidated_e8s)
            .ok_or_else(|| "pinned debt reduction does not fit; receipt is held".to_string())?;
        let new_collateral_raw = live
            .collateral_amount
            .checked_sub(plan.collateral_to_seize_raw)
            .and_then(|remaining| remaining.checked_sub(plan.excess_collateral_raw))
            .ok_or_else(|| {
                "pinned collateral reduction does not fit; receipt is held".to_string()
            })?;
        let new_interest_e8s = live
            .accrued_interest
            .to_u64()
            .checked_sub(plan.interest_share_e8s)
            .ok_or_else(|| "pinned interest reduction does not fit; receipt is held".to_string())?;
        let routing = plan.interest_routing_plan.as_ref().ok_or_else(|| {
            "pinned interest routing plan is missing; receipt is held".to_string()
        })?;
        match &row.route {
            crate::ManualLiquidationRoute::PartialStable { token_type } => {
                crate::treasury::pin_stablecoin_interest_distribution_in_state(
                    s,
                    plan.interest_share_e8s,
                    plan.vault.collateral_type,
                    token_type.clone(),
                    row.ledger,
                    routing,
                )?;
                crate::treasury::queue_manual_liquidation_stable_surcharge_in_state(
                    s,
                    row.ledger,
                    routing.stable_treasury,
                    plan.stable_surcharge_e6s,
                    token_type.clone(),
                );
            }
            crate::ManualLiquidationRoute::FullIcusd
            | crate::ManualLiquidationRoute::PartialIcusd => {
                crate::treasury::pin_icusd_interest_distribution_in_state(
                    s,
                    ICUSD::from(plan.interest_share_e8s),
                    plan.vault.collateral_type,
                    routing,
                )?;
            }
        }
        let vault_owner = live.owner;
        let liquidator = row.owner;
        let collateral_type = live.collateral_type;
        let debt = ICUSD::from(plan.debt_liquidated_e8s);
        let collateral_price_usd = UsdIcp::from(plan.collateral_price);
        let is_full = plan.debt_liquidated_e8s == live.borrowed_icusd_amount.to_u64();
        let liquidator_payout = plan.collateral_to_liquidator_raw;
        let excess = if is_full {
            plan.excess_collateral_raw
        } else {
            0
        };

        // All validation and arithmetic checks precede the first write. The
        // receipt, pinned debt reduction, event, and payout outbox are committed
        // together; there is no post-payment partial-debt fallback.
        let vault = s
            .vault_id_to_vaults
            .get_mut(&row.vault_id)
            .expect("manual liquidation vault was synchronously validated");
        vault.borrowed_icusd_amount = ICUSD::from(new_debt_e8s);
        vault.collateral_amount = new_collateral_raw;
        vault.accrued_interest = ICUSD::from(new_interest_e8s);
        crate::event::record_liquidation_for_breaker(s, plan.debt_liquidated_e8s);
        let seized_usd = crate::numeric::collateral_usd_value(
            plan.collateral_to_seize_raw,
            plan.collateral_price,
            plan.collateral_decimals,
        );
        if seized_usd < debt {
            let shortfall = debt - seized_usd;
            crate::event::record_deficit_accrued(
                s,
                crate::event::DeficitSource::Liquidation {
                    vault_id: row.vault_id,
                },
                shortfall,
                ic_cdk::api::time(),
            );
            s.check_deficit_readonly_latch();
        }
        let event = if is_full {
            crate::event::Event::LiquidateVault {
                vault_id: row.vault_id,
                mode: plan.mode,
                icp_rate: collateral_price_usd,
                liquidator: Some(liquidator),
                timestamp: Some(ic_cdk::api::time()),
                repay_amount: Some(debt),
                collateral_seized_raw: Some(plan.collateral_to_seize_raw),
            }
        } else {
            crate::event::Event::PartialLiquidateVault {
                vault_id: row.vault_id,
                liquidator_payment: debt,
                icp_to_liquidator: ICP::from(liquidator_payout),
                liquidator: Some(liquidator),
                icp_rate: Some(collateral_price_usd),
                protocol_fee_collateral: (plan.protocol_cut_raw > 0)
                    .then_some(plan.protocol_cut_raw),
                timestamp: Some(ic_cdk::api::time()),
                three_usd_reserves_e8s: None,
            }
        };
        crate::storage::record_event(&event);
        let payout_nonce = s.next_op_nonce();
        let xrp_claim_id = queue_collateral_payout(
            s,
            row.vault_id,
            vault_owner,
            liquidator,
            ICP::from(liquidator_payout),
            collateral_type,
            payout_nonce,
            ic_cdk::api::time(),
            &mut immediate_payouts,
        );
        if excess > 0 {
            if s.get_collateral_config(&collateral_type)
                .map(|c| c.is_native_xrp())
                .unwrap_or(false)
            {
                record_xrp_claim(
                    s,
                    vault_owner,
                    vault_owner,
                    row.vault_id,
                    excess,
                    ic_cdk::api::time(),
                );
            } else {
                let excess_nonce = s.next_op_nonce();
                let (ledger, fee) = s
                    .get_collateral_config(&collateral_type)
                    .map(|config| (config.ledger_canister_id, config.ledger_fee))
                    .unwrap_or((s.icp_ledger_principal, s.icp_ledger_fee.to_u64()));
                let transfer_amount_raw = excess.saturating_sub(fee);
                let transfer = PendingMarginTransfer {
                    vault_id: row.vault_id,
                    owner: vault_owner,
                    margin: ICP::from(excess),
                    collateral_type,
                    retry_count: 0,
                    op_nonce: excess_nonce,
                    ledger: Some(ledger),
                    transfer_amount_raw: Some(transfer_amount_raw),
                    redemption_transfer: None,
                    held_for_manual_retry: transfer_amount_raw == 0,
                    reconciliation_required: transfer_amount_raw == 0,
                    min_net_collateral_raw: None,
                };
                crate::event::record_pending_payout_queued(
                    s,
                    excess_nonce,
                    crate::event::PendingPayoutKind::Excess,
                    transfer,
                );
                immediate_payouts.push((crate::event::PendingPayoutKind::Excess, excess_nonce));
            }
        }
        s.cleanup_if_drained(row.vault_id);
        if plan.protocol_cut_raw > 0 {
            if collateral_type == crate::state::xrp_collateral_principal() {
                let developer = s.developer_principal;
                record_xrp_claim(
                    s,
                    developer,
                    vault_owner,
                    row.vault_id,
                    plan.protocol_cut_raw,
                    ic_cdk::api::time(),
                );
            } else {
                let collateral_ledger = s
                    .get_collateral_config(&collateral_type)
                    .map(|config| config.ledger_canister_id)
                    .unwrap_or(s.icp_ledger_principal);
                crate::treasury::queue_liquidation_fee_obligation_in_state(
                    s,
                    collateral_ledger,
                    plan.protocol_cut_raw,
                );
            }
        }
        let result = crate::ManualLiquidationV2Result {
            ledger: row.ledger,
            block_index,
            debt_liquidated_e8s: plan.debt_liquidated_e8s,
            collateral_to_liquidator_raw: liquidator_payout,
            collateral_to_seize_raw: plan.collateral_to_seize_raw,
            liquidator_xrp_claim_id: xrp_claim_id,
        };
        let saved = s
            .manual_liquidation_v2_active
            .get_mut(&owner)
            .expect("validated manual liquidation journal remains present");
        saved.result = Some(result);
        saved.phase = crate::ManualLiquidationV2Phase::CommittedPayoutQueued;
        saved.last_error = None;
        crate::state::finish_manual_liquidation_v2(s, owner, request_id)?;
        crate::storage::save_state_to_stable(s);
        Ok(())
    });
    if let Err(message) = commit {
        row = read_state(|s| s.manual_liquidation_v2_active.get(&owner).cloned()).unwrap_or(row);
        row.phase = crate::ManualLiquidationV2Phase::HeldPull;
        row.had_ambiguous_attempt = true;
        row.last_error = Some(format!("receipt-backed commit failed after preflight; state/outbox outcome requires reconciliation, no automatic refund was issued: {message}"));
        mutate_state(|s| crate::state::save_manual_liquidation_v2(s, row.clone()))
            .map_err(ProtocolError::GenericError)?;
        return Ok(row.status_view());
    }
    // The receipt-backed commit already created every recipient obligation
    // with its immutable identity. Schedule durable workers before any
    // post-commit await; a trap here can delay delivery but cannot erase debt,
    // fee, interest, or collateral-payout obligations.
    ic_cdk_timers::set_timer(std::time::Duration::from_secs(2), || {
        ic_cdk::spawn(crate::process_pending_transfer())
    });
    ic_cdk_timers::set_timer(std::time::Duration::from_secs(2), || {
        ic_cdk::spawn(crate::treasury::process_pending_treasury_payments())
    });
    ic_cdk_timers::set_timer(std::time::Duration::from_secs(2), || {
        ic_cdk::spawn(crate::treasury::process_pending_stability_pool_interest_mints())
    });
    ic_cdk_timers::set_timer(std::time::Duration::from_secs(2), || {
        ic_cdk::spawn(crate::treasury::flush_pending_three_pool_donations())
    });
    ic_cdk_timers::set_timer(std::time::Duration::from_secs(2), || {
        ic_cdk::spawn(crate::treasury::flush_pending_amm1_donations())
    });
    if !immediate_payouts.is_empty() {
        let vault_id = row.vault_id;
        if try_process_pending_transfers_immediate(&immediate_payouts)
            .await
            .is_err()
        {
            schedule_transfer_retry(vault_id, immediate_payouts.clone(), 0);
        }
    }
    read_state(|s| {
        s.manual_liquidation_v2_latest_result
            .get(&owner)
            .map(|r| r.status_view())
    })
    .ok_or_else(|| {
        ProtocolError::TemporarilyUnavailable(
            "manual liquidation committed but result is not readable".into(),
        )
    })
}

const MANUAL_LIQUIDATION_REFUND_HISTORY_PAGE: u64 = 8;

fn manual_liquidation_refund_history_page_bounds(
    cursor: u64,
    log_length: u64,
) -> Option<(u64, u64)> {
    if cursor > log_length {
        return None;
    }
    Some((
        cursor,
        cursor
            .saturating_add(MANUAL_LIQUIDATION_REFUND_HISTORY_PAGE)
            .min(log_length),
    ))
}

fn manual_liquidation_v2_refund_receipt_matches(
    refund: &crate::ManualLiquidationV2Refund,
    block: &crate::icrc3_proof::DecodedBlock,
) -> bool {
    let destination = Account {
        owner: refund.recipient,
        subaccount: None,
    };
    match (&refund.kind, refund.fee_raw) {
        (crate::ManualLiquidationV2RefundKind::IcusdMint { .. }, None) => {
            crate::icrc3_proof::validate_icrc3_fee_free_mint_block(
                block,
                destination,
                refund.amount_e8s,
                &refund.memo,
                refund.created_at_time_ns,
            )
            .is_ok()
        }
        (crate::ManualLiquidationV2RefundKind::StableTransfer { .. }, Some(fee_raw)) => {
            crate::icrc3_proof::validate_icrc3_transfer_block_with_fee(
                block,
                refund.source.clone(),
                destination,
                refund.amount_e8s,
                fee_raw,
                &refund.memo,
                refund.created_at_time_ns,
            )
            .is_ok()
                && block.fee == Some(u128::from(fee_raw))
                && block.expected_allowance.is_none()
                && block.expires_at.is_none()
        }
        _ => false,
    }
}

fn manual_liquidation_v2_refund_memo(
    owner: Principal,
    request_id: u128,
    vault_id: u64,
    pull_block_index: u64,
    ledger: Principal,
    amount_raw: u64,
    fee_raw: Option<u64>,
    op_nonce: u128,
) -> Vec<u8> {
    let mut identity = b"RUMI-MANUAL-LIQ-REFUND-V1".to_vec();
    identity.extend_from_slice(owner.as_slice());
    identity.extend_from_slice(&request_id.to_be_bytes());
    identity.extend_from_slice(&vault_id.to_be_bytes());
    identity.extend_from_slice(&pull_block_index.to_be_bytes());
    identity.extend_from_slice(ledger.as_slice());
    identity.extend_from_slice(&amount_raw.to_be_bytes());
    identity.extend_from_slice(&fee_raw.unwrap_or(u64::MAX).to_be_bytes());
    identity.extend_from_slice(&op_nonce.to_be_bytes());
    Sha256::digest(identity).to_vec()
}

fn manual_liquidation_refund_dispatch_is_allowed(
    refund: &crate::ManualLiquidationV2Refund,
) -> bool {
    if refund.dispatch_attempts == 0 {
        return true;
    }
    if refund.dispatch_attempts >= crate::MAX_MANUAL_LIQUIDATION_REFUND_DISPATCHES {
        return false;
    }
    let Some(evidence) = refund.no_effect_attempts.last() else {
        return false;
    };
    if evidence.dispatch_attempt != refund.dispatch_attempts {
        return false;
    }
    let same_tuple = evidence.fee_raw == refund.fee_raw
        && evidence.op_nonce == refund.op_nonce
        && evidence.created_at_time_ns == refund.created_at_time_ns
        && evidence.memo == refund.memo;
    let fee_rotated_after_bad_fee = evidence
        .expected_fee_raw
        .is_some_and(|expected| refund.fee_raw == Some(expected));
    same_tuple || fee_rotated_after_bad_fee
}

fn manual_liquidation_refund_error_is_proven_no_effect(
    error: &icrc_ledger_types::icrc1::transfer::TransferError,
) -> bool {
    matches!(
        error,
        icrc_ledger_types::icrc1::transfer::TransferError::BadFee { .. }
            | icrc_ledger_types::icrc1::transfer::TransferError::BadBurn { .. }
            | icrc_ledger_types::icrc1::transfer::TransferError::InsufficientFunds { .. }
            | icrc_ledger_types::icrc1::transfer::TransferError::TooOld
            | icrc_ledger_types::icrc1::transfer::TransferError::CreatedInFuture { .. }
    )
}

fn manual_liquidation_refund_expected_fee(
    error: &icrc_ledger_types::icrc1::transfer::TransferError,
) -> Option<u64> {
    match error {
        icrc_ledger_types::icrc1::transfer::TransferError::BadFee { expected_fee } => {
            expected_fee.0.to_u64()
        }
        _ => None,
    }
}

fn manual_liquidation_refund_ledger_is_current(
    state: &crate::state::State,
    row: &crate::state::ManualLiquidationV2Journal,
    refund: &crate::ManualLiquidationV2Refund,
) -> bool {
    let route_ledger = match (&row.route, &refund.kind) {
        (
            crate::ManualLiquidationRoute::FullIcusd | crate::ManualLiquidationRoute::PartialIcusd,
            crate::ManualLiquidationV2RefundKind::IcusdMint { .. },
        ) => Some(state.icusd_ledger_principal),
        (
            crate::ManualLiquidationRoute::PartialStable { token_type },
            crate::ManualLiquidationV2RefundKind::StableTransfer { .. },
        ) => match token_type {
            crate::StableTokenType::CKUSDT => state.ckusdt_ledger_principal,
            crate::StableTokenType::CKUSDC => state.ckusdc_ledger_principal,
        },
        _ => None,
    };
    route_ledger == Some(refund.ledger)
        && row.owner == refund.caller
        && row.owner == refund.recipient
        && row.request_id == refund.request_id
        && row.vault_id == refund.vault_id
        && row.ledger == refund.ledger
        && row.pull_amount_raw == refund.amount_e8s
        && row
            .candidate_block_index
            .is_some_and(|index| match &refund.kind {
                crate::ManualLiquidationV2RefundKind::IcusdMint { burn_block_index } => {
                    index == *burn_block_index
                }
                crate::ManualLiquidationV2RefundKind::StableTransfer {
                    pull_block_index, ..
                } => index == *pull_block_index,
            })
}

async fn begin_manual_liquidation_v2_refund(
    mut row: crate::state::ManualLiquidationV2Journal,
    reason: String,
) -> Result<crate::ManualLiquidationV2StatusView, ProtocolError> {
    if row.refund.is_none() {
        // This tip is captured before the refund tuple can be dispatched. If
        // the query is unavailable, start at zero; that remains safe and the
        // bounded scanner can still progress without claiming absence.
        let (history_cursor, tip_error) =
            match crate::icrc3_proof::icrc3_log_length(row.ledger).await {
                Ok(tip) => (tip, None),
                Err(error) => (
                    0,
                    Some(format!(
                        "pre-mint log tip unavailable; archive scan starts at zero: {error}"
                    )),
                ),
            };
        let configured_icusd_ledger = read_state(|state| state.icusd_ledger_principal);
        let pull_block_index = row.candidate_block_index.ok_or_else(|| {
            ProtocolError::TemporarilyUnavailable(
                "manual liquidation refund requires a proved pull block".into(),
            )
        })?;
        let refund_fee = match &row.route {
            crate::ManualLiquidationRoute::FullIcusd
            | crate::ManualLiquidationRoute::PartialIcusd => None,
            crate::ManualLiquidationRoute::PartialStable { .. } => {
                match management::get_ledger_fee(row.ledger).await {
                    Ok(fee) => Some(fee),
                    Err(error) => {
                        row.phase = crate::ManualLiquidationV2Phase::HeldPull;
                        row.last_error = Some(format!(
                            "stable refund fee could not be pinned; paid pull remains durably held: {error}"
                        ));
                        mutate_state(|s| crate::state::save_manual_liquidation_v2(s, row.clone()))
                            .map_err(ProtocolError::GenericError)?;
                        return Ok(row.status_view());
                    }
                }
            }
        };
        row = mutate_state(|s| {
            let current = s.manual_liquidation_v2_active.get(&row.owner)
                .filter(|current| current.request_id == row.request_id
                    && current.tuple == row.tuple
                    && current.candidate_block_index == row.candidate_block_index
                    && current.result.is_none())
                .ok_or_else(|| "manual liquidation row changed before refund obligation creation".to_string())?;
            if let Some(existing) = current.refund.as_ref() {
                row.refund = Some(existing.clone());
                row.phase = crate::ManualLiquidationV2Phase::RefundPending;
                return Ok(row.clone());
            }
            let kind = match (&row.route, refund_fee) {
                (
                    crate::ManualLiquidationRoute::FullIcusd
                    | crate::ManualLiquidationRoute::PartialIcusd,
                    None,
                ) if row.ledger == configured_icusd_ledger => {
                    crate::ManualLiquidationV2RefundKind::IcusdMint { burn_block_index: pull_block_index }
                }
                (
                    crate::ManualLiquidationRoute::PartialStable { .. },
                    Some(fee_raw),
                ) => crate::ManualLiquidationV2RefundKind::StableTransfer {
                    pull_block_index,
                    fee_raw,
                },
                _ => return Err("manual liquidation route is not eligible for its exact refund rail".into()),
            };
            if row.commit_started {
                return Err("a possible partial financial commit cannot enter automatic refund".into());
            }
            let op_nonce = s.next_op_nonce();
            row.refund = Some(crate::ManualLiquidationV2Refund {
                caller: row.owner,
                request_id: row.request_id,
                vault_id: row.vault_id,
                kind,
                ledger: row.ledger,
                source: Account { owner: ic_cdk::id(), subaccount: None },
                recipient: row.owner,
                amount_e8s: row.pull_amount_raw,
                fee_raw: refund_fee,
                op_nonce,
                created_at_time_ns: management::nonce_to_created_at_time(op_nonce),
                memo: manual_liquidation_v2_refund_memo(
                    row.owner,
                    row.request_id,
                    row.vault_id,
                    pull_block_index,
                    row.ledger,
                    row.pull_amount_raw,
                    refund_fee,
                    op_nonce,
                ),
                history_cursor,
                history_end_exclusive: None,
                dispatch_attempts: 0,
                no_effect_attempts: Vec::new(),
                candidate_block_index: None,
                last_error: tip_error,
            });
            row.phase = crate::ManualLiquidationV2Phase::RefundPending;
            row.last_error = Some(format!("proved stable pull could not commit to the pinned vault plan; exact refund obligation created: {reason}"));
            crate::state::save_manual_liquidation_v2(s, row.clone())?;
            Ok(row.clone())
        }).map_err(ProtocolError::GenericError)?;
    }
    resume_manual_liquidation_v2_refund(row).await
}

async fn resume_manual_liquidation_v2_refund(
    mut row: crate::state::ManualLiquidationV2Journal,
) -> Result<crate::ManualLiquidationV2StatusView, ProtocolError> {
    if row.phase != crate::ManualLiquidationV2Phase::RefundPending {
        return Ok(row.status_view());
    }
    let Some(mut refund) = row.refund.clone() else {
        row.phase = crate::ManualLiquidationV2Phase::HeldPull;
        row.last_error =
            Some("manual liquidation refund phase has no persisted refund identity".into());
        mutate_state(|s| crate::state::save_manual_liquidation_v2(s, row.clone()))
            .map_err(ProtocolError::GenericError)?;
        return Ok(row.status_view());
    };

    if manual_liquidation_refund_dispatch_is_allowed(&refund) {
        if matches!(
            &refund.kind,
            crate::ManualLiquidationV2RefundKind::IcusdMint { .. }
        ) {
            if let Err(error) = crate::sp_burn_refund::verify_mint_authority(refund.ledger).await {
                refund.last_error = Some(format!(
                    "icUSD mint authority preflight failed; no refund dispatch occurred: {error:?}"
                ));
                row.refund = Some(refund);
                row.last_error = row
                    .refund
                    .as_ref()
                    .and_then(|entry| entry.last_error.clone());
                mutate_state(|s| crate::state::save_manual_liquidation_v2(s, row.clone()))
                    .map_err(ProtocolError::GenericError)?;
                return Ok(row.status_view());
            }
        }
        if !read_state(|s| {
            s.manual_liquidation_v2_active
                .get(&row.owner)
                .is_some_and(|saved| {
                    saved.request_id == row.request_id
                        && saved.phase == crate::ManualLiquidationV2Phase::RefundPending
                        && saved.refund.as_ref() == Some(&refund)
                        && manual_liquidation_refund_ledger_is_current(s, saved, &refund)
                })
        }) {
            refund.last_error = Some("manual liquidation refund ledger or exact journal identity changed during preflight".into());
            row.refund = Some(refund);
            mutate_state(|s| crate::state::save_manual_liquidation_v2(s, row.clone()))
                .map_err(ProtocolError::GenericError)?;
            return Ok(row.status_view());
        }
        refund.dispatch_attempts = refund.dispatch_attempts.saturating_add(1);
        refund.last_error = None;
        row.refund = Some(refund.clone());
        mutate_state(|s| crate::state::save_manual_liquidation_v2(s, row.clone()))
            .map_err(ProtocolError::GenericError)?;

        let transfer = match (&refund.kind, refund.fee_raw) {
            (crate::ManualLiquidationV2RefundKind::IcusdMint { .. }, None) => {
                management::transfer_idempotent(
                    refund.ledger,
                    None,
                    Account {
                        owner: refund.recipient,
                        subaccount: None,
                    },
                    u128::from(refund.amount_e8s),
                    refund.op_nonce,
                    Some(icrc_ledger_types::icrc1::transfer::Memo::from(
                        refund.memo.clone(),
                    )),
                )
                .await
            }
            (crate::ManualLiquidationV2RefundKind::StableTransfer { .. }, Some(fee_raw)) => {
                management::transfer_idempotent_exact(
                    refund.ledger,
                    None,
                    Account {
                        owner: refund.recipient,
                        subaccount: None,
                    },
                    u128::from(refund.amount_e8s),
                    fee_raw,
                    icrc_ledger_types::icrc1::transfer::Memo::from(refund.memo.clone()),
                    refund.created_at_time_ns,
                )
                .await
            }
            _ => {
                refund.last_error = Some(
                    "persisted refund rail and fee tuple are inconsistent; no dispatch occurred"
                        .into(),
                );
                row.refund = Some(refund);
                mutate_state(|s| crate::state::save_manual_liquidation_v2(s, row.clone()))
                    .map_err(ProtocolError::GenericError)?;
                return Ok(row.status_view());
            }
        };
        match transfer {
            Ok(block_index) => {
                refund.candidate_block_index = Some(block_index);
                refund.last_error = None;
                row.refund = Some(refund.clone());
                mutate_state(|s| crate::state::save_manual_liquidation_v2(s, row.clone()))
                    .map_err(ProtocolError::GenericError)?;
                if verify_manual_liquidation_v2_refund_block(&refund, block_index)
                    .await
                    .is_ok()
                {
                    return finish_manual_liquidation_v2_refund(row).map(|done| done.status_view());
                }
                refund.candidate_block_index = None;
                refund.last_error = Some("refund transfer returned a candidate block that did not prove the exact persisted compensation tuple; history recovery remains active".into());
                row.refund = Some(refund.clone());
                row.last_error = refund.last_error.clone();
                mutate_state(|s| crate::state::save_manual_liquidation_v2(s, row.clone()))
                    .map_err(ProtocolError::GenericError)?;
            }
            Err(error) => {
                if manual_liquidation_refund_error_is_proven_no_effect(&error) {
                    let expected_fee_raw = manual_liquidation_refund_expected_fee(&error);
                    refund
                        .no_effect_attempts
                        .push(crate::ManualLiquidationV2RefundNoEffect {
                            dispatch_attempt: refund.dispatch_attempts,
                            fee_raw: refund.fee_raw,
                            op_nonce: refund.op_nonce,
                            created_at_time_ns: refund.created_at_time_ns,
                            memo: refund.memo.clone(),
                            expected_fee_raw,
                            error: format!("{error:?}"),
                        });
                    let rotate_fee = matches!(
                        &refund.kind,
                        crate::ManualLiquidationV2RefundKind::StableTransfer { .. }
                    ) && expected_fee_raw.is_some()
                        && refund.fee_raw != expected_fee_raw;
                    if rotate_fee {
                        let expected_fee = expected_fee_raw.expect("checked above");
                        let pull_block_index = match &refund.kind {
                            crate::ManualLiquidationV2RefundKind::StableTransfer {
                                pull_block_index,
                                ..
                            } => *pull_block_index,
                            crate::ManualLiquidationV2RefundKind::IcusdMint { .. } => {
                                unreachable!()
                            }
                        };
                        let new_nonce =
                            mutate_state(|s| {
                                let current =
                                    s.manual_liquidation_v2_active
                                        .get(&row.owner)
                                        .filter(|saved| {
                                            saved.request_id == row.request_id
                                    && saved.phase == crate::ManualLiquidationV2Phase::RefundPending
                                    && saved.refund.as_ref().is_some_and(|active| active == &refund)
                                        })
                                        .ok_or_else(|| {
                                            "manual liquidation refund changed after typed BadFee"
                                                .to_string()
                                        })?;
                                let _ = current;
                                Ok::<_, String>(s.next_op_nonce())
                            })
                            .map_err(ProtocolError::GenericError)?;
                        refund.fee_raw = Some(expected_fee);
                        refund.kind = crate::ManualLiquidationV2RefundKind::StableTransfer {
                            pull_block_index,
                            fee_raw: expected_fee,
                        };
                        refund.op_nonce = new_nonce;
                        refund.created_at_time_ns = management::nonce_to_created_at_time(new_nonce);
                        refund.memo = manual_liquidation_v2_refund_memo(
                            refund.caller,
                            refund.request_id,
                            refund.vault_id,
                            pull_block_index,
                            refund.ledger,
                            refund.amount_e8s,
                            Some(expected_fee),
                            new_nonce,
                        );
                        refund.candidate_block_index = None;
                    }
                    refund.last_error = Some(format!(
                        "typed no-effect ledger rejection ({error:?}); same tuple remains safe to retry{}",
                        if rotate_fee { " after the persisted BadFee identity rotation" } else { "" }
                    ));
                    row.refund = Some(refund.clone());
                    row.last_error = refund.last_error.clone();
                    mutate_state(|s| crate::state::save_manual_liquidation_v2(s, row.clone()))
                        .map_err(ProtocolError::GenericError)?;
                } else {
                    refund.last_error = Some(format!("ledger call outcome is ambiguous ({error:?}); no new transfer identity will be dispatched; archive-aware receipt scan remains active"));
                    row.refund = Some(refund.clone());
                    row.last_error = refund.last_error.clone();
                    mutate_state(|s| crate::state::save_manual_liquidation_v2(s, row.clone()))
                        .map_err(ProtocolError::GenericError)?;
                }
            }
        }
    } else if let Some(block_index) = refund.candidate_block_index {
        if verify_manual_liquidation_v2_refund_block(&refund, block_index)
            .await
            .is_ok()
        {
            return finish_manual_liquidation_v2_refund(row).map(|done| done.status_view());
        }
        refund.candidate_block_index = None;
        refund.last_error = Some("persisted refund candidate did not prove the exact fee-free mint; archive-aware history recovery remains active".into());
        row.refund = Some(refund.clone());
        row.last_error = refund.last_error.clone();
        mutate_state(|s| crate::state::save_manual_liquidation_v2(s, row.clone()))
            .map_err(ProtocolError::GenericError)?;
    }

    // Unknown-index recovery scans at most eight immutable block indexes per
    // invocation. `fetch_icrc3_block` follows the ledger's exact archive
    // descriptor for each block and fails closed when the direct/archive
    // response is missing or malformed. Only a positive exact mint is
    // terminal; an empty snapshot merely advances the cursor for a later pass.
    let log_length = match crate::icrc3_proof::icrc3_log_length(refund.ledger).await {
        Ok(length) => length,
        Err(error) => {
            refund.last_error = Some(format!("refund receipt history tip unavailable: {error}"));
            row.refund = Some(refund);
            row.last_error = row
                .refund
                .as_ref()
                .and_then(|entry| entry.last_error.clone());
            mutate_state(|s| crate::state::save_manual_liquidation_v2(s, row.clone()))
                .map_err(ProtocolError::GenericError)?;
            return Ok(row.status_view());
        }
    };
    if refund.history_cursor > log_length {
        refund.last_error = Some(
            "refund history cursor exceeds the ledger log length; obligation remains held".into(),
        );
        row.refund = Some(refund);
        mutate_state(|s| crate::state::save_manual_liquidation_v2(s, row.clone()))
            .map_err(ProtocolError::GenericError)?;
        return Ok(row.status_view());
    }
    let (start, end) =
        manual_liquidation_refund_history_page_bounds(refund.history_cursor, log_length)
            .ok_or_else(|| {
                ProtocolError::GenericError("refund history cursor exceeds log length".into())
            })?;
    refund.history_end_exclusive = Some(log_length);
    for index in start..end {
        let block = match crate::icrc3_proof::fetch_icrc3_block(refund.ledger, index).await {
            Ok(block) => block,
            Err(error) => {
                refund.last_error = Some(format!(
                    "refund archive-aware scan is incomplete at block {index}: {error}"
                ));
                row.refund = Some(refund);
                row.last_error = row
                    .refund
                    .as_ref()
                    .and_then(|entry| entry.last_error.clone());
                mutate_state(|s| crate::state::save_manual_liquidation_v2(s, row.clone()))
                    .map_err(ProtocolError::GenericError)?;
                return Ok(row.status_view());
            }
        };
        let exact = manual_liquidation_v2_refund_receipt_matches(&refund, &block);
        if exact {
            refund.candidate_block_index = Some(index);
            refund.last_error = None;
            row.refund = Some(refund.clone());
            mutate_state(|s| crate::state::save_manual_liquidation_v2(s, row.clone()))
                .map_err(ProtocolError::GenericError)?;
            return finish_manual_liquidation_v2_refund(row).map(|done| done.status_view());
        }
    }
    refund.history_cursor = end;
    refund.history_end_exclusive = Some(log_length);
    refund.last_error = Some(if end == log_length {
        "current refund history prefix contains no exact compensation receipt; later scans will include newly appended blocks; tuple remains held".into()
    } else {
        format!(
            "refund history scan advanced through block {}; more blocks remain",
            end.saturating_sub(1)
        )
    });
    row.refund = Some(refund.clone());
    row.last_error = refund.last_error.clone();
    mutate_state(|s| crate::state::save_manual_liquidation_v2(s, row.clone()))
        .map_err(ProtocolError::GenericError)?;
    Ok(row.status_view())
}

async fn verify_manual_liquidation_v2_refund_block(
    refund: &crate::ManualLiquidationV2Refund,
    block_index: u64,
) -> Result<(), String> {
    let block = crate::icrc3_proof::fetch_icrc3_block(refund.ledger, block_index).await?;
    if manual_liquidation_v2_refund_receipt_matches(refund, &block) {
        Ok(())
    } else {
        Err("block does not prove the exact persisted manual-liquidation compensation tuple".into())
    }
}

fn finish_manual_liquidation_v2_refund(
    mut row: crate::state::ManualLiquidationV2Journal,
) -> Result<crate::state::ManualLiquidationV2Journal, ProtocolError> {
    let refund = row
        .refund
        .as_mut()
        .ok_or_else(|| ProtocolError::GenericError("manual refund journal disappeared".into()))?;
    if refund.candidate_block_index.is_none() {
        return Err(ProtocolError::GenericError(
            "manual refund cannot finish without a proved candidate".into(),
        ));
    }
    refund.last_error = None;
    row.phase = crate::ManualLiquidationV2Phase::Refunded;
    row.last_error = None;
    mutate_state(|s| {
        crate::state::save_manual_liquidation_v2(s, row.clone())?;
        crate::state::finish_manual_liquidation_v2(s, row.owner, row.request_id)
    })
    .map_err(ProtocolError::GenericError)?;
    Ok(row)
}

async fn verify_manual_liquidation_pull_receipt(
    row: &crate::state::ManualLiquidationV2Journal,
    block_index: u64,
) -> Result<(), String> {
    match &row.route {
        crate::ManualLiquidationRoute::FullIcusd | crate::ManualLiquidationRoute::PartialIcusd => {
            crate::icrc3_proof::verify_sp_liquidation_icusd_burn_block(&row.tuple, block_index)
                .await
        }
        crate::ManualLiquidationRoute::PartialStable { .. } => {
            crate::icrc3_proof::verify_icrc3_transfer_from_block(&row.tuple, block_index).await
        }
    }
}

/// Pure preflight for the receipt-backed commit. Keep every fallible check
/// before the first recipient-outbox or vault write: `mutate_state` does not
/// roll back Rust mutations when a closure returns `Err`.
fn manual_liquidation_v2_commit_preflight(
    state: &crate::state::State,
    row: &crate::state::ManualLiquidationV2Journal,
    block_index: u64,
) -> Result<(), String> {
    let saved = state
        .manual_liquidation_v2_active
        .get(&row.owner)
        .filter(|saved| {
            saved.request_id == row.request_id
                && saved.tuple == row.tuple
                && saved.candidate_block_index == Some(block_index)
                && saved.result.is_none()
        })
        .ok_or_else(|| "manual liquidation row changed before receipt-backed commit".to_string())?;
    if saved.commit_started {
        return Err("manual liquidation commit already started; exact receipt remains held".into());
    }
    let live = state.vault_id_to_vaults.get(&row.vault_id).ok_or_else(|| {
        "vault disappeared after payment; manual liquidation remains held".to_string()
    })?;
    if live != &row.plan.vault
        || live.bot_processing
        || state
            .pending_collateral_withdrawals
            .contains_key(&row.vault_id)
        || state
            .pending_borrow_mints
            .values()
            .any(|pending| pending.vault_id == row.vault_id)
    {
        return Err("vault or pinned liquidation quote changed after payment; exact receipt is held for reconciliation".into());
    }
    let plan = &row.plan;
    let full = plan.debt_liquidated_e8s == live.borrowed_icusd_amount.to_u64();
    if plan.debt_liquidated_e8s == 0
        || plan.debt_liquidated_e8s > live.borrowed_icusd_amount.to_u64()
        || plan.interest_share_e8s > live.accrued_interest.to_u64()
        || plan.collateral_to_seize_raw > live.collateral_amount
        || (!full && plan.excess_collateral_raw != 0)
        || plan.protocol_cut_raw > plan.collateral_to_seize_raw
        || plan
            .collateral_to_liquidator_raw
            .checked_add(plan.protocol_cut_raw)
            .is_none_or(|sum| sum > plan.collateral_to_seize_raw)
        || plan
            .collateral_to_seize_raw
            .checked_add(if full { plan.excess_collateral_raw } else { 0 })
            .is_none_or(|sum| sum > live.collateral_amount)
        || live
            .borrowed_icusd_amount
            .to_u64()
            .checked_sub(plan.debt_liquidated_e8s)
            .is_none()
        || live
            .collateral_amount
            .checked_sub(plan.collateral_to_seize_raw)
            .and_then(|remaining| {
                remaining.checked_sub(if full { plan.excess_collateral_raw } else { 0 })
            })
            .is_none()
        || live
            .accrued_interest
            .to_u64()
            .checked_sub(plan.interest_share_e8s)
            .is_none()
    {
        return Err(
            "pinned liquidation arithmetic no longer fits the exact vault state; receipt is held"
                .into(),
        );
    }
    let routing = plan
        .interest_routing_plan
        .as_ref()
        .ok_or_else(|| "pinned interest routing plan is missing; receipt is held".to_string())?;
    if plan.interest_share_e8s > 0 {
        if let crate::ManualLiquidationRoute::PartialStable { .. } = &row.route {
            let bps = routing
                .split
                .iter()
                .try_fold(0u64, |sum, recipient| sum.checked_add(recipient.bps));
            if bps != Some(10_000) {
                return Err("pinned stable-interest split is invalid; receipt is held".into());
            }
            let requires_icusd = routing.split.iter().any(|recipient| {
                recipient.bps > 0
                    && matches!(
                        recipient.destination,
                        crate::state::InterestDestination::StabilityPool
                            | crate::state::InterestDestination::ThreePool
                            | crate::state::InterestDestination::Amm1
                    )
            });
            if requires_icusd && routing.icusd_ledger == Principal::anonymous() {
                return Err("pinned icUSD interest ledger is unavailable; receipt is held".into());
            }
            if routing.split.iter().any(|recipient| {
                recipient.bps > 0
                    && recipient.destination == crate::state::InterestDestination::StabilityPool
            }) && routing.stability_pool.is_none()
            {
                return Err(
                    "pinned Stability Pool interest route is unavailable; receipt is held".into(),
                );
            }
        }
        let amm_rows = routing
            .split
            .iter()
            .filter(|recipient| {
                recipient.bps > 0
                    && recipient.destination == crate::state::InterestDestination::Amm1
                    && routing.amm1.is_some()
            })
            .count() as u64;
        if state.amm1_donation_nonce.checked_add(amm_rows).is_none() {
            return Err("AMM donation nonce exhausted before interest outbox creation".into());
        }
    }
    Ok(())
}

pub async fn attach_manual_liquidation_v2_candidate(
    request_id: u128,
    block_index: u64,
) -> Result<crate::ManualLiquidationV2StatusView, ProtocolError> {
    let owner = ic_cdk::api::caller();
    const WINDOW_NS: u64 = 60_000_000_000;
    const MAX_ATTEMPTS: u8 = 8;
    let mut row = read_state(|s| s.manual_liquidation_v2_active.get(&owner).cloned())
        .filter(|row| row.request_id == request_id)
        .ok_or_else(|| {
            ProtocolError::GenericError("manual liquidation request is not active".into())
        })?;
    if row.candidate_block_index.is_some() {
        return Err(ProtocolError::GenericError(
            "manual liquidation receipt candidate is already attached".into(),
        ));
    }
    let now = ic_cdk::api::time();
    if row.candidate_attach_window_start_ns == 0
        || now.saturating_sub(row.candidate_attach_window_start_ns) >= WINDOW_NS
    {
        row.candidate_attach_window_start_ns = now;
        row.candidate_attach_attempts = 0;
    }
    if row.candidate_attach_attempts >= MAX_ATTEMPTS {
        return Err(ProtocolError::TemporarilyUnavailable(
            "manual liquidation candidate verification rate limit reached".into(),
        ));
    }
    row.candidate_attach_attempts += 1;
    mutate_state(|s| crate::state::save_manual_liquidation_v2(s, row.clone()))
        .map_err(ProtocolError::GenericError)?;
    if let Err(error) = verify_manual_liquidation_pull_receipt(&row, block_index).await {
        row.phase = crate::ManualLiquidationV2Phase::HeldPull;
        row.had_ambiguous_attempt = true;
        row.last_error = Some(format!(
            "attached manual liquidation block was not an exact receipt: {error}"
        ));
        mutate_state(|s| crate::state::save_manual_liquidation_v2(s, row.clone()))
            .map_err(ProtocolError::GenericError)?;
        return Ok(row.status_view());
    }
    row.candidate_block_index = Some(block_index);
    row.phase = crate::ManualLiquidationV2Phase::PendingPull;
    row.last_error = None;
    mutate_state(|s| crate::state::save_manual_liquidation_v2(s, row.clone()))
        .map_err(ProtocolError::GenericError)?;
    settle_manual_liquidation_v2(row).await
}

async fn manual_liquidation_v2(
    request_id: u128,
    vault_id: u64,
    route: crate::ManualLiquidationRoute,
    requested_amount_e8s: u64,
) -> Result<crate::ManualLiquidationV2StatusView, ProtocolError> {
    let caller = ic_cdk::api::caller();
    if request_id == 0
        || (matches!(
            &route,
            crate::ManualLiquidationRoute::PartialIcusd
                | crate::ManualLiquidationRoute::PartialStable { .. }
        ) && requested_amount_e8s == 0)
    {
        return Err(ProtocolError::GenericError(
            "manual liquidation request ID and partial amount must be nonzero".into(),
        ));
    }

    // Resolve an exact replay before reading mutable vault or oracle state.
    // A lost outer reply therefore resumes the original request and tuple.
    if let Some(row) = read_state(|s| {
        s.manual_liquidation_v2_active
            .get(&caller)
            .cloned()
            .or_else(|| s.manual_liquidation_v2_latest_result.get(&caller).cloned())
    }) {
        if row.request_id == request_id {
            if !manual_liquidation_v2_matches(
                &row,
                caller,
                request_id,
                vault_id,
                &route,
                requested_amount_e8s,
            ) {
                return Err(ProtocolError::GenericError(
                    "manual liquidation request ID is bound to another payload".into(),
                ));
            }
            if matches!(
                row.phase,
                crate::ManualLiquidationV2Phase::CommittedPayoutQueued
                    | crate::ManualLiquidationV2Phase::Refunded
                    | crate::ManualLiquidationV2Phase::Rejected
            ) {
                return Ok(row.status_view());
            }
            return settle_manual_liquidation_v2(row).await;
        }
        if read_state(|s| s.manual_liquidation_v2_active.contains_key(&caller)) {
            return Err(ProtocolError::AlreadyProcessing);
        }
        if request_id <= row.request_id {
            return Err(ProtocolError::GenericError(
                "manual liquidation request ID is older than the retained result".into(),
            ));
        }
    }

    if !MANUAL_LIQUIDATION_V2_ADMISSION_ENABLED {
        return Err(ProtocolError::TemporarilyUnavailable(
            "manual liquidation V2 admission is held pending exact post-payment compensation and payout proof".into(),
        ));
    }

    let _vault_guard = VaultLiquidationGuard::new(vault_id)?;
    reject_if_bot_processing(vault_id)?;
    reject_active_xrp_sp_absorb_preflight(vault_id, ic_cdk::api::time())?;
    reject_pending_collateral_withdrawal(vault_id)?;

    // Resolve the payment ledger and fee before pinning the vault quote. This
    // call has no token effect; the request's complete quote is captured only
    // after it returns.
    let (ledger, token_fee, stable_fee_rate) = match &route {
        crate::ManualLiquidationRoute::FullIcusd | crate::ManualLiquidationRoute::PartialIcusd => {
            let ledger = read_state(|s| s.icusd_ledger_principal);
            // The bundled icUSD ledger burns fee-free transfers to its minter
            // account. The pinned tuple and burn proof both require fee zero.
            (ledger, 0, None)
        }
        crate::ManualLiquidationRoute::PartialStable { token_type } => {
            let (ledger, fee_rate, enabled) = read_state(|s| {
                let ledger = match token_type {
                    crate::StableTokenType::CKUSDT => s.ckusdt_ledger_principal,
                    crate::StableTokenType::CKUSDC => s.ckusdc_ledger_principal,
                };
                let enabled = match token_type {
                    crate::StableTokenType::CKUSDT => s.ckusdt_enabled,
                    crate::StableTokenType::CKUSDC => s.ckusdc_enabled,
                };
                (ledger, s.ckstable_repay_fee, enabled)
            });
            if !enabled {
                return Err(ProtocolError::TemporarilyUnavailable(
                    "selected stablecoin liquidation route is disabled".into(),
                ));
            }
            let ledger = ledger.ok_or_else(|| {
                ProtocolError::TemporarilyUnavailable(
                    "selected stable ledger is not configured".into(),
                )
            })?;
            let fee = management::get_ledger_fee(ledger)
                .await
                .map_err(ProtocolError::GenericError)?;
            (ledger, fee, Some(fee_rate))
        }
    };

    let now = ic_cdk::api::time();
    mutate_state(|s| s.accrue_single_vault(vault_id, now));
    let (plan, pull_amount_raw) = read_state(|s| -> Result<_, ProtocolError> {
        let vault = s
            .vault_id_to_vaults
            .get(&vault_id)
            .cloned()
            .ok_or_else(|| ProtocolError::GenericError(format!("Vault #{vault_id} not found")))?;
        if vault.bot_processing || s.pending_collateral_withdrawals.contains_key(&vault_id) {
            return Err(ProtocolError::TemporarilyUnavailable(
                "vault is reserved by another operation".into(),
            ));
        }
        if let Some(status) = s.get_collateral_status(&vault.collateral_type) {
            if !status.allows_liquidation() {
                return Err(ProtocolError::TemporarilyUnavailable(
                    "liquidation is not allowed for this collateral type".into(),
                ));
            }
        }
        let price = s
            .get_collateral_price_decimal(&vault.collateral_type)
            .ok_or_else(|| {
                ProtocolError::TemporarilyUnavailable("no collateral price available".into())
            })?;
        let decimals = s
            .get_collateral_config(&vault.collateral_type)
            .map(|c| c.decimals)
            .unwrap_or(8);
        let collateral_price_usd = UsdIcp::from(price);
        let ratio = compute_collateral_ratio(&vault, collateral_price_usd, s);
        if ratio >= s.get_min_liquidation_ratio_for(&vault.collateral_type) {
            return Err(ProtocolError::GenericError(format!(
                "Vault #{vault_id} is not liquidatable"
            )));
        }
        let requested = match &route {
            crate::ManualLiquidationRoute::FullIcusd => None,
            _ => Some(ICUSD::from(requested_amount_e8s)),
        };
        let debt = s.effective_liquidation_amount(&vault, collateral_price_usd, requested);
        if debt == ICUSD::new(0) || debt > vault.borrowed_icusd_amount {
            return Err(ProtocolError::GenericError(
                "liquidation debt is zero or exceeds vault debt".into(),
            ));
        }
        if debt < s.min_icusd_amount && debt != vault.borrowed_icusd_amount {
            return Err(ProtocolError::AmountTooLow {
                minimum_amount: s.min_icusd_amount.to_u64(),
            });
        }
        let collateral_raw = crate::numeric::try_icusd_to_collateral_amount(debt, price, decimals)
            .ok_or_else(|| {
                ProtocolError::GenericError("liquidation collateral conversion overflow".into())
            })?;
        let total_to_seize = (ICP::from(collateral_raw)
            * s.get_liquidation_bonus_for(&vault.collateral_type))
        .min(ICP::from(vault.collateral_amount));
        let bonus = total_to_seize.to_u64().saturating_sub(collateral_raw);
        let protocol_cut = (rust_decimal::Decimal::from(bonus)
            * s.get_liquidation_protocol_share().0)
            .to_u64()
            .ok_or_else(|| {
                ProtocolError::GenericError("protocol cut conversion overflow".into())
            })?;
        let collateral_to_liquidator = total_to_seize
            .to_u64()
            .checked_sub(protocol_cut)
            .ok_or_else(|| ProtocolError::GenericError("protocol cut exceeds seizure".into()))?;
        if total_to_seize == ICP::new(0) || collateral_to_liquidator == 0 {
            return Err(ProtocolError::GenericError(
                "liquidation would produce no collateral payout".into(),
            ));
        }
        let is_partial = debt < vault.borrowed_icusd_amount;
        let excess = if is_partial {
            0
        } else {
            vault
                .collateral_amount
                .saturating_sub(total_to_seize.to_u64())
        };
        let interest = if vault.borrowed_icusd_amount.0 > 0 {
            crate::numeric::proportional_interest_share(
                debt.0,
                vault.accrued_interest.0,
                vault.borrowed_icusd_amount.0,
            )
            .min(vault.accrued_interest.0)
        } else {
            0
        };
        let interest_routing_plan = crate::state::StableRepaymentV2InterestRoutingPlan {
            split: s.interest_split.clone(),
            stable_treasury: s.treasury_principal,
            icusd_ledger: s.icusd_ledger_principal,
            stability_pool: s.stability_pool_canister,
            three_pool: s.three_pool_canister,
            amm1: s.amm1_canister,
            amm1_pool_id: s.amm1_pool_id.clone(),
        };
        let plan = crate::state::ManualLiquidationPinnedPlan {
            vault,
            mode: s.mode,
            collateral_price: price,
            collateral_decimals: decimals,
            debt_liquidated_e8s: debt.to_u64(),
            collateral_to_liquidator_raw: collateral_to_liquidator,
            collateral_to_seize_raw: total_to_seize.to_u64(),
            protocol_cut_raw: protocol_cut,
            excess_collateral_raw: excess,
            interest_share_e8s: interest,
            stable_surcharge_e6s: 0,
            interest_routing_plan: Some(interest_routing_plan),
        };
        let (pull_amount, surcharge_e6s) = match (&route, stable_fee_rate) {
            (crate::ManualLiquidationRoute::PartialStable { .. }, Some(rate)) => {
                let (principal, surcharge, total) = stable_repay_pull_e6s(debt, rate)?;
                if principal.checked_add(surcharge) != Some(total) {
                    return Err(ProtocolError::GenericError(
                        "stable liquidation pull arithmetic overflow".into(),
                    ));
                }
                (total, surcharge)
            }
            _ => (debt.to_u64(), 0),
        };
        if pull_amount == 0 {
            return Err(ProtocolError::GenericError(
                "liquidation pull amount is zero".into(),
            ));
        }
        let mut plan = plan;
        plan.stable_surcharge_e6s = surcharge_e6s;
        Ok((plan, pull_amount))
    })?;

    let expected_id = read_state(|s| {
        s.manual_liquidation_v2_high_water
            .get(&caller)
            .copied()
            .unwrap_or(0)
            .checked_add(1)
    })
    .ok_or_else(|| {
        ProtocolError::GenericError("manual liquidation request ID sequence exhausted".into())
    })?;
    if request_id != expected_id {
        return Err(ProtocolError::GenericError(format!(
            "manual liquidation request ID must be {expected_id}"
        )));
    }
    let op_nonce = mutate_state(|s| s.next_op_nonce());
    let tuple = crate::SpLiquidationStablePullTuple {
        op_nonce,
        ledger,
        from: icrc_ledger_types::icrc1::account::Account {
            owner: caller,
            subaccount: None,
        },
        spender: icrc_ledger_types::icrc1::account::Account {
            owner: ic_cdk::id(),
            subaccount: None,
        },
        to: icrc_ledger_types::icrc1::account::Account {
            owner: ic_cdk::id(),
            subaccount: None,
        },
        amount_raw: pull_amount_raw,
        fee_raw: token_fee,
        memo: management::nonce_to_memo(op_nonce).0.to_vec(),
        created_at_time_ns: management::nonce_to_created_at_time(op_nonce),
    };
    let row = crate::state::ManualLiquidationV2Journal {
        owner: caller,
        request_id,
        vault_id,
        route,
        requested_amount_e8s,
        pull_amount_raw,
        ledger,
        tuple,
        created_at_ns: now,
        plan,
        phase: crate::ManualLiquidationV2Phase::PendingPull,
        candidate_block_index: None,
        candidate_attach_window_start_ns: 0,
        candidate_attach_attempts: 0,
        had_ambiguous_attempt: false,
        dispatch_attempts: 0,
        result: None,
        refund: None,
        commit_started: false,
        last_error: None,
    };
    mutate_state(|s| crate::state::admit_manual_liquidation_v2(s, row.clone()))
        .map_err(ProtocolError::GenericError)?;
    drop(_vault_guard);
    settle_manual_liquidation_v2(row).await
}

pub async fn partial_liquidate_vault(arg: VaultArg) -> Result<SuccessWithFee, ProtocolError> {
    let caller = ic_cdk::api::caller();
    let guard_principal =
        GuardPrincipal::new(caller, &format!("partial_liquidate_vault_{}", arg.vault_id))?;
    reject_if_bot_processing(arg.vault_id)?; // LIQ-101: don't double-seize a bot-claimed vault
    let _vault_liq_guard = VaultLiquidationGuard::new(arg.vault_id)?; // BK-001/002 per-vault lock
    if let Err(e) = reject_active_xrp_sp_absorb_preflight(arg.vault_id, ic_cdk::api::time()) {
        guard_principal.fail();
        return Err(e);
    }

    // Wave-8b LIQ-002 band gate deactivated 2026-05-18 (see
    // `liquidate_vault_partial` above for rationale).

    let liquidator_payment: ICUSD = arg.amount.into();

    // LIQ-0XX: a requested amount of exactly zero is always rejected,
    // regardless of the dust rule below (which can still close a vault
    // whose FULL debt is small, but never accepts a caller-requested 0).
    if liquidator_payment == ICUSD::new(0) {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "Cannot liquidate zero amount".to_string(),
        ));
    }

    // Accrue interest before liquidation so CR check uses up-to-date debt.
    let now = ic_cdk::api::time();
    mutate_state(|s| s.accrue_single_vault(arg.vault_id, now));

    // Step 1: Validate vault is liquidatable
    let (vault, collateral_price, config_decimals, collateral_price_usd, _mode) =
        match read_state(|s| {
            match s.vault_id_to_vaults.get(&arg.vault_id) {
                Some(vault) => {
                    // Check collateral status allows liquidation
                    if let Some(status) = s.get_collateral_status(&vault.collateral_type) {
                        if !status.allows_liquidation() {
                            return Err(
                                "Liquidation is not allowed for this collateral type.".to_string()
                            );
                        }
                    }

                    let price = s
                        .get_collateral_price_decimal(&vault.collateral_type)
                        .ok_or_else(|| {
                            "No price available for collateral. Price feed may be down.".to_string()
                        })?;
                    let decimals = s
                        .get_collateral_config(&vault.collateral_type)
                        .map(|c| c.decimals)
                        .unwrap_or(8);
                    let collateral_price_usd = UsdIcp::from(price);
                    let ratio = compute_collateral_ratio(vault, collateral_price_usd, s);
                    let min_liq_ratio = s.get_min_liquidation_ratio_for(&vault.collateral_type);

                    if ratio >= min_liq_ratio {
                        Err(format!(
                            "Vault #{} is not liquidatable. Current ratio: {}, minimum: {}",
                            arg.vault_id,
                            ratio.to_f64(),
                            min_liq_ratio.to_f64()
                        ))
                    } else {
                        Ok((vault.clone(), price, decimals, collateral_price_usd, s.mode))
                    }
                }
                None => Err(format!("Vault #{} not found", arg.vault_id)),
            }
        }) {
            Ok(result) => result,
            Err(msg) => {
                guard_principal.fail();
                return Err(ProtocolError::GenericError(msg));
            }
        };

    // Step 2: LIQ-0XX single shared decision point for the amount — dust-
    // vault full-close, cap (recovery/partial), requested amount, min floor,
    // and the LIQ-003 residual round-up.
    let liquidator_payment = read_state(|s| {
        s.effective_liquidation_amount(&vault, collateral_price_usd, Some(liquidator_payment))
    });

    if liquidator_payment == ICUSD::new(0) {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "Cannot liquidate zero amount".to_string(),
        ));
    }

    // min_icusd_amount applies to the FINAL amount, and is skipped when the
    // final amount closes the vault fully (a dust vault must be closable
    // even if the liquidator requested less than the floor).
    if liquidator_payment < read_state(|s| s.min_icusd_amount)
        && liquidator_payment != vault.borrowed_icusd_amount
    {
        guard_principal.fail();
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: read_state(|s| s.min_icusd_amount).to_u64(),
        });
    }

    if liquidator_payment > vault.borrowed_icusd_amount {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(format!(
            "cannot liquidate more than borrowed: {} ICUSD, liquidate: {} ICUSD",
            vault.borrowed_icusd_amount, liquidator_payment
        )));
    }

    // Step 3: Calculate liquidation amounts with liquidation bonus and protocol fee
    let (liq_bonus, protocol_share) = read_state(|s| {
        (
            s.get_liquidation_bonus_for(&vault.collateral_type),
            s.get_liquidation_protocol_share(),
        )
    });
    let collateral_raw = crate::numeric::try_icusd_to_collateral_amount(
        liquidator_payment,
        collateral_price,
        config_decimals,
    );
    let Some(collateral_raw) = collateral_raw else {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "Cannot safely size liquidation collateral: conversion is unrepresentable".to_string(),
        ));
    };
    let icp_with_bonus = ICP::from(collateral_raw) * liq_bonus;
    let total_to_seize = icp_with_bonus.min(ICP::from(vault.collateral_amount));

    // Split: protocol gets a share of the bonus portion (liquidator's profit)
    let bonus_portion = total_to_seize.to_u64().saturating_sub(collateral_raw);
    let protocol_cut = (rust_decimal::Decimal::from(bonus_portion) * protocol_share.0)
        .to_u64()
        .unwrap_or(0);
    let collateral_to_liquidator = ICP::from(total_to_seize.to_u64() - protocol_cut);

    if collateral_to_liquidator == ICP::new(0) || total_to_seize == ICP::new(0) {
        guard_principal.fail();
        return Err(ProtocolError::GenericError(
            "Liquidation would produce no collateral payout".to_string(),
        ));
    }

    log!(INFO,
        "[partial_liquidate_vault] Vault #{}: liquidator pays {} icUSD, gets {} ICP (protocol fee: {} ICP, bonus: {})",
        arg.vault_id,
        liquidator_payment.to_u64(),
        collateral_to_liquidator.to_u64(),
        protocol_cut,
        liq_bonus.to_f64()
    );

    // Step 4: Take icUSD from liquidator
    reject_pending_collateral_withdrawal(arg.vault_id)?;
    let icusd_block_index = match transfer_icusd_from(liquidator_payment, caller).await {
        Ok(block_index) => {
            log!(
                INFO,
                "[partial_liquidate_vault] Received {} icUSD from liquidator",
                liquidator_payment.to_u64()
            );
            block_index
        }
        Err(transfer_from_error) => {
            guard_principal.fail();
            return Err(ProtocolError::TransferFromError(
                transfer_from_error,
                liquidator_payment.to_u64(),
            ));
        }
    };

    // Step 5: Update protocol state ATOMICALLY
    let mut immediate_payouts = Vec::new();
    let (interest_share, xrp_claim_id) = mutate_state(|s| {
        // Compute proportional interest share before reducing debt
        let interest_share = if let Some(vault) = s.vault_id_to_vaults.get(&arg.vault_id) {
            if vault.accrued_interest.0 > 0 && vault.borrowed_icusd_amount.0 > 0 {
                ICUSD::new(crate::numeric::proportional_interest_share(
                    liquidator_payment.0,
                    vault.accrued_interest.0,
                    vault.borrowed_icusd_amount.0,
                ))
            } else {
                ICUSD::new(0)
            }
        } else {
            ICUSD::new(0)
        };

        // Reduce the vault's debt by the liquidator payment amount
        // Vault loses total_to_seize (liquidator + protocol cut)
        //
        // AR-B-001/BK-001 (audit 2026-06-09): capture applied amounts and
        // re-cap the payout, mirroring `liquidate_vault_partial`.
        let mut debt_applied = liquidator_payment;
        let mut collateral_applied = total_to_seize.to_u64();
        if let Some(vault) = s.vault_id_to_vaults.get_mut(&arg.vault_id) {
            // ASYNC-001: cap each reduction to the CURRENT vault state and
            // saturating_sub (same race as the other partial-liq paths).
            debt_applied = liquidator_payment.min(vault.borrowed_icusd_amount);
            collateral_applied = total_to_seize.to_u64().min(vault.collateral_amount);
            let interest_applied = interest_share.min(vault.accrued_interest);
            vault.borrowed_icusd_amount = vault.borrowed_icusd_amount.saturating_sub(debt_applied);
            vault.collateral_amount = vault.collateral_amount.saturating_sub(collateral_applied);
            vault.accrued_interest = vault.accrued_interest.saturating_sub(interest_applied);
        }
        let payout_to_liquidator = ICP::from(collateral_applied.saturating_sub(protocol_cut));

        // Wave-10 LIQ-008: append the gross debt cleared to the rolling-
        // window log for the mass-liquidation circuit breaker.
        crate::event::record_liquidation_for_breaker(s, liquidator_payment.to_u64());

        // Wave-8e LIQ-005: per-call deficit accrual against the APPLIED amounts.
        let seized_usd = crate::numeric::collateral_usd_value(
            collateral_applied,
            collateral_price,
            config_decimals,
        );
        let shortfall = if seized_usd < debt_applied {
            debt_applied - seized_usd
        } else {
            ICUSD::new(0)
        };
        if shortfall.0 > 0 {
            crate::event::record_deficit_accrued(
                s,
                crate::event::DeficitSource::Liquidation {
                    vault_id: arg.vault_id,
                },
                shortfall,
                ic_cdk::api::time(),
            );
            if s.check_deficit_readonly_latch() {
                log!(INFO,
                    "[LIQ-005] deficit threshold {} crossed by partial_liquidate_vault #{} shortfall {}; auto-latched ReadOnly",
                    s.deficit_readonly_threshold_e8s, arg.vault_id, shortfall.to_u64()
                );
            }
        }

        // Record the partial liquidation event (applied payout, replay-exact)
        let event = crate::event::Event::PartialLiquidateVault {
            vault_id: arg.vault_id,
            liquidator_payment,
            icp_to_liquidator: payout_to_liquidator,
            liquidator: Some(caller),
            icp_rate: Some(collateral_price_usd),
            protocol_fee_collateral: if protocol_cut > 0 {
                Some(protocol_cut.min(collateral_applied))
            } else {
                None
            },
            timestamp: Some(ic_cdk::api::time()),
            three_usd_reserves_e8s: None,
        };
        crate::storage::record_event(&event);

        // Create pending transfer for liquidator reward (minus protocol cut)
        let nonce = s.next_op_nonce();
        let xrp_claim_id = queue_collateral_payout(
            s,
            arg.vault_id,
            vault.owner,
            caller,
            payout_to_liquidator,
            vault.collateral_type,
            nonce,
            ic_cdk::api::time(),
            &mut immediate_payouts,
        );

        // Shared drain rule (see state::cleanup_if_drained): remove the vault
        // if this liquidation emptied it, else re-key its CR index entry.
        if s.cleanup_if_drained(arg.vault_id) {
            log!(
                INFO,
                "[partial_liquidate_vault] Vault #{} fully liquidated — removed",
                arg.vault_id
            );
        }

        log!(
            INFO,
            "[partial_liquidate_vault] Protocol state updated, pending transfer created"
        );
        (interest_share, xrp_claim_id)
    });

    // Route interest share via N-way split
    // IC-B-002 (audit 2026-06-09): re-queue any unminted interest share so the
    // next flush retries it instead of silently dropping treasury revenue.
    let unminted_interest =
        crate::treasury::distribute_interest(interest_share, vault.collateral_type).await;
    if unminted_interest.to_u64() > 0 {
        mutate_state(|s| {
            s.restore_pending_interest_for_pool(vault.collateral_type, unminted_interest.to_u64())
        });
    }

    // Send protocol's liquidation fee cut to treasury (fire-and-forget)
    if protocol_cut > 0 {
        if vault.collateral_type == crate::state::xrp_collateral_principal() {
            // P5: native-XRP protocol fee -> a developer-settleable XrpClaim (the
            // ICRC treasury transfer cannot target the synthetic XRP ledger). Keyed
            // by collateral_type (not a vault lookup, since the vault may already be
            // drained/removed by cleanup_if_drained above).
            let dev = read_state(|s| s.developer_principal);
            let now_ns = ic_cdk::api::time();
            mutate_state(|s| {
                record_xrp_claim(
                    s,
                    dev,
                    vault.owner,
                    vault.vault_id,
                    protocol_cut.to_u64().unwrap_or(0),
                    now_ns,
                );
            });
        } else {
            let asset_type = crate::treasury::collateral_to_asset_type(&vault.collateral_type);
            crate::treasury::send_liquidation_fee_to_treasury(
                protocol_cut,
                vault.collateral_type,
                asset_type,
            )
            .await;
        }
    }

    // Step 6: Attempt immediate transfer processing
    log!(
        INFO,
        "[partial_liquidate_vault] Attempting immediate transfer processing..."
    );

    match try_process_pending_transfers_immediate(&immediate_payouts).await {
        Ok(processed_count) => {
            log!(
                INFO,
                "[partial_liquidate_vault] Successfully processed {} transfers immediately",
                processed_count
            );
        }
        Err(e) => {
            log!(INFO, "[partial_liquidate_vault] Immediate processing failed: {}. Transfers will be retried via timer", e);
            schedule_transfer_retry(arg.vault_id, immediate_payouts.clone(), 0);
        }
    }

    // Step 7: Schedule backup timer
    ic_cdk_timers::set_timer(std::time::Duration::from_secs(2), move || {
        ic_cdk::spawn(async move {
            log!(
                INFO,
                "[partial_liquidate_vault] Backup timer processing transfers for vault #{}",
                arg.vault_id
            );
            let _ = crate::process_pending_transfer().await;
        })
    });

    // Step 8: Liquidation is successful
    guard_principal.complete();

    // Calculate fee (the 10% discount is the fee)
    let liquidator_value_received = crate::numeric::collateral_usd_value(
        collateral_to_liquidator.to_u64(),
        collateral_price,
        config_decimals,
    );
    let fee_amount = if liquidator_value_received > liquidator_payment {
        liquidator_value_received - liquidator_payment
    } else {
        ICUSD::new(0)
    };

    log!(INFO, "[partial_liquidate_vault] Partial liquidation completed successfully. Block index: {}, Fee: {}, Collateral: {}",
         icusd_block_index, fee_amount.to_u64(), collateral_to_liquidator.to_u64());

    Ok(SuccessWithFee {
        block_index: icusd_block_index,
        fee_amount_paid: fee_amount.to_u64(),
        collateral_amount_received: Some(collateral_to_liquidator.to_u64()),
        debt_liquidated_e8s: None, // SP-101
        stable_pulled_e6s: None,   // SP-110
        xrp_claim_id,
    })
}

#[cfg(test)]
mod sp_writedown_native_xrp_guard_tests {
    use super::*;
    use crate::icrc3_proof::{SpProofLedger, SpWritedownProof};
    use crate::state::{replace_state, xrp_collateral_principal, CustodyKind, State};

    /// Install a thread-local state holding two vaults: an ICRC (ICP) vault at
    /// `icp_vault_id` and a native-XRP vault at `xrp_vault_id`. The native-XRP
    /// collateral config is the ICP config cloned with `custody_kind = NativeXrp`,
    /// mirroring the P4/P5 registration shape.
    fn install_two_collateral_state(xrp_vault_id: u64, icp_vault_id: u64) {
        let mut s = State::from(crate::InitArg {
            xrc_principal: Principal::anonymous(),
            icusd_ledger_principal: Principal::anonymous(),
            icp_ledger_principal: Principal::anonymous(),
            fee_e8s: 0,
            developer_principal: Principal::anonymous(),
            treasury_principal: None,
            stability_pool_principal: None,
            ckusdt_ledger_principal: None,
            ckusdc_ledger_principal: None,
        });
        // Deterministic: never trip the min-amount gate before reaching the guard.
        s.min_icusd_amount = ICUSD::new(0);

        let icp = s.icp_collateral_type();
        if let Some(c) = s.collateral_configs.get_mut(&icp) {
            c.last_price = Some(5.0);
        }
        s.open_vault(Vault {
            owner: Principal::anonymous(),
            vault_id: icp_vault_id,
            borrowed_icusd_amount: ICUSD::new(1_000_000_000),
            collateral_amount: 1_000_000_000,
            collateral_type: icp,
            accrued_interest: ICUSD::new(0),
            last_accrual_time: 0,
            bot_processing: false,
        });

        let xrp = xrp_collateral_principal();
        let mut xrp_cfg = s.collateral_configs.get(&icp).unwrap().clone();
        xrp_cfg.ledger_canister_id = xrp;
        xrp_cfg.custody_kind = Some(CustodyKind::NativeXrp);
        xrp_cfg.last_price = Some(0.5);
        s.collateral_configs.insert(xrp, xrp_cfg);
        s.open_vault(Vault {
            owner: Principal::anonymous(),
            vault_id: xrp_vault_id,
            borrowed_icusd_amount: ICUSD::new(1_000_000_000),
            collateral_amount: 5_000_000,
            collateral_type: xrp,
            accrued_interest: ICUSD::new(0),
            last_accrual_time: 0,
            bot_processing: false,
        });

        replace_state(s);
    }

    fn dummy_proof(vault_id_memo: u64) -> SpWritedownProof {
        SpWritedownProof {
            block_index: 0,
            ledger_kind: SpProofLedger::IcusdBurn,
            vault_id_memo,
        }
    }

    /// Defense-in-depth: the SP write-down core must reject a native-XRP vault
    /// before doing anything else, regardless of caller. The SP settles against
    /// the icUSD/3pool ledger but the seized XRP lives on XRPL as an XrpClaim the
    /// SP cannot settle, so a write-down here would strand the XRP and burn SP
    /// depositors. Native-XRP is liquidated only via the manual paths.
    #[test]
    fn sp_writedown_rejects_native_xrp_vault() {
        install_two_collateral_state(2, 1);
        let caller = Principal::from_slice(&[0xcc; 16]);
        let result = futures::executor::block_on(liquidate_vault_debt_already_burned(
            2,
            1_000_000_000,
            caller,
            None,
            dummy_proof(2),
        ));
        match result {
            Err(ProtocolError::GenericError(msg)) => assert!(
                msg.contains(
                    "Native-XRP collateral cannot be liquidated via the SP write-down path"
                ),
                "expected the native-XRP reject, got: {msg}"
            ),
            other => panic!("expected a native-XRP GenericError reject, got {other:?}"),
        }
    }

    /// The predicate is scoped to native-XRP custody: ICRC collateral and a
    /// missing vault must both read false, so the guard never short-circuits the
    /// legitimate ICRC write-down flow nor masks the normal not-found error.
    #[test]
    fn vault_is_native_xrp_is_scoped_to_xrp_custody() {
        install_two_collateral_state(2, 1);
        assert!(vault_is_native_xrp(2), "native-XRP vault must be flagged");
        assert!(
            !vault_is_native_xrp(1),
            "ICRC (ICP) vault must not be flagged"
        );
        assert!(
            !vault_is_native_xrp(999),
            "missing vault must not be flagged"
        );
    }
}

#[cfg(test)]
mod xrp_sp_absorb_contract_tests {
    use super::*;
    use crate::icrc3_proof::{SpProofLedger, SpWritedownProof};
    use crate::state::{
        xrp_collateral_principal, CollateralStatus, CustodyKind, State, StoredSpBurnRefund,
        StoredXrpSpAbsorbResult, MAX_SP_XRP_ABSORB_RESULTS_BY_PROOF,
    };
    use crate::{XrpSpAbsorbRequest, XrpSpPayoutAllocation, MAX_XRP_SP_PAYOUT_ALLOCATIONS};

    const E8: u64 = 100_000_000;
    const VAULT_ID: u64 = 7;

    fn principal(byte: u8) -> Principal {
        Principal::from_slice(&[byte; 29])
    }

    fn sp() -> Principal {
        principal(0x53)
    }

    fn depositor_a() -> Principal {
        principal(0xa1)
    }

    fn depositor_b() -> Principal {
        principal(0xb2)
    }

    fn test_state_with_xrp_vault() -> State {
        let mut state = State::from(crate::InitArg {
            xrc_principal: Principal::anonymous(),
            icusd_ledger_principal: principal(0x10),
            icp_ledger_principal: principal(0x11),
            fee_e8s: 0,
            developer_principal: principal(0xdd),
            treasury_principal: None,
            stability_pool_principal: Some(sp()),
            ckusdt_ledger_principal: None,
            ckusdc_ledger_principal: None,
        });
        state.min_icusd_amount = ICUSD::new(0);
        state.liquidation_protocol_share = Ratio::from(Decimal::ZERO);

        let icp = state.icp_collateral_type();
        if let Some(cfg) = state.collateral_configs.get_mut(&icp) {
            cfg.last_price = Some(10.0);
        }
        state.open_vault(Vault {
            owner: principal(0x99),
            vault_id: 1,
            borrowed_icusd_amount: ICUSD::new(100 * E8),
            collateral_amount: 2_000_000_000,
            collateral_type: icp,
            last_accrual_time: 0,
            accrued_interest: ICUSD::new(0),
            bot_processing: false,
        });

        let xrp = xrp_collateral_principal();
        let mut xrp_cfg = crate::state::xrp_collateral_config(
            Ratio::from(Decimal::ZERO),
            Ratio::from(Decimal::ZERO),
            Ratio::new(dec!(1.033333333333333333)),
        );
        xrp_cfg.last_price = Some(0.5);
        xrp_cfg.status = CollateralStatus::Active;
        xrp_cfg.custody_kind = Some(CustodyKind::NativeXrp);
        state.collateral_configs.insert(xrp, xrp_cfg);
        state.open_vault(Vault {
            owner: principal(0x42),
            vault_id: VAULT_ID,
            borrowed_icusd_amount: ICUSD::new(100 * E8),
            collateral_amount: 100_000_000,
            collateral_type: xrp,
            last_accrual_time: 0,
            accrued_interest: ICUSD::new(0),
            bot_processing: false,
        });
        state
    }

    fn proof(block_index: u64) -> SpWritedownProof {
        SpWritedownProof {
            block_index,
            ledger_kind: SpProofLedger::IcusdBurn,
            vault_id_memo: VAULT_ID,
        }
    }

    fn valid_request(block_index: u64) -> XrpSpAbsorbRequest {
        XrpSpAbsorbRequest {
            vault_id: VAULT_ID,
            icusd_burned_e8s: 100 * E8,
            proof: proof(block_index),
            allocations: vec![
                XrpSpPayoutAllocation {
                    claimant: depositor_a(),
                    payout_address: "rA".to_string(),
                    destination_tag: Some(7),
                    drops: 60_000_000,
                },
                XrpSpPayoutAllocation {
                    claimant: depositor_b(),
                    payout_address: "rB".to_string(),
                    destination_tag: None,
                    drops: 40_000_000,
                },
            ],
        }
    }

    fn preflight(state: &mut State, now_ns: u64) {
        stability_pool_preflight_xrp_absorb_in_state(state, sp(), VAULT_ID, 100 * E8, now_ns)
            .expect("preflight reservation");
    }

    fn assert_no_xrp_absorb_mutation(state: &State) {
        assert!(state.xrp_claims.is_empty());
        assert_eq!(state.next_xrp_claim_id, 0);
        assert!(state.sp_xrp_absorb_results_by_proof.is_empty());
        let vault = state.vault_id_to_vaults.get(&VAULT_ID).expect("vault");
        assert_eq!(vault.borrowed_icusd_amount, ICUSD::new(100 * E8));
        assert_eq!(vault.collateral_amount, 100_000_000);
    }

    #[test]
    fn releasing_unburned_preflight_unblocks_vault_operations() {
        // A preflight reservation blocks every vault-mutating entry point,
        // including the manual liquidation paths. If the SP gives up AFTER
        // reserving but BEFORE burning (e.g. no opted-in depositor holds
        // icUSD), it must be able to hand the reservation back instead of
        // leaving the vault unliquidatable by any path until the 15-minute
        // TTL expires.
        let mut state = test_state_with_xrp_vault();
        stability_pool_preflight_xrp_absorb_in_state(&mut state, sp(), VAULT_ID, 100 * E8, 10)
            .expect("preflight reservation");
        assert!(
            ensure_no_active_xrp_sp_absorb_preflight(&state, VAULT_ID, 20).is_err(),
            "an active reservation must block vault operations"
        );

        let released = stability_pool_release_xrp_absorb_preflight_in_state(
            &mut state,
            sp(),
            VAULT_ID,
            100 * E8,
        )
        .expect("registered SP may release its own unburned reservation");
        assert!(
            released,
            "release must report that a reservation was cleared"
        );
        assert!(state.sp_xrp_absorb_preflights.is_empty());
        assert!(
            ensure_no_active_xrp_sp_absorb_preflight(&state, VAULT_ID, 20).is_ok(),
            "releasing the reservation must unblock vault operations immediately"
        );

        // Idempotent: a retried release after the reservation is gone is not an
        // error (the SP may retry its cleanup after a lost reply).
        let again = stability_pool_release_xrp_absorb_preflight_in_state(
            &mut state,
            sp(),
            VAULT_ID,
            100 * E8,
        )
        .expect("release is idempotent");
        assert!(!again, "second release must report nothing was cleared");
    }

    #[test]
    fn preflight_release_rejects_non_sp_and_mismatched_reservations() {
        // The reservation is the SP's sizing snapshot for a burn it is about to
        // perform. Releasing someone else's reservation, or releasing under a
        // different burn amount than was reserved, would let a stale or hostile
        // caller drop a reservation the SP still intends to consume -- which
        // would strand an in-flight burn. Both are refused, and the
        // reservation survives.
        let mut state = test_state_with_xrp_vault();
        stability_pool_preflight_xrp_absorb_in_state(&mut state, sp(), VAULT_ID, 100 * E8, 10)
            .expect("preflight reservation");

        let not_sp = principal(0x77);
        stability_pool_release_xrp_absorb_preflight_in_state(
            &mut state,
            not_sp,
            VAULT_ID,
            100 * E8,
        )
        .expect_err("only the registered stability pool may release a reservation");
        assert!(
            state.sp_xrp_absorb_preflights.contains_key(&VAULT_ID),
            "a rejected release must not clear the reservation"
        );

        stability_pool_release_xrp_absorb_preflight_in_state(&mut state, sp(), VAULT_ID, 50 * E8)
            .expect_err("release must match the reserved burn amount");
        assert!(
            state.sp_xrp_absorb_preflights.contains_key(&VAULT_ID),
            "an amount-mismatched release must not clear the reservation"
        );
    }

    #[test]
    fn sp_settle_on_behalf_validation_gates() {
        // `stability_pool_settle_xrp_claim` lets the SP settle a depositor's
        // payout claim to the address the depositor registered. The pure
        // validation must enforce: caller is the registered SP, the claim
        // exists, the claimant matches the SP's record, and quarantined claims
        // are never auto-settled (F-03: they may already be paid under a
        // divergent hash and need admin reconciliation).
        let mut state = test_state_with_xrp_vault();
        let depositor = principal(0x77);
        state.xrp_claims.insert(
            9,
            crate::state::XrpClaim {
                claimant: depositor,
                drops: 11_529,
                custody_owner: principal(0x99),
                custody_nonce: VAULT_ID,
                created_at_ns: 1,
                settlement: None,
                quarantine_reason: None,
            },
        );

        assert!(
            validate_sp_settle_xrp_claim_in_state(&state, sp(), 9, depositor).is_ok(),
            "registered SP with matching claimant must pass"
        );
        assert!(
            validate_sp_settle_xrp_claim_in_state(&state, principal(0x66), 9, depositor).is_err(),
            "non-SP caller must be rejected"
        );
        assert!(
            validate_sp_settle_xrp_claim_in_state(&state, sp(), 9, principal(0x66)).is_err(),
            "claimant mismatch must be rejected"
        );
        let missing = validate_sp_settle_xrp_claim_in_state(&state, sp(), 10, depositor);
        assert!(
            format!("{missing:?}").contains("No such XRP claim"),
            "missing claim must use the settled-or-unknown wording the sweep keys off: {missing:?}"
        );

        state.xrp_claims.get_mut(&9).unwrap().quarantine_reason = Some("diverged".to_string());
        let quarantined = validate_sp_settle_xrp_claim_in_state(&state, sp(), 9, depositor);
        assert!(
            format!("{quarantined:?}").contains("quarantined"),
            "quarantined claim must be refused: {quarantined:?}"
        );
    }

    #[test]
    fn automated_dispatch_amount_is_accepted_by_xrp_preflight() {
        // End-to-end pin across the two halves of automated XRP absorption:
        // the amount `check_vaults` puts in `recommended_liquidation_amount`
        // must be an amount `stability_pool_preflight_xrp_absorb_in_state`
        // actually accepts. Regression for the sizing mismatch where the
        // dispatch sent the generic partial cap while the absorb path requires
        // the full live debt, so every automated absorb failed at preflight.
        //
        // The vault is at an ORDINARY breach (CR 130%: under the 133%
        // liquidation floor, above the 112% bonus). The deep-breach fixture
        // used elsewhere hides this bug, because the partial cap saturates to
        // the full debt once a vault is far enough underwater.
        let mut state = test_state_with_xrp_vault();
        let xrp = xrp_collateral_principal();
        if let Some(cfg) = state.collateral_configs.get_mut(&xrp) {
            cfg.last_price = Some(1.30);
        }
        let vault = state
            .vault_id_to_vaults
            .get(&VAULT_ID)
            .expect("vault")
            .clone();
        let dummy = UsdIcp::from(Decimal::ZERO);

        // The generic cap is a strict partial here — the pre-fix dispatch value.
        let generic_cap = state.compute_partial_liquidation_cap(&vault, dummy);
        assert!(
            generic_cap < vault.borrowed_icusd_amount,
            "premise: ordinary breach must yield a partial cap, got {generic_cap:?}"
        );
        let err = stability_pool_preflight_xrp_absorb_in_state(
            &mut state,
            sp(),
            VAULT_ID,
            generic_cap.to_u64(),
            10,
        )
        .expect_err("partial burn must be rejected by the full-debt-only absorb path");
        assert!(
            format!("{err:?}").contains("does not match live debt"),
            "unexpected error: {err:?}"
        );
        assert!(
            state.sp_xrp_absorb_preflights.is_empty(),
            "a rejected preflight must not leave a reservation behind"
        );

        // What the dispatch actually sends now is accepted.
        let dispatched = state.recommended_liquidation_amount_for(&vault, dummy);
        let preflight = stability_pool_preflight_xrp_absorb_in_state(
            &mut state,
            sp(),
            VAULT_ID,
            dispatched.to_u64(),
            20,
        )
        .expect("automated dispatch amount must be accepted by the preflight");
        assert_eq!(
            preflight.icusd_burn_e8s,
            vault.borrowed_icusd_amount.to_u64()
        );
    }

    #[test]
    fn xrp_sp_preflight_rejects_non_xrp_and_stores_reservation() {
        let mut state = test_state_with_xrp_vault();
        let err = stability_pool_preflight_xrp_absorb_in_state(&mut state, sp(), 1, 100 * E8, 1)
            .unwrap_err();
        assert!(
            format!("{err:?}").contains("native-XRP"),
            "unexpected error: {err:?}"
        );
        assert!(state.sp_xrp_absorb_preflights.is_empty());

        let result =
            stability_pool_preflight_xrp_absorb_in_state(&mut state, sp(), VAULT_ID, 100 * E8, 10)
                .expect("xrp preflight accepted");
        assert_eq!(result.vault_id, VAULT_ID);
        assert_eq!(result.icusd_burn_e8s, 100 * E8);
        assert_eq!(result.collateral_received_drops, 100_000_000);
        assert_eq!(result.collateral_price_e8s, 50_000_000);
        assert!(result.expires_at_ns > 10);

        let stored = state.sp_xrp_absorb_preflights.get(&VAULT_ID).unwrap();
        assert_eq!(stored.caller, sp());
        assert_eq!(stored.vault_id, VAULT_ID);
        assert_eq!(stored.icusd_burn_e8s, 100 * E8);
        assert_eq!(stored.collateral_received_drops, 100_000_000);
        assert_eq!(stored.collateral_price_e8s, 50_000_000);
        assert_eq!(stored.expires_at_ns, result.expires_at_ns);
    }

    #[test]
    fn xrp_sp_preflight_rejects_frozen_and_disabled_before_mutation() {
        let mut frozen = test_state_with_xrp_vault();
        frozen.frozen = true;
        assert!(stability_pool_preflight_xrp_absorb_in_state(
            &mut frozen,
            sp(),
            VAULT_ID,
            100 * E8,
            10,
        )
        .is_err());
        assert!(frozen.sp_xrp_absorb_preflights.is_empty());

        let mut disabled = test_state_with_xrp_vault();
        disabled.sp_writedown_disabled = true;
        assert!(stability_pool_preflight_xrp_absorb_in_state(
            &mut disabled,
            sp(),
            VAULT_ID,
            100 * E8,
            10,
        )
        .is_err());
        assert!(disabled.sp_xrp_absorb_preflights.is_empty());
    }

    #[test]
    fn xrp_sp_preflight_rejects_when_vault_operation_in_flight() {
        let mut state = test_state_with_xrp_vault();
        crate::state::replace_state(crate::state::State::default());
        let guard = crate::guard::VaultLiquidationGuard::new(VAULT_ID).expect("lock vault");
        let err =
            stability_pool_preflight_xrp_absorb_in_state(&mut state, sp(), VAULT_ID, 100 * E8, 10)
                .unwrap_err();

        assert!(
            matches!(err, ProtocolError::TemporarilyUnavailable(_)),
            "unexpected error: {err:?}"
        );
        assert!(state.sp_xrp_absorb_preflights.is_empty());
        drop(guard);
    }

    #[test]
    fn xrp_sp_active_preflight_blocks_vault_mutations_until_expiry() {
        let mut state = test_state_with_xrp_vault();
        let pf =
            stability_pool_preflight_xrp_absorb_in_state(&mut state, sp(), VAULT_ID, 100 * E8, 10)
                .expect("preflight accepted");

        assert!(ensure_no_active_xrp_sp_absorb_preflight(&state, VAULT_ID, 20).is_err());
        assert!(
            ensure_no_active_xrp_sp_absorb_preflight(&state, VAULT_ID, pf.expires_at_ns + 1)
                .is_ok()
        );
    }

    #[test]
    fn xrp_sp_absorb_requires_registered_sp_and_matching_preflight() {
        let mut no_preflight = test_state_with_xrp_vault();
        assert!(stability_pool_liquidate_xrp_vault_in_state(
            &mut no_preflight,
            sp(),
            valid_request(44),
            20,
        )
        .is_err());
        assert_no_xrp_absorb_mutation(&no_preflight);

        let mut wrong_caller = test_state_with_xrp_vault();
        preflight(&mut wrong_caller, 10);
        assert!(stability_pool_liquidate_xrp_vault_in_state(
            &mut wrong_caller,
            principal(0xee),
            valid_request(44),
            20,
        )
        .is_err());
        assert_no_xrp_absorb_mutation(&wrong_caller);

        // NOTE: an expired-but-present reservation is intentionally HONORED on the
        // post-burn submit (see xrp_sp_absorb_submit_honors_expired_preflight_after_burn);
        // only a wholly absent / wrong-caller / mismatched-burn reservation rejects.

        let mut mismatched = test_state_with_xrp_vault();
        preflight(&mut mismatched, 10);
        let mut req = valid_request(44);
        req.icusd_burned_e8s -= 1;
        assert!(
            stability_pool_liquidate_xrp_vault_in_state(&mut mismatched, sp(), req, 20).is_err()
        );
        assert_no_xrp_absorb_mutation(&mismatched);
    }

    #[test]
    fn xrp_sp_absorb_rejects_non_xrp_before_mutation() {
        let mut non_xrp = test_state_with_xrp_vault();
        non_xrp.sp_xrp_absorb_preflights.insert(
            1,
            crate::state::StoredXrpSpAbsorbPreflight {
                caller: sp(),
                vault_id: 1,
                icusd_burn_e8s: 100 * E8,
                total_to_seize_drops: 100_000_000,
                collateral_received_drops: 100_000_000,
                collateral_price_e8s: 1_000_000_000,
                expires_at_ns: 100,
            },
        );
        let mut non_xrp_req = valid_request(44);
        non_xrp_req.vault_id = 1;
        non_xrp_req.proof.vault_id_memo = 1;
        assert!(
            stability_pool_liquidate_xrp_vault_in_state(&mut non_xrp, sp(), non_xrp_req, 20)
                .is_err()
        );
        assert!(non_xrp.xrp_claims.is_empty());
    }

    #[test]
    fn xrp_sp_preflight_rejects_healthy_vault_before_burn() {
        let mut healthy = test_state_with_xrp_vault();
        healthy
            .vault_id_to_vaults
            .get_mut(&VAULT_ID)
            .unwrap()
            .collateral_amount = 1_000_000_000_000;
        assert!(stability_pool_preflight_xrp_absorb_in_state(
            &mut healthy,
            sp(),
            VAULT_ID,
            100 * E8,
            10
        )
        .is_err());
        assert!(healthy.xrp_claims.is_empty());
        assert!(healthy.sp_xrp_absorb_preflights.is_empty());
    }

    #[test]
    fn xrp_sp_absorb_validates_allocations_before_mutation() {
        let invalid_cases: Vec<XrpSpAbsorbRequest> = {
            let mut sum_mismatch = valid_request(44);
            sum_mismatch.allocations[0].drops -= 1;

            let mut empty_address = valid_request(44);
            empty_address.allocations[0].payout_address = "  ".to_string();

            let mut zero_drops = valid_request(44);
            zero_drops.allocations[0].drops = 0;
            zero_drops.allocations[1].drops = 100_000_000;

            let mut too_many = valid_request(44);
            too_many.allocations = (0..=MAX_XRP_SP_PAYOUT_ALLOCATIONS)
                .map(|i| XrpSpPayoutAllocation {
                    claimant: principal((i % 200) as u8),
                    payout_address: format!("r{i}"),
                    destination_tag: None,
                    drops: 1,
                })
                .collect();

            vec![
                XrpSpAbsorbRequest {
                    allocations: vec![],
                    ..valid_request(44)
                },
                sum_mismatch,
                empty_address,
                zero_drops,
                too_many,
            ]
        };

        for request in invalid_cases {
            let mut state = test_state_with_xrp_vault();
            preflight(&mut state, 10);
            assert!(
                stability_pool_liquidate_xrp_vault_in_state(&mut state, sp(), request, 20).is_err()
            );
            assert_no_xrp_absorb_mutation(&state);
        }
    }

    #[test]
    fn xrp_sp_absorb_uses_reserved_preflight_amount_for_write_down() {
        let mut state = test_state_with_xrp_vault();
        state.sp_xrp_absorb_preflights.insert(
            VAULT_ID,
            crate::state::StoredXrpSpAbsorbPreflight {
                caller: sp(),
                vault_id: VAULT_ID,
                icusd_burn_e8s: 100 * E8,
                total_to_seize_drops: 80_000_000,
                collateral_received_drops: 80_000_000,
                collateral_price_e8s: 50_000_000,
                expires_at_ns: 100,
            },
        );
        let mut request = valid_request(44);
        request.allocations[0].drops = 48_000_000;
        request.allocations[1].drops = 32_000_000;

        let result = stability_pool_liquidate_xrp_vault_in_state(&mut state, sp(), request, 20)
            .expect("absorb accepted");

        assert_eq!(result.collateral_received_drops, 80_000_000);
        let vault = state
            .vault_id_to_vaults
            .get(&VAULT_ID)
            .expect("non-drained vault remains for excess collateral");
        assert_eq!(vault.borrowed_icusd_amount, ICUSD::new(0));
        assert_eq!(vault.collateral_amount, 20_000_000);
        assert_eq!(state.xrp_claims.get(&0).unwrap().drops, 48_000_000);
        assert_eq!(state.xrp_claims.get(&1).unwrap().drops, 32_000_000);
    }

    #[test]
    fn xrp_sp_absorb_routes_protocol_cut_to_developer_claim() {
        // total_to_seize (gross) > collateral_received (net) => a 3M-drop protocol cut.
        let mut state = test_state_with_xrp_vault();
        state.sp_xrp_absorb_preflights.insert(
            VAULT_ID,
            crate::state::StoredXrpSpAbsorbPreflight {
                caller: sp(),
                vault_id: VAULT_ID,
                icusd_burn_e8s: 100 * E8,
                total_to_seize_drops: 100_000_000,
                collateral_received_drops: 97_000_000,
                collateral_price_e8s: 50_000_000,
                expires_at_ns: 100,
            },
        );
        let mut request = valid_request(44);
        request.allocations[0].drops = 57_000_000;
        request.allocations[1].drops = 40_000_000; // depositor sum = 97_000_000 (net)

        let result = stability_pool_liquidate_xrp_vault_in_state(&mut state, sp(), request, 20)
            .expect("absorb accepted");

        // Depositor payout claims cover only the NET amount.
        assert_eq!(result.collateral_received_drops, 97_000_000);
        assert_eq!(result.payout_claims.len(), 2);
        let depositor_total: u64 = result.payout_claims.iter().map(|c| c.drops).sum();
        assert_eq!(depositor_total, 97_000_000);

        // The 3M-drop protocol cut is routed to a developer-settleable claim.
        let developer = principal(0xdd);
        let dev_total: u64 = state
            .xrp_claims
            .values()
            .filter(|c| c.claimant == developer)
            .map(|c| c.drops)
            .sum();
        assert_eq!(
            dev_total, 3_000_000,
            "protocol cut must route to a developer claim"
        );

        // Conservation: every drop debited from the vault is covered by a claim.
        let claimed_total: u64 = state.xrp_claims.values().map(|c| c.drops).sum();
        assert_eq!(
            claimed_total, 100_000_000,
            "sum of all XrpClaims must equal the gross collateral seized"
        );
        assert!(state.vault_id_to_vaults.get(&VAULT_ID).is_none());
    }

    #[test]
    fn xrp_sp_absorb_submit_honors_expired_preflight_after_burn() {
        // The SP burns icUSD before submitting. If the backend is unreachable past the
        // preflight TTL, the persisted reservation must still finalize the absorb
        // rather than stranding the burned icUSD forever (review blocker B-2).
        let mut state = test_state_with_xrp_vault();
        let pf =
            stability_pool_preflight_xrp_absorb_in_state(&mut state, sp(), VAULT_ID, 100 * E8, 10)
                .expect("preflight reservation");

        let result = stability_pool_liquidate_xrp_vault_in_state(
            &mut state,
            sp(),
            valid_request(44),
            pf.expires_at_ns + 1,
        )
        .expect("expired-but-present reservation is honored post-burn");

        assert_eq!(result.collateral_received_drops, 100_000_000);
        assert_eq!(state.xrp_claims.get(&0).unwrap().drops, 60_000_000);
        assert_eq!(state.xrp_claims.get(&1).unwrap().drops, 40_000_000);
        assert!(state.sp_xrp_absorb_preflights.get(&VAULT_ID).is_none());
    }

    #[test]
    fn xrp_sp_absorb_rejects_when_vault_collateral_below_reserved_seizure() {
        // If the vault's collateral fell below the reserved gross seizure since the
        // preflight (now possible because an expired reservation is honored), the
        // submit must abort BEFORE minting any claim, to preserve conservation.
        let mut state = test_state_with_xrp_vault();
        state.sp_xrp_absorb_preflights.insert(
            VAULT_ID,
            crate::state::StoredXrpSpAbsorbPreflight {
                caller: sp(),
                vault_id: VAULT_ID,
                icusd_burn_e8s: 100 * E8,
                total_to_seize_drops: 100_000_000,
                collateral_received_drops: 100_000_000,
                collateral_price_e8s: 50_000_000,
                expires_at_ns: 100,
            },
        );
        // Vault shrank below the reserved seizure since the preflight.
        state
            .vault_id_to_vaults
            .get_mut(&VAULT_ID)
            .unwrap()
            .collateral_amount = 60_000_000;

        assert!(stability_pool_liquidate_xrp_vault_in_state(
            &mut state,
            sp(),
            valid_request(44),
            20
        )
        .is_err());
        assert!(state.xrp_claims.is_empty());
        assert_eq!(state.next_xrp_claim_id, 0);
        assert!(state.sp_xrp_absorb_results_by_proof.is_empty());
        let vault = state.vault_id_to_vaults.get(&VAULT_ID).unwrap();
        assert_eq!(vault.collateral_amount, 60_000_000);
        assert_eq!(vault.borrowed_icusd_amount, ICUSD::new(100 * E8));
    }

    #[test]
    fn xrp_sp_absorb_rejects_when_vault_debt_below_reserved_burn() {
        // Now that an expired reservation is honored on submit, a manual partial
        // liquidation can reduce the vault's debt below the reserved burn during the
        // window. The submit must abort BEFORE mutation rather than over-burning icUSD
        // (the silent `.min(borrowed)` would otherwise clear less debt than was burned).
        let mut state = test_state_with_xrp_vault();
        state.sp_xrp_absorb_preflights.insert(
            VAULT_ID,
            crate::state::StoredXrpSpAbsorbPreflight {
                caller: sp(),
                vault_id: VAULT_ID,
                icusd_burn_e8s: 100 * E8,
                total_to_seize_drops: 100_000_000,
                collateral_received_drops: 100_000_000,
                collateral_price_e8s: 50_000_000,
                expires_at_ns: 100,
            },
        );
        // Debt shrank below the reserved burn (collateral still ample for the seizure).
        state
            .vault_id_to_vaults
            .get_mut(&VAULT_ID)
            .unwrap()
            .borrowed_icusd_amount = ICUSD::new(50 * E8);

        assert!(stability_pool_liquidate_xrp_vault_in_state(
            &mut state,
            sp(),
            valid_request(44),
            20
        )
        .is_err());
        assert!(state.xrp_claims.is_empty());
        assert_eq!(state.next_xrp_claim_id, 0);
        assert!(state.sp_xrp_absorb_results_by_proof.is_empty());
        let vault = state.vault_id_to_vaults.get(&VAULT_ID).unwrap();
        assert_eq!(vault.borrowed_icusd_amount, ICUSD::new(50 * E8));
        assert_eq!(vault.collateral_amount, 100_000_000);
    }

    #[test]
    fn xrp_sp_absorb_partial_burn_below_debt_is_allowed() {
        // A pool-limited PARTIAL absorb (burn < live debt) must still succeed: the
        // debt guard is directional (abort only when debt fell BELOW the burn), so it
        // does not reject legitimate partial liquidations.
        let mut state = test_state_with_xrp_vault();
        state
            .vault_id_to_vaults
            .get_mut(&VAULT_ID)
            .unwrap()
            .collateral_amount = 1_000_000_000_000;
        state.sp_xrp_absorb_preflights.insert(
            VAULT_ID,
            crate::state::StoredXrpSpAbsorbPreflight {
                caller: sp(),
                vault_id: VAULT_ID,
                icusd_burn_e8s: 40 * E8, // burning less than the 100-icUSD debt
                total_to_seize_drops: 80_000_000,
                collateral_received_drops: 80_000_000,
                collateral_price_e8s: 50_000_000,
                expires_at_ns: 100,
            },
        );
        let mut request = valid_request(44);
        request.icusd_burned_e8s = 40 * E8;
        request.allocations[0].drops = 50_000_000;
        request.allocations[1].drops = 30_000_000; // sum = 80_000_000

        let result = stability_pool_liquidate_xrp_vault_in_state(&mut state, sp(), request, 20)
            .expect("partial absorb (burn < debt) accepted");
        assert_eq!(result.liquidated_debt_e8s, 40 * E8);
        let vault = state.vault_id_to_vaults.get(&VAULT_ID).unwrap();
        assert_eq!(vault.borrowed_icusd_amount, ICUSD::new(60 * E8)); // 100 - 40
    }

    #[test]
    fn xrp_sp_absorb_replay_does_not_remint_developer_claim() {
        // Replaying an accepted absorb that minted a developer protocol-cut claim must
        // return the cached result and re-mint nothing (idempotency on the dev claim).
        let mut state = test_state_with_xrp_vault();
        state.sp_xrp_absorb_preflights.insert(
            VAULT_ID,
            crate::state::StoredXrpSpAbsorbPreflight {
                caller: sp(),
                vault_id: VAULT_ID,
                icusd_burn_e8s: 100 * E8,
                total_to_seize_drops: 100_000_000,
                collateral_received_drops: 97_000_000,
                collateral_price_e8s: 50_000_000,
                expires_at_ns: 100,
            },
        );
        let mut req1 = valid_request(44);
        req1.allocations[0].drops = 57_000_000;
        req1.allocations[1].drops = 40_000_000;
        let mut req2 = valid_request(44);
        req2.allocations[0].drops = 57_000_000;
        req2.allocations[1].drops = 40_000_000;

        let first = stability_pool_liquidate_xrp_vault_in_state(&mut state, sp(), req1, 20)
            .expect("absorb accepted");
        // 2 depositor claims + 1 developer claim.
        assert_eq!(state.xrp_claims.len(), 3);
        assert_eq!(state.next_xrp_claim_id, 3);
        let claims_after_first = state.xrp_claims.clone();

        let replay = stability_pool_liquidate_xrp_vault_in_state(&mut state, sp(), req2, 999)
            .expect("exact replay returns cached result");
        assert_eq!(replay, first);
        assert_eq!(state.xrp_claims, claims_after_first);
        assert_eq!(
            state.xrp_claims.len(),
            3,
            "replay must not re-mint the dev claim"
        );
        assert_eq!(state.next_xrp_claim_id, 3);
    }

    #[test]
    fn xrp_sp_absorb_submit_honors_reserved_preflight_after_vault_recovers() {
        let mut state = test_state_with_xrp_vault();
        preflight(&mut state, 10);
        state
            .vault_id_to_vaults
            .get_mut(&VAULT_ID)
            .unwrap()
            .collateral_amount = 1_000_000_000_000;

        let result =
            stability_pool_liquidate_xrp_vault_in_state(&mut state, sp(), valid_request(44), 20)
                .expect("post-burn submit consumes reservation");

        assert_eq!(result.collateral_received_drops, 100_000_000);
        let vault = state
            .vault_id_to_vaults
            .get(&VAULT_ID)
            .expect("recovered vault remains with excess collateral");
        assert_eq!(vault.borrowed_icusd_amount, ICUSD::new(0));
        assert_eq!(vault.collateral_amount, 999_900_000_000);
        assert_eq!(state.xrp_claims.get(&0).unwrap().drops, 60_000_000);
        assert_eq!(state.xrp_claims.get(&1).unwrap().drops, 40_000_000);
        assert!(state.sp_xrp_absorb_preflights.get(&VAULT_ID).is_none());
    }

    #[test]
    fn xrp_sp_absorb_writes_down_claims_and_exact_replay_returns_same_claim_ids() {
        let mut state = test_state_with_xrp_vault();
        preflight(&mut state, 10);

        let result =
            stability_pool_liquidate_xrp_vault_in_state(&mut state, sp(), valid_request(44), 20)
                .expect("absorb accepted");
        assert!(result.success);
        assert_eq!(result.vault_id, VAULT_ID);
        assert_eq!(result.liquidated_debt_e8s, 100 * E8);
        assert_eq!(result.collateral_received_drops, 100_000_000);
        assert_eq!(result.payout_claims.len(), 2);
        assert_eq!(result.payout_claims[0].claimant, depositor_a());
        assert_eq!(result.payout_claims[0].claim_id, 0);
        assert_eq!(result.payout_claims[1].claimant, depositor_b());
        assert_eq!(result.payout_claims[1].claim_id, 1);
        assert_eq!(state.next_xrp_claim_id, 2);
        assert!(state.vault_id_to_vaults.get(&VAULT_ID).is_none());
        assert!(state.sp_xrp_absorb_preflights.get(&VAULT_ID).is_none());
        assert!(state
            .sp_xrp_absorb_results_by_proof
            .contains_key(&(SpProofLedger::IcusdBurn, 44,)));
        assert_eq!(state.xrp_claims.get(&0).unwrap().claimant, depositor_a());
        assert_eq!(state.xrp_claims.get(&1).unwrap().claimant, depositor_b());

        let replay =
            stability_pool_liquidate_xrp_vault_in_state(&mut state, sp(), valid_request(44), 999)
                .expect("exact replay returns cached result");
        assert_eq!(replay, result);
        assert_eq!(state.next_xrp_claim_id, 2);
        assert_eq!(state.xrp_claims.len(), 2);
    }

    #[test]
    fn xrp_sp_absorb_conflicting_replay_rejects_without_mutation() {
        let mut state = test_state_with_xrp_vault();
        preflight(&mut state, 10);
        let result =
            stability_pool_liquidate_xrp_vault_in_state(&mut state, sp(), valid_request(44), 20)
                .unwrap();
        let claims_before = state.xrp_claims.clone();
        let next_before = state.next_xrp_claim_id;
        let results_before = state.sp_xrp_absorb_results_by_proof.clone();

        let mut conflicting = valid_request(44);
        conflicting.allocations[0].payout_address = "rDifferent".to_string();
        assert!(
            stability_pool_liquidate_xrp_vault_in_state(&mut state, sp(), conflicting, 30).is_err()
        );
        assert_eq!(state.xrp_claims, claims_before);
        assert_eq!(state.next_xrp_claim_id, next_before);
        assert_eq!(state.sp_xrp_absorb_results_by_proof, results_before);
        assert_eq!(
            state
                .sp_xrp_absorb_results_by_proof
                .get(&(SpProofLedger::IcusdBurn, 44))
                .unwrap()
                .result,
            result,
        );
    }

    fn stored_refund_for(
        block_index: u64,
        caller: Principal,
        amount_e8s: u64,
    ) -> StoredSpBurnRefund {
        StoredSpBurnRefund {
            caller,
            vault_id: VAULT_ID,
            amount_e8s,
            ledger: principal(0x10),
            burn_block_index: block_index,
            op_nonce: 1,
            refund_created_at_time: 1,
            refund_memo: vec![],
            refund_block_index: None,
            attempt_history: Vec::new(),
            history_scan: None,
            no_effect_evidence: None,
            attempt_no_effect_evidence: Vec::new(),
        }
    }

    #[test]
    fn xrp_absorb_status_requires_matching_terminal_refund_record() {
        let mut state = test_state_with_xrp_vault();
        let request = valid_request(71);
        assert_eq!(
            xrp_sp_absorb_status_in_state(&state, sp(), &request).unwrap(),
            crate::XrpSpAbsorbStatus::Unseen
        );
        let key = (SpProofLedger::IcusdBurn, 71);
        state.consumed_writedown_proofs.insert(key);
        assert_eq!(
            xrp_sp_absorb_status_in_state(&state, sp(), &request).unwrap(),
            crate::XrpSpAbsorbStatus::ConsumedWithoutResult
        );
        state
            .sp_burn_refunds_by_proof
            .insert(key, stored_refund_for(71, sp(), request.icusd_burned_e8s));
        assert_eq!(
            xrp_sp_absorb_status_in_state(&state, sp(), &request).unwrap(),
            crate::XrpSpAbsorbStatus::RefundJournaled
        );

        let mut mismatch = stored_refund_for(71, sp(), request.icusd_burned_e8s);
        mismatch.vault_id += 1;
        state.sp_burn_refunds_by_proof.insert(key, mismatch);
        assert_eq!(
            xrp_sp_absorb_status_in_state(&state, sp(), &request).unwrap(),
            crate::XrpSpAbsorbStatus::ConsumedWithoutResult
        );
        assert!(xrp_sp_absorb_status_in_state(&state, depositor_a(), &request).is_err());
    }

    #[test]
    fn xrp_absorb_status_accepts_only_exact_consumed_cached_request() {
        let mut state = test_state_with_xrp_vault();
        let request = valid_request(72);
        let key = (SpProofLedger::IcusdBurn, 72);
        let allocations = canonical_xrp_allocations(&request.allocations);
        let fingerprint = xrp_sp_allocation_fingerprint(sp(), &request, &allocations);
        let result = crate::XrpSpAbsorbResult {
            success: true,
            vault_id: VAULT_ID,
            liquidated_debt_e8s: request.icusd_burned_e8s,
            collateral_received_drops: 100_000_000,
            payout_claims: vec![],
            block_index: 72,
            collateral_price_e8s: 50_000_000,
        };
        state.consumed_writedown_proofs.insert(key);
        state.sp_xrp_absorb_results_by_proof.insert(
            key,
            StoredXrpSpAbsorbResult {
                caller: sp(),
                vault_id: VAULT_ID,
                icusd_burned_e8s: request.icusd_burned_e8s,
                proof_ledger: SpProofLedger::IcusdBurn,
                proof_block_index: 72,
                allocation_fingerprint: fingerprint,
                result: result.clone(),
                accepted_at_ns: 10,
            },
        );
        assert_eq!(
            xrp_sp_absorb_status_in_state(&state, sp(), &request).unwrap(),
            crate::XrpSpAbsorbStatus::Accepted(result)
        );

        state
            .sp_burn_refunds_by_proof
            .insert(key, stored_refund_for(72, sp(), request.icusd_burned_e8s));
        assert_eq!(
            xrp_sp_absorb_status_in_state(&state, sp(), &request).unwrap(),
            crate::XrpSpAbsorbStatus::ConsumedWithoutResult
        );
        state.sp_burn_refunds_by_proof.remove(&key);
        let mut conflicting = request;
        conflicting.allocations[0].payout_address = "other-address".to_string();
        assert_eq!(
            xrp_sp_absorb_status_in_state(&state, sp(), &conflicting).unwrap(),
            crate::XrpSpAbsorbStatus::ConsumedWithoutResult
        );
    }

    fn stored_result_for(block_index: u64) -> StoredXrpSpAbsorbResult {
        StoredXrpSpAbsorbResult {
            caller: sp(),
            vault_id: block_index,
            icusd_burned_e8s: 100 * E8,
            proof_ledger: SpProofLedger::IcusdBurn,
            proof_block_index: block_index,
            allocation_fingerprint: vec![block_index as u8; 32],
            result: crate::XrpSpAbsorbResult {
                success: true,
                vault_id: block_index,
                liquidated_debt_e8s: 100 * E8,
                collateral_received_drops: 100_000_000,
                payout_claims: vec![],
                block_index,
                collateral_price_e8s: 50_000_000,
            },
            accepted_at_ns: block_index,
        }
    }

    #[test]
    fn xrp_sp_absorb_result_cache_keeps_just_accepted_proof() {
        let mut state = State::default();
        for block_index in 1..=(MAX_SP_XRP_ABSORB_RESULTS_BY_PROOF as u64) {
            record_sp_xrp_absorb_result_bounded(
                &mut state,
                (SpProofLedger::IcusdBurn, block_index),
                stored_result_for(block_index),
            );
        }

        record_sp_xrp_absorb_result_bounded(
            &mut state,
            (SpProofLedger::IcusdBurn, 0),
            stored_result_for(0),
        );

        assert_eq!(
            state.sp_xrp_absorb_results_by_proof.len(),
            MAX_SP_XRP_ABSORB_RESULTS_BY_PROOF,
        );
        assert!(
            state
                .sp_xrp_absorb_results_by_proof
                .contains_key(&(SpProofLedger::IcusdBurn, 0)),
            "the just-accepted proof must remain replayable",
        );
        assert!(!state
            .sp_xrp_absorb_results_by_proof
            .contains_key(&(SpProofLedger::IcusdBurn, 1)));
    }
}

#[cfg(test)]
mod redemption_ranking_completeness_tests {
    use super::{
        max_input_matching, redemption_price_timestamp_is_fresh, redemption_ranking_is_complete,
        simulated_collateral_total_raw, REDEMPTION_PRICE_MAX_AGE_NS,
    };
    use candid::Principal;

    #[test]
    fn missing_competitor_price_cannot_look_like_a_complete_rank() {
        let icp = Principal::from_slice(&[1]);
        let xaut = Principal::from_slice(&[2]);

        assert!(!redemption_ranking_is_complete(
            &[icp, xaut],
            &[icp],
            false,
            64,
        ));
        assert!(redemption_ranking_is_complete(
            &[icp, xaut],
            &[icp, xaut],
            true,
            64,
        ));
    }

    #[test]
    fn redemption_candidate_limit_accepts_64_and_rejects_65() {
        let candidates: Vec<_> = (0..65).map(|i| Principal::from_slice(&[i])).collect();
        let first_64: Vec<_> = candidates.iter().take(64).copied().collect();
        assert!(redemption_ranking_is_complete(
            &first_64, &first_64, true, 64,
        ));
        assert!(!redemption_ranking_is_complete(
            &candidates,
            &candidates,
            true,
            64,
        ));
    }

    #[test]
    fn redemption_capacity_search_covers_low_rmr_and_checks_next_unit() {
        // 50% redemption fee and 1% RMR yield 0.5% effective debt reduction.
        // A 100-unit debt therefore supports 20,000 input units, far beyond
        // the old arbitrary 100x search ceiling.
        let capacity = max_input_matching(u64::MAX, |input| {
            input / 200 + u64::from(input % 200 != 0) <= 100
        });
        assert_eq!(capacity, 20_000);
        assert!(capacity / 200 <= 100);
        assert!((capacity + 1) / 200 + u64::from((capacity + 1) % 200 != 0) > 100);
    }

    #[test]
    fn redemption_payout_rejects_native_sum_above_u64() {
        let large = 10_000_000_000_000_000_000u64;
        let simulated = vec![
            crate::event::VaultRedemption {
                vault_id: 1,
                icusd_redeemed_e8s: 1,
                collateral_seized: large,
            },
            crate::event::VaultRedemption {
                vault_id: 2,
                icusd_redeemed_e8s: 1,
                collateral_seized: large,
            },
        ];
        assert!(simulated_collateral_total_raw(&simulated).is_none());

        let representable = vec![crate::event::VaultRedemption {
            vault_id: 1,
            icusd_redeemed_e8s: 1,
            collateral_seized: u64::MAX,
        }];
        assert_eq!(
            simulated_collateral_total_raw(&representable),
            Some(u64::MAX)
        );
    }

    #[test]
    fn redemption_price_freshness_rejects_future_timestamps() {
        let now = 1_000_000_000_000;
        assert!(redemption_price_timestamp_is_fresh(
            now - REDEMPTION_PRICE_MAX_AGE_NS,
            now
        ));
        assert!(!redemption_price_timestamp_is_fresh(now + 1, now));
        assert!(!redemption_price_timestamp_is_fresh(
            now - REDEMPTION_PRICE_MAX_AGE_NS - 1,
            now
        ));
    }
}

#[cfg(test)]
mod redemption_await_boundary_tests {
    use super::{
        build_redemption_queue_and_quote, cached_redemption_offer_is_fresh, checked_margin_balance,
        current_fresh_legacy_reserve_run_for_snapshot, current_legacy_reserve_run_for_snapshot,
        legacy_redemption_run_for_request, legacy_reserve_spillover_run,
        persist_rejected_redemption_refund, prepared_offer_from_current_state,
        redemption_offer_price_refresh_snapshot, redemption_ranking_is_fresh,
        redemption_raw_refund, redemption_run_snapshot_error, redemption_tail_raw_refund,
        refresh_stale_redemption_candidates_for_offer_with, reserve_post_settlement_raw_refund,
        reserve_spillover_raw_refund_budget, reserve_spillover_snapshot_mismatch_refund,
        stale_redemption_candidate_types, validate_borrow_mint_receipt, BorrowMintDispatchGuard,
        RedemptionOfferRefreshGateState, RedemptionRunSnapshot, Vault,
        REDEMPTION_OFFER_REFRESH_COOLDOWN_NS, REDEMPTION_OFFER_REFRESH_LEASE_NS,
        REDEMPTION_PRICE_MAX_AGE_NS,
    };
    use crate::management;
    use crate::numeric::{Ratio, ICUSD};
    use crate::state::State;
    use candid::Principal;
    use rust_decimal::prelude::ToPrimitive;
    use rust_decimal::Decimal;
    use rust_decimal_macros::dec;

    const E8: u64 = 100_000_000;

    fn principal(byte: u8) -> Principal {
        Principal::from_slice(&[byte; 29])
    }

    fn redemption_state(xaut_collateral_raw: u64) -> (State, Principal, Principal) {
        let icp = principal(0x11);
        let xaut = principal(0x22);
        let mut state = State::from(crate::InitArg {
            xrc_principal: Principal::anonymous(),
            icusd_ledger_principal: principal(0x33),
            icp_ledger_principal: icp,
            fee_e8s: 0,
            developer_principal: principal(0x44),
            treasury_principal: None,
            stability_pool_principal: None,
            ckusdt_ledger_principal: None,
            ckusdc_ledger_principal: None,
        });
        state.min_icusd_amount = ICUSD::new(0);

        let mut icp_config = state.collateral_configs.get(&icp).unwrap().clone();
        icp_config.last_price = Some(1.0);
        icp_config.last_price_timestamp = Some(1);
        icp_config.borrow_threshold_ratio = Ratio::from(dec!(1.50));
        icp_config.liquidation_ratio = Ratio::from(dec!(1.33));
        icp_config.symbol = Some("ICP".to_string());
        state.collateral_configs.insert(icp, icp_config.clone());

        let mut xaut_config = icp_config;
        xaut_config.ledger_canister_id = xaut;
        xaut_config.borrow_threshold_ratio = Ratio::from(dec!(1.18));
        xaut_config.liquidation_ratio = Ratio::from(dec!(1.12));
        xaut_config.symbol = Some("ckXAUT".to_string());
        state.collateral_configs.insert(xaut, xaut_config);

        state.open_vault(Vault {
            owner: principal(0x55),
            vault_id: 1,
            borrowed_icusd_amount: ICUSD::new(E8),
            collateral_amount: 150_000_000,
            collateral_type: icp,
            last_accrual_time: 0,
            accrued_interest: ICUSD::new(0),
            bot_processing: false,
        });
        state.open_vault(Vault {
            owner: principal(0x66),
            vault_id: 2,
            borrowed_icusd_amount: ICUSD::new(E8),
            collateral_amount: xaut_collateral_raw,
            collateral_type: xaut,
            last_accrual_time: 0,
            accrued_interest: ICUSD::new(0),
            bot_processing: false,
        });
        (state, icp, xaut)
    }

    fn stale_lst_only_redemption_state() -> (State, Principal, Principal) {
        let (mut state, icp, lst) = redemption_state(120_000_000);
        state
            .vault_id_to_vaults
            .get_mut(&1)
            .unwrap()
            .borrowed_icusd_amount = ICUSD::new(0);
        state.set_icp_rate(crate::numeric::UsdIcp::from(dec!(1)), Some(1));
        let mut config = state.collateral_configs.get(&lst).unwrap().clone();
        config.price_source = crate::state::PriceSource::LstWrapped {
            base_asset: "ICP".to_string(),
            base_asset_class: crate::state::XrcAssetClass::Cryptocurrency,
            quote_asset: "USD".to_string(),
            quote_asset_class: crate::state::XrcAssetClass::FiatCurrency,
            rate_canister_id: principal(0x77),
            rate_method: "get_info".to_string(),
            haircut: 0.0,
        };
        config.last_price_timestamp = Some(1);
        state.collateral_configs.insert(lst, config);
        (state, icp, lst)
    }

    fn vault_balances(state: &State) -> Vec<(u64, u64, u64)> {
        state
            .vault_id_to_vaults
            .values()
            .map(|vault| {
                (
                    vault.vault_id,
                    vault.borrowed_icusd_amount.to_u64(),
                    vault.collateral_amount,
                )
            })
            .collect()
    }

    #[test]
    fn legacy_icp_request_rejects_non_icp_first_run_before_any_accounting_change() {
        let (state, icp, xaut) = redemption_state(120_000_000);
        let first = state.redemption_runs().into_iter().next().unwrap();
        assert_eq!(first.collateral_type, xaut);
        let before_balances = vault_balances(&state);
        let before_pending_refunds = state.pending_refunds.len();
        let before_pending_payouts = state.pending_redemption_transfer.len();
        let before_base_rate = state.collateral_configs[&xaut].current_base_rate;

        let error = legacy_redemption_run_for_request(&state, icp).unwrap_err();
        let crate::ProtocolError::GenericError(message) = error else {
            panic!("expected a pre-pull requested-asset rejection");
        };
        assert!(message.contains("ckXAUT") && message.contains("ICP"));
        assert!(message.contains("review a new quote"));
        assert_eq!(vault_balances(&state), before_balances);
        assert_eq!(state.pending_refunds.len(), before_pending_refunds);
        assert_eq!(
            state.pending_redemption_transfer.len(),
            before_pending_payouts
        );
        assert_eq!(
            state.collateral_configs[&xaut].current_base_rate,
            before_base_rate
        );

        assert_eq!(
            legacy_redemption_run_for_request(&state, xaut)
                .unwrap()
                .collateral_type,
            xaut,
            "matching the global first asset remains allowed"
        );
    }

    #[test]
    fn direct_post_pull_guard_rejects_same_asset_price_and_order_changes() {
        let (mut state, icp, xaut) = redemption_state(0);
        state
            .vault_id_to_vaults
            .get_mut(&2)
            .unwrap()
            .borrowed_icusd_amount = ICUSD::new(0);
        let first = state.redemption_runs().into_iter().next().unwrap();
        assert_eq!(first.collateral_type, icp);
        let expected = RedemptionRunSnapshot::capture(&first);
        let before = vault_balances(&state);

        state.collateral_configs.get_mut(&icp).unwrap().last_price = Some(1.01);
        let changed_price = state.redemption_runs().into_iter().next().unwrap();
        assert_eq!(changed_price.collateral_type, icp);
        assert!(redemption_run_snapshot_error(&expected, &changed_price).is_some());
        assert_eq!(vault_balances(&state), before);

        // Restore the price, add a second same-asset vault, and change its
        // health so the ordered IDs reverse while the selected token stays ICP.
        state.collateral_configs.get_mut(&icp).unwrap().last_price = Some(1.0);
        state.open_vault(Vault {
            owner: principal(0x77),
            vault_id: 3,
            borrowed_icusd_amount: ICUSD::new(E8),
            collateral_amount: 148_000_000,
            collateral_type: icp,
            last_accrual_time: 0,
            accrued_interest: ICUSD::new(0),
            bot_processing: false,
        });
        let before_order = state.redemption_runs().into_iter().next().unwrap();
        let order_snapshot = RedemptionRunSnapshot::capture(&before_order);
        state
            .vault_id_to_vaults
            .get_mut(&1)
            .unwrap()
            .collateral_amount = 147_000_000;
        state
            .vault_id_to_vaults
            .get_mut(&3)
            .unwrap()
            .collateral_amount = 151_000_000;
        let reordered = state.redemption_runs().into_iter().next().unwrap();
        assert_eq!(reordered.collateral_type, icp);
        assert_ne!(reordered.vault_ids, before_order.vault_ids);
        assert!(redemption_run_snapshot_error(&order_snapshot, &reordered).is_some());
    }

    #[test]
    fn reserve_spillover_is_icp_only_but_stable_only_remains_available() {
        let (state, icp, xaut) = redemption_state(120_000_000);
        assert_eq!(state.redemption_runs()[0].collateral_type, xaut);
        assert_eq!(legacy_reserve_spillover_run(&state, 0).unwrap(), None);
        assert!(legacy_reserve_spillover_run(&state, 1).is_err());

        let (mut state, icp, xaut) = redemption_state(145_000_000);
        let icp_run = legacy_reserve_spillover_run(&state, 1).unwrap().unwrap();
        assert_eq!(icp_run.collateral_type, icp);
        let snapshot = RedemptionRunSnapshot::capture(&icp_run);
        let before = vault_balances(&state);

        // Simulate a post-stable-settlement oracle update that moves ICP behind
        // ckXAUT. The synchronous final gate must decline the native event and
        // return the entire unconsumed spillover tail for refund.
        state.collateral_configs.get_mut(&icp).unwrap().last_price = Some(2.0);
        let new_first = state.redemption_runs().into_iter().next().unwrap();
        assert_eq!(new_first.collateral_type, xaut);
        assert!(current_legacy_reserve_run_for_snapshot(&state, &snapshot).is_none());
        let rmr = Ratio::from(dec!(0.9));
        assert_eq!(
            reserve_spillover_snapshot_mismatch_refund(700, rmr, 777),
            777
        );
        assert_eq!(vault_balances(&state), before);
        assert!(state.pending_redemption_transfer.is_empty());
    }

    #[test]
    fn reserve_final_gate_rejects_unchanged_but_stale_global_price_snapshot() {
        let (mut state, icp, xaut) = redemption_state(145_000_000);
        let first = state.redemption_runs().into_iter().next().unwrap();
        assert_eq!(first.collateral_type, icp);
        let snapshot = RedemptionRunSnapshot::capture(&first);
        let boundary_now = 1 + REDEMPTION_PRICE_MAX_AGE_NS;
        assert!(redemption_ranking_is_fresh(
            &state,
            &state.redemption_runs(),
            boundary_now
        ));
        assert!(
            current_fresh_legacy_reserve_run_for_snapshot(&state, &snapshot, boundary_now)
                .is_some()
        );

        // Both prices and the selected IDs remain identical, but the stable
        // transfer crossed the ten-minute cache-age limit before seizure.
        let stale_now = boundary_now + 1;
        let unchanged_first = state.redemption_runs().into_iter().next().unwrap();
        assert!(snapshot.matches(&unchanged_first));
        assert!(current_legacy_reserve_run_for_snapshot(&state, &snapshot).is_some());
        assert!(!redemption_ranking_is_fresh(
            &state,
            &state.redemption_runs(),
            stale_now
        ));
        assert!(
            current_fresh_legacy_reserve_run_for_snapshot(&state, &snapshot, stale_now).is_none()
        );

        // A future competitor timestamp also makes the complete ordering
        // unavailable, even though the selected ICP quote itself is unchanged.
        state
            .collateral_configs
            .get_mut(&xaut)
            .unwrap()
            .last_price_timestamp = Some(stale_now + 1);
        assert!(!redemption_ranking_is_fresh(
            &state,
            &state.redemption_runs(),
            stale_now
        ));
    }

    #[test]
    fn reserve_partial_spillover_refund_conserves_raw_input_through_rmr_and_fees() {
        let (mut state, icp, _xaut) = redemption_state(0);
        state
            .vault_id_to_vaults
            .get_mut(&2)
            .unwrap()
            .borrowed_icusd_amount = ICUSD::new(0);
        state
            .vault_id_to_vaults
            .get_mut(&1)
            .unwrap()
            .borrowed_icusd_amount = ICUSD::new(40_095_000);
        state.rmr_floor = Ratio::from(dec!(0.9));
        state.total_collateral_ratio = state.rmr_floor_cr;
        state.reserve_redemption_fee = Ratio::from(dec!(0.1));
        let mut config = state.collateral_configs.get(&icp).unwrap().clone();
        config.redemption_fee_floor = Ratio::from(dec!(0.01));
        config.redemption_fee_ceiling = Ratio::from(dec!(0.01));
        state.collateral_configs.insert(icp, config);

        let input_e8s = 100_000_000u64;
        let rmr = state.get_redemption_margin_ratio();
        assert_eq!(rmr, Ratio::from(dec!(0.9)));
        let reserve_fee = ICUSD::from(input_e8s) * state.reserve_redemption_fee;
        assert_eq!(reserve_fee.to_u64(), 10_000_000);
        let net_after_reserve_fee = ICUSD::from(input_e8s) - reserve_fee;
        let total_effective = net_after_reserve_fee * rmr;
        let available_for_user = 405_000;
        let stable_effective_e8s = available_for_user * 100;
        let spillover_e8s = total_effective.to_u64() - stable_effective_e8s;
        assert_eq!(spillover_e8s, 40_500_000);
        let raw_budget = reserve_spillover_raw_refund_budget(
            net_after_reserve_fee.to_u64(),
            available_for_user,
            rmr,
        );
        assert_eq!(raw_budget, 45_000_000);

        // The fixture config pins both fee bounds to 1%, avoiding the dynamic
        // fee helper's canister-clock read in this host-side unit test.
        let base_fee = Ratio::from(dec!(0.01));
        assert_eq!(base_fee, Ratio::from(dec!(0.01)));
        let vault_fee = ICUSD::from(spillover_e8s) * base_fee;
        assert_eq!(vault_fee.to_u64(), 405_000);
        let effective_spillover = ICUSD::from(spillover_e8s) - vault_fee;
        let run = state.redemption_runs().into_iter().next().unwrap();
        assert_eq!(run.collateral_type, icp);
        let mut persisted_events = Vec::new();
        let outcome = crate::event::record_redemption_on_vault_run_at(
            &mut state,
            principal(0x99),
            effective_spillover,
            vault_fee,
            crate::numeric::UsdIcp::from(dec!(1)),
            700,
            icp,
            &run.vault_ids,
            None,
            1_000_000_000_000,
            &mut persisted_events,
        )
        .unwrap();
        assert_eq!(outcome.consumed.to_u64(), 40_095_000);
        assert_eq!(outcome.margin.to_u64(), 40_095_000);
        assert_eq!(
            state.vault_id_to_vaults[&1].borrowed_icusd_amount.to_u64(),
            0,
            "the selected-run execution really retires the fixture vault debt"
        );
        let consumed = outcome.consumed;
        let [crate::event::Event::RedemptionOnVaults {
            vault_redemptions: Some(redemptions),
            payout_collateral_raw: Some(payout_raw),
            ..
        }] = persisted_events.as_slice()
        else {
            panic!("the checked recorder should persist one pinned redemption event");
        };
        assert_eq!(*payout_raw, 40_095_000);
        assert_eq!(
            redemptions
                .iter()
                .map(|redemption| redemption.icusd_redeemed_e8s)
                .sum::<u64>(),
            consumed.to_u64()
        );
        let pending = state.pending_redemption_transfer.get(&700).unwrap();
        assert_eq!(pending.margin.to_u64(), *payout_raw);
        assert_eq!(pending.collateral_type, icp);

        let refund_raw = reserve_post_settlement_raw_refund(
            net_after_reserve_fee.to_u64(),
            available_for_user,
            vault_fee.to_u64(),
            consumed.to_u64(),
            rmr,
        );
        assert_eq!(refund_raw, 0);
        // Independent oracle: stable payout plus committed native fee/debt is
        // 81,000,000 effective e8s; / .9 consumes the full 90,000,000 raw
        // post-reserve-fee budget exactly once.
        let raw_native_leg = (rust_decimal::Decimal::from(vault_fee.to_u64() + consumed.to_u64())
            / rmr.0)
            .to_u64()
            .unwrap();
        assert_eq!(raw_native_leg, 45_000_000);
        let stable_raw_leg = (Decimal::from(available_for_user * 100) / rmr.0)
            .ceil()
            .to_u64()
            .unwrap();
        assert_eq!(stable_raw_leg, 45_000_000);
        assert_eq!(
            reserve_fee.to_u64() + refund_raw + raw_native_leg + stable_raw_leg,
            input_e8s,
            "reserve fee, settled stable leg, successful native fee/debt reduction, and raw refund conserve pulled icUSD"
        );

        // The cap also excludes the raw equivalent of the stable payout.
        let stable_raw_budget = reserve_spillover_raw_refund_budget(90_000_000, 405_000, rmr);
        assert_eq!(stable_raw_budget, 45_000_000);
        assert_eq!(
            redemption_tail_raw_refund(Decimal::from(40_500_000), rmr, stable_raw_budget),
            45_000_000
        );
    }

    #[test]
    fn reserve_partial_native_consumption_refunds_actual_recorder_tail() {
        let (mut state, icp, _xaut) = redemption_state(0);
        state
            .vault_id_to_vaults
            .get_mut(&2)
            .unwrap()
            .borrowed_icusd_amount = ICUSD::new(0);
        state
            .vault_id_to_vaults
            .get_mut(&1)
            .unwrap()
            .borrowed_icusd_amount = ICUSD::new(40_095_000);
        state.rmr_floor = Ratio::from(dec!(0.9));
        state.total_collateral_ratio = state.rmr_floor_cr;
        state.reserve_redemption_fee = Ratio::from(dec!(0.1));
        let mut config = state.collateral_configs.get(&icp).unwrap().clone();
        config.redemption_fee_floor = Ratio::from(dec!(0.01));
        config.redemption_fee_ceiling = Ratio::from(dec!(0.01));
        state.collateral_configs.insert(icp, config);

        let input_e8s = 100_000_000u64;
        let rmr = state.get_redemption_margin_ratio();
        let reserve_fee = ICUSD::from(input_e8s) * state.reserve_redemption_fee;
        let net_after_reserve_fee = ICUSD::from(input_e8s) - reserve_fee;
        let spillover_e8s = (net_after_reserve_fee * rmr).to_u64();
        assert_eq!(spillover_e8s, 81_000_000);
        let raw_budget = net_after_reserve_fee.to_u64();
        let raw_budget_after_stable = reserve_spillover_raw_refund_budget(raw_budget, 0, rmr);
        assert_eq!(raw_budget_after_stable, 90_000_000);

        // Pin both fee bounds to 1% so the actual recorder is independent of
        // canister time in this host-side test.
        let vault_fee = ICUSD::from(spillover_e8s) * Ratio::from(dec!(0.01));
        assert_eq!(vault_fee.to_u64(), 810_000);
        let effective_spillover = ICUSD::from(spillover_e8s) - vault_fee;
        let run = state.redemption_runs().into_iter().next().unwrap();
        assert_eq!(run.collateral_type, icp);
        let mut persisted_events = Vec::new();
        let outcome = crate::event::record_redemption_on_vault_run_at(
            &mut state,
            principal(0x99),
            effective_spillover,
            vault_fee,
            crate::numeric::UsdIcp::from(dec!(1)),
            702,
            icp,
            &run.vault_ids,
            None,
            1_000_000_000_000,
            &mut persisted_events,
        )
        .unwrap();
        assert_eq!(outcome.consumed.to_u64(), 40_095_000);
        assert_eq!(
            state.vault_id_to_vaults[&1].borrowed_icusd_amount.to_u64(),
            0
        );
        let [crate::event::Event::RedemptionOnVaults {
            payout_collateral_raw: Some(payout_raw),
            ..
        }] = persisted_events.as_slice()
        else {
            panic!("the checked recorder should persist its actual native payout");
        };
        assert_eq!(*payout_raw, 40_095_000);

        let refund_raw = reserve_post_settlement_raw_refund(
            raw_budget,
            0,
            vault_fee.to_u64(),
            outcome.consumed.to_u64(),
            rmr,
        );
        // Independent oracle: V + C = 40,905,000 effective e8s;
        // ceil(40,905,000 / .9) = 45,450,000 raw, leaving 44,550,000.
        assert_eq!(refund_raw, 44_550_000);
        let native_raw_spent = (Decimal::from(vault_fee.to_u64() + outcome.consumed.to_u64())
            / rmr.0)
            .ceil()
            .to_u64()
            .unwrap();
        assert_eq!(native_raw_spent, 45_450_000);
        assert_eq!(
            reserve_fee.to_u64() + refund_raw + native_raw_spent,
            input_e8s,
            "reserve fee, committed native leg, and actual-recorder tail refund conserve input"
        );
    }

    #[test]
    fn fractional_effective_tail_is_divided_by_rmr_before_flooring() {
        let rmr_09 = Ratio::from(dec!(0.9));
        let input = Decimal::from(100_000_001u64);
        let effective = input * rmr_09.0;
        let unconsumed_after_one = effective - Decimal::ONE;
        // Independent oracle: (90,000,000.9 - 1) / .9 floors to
        // 99,999,999. Flooring the effective tail first under-refunds one.
        assert_eq!(
            redemption_tail_raw_refund(unconsumed_after_one, rmr_09, 100_000_001),
            99_999_999
        );
        assert_eq!(
            redemption_raw_refund(100_000_001, 1, rmr_09, 100_000_001),
            99_999_999,
            "the production direct/quoted calculation reconstructs unrounded E before subtracting actual consumption"
        );

        let fully_consumed_native = effective - Decimal::from(90_000_000u64);
        assert_eq!(fully_consumed_native, Decimal::new(9, 1));
        // This sub-unit tail still represents one raw icUSD unit after
        // unscaling; an early `to_u64() == 0` guard would incorrectly drop it.
        assert_eq!(
            redemption_tail_raw_refund(fully_consumed_native, rmr_09, 100_000_001),
            1
        );
        assert_eq!(
            redemption_raw_refund(100_000_001, 90_000_000, rmr_09, 100_000_001),
            1,
            "the actual integer Token execution still refunds the unrounded 0.9 remainder"
        );

        let rmr_half = Ratio::from(dec!(0.5));
        assert_eq!(
            redemption_tail_raw_refund(Decimal::new(99, 2), rmr_half, 100),
            1,
            "a 0.99 effective tail at 50% RMR floors to one raw input unit"
        );
        assert_eq!(
            redemption_tail_raw_refund(Decimal::from(100u64), rmr_half, 17),
            17,
            "the result remains capped by its original raw-input budget"
        );
        let above_u64 = Decimal::from(u64::MAX) + Decimal::ONE;
        assert_eq!(
            redemption_tail_raw_refund(above_u64, rmr_half, u64::MAX),
            u64::MAX,
            "cap the Decimal result before converting to u64"
        );
    }

    #[test]
    fn reserve_recorder_error_refunds_spillover_without_charging_uncommitted_vault_fee() {
        let (mut state, icp, _xaut) = redemption_state(0);
        state
            .vault_id_to_vaults
            .get_mut(&2)
            .unwrap()
            .borrowed_icusd_amount = ICUSD::new(0);
        state
            .vault_id_to_vaults
            .get_mut(&1)
            .unwrap()
            .borrowed_icusd_amount = ICUSD::new(100_000_000);
        state.rmr_floor = Ratio::from(dec!(0.9));
        state.total_collateral_ratio = state.rmr_floor_cr;
        state.reserve_redemption_fee = Ratio::from(dec!(0.1));
        let mut config = state.collateral_configs.get(&icp).unwrap().clone();
        config.redemption_fee_floor = Ratio::from(dec!(0.01));
        config.redemption_fee_ceiling = Ratio::from(dec!(0.01));
        state.collateral_configs.insert(icp, config);

        let input_e8s = 100_000_000;
        let rmr = state.get_redemption_margin_ratio();
        let reserve_fee = ICUSD::from(input_e8s) * state.reserve_redemption_fee;
        let net_after_reserve_fee = ICUSD::from(input_e8s) - reserve_fee;
        let spillover_e8s = (net_after_reserve_fee * rmr).to_u64();
        let raw_budget =
            reserve_spillover_raw_refund_budget(net_after_reserve_fee.to_u64(), 0, rmr);
        let planned_vault_fee = ICUSD::from(spillover_e8s) * Ratio::from(dec!(0.01));
        assert_eq!(planned_vault_fee.to_u64(), 810_000);
        let effective_spillover = ICUSD::from(spillover_e8s) - planned_vault_fee;
        let run = state.redemption_runs().into_iter().next().unwrap();
        assert_eq!(run.collateral_type, icp);
        let before = vault_balances(&state);
        let before_pending = state.pending_redemption_transfer.len();
        let before_base_rate = state.collateral_configs[&icp].current_base_rate;
        let mut persisted_events = Vec::new();
        let result = crate::event::record_redemption_on_vault_run_at(
            &mut state,
            principal(0x99),
            effective_spillover,
            planned_vault_fee,
            crate::numeric::UsdIcp::from(dec!(1)),
            701,
            icp,
            &run.vault_ids,
            Some(u64::MAX),
            1_000_000_000_000,
            &mut persisted_events,
        );
        assert!(matches!(
            result,
            Err(crate::event::RedemptionRecordError::MinimumNotMet { .. })
        ));
        assert_eq!(vault_balances(&state), before);
        assert_eq!(state.pending_redemption_transfer.len(), before_pending);
        assert_eq!(
            state.collateral_configs[&icp].current_base_rate,
            before_base_rate
        );
        assert!(persisted_events.is_empty());

        // The recorder failed before committing the native fee. Preserve the
        // settled reserve fee, refund the entire effective spillover tail, and
        // do not subtract the locally calculated but uncommitted 1% fee.
        assert_eq!(
            reserve_post_settlement_raw_refund(net_after_reserve_fee.to_u64(), 0, 0, 0, rmr,),
            90_000_000
        );
        assert_eq!(
            reserve_spillover_snapshot_mismatch_refund(spillover_e8s, rmr, raw_budget),
            90_000_000,
            "a recorder error commits neither its planned vault fee nor debt consumption"
        );
        assert_eq!(input_e8s - 90_000_000, reserve_fee.to_u64());
    }

    #[test]
    fn reserve_full_stable_leg_returns_sub_e6_raw_dust() {
        let rmr = Ratio::from(dec!(0.9));
        let raw_after_fee = 100_000_111u64;
        let effective_e8s = (ICUSD::from(raw_after_fee) * rmr).to_u64();
        let net_e6s = effective_e8s / 100;
        let stable_paid_e6s = effective_e8s / 100;
        let spillover_e8s = net_e6s * 100 - stable_paid_e6s * 100;
        assert_eq!(effective_e8s, 90_000_099);
        assert_eq!(stable_paid_e6s, 900_000);
        assert_eq!(spillover_e8s, 0);
        assert_eq!(effective_e8s - stable_paid_e6s * 100, 99);

        let raw_budget = reserve_spillover_raw_refund_budget(raw_after_fee, stable_paid_e6s, rmr);
        assert_eq!(raw_budget, 111);
        assert_eq!(
            reserve_post_settlement_raw_refund(raw_after_fee, stable_paid_e6s, 0, 0, rmr),
            111,
            "stable e6 quantization must not retain the remaining raw icUSD budget"
        );
    }

    #[test]
    fn rejected_redemption_refund_failure_persists_the_original_claim_and_nonce() {
        let (mut state, _, _) = redemption_state(0);
        let owner = principal(0x88);
        let before = vault_balances(&state);
        let block = 123;
        let nonce = 456;
        persist_rejected_redemption_refund(&mut state, owner, block, 987_654, nonce);
        let claim = state.pending_refunds.get(&block).unwrap();
        assert_eq!(claim.user, owner);
        assert_eq!(claim.amount_e8s, 987_654);
        assert_eq!(claim.retry_count, 0);
        assert_eq!(claim.op_nonce, nonce);
        assert_eq!(vault_balances(&state), before);
        assert!(state.pending_redemption_transfer.is_empty());
    }

    #[test]
    fn advisory_preview_can_quote_old_complete_prices_but_never_marks_them_fresh() {
        let (state, _icp, _xaut) = redemption_state(120_000_000);
        let now = 1 + REDEMPTION_PRICE_MAX_AGE_NS + 1;
        let before_vaults = vault_balances(&state);
        let before_refunds = state.pending_refunds.len();
        let before_payouts = state.pending_redemption_transfer.len();
        let (queue, estimate) = build_redemption_queue_and_quote(&state, now, E8, true);

        assert!(!queue.ranking_fresh);
        assert!(queue.entries.len() == 2);
        assert!(queue.entries.iter().all(|entry| !entry.price_fresh));
        let quote = estimate.expect("complete stale cached prices remain advisory-quotable");
        assert!(!quote.ranking_fresh);
        assert!(!quote.price_fresh);
        assert!(quote.net_collateral_raw > 0);
        assert_eq!(vault_balances(&state), before_vaults);
        assert_eq!(state.pending_refunds.len(), before_refunds);
        assert_eq!(state.pending_redemption_transfer.len(), before_payouts);

        let (_, executable_quote) = build_redemption_queue_and_quote(&state, now, E8, false);
        assert!(matches!(
            executable_quote,
            Err(crate::RedemptionError::RedemptionQuoteUnavailable(_))
        ));
    }

    #[test]
    fn incomplete_cached_ranking_does_not_invent_an_advisory_quote() {
        let (mut state, _icp, xaut) = redemption_state(120_000_000);
        state.collateral_configs.get_mut(&xaut).unwrap().last_price = None;
        let (queue, estimate) = build_redemption_queue_and_quote(&state, 10, E8, true);
        assert!(!queue.ranking_fresh);
        assert_eq!(queue.entries.len(), 1);
        assert!(matches!(
            estimate,
            Err(crate::RedemptionError::RedemptionQuoteUnavailable(_))
        ));
    }

    #[test]
    fn refreshed_snapshot_reranks_every_candidate_and_preserves_fresh_queue_on_capacity_error() {
        let (mut state, icp, xaut) = redemption_state(120_000_000);
        let now = 10_000_000_000_000;
        for ct in [icp, xaut] {
            state
                .collateral_configs
                .get_mut(&ct)
                .unwrap()
                .last_price_timestamp = Some(now);
        }
        let (initial, initial_quote) = build_redemption_queue_and_quote(&state, now, E8, false);
        assert!(initial.ranking_fresh);
        assert_eq!(initial.entries[0].collateral_type, xaut);
        assert_eq!(initial_quote.unwrap().collateral_type, xaut);

        // A new price for the competing asset changes the global health order;
        // the offer builder must recalculate the whole ranking before quoting.
        state.collateral_configs.get_mut(&xaut).unwrap().last_price = Some(2.0);
        let (reranked, quote) = build_redemption_queue_and_quote(&state, now, E8, false);
        assert!(reranked.ranking_fresh);
        assert_eq!(reranked.entries[0].collateral_type, icp);
        assert_eq!(quote.unwrap().collateral_type, icp);

        let (fresh_queue, too_large) =
            build_redemption_queue_and_quote(&state, now, u64::MAX, false);
        assert!(fresh_queue.ranking_fresh);
        assert!(matches!(
            too_large,
            Err(crate::RedemptionError::RedemptionCapacityExceeded { .. })
        ));
        assert_eq!(fresh_queue.entries[0].collateral_type, icp);
    }

    #[test]
    fn fresh_offer_snapshot_is_cache_only_and_leaves_redemption_state_unchanged() {
        let (mut state, icp, xaut) = redemption_state(120_000_000);
        let now = 10_000_000_000_000;
        for ct in [icp, xaut] {
            state
                .collateral_configs
                .get_mut(&ct)
                .unwrap()
                .last_price_timestamp = Some(now);
        }
        let mut xaut_config = state.collateral_configs.get(&xaut).unwrap().clone();
        xaut_config.redemption_fee_floor = Ratio::from(dec!(0.01));
        xaut_config.redemption_fee_ceiling = Ratio::from(dec!(0.01));
        state.collateral_configs.insert(xaut, xaut_config);
        assert!(cached_redemption_offer_is_fresh(&state, now));
        assert!(stale_redemption_candidate_types(
            &state,
            &super::redemption_candidate_types(&state),
            now
        )
        .is_empty());

        let before_vaults = vault_balances(&state);
        let before_pending_refunds = state.pending_refunds.len();
        let before_pending_payouts = state.pending_redemption_transfer.len();
        let before_debt = state.total_borrowed_icusd_amount();
        let offer = prepared_offer_from_current_state(&state, now, E8, 0).unwrap();
        let quote = offer.quote.unwrap();
        assert!(offer.queue.ranking_fresh);
        assert!(quote.ranking_fresh && quote.price_fresh);
        assert_eq!(quote.quoted_at_ns, now);
        assert_eq!(
            quote
                .quoted_at_ns
                .checked_add(quote.quote_validity_window_ns),
            Some(now + 60 * 1_000_000_000)
        );
        assert_eq!(quote.fee_e8s, E8 / 100, "configured 1% fee is included");
        assert!(quote.ledger_fee_raw > 0);
        assert!(quote.net_collateral_raw > 0);
        assert_eq!(vault_balances(&state), before_vaults);
        assert_eq!(state.pending_refunds.len(), before_pending_refunds);
        assert_eq!(
            state.pending_redemption_transfer.len(),
            before_pending_payouts
        );
        assert_eq!(state.total_borrowed_icusd_amount(), before_debt);

        let refresh_calls = std::cell::Cell::new(0usize);
        let refresh_result =
            futures::executor::block_on(refresh_stale_redemption_candidates_for_offer_with(
                || now,
                |at| redemption_offer_price_refresh_snapshot(&state, at),
                |_| {
                    refresh_calls.set(refresh_calls.get() + 1);
                    std::future::ready(Ok::<(), crate::RedemptionOfferRefreshError>(()))
                },
            ));
        assert!(refresh_result.is_ok());
        assert_eq!(
            refresh_calls.get(),
            0,
            "all-fresh cache bypasses oracle work"
        );
    }

    #[test]
    fn stale_candidate_refresh_selection_skips_fresh_competitors() {
        let (mut state, icp, xaut) = redemption_state(120_000_000);
        let now = 1 + REDEMPTION_PRICE_MAX_AGE_NS + 1;
        state
            .collateral_configs
            .get_mut(&icp)
            .unwrap()
            .last_price_timestamp = Some(now);
        let candidates = super::redemption_candidate_types(&state);
        assert_eq!(candidates.len(), 2);
        assert_eq!(
            stale_redemption_candidate_types(&state, &candidates, now),
            vec![xaut],
            "the fresh ICP competitor must not cause an unnecessary oracle call"
        );
        assert_eq!(
            super::MAX_REDEMPTION_OFFER_REFRESH_PASSES * super::MAX_REDEMPTION_PRICE_CANDIDATES,
            128,
            "the offer refresh loop has two passes over at most 64 candidates"
        );
        assert_eq!(
            super::MAX_REDEMPTION_OFFER_REFRESH_TARGETS_PER_PASS,
            65,
            "at most 64 eligible assets plus the one deduplicated ICP dependency"
        );
        assert_eq!(
            super::MAX_REDEMPTION_OFFER_EXTERNAL_CALLS,
            518,
            "bound includes the optional ICP dependency and two possible coupled LST waves"
        );
    }

    #[test]
    fn nicp_only_offer_refreshes_stale_icp_dependency_before_quoting() {
        use std::cell::RefCell;
        use std::rc::Rc;

        let (initial_state, icp, nicp) = stale_lst_only_redemption_state();

        let now = 1_000_000_000_000;
        let initial_snapshot = redemption_offer_price_refresh_snapshot(&initial_state, now);
        assert_eq!(initial_snapshot.candidates, vec![nicp]);
        assert_eq!(
            initial_snapshot.refresh_targets,
            vec![icp, nicp],
            "the missing ICP source must be requested first even without ICP debt"
        );
        assert!(!initial_snapshot.ranking_fresh);

        // Inject only the price-source await. This runs the same offer refresh
        // orchestration and revalidation used by the public endpoint; accepting
        // ICP also simulates the existing LST coupling publication.
        let state = Rc::new(RefCell::new(initial_state));
        let requested = Rc::new(RefCell::new(Vec::new()));
        let result =
            futures::executor::block_on(refresh_stale_redemption_candidates_for_offer_with(
                || now,
                {
                    let state = Rc::clone(&state);
                    move |at| redemption_offer_price_refresh_snapshot(&state.borrow(), at)
                },
                {
                    let state = Rc::clone(&state);
                    let requested = Rc::clone(&requested);
                    move |collateral_type| {
                        let state = Rc::clone(&state);
                        let requested = Rc::clone(&requested);
                        async move {
                            requested.borrow_mut().push(collateral_type);
                            if collateral_type == icp {
                                let mut state = state.borrow_mut();
                                state.set_icp_rate(
                                    crate::numeric::UsdIcp::from(dec!(1.05)),
                                    Some(now),
                                );
                                let lst_price = crate::management::compute_lst_wrapped_price(
                                    dec!(1.05),
                                    E8,
                                    0.0,
                                )
                                .unwrap()
                                .to_f64()
                                .unwrap();
                                state.on_collateral_price_change(&nicp, lst_price);
                                state
                                    .collateral_configs
                                    .get_mut(&nicp)
                                    .unwrap()
                                    .last_price_timestamp = Some(now);
                            }
                            Ok(())
                        }
                    }
                },
            ));
        assert!(
            result.is_ok(),
            "a fresh accepted dependency permits the offer"
        );
        assert_eq!(*requested.borrow(), vec![icp, nicp]);

        let state = state.borrow();
        assert_eq!(state.last_icp_timestamp, Some(now));
        assert_eq!(
            state.collateral_configs[&nicp].last_price_timestamp,
            Some(now)
        );
        let (queue, quote) = build_redemption_queue_and_quote(&state, now, E8, false);
        assert!(queue.ranking_fresh);
        assert_eq!(queue.entries.len(), 1);
        assert_eq!(queue.entries[0].collateral_type, nicp);
        assert_eq!(quote.unwrap().collateral_type, nicp);
    }

    #[test]
    fn stale_icp_candidate_and_lst_dependency_are_deduplicated() {
        let (mut state, icp, lst) = redemption_state(120_000_000);
        state.set_icp_rate(crate::numeric::UsdIcp::from(dec!(1)), Some(1));
        let mut config = state.collateral_configs.get(&lst).unwrap().clone();
        config.price_source = crate::state::PriceSource::LstWrapped {
            base_asset: "ICP".to_string(),
            base_asset_class: crate::state::XrcAssetClass::Cryptocurrency,
            quote_asset: "USD".to_string(),
            quote_asset_class: crate::state::XrcAssetClass::FiatCurrency,
            rate_canister_id: principal(0x77),
            rate_method: "get_info".to_string(),
            haircut: 0.0,
        };
        config.last_price_timestamp = Some(1);
        state.collateral_configs.insert(lst, config);

        let snapshot = redemption_offer_price_refresh_snapshot(&state, 1_000_000_000_000);
        assert_eq!(snapshot.candidates, vec![icp, lst]);
        assert_eq!(snapshot.refresh_targets, vec![icp, lst]);
        assert_eq!(
            snapshot
                .refresh_targets
                .iter()
                .filter(|candidate| **candidate == icp)
                .count(),
            1,
            "the ICP source is fetched once even when it is both candidate and dependency"
        );
    }

    #[test]
    fn failed_icp_dependency_refresh_returns_no_stale_offer() {
        let (state, icp, _nicp) = stale_lst_only_redemption_state();
        let now = 1_000_000_000_000;
        let result =
            futures::executor::block_on(refresh_stale_redemption_candidates_for_offer_with(
                || now,
                |at| redemption_offer_price_refresh_snapshot(&state, at),
                move |collateral_type| {
                    assert_eq!(collateral_type, icp, "dependency is refreshed before nICP");
                    std::future::ready(Err(
                        crate::RedemptionOfferRefreshError::RefreshUnavailable {
                            message: "XRC returned no acceptable ICP sample".to_string(),
                            retry_after_ns: 300_000_000_000,
                        },
                    ))
                },
            ));
        assert!(matches!(
            result,
            Err(crate::RedemptionOfferRefreshError::RefreshUnavailable {
                message,
                retry_after_ns: 300_000_000_000,
            }) if message.contains("no acceptable ICP sample")
        ));
        let (queue, quote) = build_redemption_queue_and_quote(&state, now, E8, false);
        assert!(!queue.ranking_fresh);
        assert!(matches!(
            quote,
            Err(crate::RedemptionError::RedemptionQuoteUnavailable(_))
        ));
    }

    #[test]
    fn offer_refresh_gate_has_bounded_retry_and_owner_safe_lease_recovery() {
        let start = 1_000_000_000_000;
        let mut gate = RedemptionOfferRefreshGateState::default();
        let old_owner = gate.try_acquire(start).unwrap();
        assert!(matches!(
            gate.try_acquire(start + 1),
            Err(crate::RedemptionOfferRefreshError::RefreshInProgress {
                retry_after_ns
            }) if retry_after_ns == REDEMPTION_OFFER_REFRESH_LEASE_NS - 1
        ));

        let new_start = start + REDEMPTION_OFFER_REFRESH_LEASE_NS + 1;
        let new_owner = gate.try_acquire(new_start).unwrap();
        gate.release(old_owner);
        assert_eq!(gate.in_flight, Some((new_owner, new_start)));
        gate.release(new_owner);
        assert!(matches!(
            gate.try_acquire(new_start + 1),
            Err(crate::RedemptionOfferRefreshError::RefreshCooldown {
                retry_after_ns
            }) if retry_after_ns == REDEMPTION_OFFER_REFRESH_COOLDOWN_NS - 1
        ));
        assert!(gate
            .try_acquire(new_start + REDEMPTION_OFFER_REFRESH_COOLDOWN_NS)
            .is_ok());
    }

    #[test]
    fn borrow_mint_receipt_requires_exact_typed_tuple() {
        let row = crate::state::PendingBorrowMint {
            op_nonce: 123,
            vault_id: 9,
            owner: Principal::from_slice(&[0x51]),
            collateral_type: Principal::from_slice(&[0x52]),
            ledger: Principal::from_slice(&[0x53]),
            gross_amount_e8s: 1_010,
            fee_e8s: 10,
            net_amount_e8s: 1_000,
            created_at_time_ns: management::nonce_to_created_at_time(123),
            created_at_ns: 100,
            attempts: 1,
            last_attempt_at_ns: 100,
            held_reason: None,
        };
        let exact = crate::icrc3_proof::DecodedBlock {
            btype: Some("1mint".into()),
            op: "mint".into(),
            from: None,
            to: Some(icrc_ledger_types::icrc1::account::Account {
                owner: row.owner,
                subaccount: None,
            }),
            spender: None,
            amount: row.net_amount_e8s as u128,
            transaction_fee: None,
            fee: None,
            memo: Some(management::nonce_to_memo(row.op_nonce).0.to_vec()),
            created_at_time: Some(row.created_at_time_ns),
            expected_allowance: None,
            expires_at: None,
        };
        assert!(validate_borrow_mint_receipt(&row, &exact).is_ok());

        let mut official_legacy = exact.clone();
        official_legacy.btype = None;
        assert!(validate_borrow_mint_receipt(&row, &official_legacy).is_ok());
        let mut zero_tx_fee = exact.clone();
        zero_tx_fee.transaction_fee = Some(0);
        assert!(validate_borrow_mint_receipt(&row, &zero_tx_fee).is_ok());
        let mut zero_block_fee = exact.clone();
        zero_block_fee.fee = Some(0);
        assert!(validate_borrow_mint_receipt(&row, &zero_block_fee).is_ok());
        let mut nonzero_fee = exact.clone();
        nonzero_fee.fee = Some(1);
        assert!(validate_borrow_mint_receipt(&row, &nonzero_fee).is_err());
        let mut nonzero_transaction_fee = exact.clone();
        nonzero_transaction_fee.transaction_fee = Some(1);
        assert!(validate_borrow_mint_receipt(&row, &nonzero_transaction_fee).is_err());
        let mut wrong_op = exact.clone();
        wrong_op.op = "transfer".into();
        assert!(validate_borrow_mint_receipt(&row, &wrong_op).is_err());
        let mut tuple_drift = exact.clone();
        tuple_drift.created_at_time = Some(row.created_at_time_ns + 1);
        assert!(validate_borrow_mint_receipt(&row, &tuple_drift).is_err());
        let mut wrong = exact.clone();
        wrong.btype = Some("2mint".into());
        assert!(validate_borrow_mint_receipt(&row, &wrong).is_err());
        let mut wrong = exact.clone();
        wrong.to.as_mut().unwrap().owner = Principal::from_slice(&[0x54]);
        assert!(validate_borrow_mint_receipt(&row, &wrong).is_err());
        let mut wrong = exact.clone();
        wrong.memo = Some(vec![0xff]);
        assert!(validate_borrow_mint_receipt(&row, &wrong).is_err());
        let mut wrong = exact.clone();
        wrong.amount += 1;
        assert!(validate_borrow_mint_receipt(&row, &wrong).is_err());
    }

    #[test]
    fn margin_add_rejects_unrepresentable_balance_before_transfer() {
        assert_eq!(checked_margin_balance(u64::MAX - 1, 1), Some(u64::MAX));
        assert_eq!(checked_margin_balance(u64::MAX - 1, 2), None);
    }

    #[test]
    fn borrow_mint_dispatch_guard_serializes_same_operation() {
        let first = BorrowMintDispatchGuard::try_new(77).unwrap();
        assert!(BorrowMintDispatchGuard::try_new(77).is_none());
        drop(first);
        assert!(BorrowMintDispatchGuard::try_new(77).is_some());
    }
}

#[cfg(test)]
mod manual_liquidation_v2_commit_tests {
    use super::Vault;
    use super::{
        manual_liquidation_refund_dispatch_is_allowed,
        manual_liquidation_refund_error_is_proven_no_effect,
        manual_liquidation_refund_expected_fee, manual_liquidation_refund_history_page_bounds,
        manual_liquidation_v2_commit_preflight, manual_liquidation_v2_refund_memo,
        manual_liquidation_v2_refund_receipt_matches,
    };
    use crate::state::{
        InterestDestination, InterestRecipient, ManualLiquidationPinnedPlan,
        ManualLiquidationV2Journal, Mode, StableRepaymentV2InterestRoutingPlan, State,
    };
    use crate::{ManualLiquidationRoute, ManualLiquidationV2Phase, SpLiquidationStablePullTuple};
    use candid::Principal;
    use icrc_ledger_types::icrc1::account::Account;

    fn principal(byte: u8) -> Principal {
        Principal::from_slice(&[byte])
    }

    fn fixture() -> (State, ManualLiquidationV2Journal) {
        let owner = principal(1);
        let liquidator = principal(2);
        let ledger = principal(3);
        let collateral = principal(4);
        let vault = Vault {
            owner,
            borrowed_icusd_amount: crate::numeric::ICUSD::new(100),
            collateral_amount: 1_000,
            vault_id: 7,
            collateral_type: collateral,
            last_accrual_time: 0,
            accrued_interest: crate::numeric::ICUSD::new(10),
            bot_processing: false,
        };
        let routing = StableRepaymentV2InterestRoutingPlan {
            split: vec![InterestRecipient {
                destination: InterestDestination::Treasury,
                bps: 10_000,
            }],
            stable_treasury: Some(principal(5)),
            icusd_ledger: ledger,
            stability_pool: None,
            three_pool: None,
            amm1: None,
            amm1_pool_id: None,
        };
        let mut state = State::default();
        state
            .vault_id_to_vaults
            .insert(vault.vault_id, vault.clone());
        let tuple = SpLiquidationStablePullTuple {
            op_nonce: 9,
            ledger,
            from: Account {
                owner: liquidator,
                subaccount: None,
            },
            spender: Account {
                owner: principal(6),
                subaccount: None,
            },
            to: Account {
                owner: principal(6),
                subaccount: None,
            },
            amount_raw: 50,
            fee_raw: 0,
            memo: vec![9],
            created_at_time_ns: 9,
        };
        let row = ManualLiquidationV2Journal {
            owner: liquidator,
            request_id: 1,
            vault_id: vault.vault_id,
            route: ManualLiquidationRoute::PartialIcusd,
            requested_amount_e8s: 50,
            pull_amount_raw: 50,
            ledger,
            tuple,
            created_at_ns: 1,
            plan: ManualLiquidationPinnedPlan {
                vault,
                mode: Mode::GeneralAvailability,
                collateral_price: rust_decimal::Decimal::ONE,
                collateral_decimals: 8,
                debt_liquidated_e8s: 50,
                collateral_to_liquidator_raw: 50,
                collateral_to_seize_raw: 50,
                protocol_cut_raw: 0,
                excess_collateral_raw: 0,
                interest_share_e8s: 5,
                stable_surcharge_e6s: 0,
                interest_routing_plan: Some(routing),
            },
            phase: ManualLiquidationV2Phase::PendingPull,
            candidate_block_index: Some(10),
            candidate_attach_window_start_ns: 0,
            candidate_attach_attempts: 0,
            had_ambiguous_attempt: false,
            dispatch_attempts: 1,
            result: None,
            refund: None,
            commit_started: false,
            last_error: None,
        };
        state
            .manual_liquidation_v2_active
            .insert(row.owner, row.clone());
        (state, row)
    }

    #[test]
    fn paid_vault_drift_is_rejected_before_any_accounting_or_obligation_write() {
        let (mut state, row) = fixture();
        assert!(manual_liquidation_v2_commit_preflight(&state, &row, 10).is_ok());
        let before_active = state.manual_liquidation_v2_active.clone();
        let before_treasury = state.pending_treasury_payments.clone();
        state
            .vault_id_to_vaults
            .get_mut(&row.vault_id)
            .unwrap()
            .borrowed_icusd_amount = crate::numeric::ICUSD::new(101);
        assert!(manual_liquidation_v2_commit_preflight(&state, &row, 10).is_err());
        assert_eq!(state.manual_liquidation_v2_active, before_active);
        assert_eq!(state.pending_treasury_payments, before_treasury);
        assert_eq!(
            state.vault_id_to_vaults[&row.vault_id]
                .borrowed_icusd_amount
                .to_u64(),
            101
        );
    }

    #[test]
    fn terminal_request_cannot_pass_commit_preflight_a_second_time() {
        let (mut state, mut row) = fixture();
        row.phase = ManualLiquidationV2Phase::CommittedPayoutQueued;
        row.result = Some(crate::ManualLiquidationV2Result {
            ledger: row.ledger,
            block_index: 10,
            debt_liquidated_e8s: row.plan.debt_liquidated_e8s,
            collateral_to_liquidator_raw: row.plan.collateral_to_liquidator_raw,
            collateral_to_seize_raw: row.plan.collateral_to_seize_raw,
            liquidator_xrp_claim_id: None,
        });
        state
            .manual_liquidation_v2_active
            .insert(row.owner, row.clone());
        crate::state::finish_manual_liquidation_v2(&mut state, row.owner, row.request_id).unwrap();
        assert!(manual_liquidation_v2_commit_preflight(&state, &row, 10).is_err());
        assert_eq!(
            state.manual_liquidation_v2_latest_result.get(&row.owner),
            Some(&row)
        );
    }

    #[test]
    fn refund_tuple_is_fee_free_exact_and_never_redispatched_after_upgrade() {
        let recipient = Principal::from_slice(&[0x21]);
        let refund = crate::ManualLiquidationV2Refund {
            caller: recipient,
            request_id: 3,
            vault_id: 7,
            kind: crate::ManualLiquidationV2RefundKind::IcusdMint {
                burn_block_index: 11,
            },
            ledger: Principal::from_slice(&[0x22]),
            source: Account {
                owner: Principal::from_slice(&[0x23]),
                subaccount: None,
            },
            recipient,
            amount_e8s: 500,
            fee_raw: None,
            op_nonce: 17,
            created_at_time_ns: 99,
            memo: vec![3, 4],
            history_cursor: 12,
            history_end_exclusive: Some(20),
            dispatch_attempts: 1,
            no_effect_attempts: Vec::new(),
            candidate_block_index: None,
            last_error: Some("reply lost".into()),
        };
        assert!(!manual_liquidation_refund_dispatch_is_allowed(&refund));
        let block = crate::icrc3_proof::DecodedBlock {
            btype: Some("1mint".into()),
            op: "mint".into(),
            from: None,
            to: Some(Account {
                owner: recipient,
                subaccount: None,
            }),
            spender: None,
            amount: 500,
            transaction_fee: None,
            fee: None,
            memo: Some(vec![3, 4]),
            created_at_time: Some(99),
            expected_allowance: None,
            expires_at: None,
        };
        assert!(manual_liquidation_v2_refund_receipt_matches(
            &refund, &block
        ));
        let mut wrong = block.clone();
        wrong.amount = 501;
        assert!(!manual_liquidation_v2_refund_receipt_matches(
            &refund, &wrong
        ));
        wrong = block;
        wrong.fee = Some(1);
        assert!(!manual_liquidation_v2_refund_receipt_matches(
            &refund, &wrong
        ));
        assert!(!manual_liquidation_refund_dispatch_is_allowed(&refund));
        assert!(manual_liquidation_refund_error_is_proven_no_effect(
            &icrc_ledger_types::icrc1::transfer::TransferError::BadFee {
                expected_fee: candid::Nat::from(1u8),
            }
        ));
        assert_eq!(
            manual_liquidation_refund_expected_fee(
                &icrc_ledger_types::icrc1::transfer::TransferError::BadFee {
                    expected_fee: candid::Nat::from(1u8),
                }
            ),
            Some(1)
        );
        assert!(!manual_liquidation_refund_error_is_proven_no_effect(
            &icrc_ledger_types::icrc1::transfer::TransferError::TemporarilyUnavailable
        ));
        let mut retryable = refund.clone();
        retryable
            .no_effect_attempts
            .push(crate::ManualLiquidationV2RefundNoEffect {
                dispatch_attempt: 1,
                fee_raw: None,
                op_nonce: refund.op_nonce,
                created_at_time_ns: refund.created_at_time_ns,
                memo: refund.memo.clone(),
                expected_fee_raw: None,
                error: "InsufficientFunds".into(),
            });
        assert!(manual_liquidation_refund_dispatch_is_allowed(&retryable));
        retryable.dispatch_attempts = crate::MAX_MANUAL_LIQUIDATION_REFUND_DISPATCHES;
        assert!(!manual_liquidation_refund_dispatch_is_allowed(&retryable));
    }

    #[test]
    fn stable_refund_receipt_proves_exact_source_destination_amount_fee_and_identity_memo() {
        let caller = principal(0x31);
        let backend = principal(0x32);
        let ledger = principal(0x33);
        let request_id = 41;
        let vault_id = 17;
        let pull_block_index = 91;
        let op_nonce = 101;
        let fee_raw = 7;
        let amount_raw = 500_000;
        let memo = manual_liquidation_v2_refund_memo(
            caller,
            request_id,
            vault_id,
            pull_block_index,
            ledger,
            amount_raw,
            Some(fee_raw),
            op_nonce,
        );
        let refund = crate::ManualLiquidationV2Refund {
            caller,
            request_id,
            vault_id,
            kind: crate::ManualLiquidationV2RefundKind::StableTransfer {
                pull_block_index,
                fee_raw,
            },
            ledger,
            source: Account {
                owner: backend,
                subaccount: None,
            },
            recipient: caller,
            amount_e8s: amount_raw,
            fee_raw: Some(fee_raw),
            op_nonce,
            created_at_time_ns: 123,
            memo: memo.clone(),
            history_cursor: 0,
            history_end_exclusive: None,
            dispatch_attempts: 1,
            no_effect_attempts: Vec::new(),
            candidate_block_index: None,
            last_error: None,
        };
        let mut exact = crate::icrc3_proof::DecodedBlock {
            btype: Some("1xfer".into()),
            op: "xfer".into(),
            from: Some(refund.source.clone()),
            to: Some(Account {
                owner: caller,
                subaccount: None,
            }),
            spender: None,
            amount: amount_raw as u128,
            transaction_fee: Some(fee_raw as u128),
            fee: Some(fee_raw as u128),
            memo: Some(memo.clone()),
            created_at_time: Some(123),
            expected_allowance: None,
            expires_at: None,
        };
        assert!(manual_liquidation_v2_refund_receipt_matches(
            &refund, &exact
        ));

        let mut bad_fee_retry = refund.clone();
        bad_fee_retry
            .no_effect_attempts
            .push(crate::ManualLiquidationV2RefundNoEffect {
                dispatch_attempt: 1,
                fee_raw: Some(fee_raw),
                op_nonce,
                created_at_time_ns: refund.created_at_time_ns,
                memo: memo.clone(),
                expected_fee_raw: Some(fee_raw + 1),
                error: "BadFee".into(),
            });
        bad_fee_retry.fee_raw = Some(fee_raw + 1);
        bad_fee_retry.kind = crate::ManualLiquidationV2RefundKind::StableTransfer {
            pull_block_index,
            fee_raw: fee_raw + 1,
        };
        bad_fee_retry.op_nonce += 1;
        bad_fee_retry.created_at_time_ns += 1;
        bad_fee_retry.memo = manual_liquidation_v2_refund_memo(
            caller,
            request_id,
            vault_id,
            pull_block_index,
            ledger,
            amount_raw,
            Some(fee_raw + 1),
            bad_fee_retry.op_nonce,
        );
        assert!(manual_liquidation_refund_dispatch_is_allowed(
            &bad_fee_retry
        ));
        bad_fee_retry.dispatch_attempts = crate::MAX_MANUAL_LIQUIDATION_REFUND_DISPATCHES;
        assert!(!manual_liquidation_refund_dispatch_is_allowed(
            &bad_fee_retry
        ));

        exact.transaction_fee = Some(0);
        assert!(!manual_liquidation_v2_refund_receipt_matches(
            &refund, &exact
        ));
        exact.transaction_fee = Some(fee_raw as u128);
        exact.from.as_mut().unwrap().owner = principal(0x34);
        assert!(!manual_liquidation_v2_refund_receipt_matches(
            &refund, &exact
        ));
        assert_ne!(
            manual_liquidation_v2_refund_memo(
                caller,
                request_id + 1,
                vault_id,
                pull_block_index,
                ledger,
                amount_raw,
                Some(fee_raw),
                op_nonce,
            ),
            refund.memo
        );
    }

    #[test]
    fn refund_history_scan_is_bounded_and_advances_without_claiming_absence() {
        assert_eq!(
            manual_liquidation_refund_history_page_bounds(10, 100),
            Some((10, 18))
        );
        assert_eq!(
            manual_liquidation_refund_history_page_bounds(96, 100),
            Some((96, 100))
        );
        assert_eq!(
            manual_liquidation_refund_history_page_bounds(100, 100),
            Some((100, 100))
        );
        assert_eq!(
            manual_liquidation_refund_history_page_bounds(101, 100),
            None
        );
    }
}
