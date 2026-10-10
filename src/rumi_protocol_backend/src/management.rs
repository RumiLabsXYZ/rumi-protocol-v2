use crate::numeric::{ICUSD, ICP};
use crate::state::{mutate_state, read_state, PushSweepJournal, PushSweepRequest};
use crate::StableTokenType;
use candid::{Nat, Principal};
use ic_xrc_types::{Asset, AssetClass, GetExchangeRateRequest, GetExchangeRateResult};
use icrc_ledger_types::icrc1::account::Account;
use icrc_ledger_types::icrc1::transfer::{Memo, TransferArg, TransferError};
use icrc_ledger_types::icrc2::approve::{ApproveArgs, ApproveError};
use icrc_ledger_types::icrc2::transfer_from::{TransferFromArgs, TransferFromError};
use icrc_ledger_client_cdk::{CdkRuntime, ICRC1Client};
use num_traits::ToPrimitive;
use sha2::{Sha256, Digest};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};

thread_local! {
    /// Avoid duplicate WaterNeuron rate-canister calls when the per-asset timer,
    /// an on-demand refresh, and the ICP-coupled refresh overlap. Transient by
    /// design; a trap/upgrade drops the in-flight task and resets this set.
    static LST_PRICE_FETCH_IN_FLIGHT: RefCell<HashSet<Principal>> =
        RefCell::new(HashSet::new());
}

struct LstPriceFetchGuard(Principal);

impl LstPriceFetchGuard {
    fn try_acquire(collateral_type: Principal) -> Option<Self> {
        LST_PRICE_FETCH_IN_FLIGHT.with(|in_flight| {
            let mut in_flight = in_flight.borrow_mut();
            if !in_flight.insert(collateral_type) {
                return None;
            }
            Some(Self(collateral_type))
        })
    }
}

impl Drop for LstPriceFetchGuard {
    fn drop(&mut self) {
        LST_PRICE_FETCH_IN_FLIGHT.with(|in_flight| {
            in_flight.borrow_mut().remove(&self.0);
        });
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LstPriceRefreshOutcome {
    Published,
    AlreadyCurrent,
    InFlight,
    NoIcpPrice,
    IcpTimestampChanged,
    NotLstWrapped,
    UnsupportedUnderlying,
    RateCallFailed,
    InvalidRate,
    SanityRejected,
    ConfigurationChanged,
}

async fn refresh_lst_with_one_catch_up<F, Fut, L>(
    collateral_type: Principal,
    expected_icp_timestamp_ns: u64,
    mut attempt: F,
    latest_timestamp: L,
) -> LstPriceRefreshOutcome
where
    F: FnMut(Principal, u64) -> Fut,
    Fut: std::future::Future<Output = LstPriceRefreshOutcome>,
    L: Fn() -> Option<u64>,
{
    let outcome = attempt(collateral_type, expected_icp_timestamp_ns).await;
    if outcome != LstPriceRefreshOutcome::IcpTimestampChanged {
        return outcome;
    }
    let Some(latest_timestamp_ns) = latest_timestamp()
        .filter(|latest| *latest > expected_icp_timestamp_ns)
    else {
        return outcome;
    };
    // At most one catch-up attempt: repeated source churn or a concurrent
    // fetch remains fail-closed until the next accepted sample/timer tick.
    attempt(collateral_type, latest_timestamp_ns).await
}

fn lst_refresh_preflight(
    current_icp_timestamp_ns: Option<u64>,
    expected_icp_timestamp_ns: u64,
    has_icp_price: bool,
    last_lst_timestamp_ns: Option<u64>,
) -> Result<(), LstPriceRefreshOutcome> {
    if current_icp_timestamp_ns != Some(expected_icp_timestamp_ns) {
        return Err(LstPriceRefreshOutcome::IcpTimestampChanged);
    }
    if !has_icp_price {
        return Err(LstPriceRefreshOutcome::NoIcpPrice);
    }
    if last_lst_timestamp_ns.is_some_and(|timestamp| timestamp >= expected_icp_timestamp_ns) {
        return Err(LstPriceRefreshOutcome::AlreadyCurrent);
    }
    Ok(())
}

fn lst_publication_preflight(
    current_icp_timestamp_ns: Option<u64>,
    expected_icp_timestamp_ns: u64,
    source_matches: bool,
    last_lst_timestamp_ns: Option<u64>,
) -> Result<(), LstPriceRefreshOutcome> {
    if current_icp_timestamp_ns != Some(expected_icp_timestamp_ns) {
        return Err(LstPriceRefreshOutcome::IcpTimestampChanged);
    }
    if !source_matches {
        return Err(LstPriceRefreshOutcome::ConfigurationChanged);
    }
    if last_lst_timestamp_ns.is_some_and(|timestamp| timestamp >= expected_icp_timestamp_ns) {
        return Err(LstPriceRefreshOutcome::AlreadyCurrent);
    }
    Ok(())
}

fn commit_lst_wrapped_price(
    state: &mut crate::state::State,
    collateral_type: &Principal,
    expected_icp_timestamp_ns: u64,
    source: &crate::state::PriceSource,
    final_rate: f64,
) -> LstPriceRefreshOutcome {
    let Some(config) = state.get_collateral_config(collateral_type) else {
        return LstPriceRefreshOutcome::NotLstWrapped;
    };
    if !matches!(source, crate::state::PriceSource::LstWrapped { .. }) {
        return LstPriceRefreshOutcome::NotLstWrapped;
    }
    if let Err(outcome) = lst_publication_preflight(
        state.last_icp_timestamp,
        expected_icp_timestamp_ns,
        config.price_source == *source,
        config.last_price_timestamp,
    ) {
        return outcome;
    }
    if !crate::xrc::check_price_sanity_band_at_source(
        state,
        collateral_type,
        Some(source),
        expected_icp_timestamp_ns,
        final_rate,
    ) {
        return LstPriceRefreshOutcome::SanityRejected;
    }
    state.on_collateral_price_change(collateral_type, final_rate);
    if let Some(config) = state.collateral_configs.get_mut(collateral_type) {
        config.last_price_timestamp = Some(expected_icp_timestamp_ns);
    }
    LstPriceRefreshOutcome::Published
}

#[cfg(test)]
mod lst_refresh_guard_tests {
    use super::{
        commit_lst_wrapped_price, lst_publication_preflight, lst_refresh_preflight,
        refresh_lst_with_one_catch_up, LstPriceFetchGuard, LstPriceRefreshOutcome,
    };
    use candid::Principal;
    use crate::state::{PriceSource, State, XrcAssetClass};
    use crate::{InitArg, UsdIcp};
    use rust_decimal::Decimal;

    fn configured_state() -> State {
        State::from(InitArg {
            xrc_principal: Principal::anonymous(),
            icusd_ledger_principal: Principal::anonymous(),
            icp_ledger_principal: Principal::anonymous(),
            fee_e8s: 0,
            developer_principal: Principal::anonymous(),
            treasury_principal: None,
            stability_pool_principal: None,
            ckusdt_ledger_principal: None,
            ckusdc_ledger_principal: None,
        })
    }

    fn add_lst_config(state: &mut State, collateral: Principal) -> PriceSource {
        let icp = state.icp_collateral_type();
        let mut config = state.get_collateral_config(&icp).unwrap().clone();
        let source = PriceSource::LstWrapped {
            base_asset: "ICP".to_string(),
            base_asset_class: XrcAssetClass::Cryptocurrency,
            quote_asset: "USD".to_string(),
            quote_asset_class: XrcAssetClass::FiatCurrency,
            rate_canister_id: Principal::from_slice(&[42]),
            rate_method: "get_info".to_string(),
            haircut: 0.05,
        };
        config.ledger_canister_id = collateral;
        config.price_source = source.clone();
        config.last_price = Some(1.0);
        config.last_price_timestamp = Some(90);
        state.collateral_configs.insert(collateral, config);
        state.set_icp_rate(UsdIcp::from(Decimal::ONE), Some(100));
        source
    }

    #[test]
    fn lst_inflight_guard_rejects_duplicate_and_releases_after_completion() {
        let collateral = Principal::from_slice(&[249, 1]);
        let first = LstPriceFetchGuard::try_acquire(collateral)
            .expect("first refresh acquires the per-collateral guard");
        assert!(LstPriceFetchGuard::try_acquire(collateral).is_none());
        drop(first);
        assert!(LstPriceFetchGuard::try_acquire(collateral).is_some());
    }

    #[test]
    fn lst_refresh_skips_unchanged_source_and_rejects_old_icp_samples() {
        assert_eq!(
            lst_refresh_preflight(Some(10), 10, true, Some(10)),
            Err(LstPriceRefreshOutcome::AlreadyCurrent)
        );
        assert_eq!(
            lst_refresh_preflight(Some(11), 10, true, Some(9)),
            Err(LstPriceRefreshOutcome::IcpTimestampChanged)
        );
        assert_eq!(
            lst_refresh_preflight(Some(10), 10, false, Some(9)),
            Err(LstPriceRefreshOutcome::NoIcpPrice)
        );
        assert_eq!(lst_refresh_preflight(Some(10), 10, true, Some(9)), Ok(()));
    }

    #[test]
    fn delayed_lst_callback_cannot_publish_over_newer_sample_or_config() {
        assert_eq!(
            lst_publication_preflight(Some(11), 10, true, Some(9)),
            Err(LstPriceRefreshOutcome::IcpTimestampChanged)
        );
        assert_eq!(
            lst_publication_preflight(Some(10), 10, false, Some(9)),
            Err(LstPriceRefreshOutcome::ConfigurationChanged)
        );
        assert_eq!(
            lst_publication_preflight(Some(10), 10, true, Some(10)),
            Err(LstPriceRefreshOutcome::AlreadyCurrent)
        );
        assert_eq!(lst_publication_preflight(Some(10), 10, true, Some(9)), Ok(()));
    }

    #[test]
    fn stale_timer_refresh_catches_up_once_after_new_sample_hit_inflight_guard() {
        use std::cell::RefCell;
        use std::collections::VecDeque;

        let collateral = Principal::from_slice(&[249, 3]);
        // Represents the t0 timer/submit callback while its rate-canister call
        // is still pending. The t1 ICP-coupled request must not overlap it.
        let old_refresh = LstPriceFetchGuard::try_acquire(collateral).unwrap();
        let t1_result = futures::executor::block_on(refresh_lst_with_one_catch_up(
            collateral,
            11,
            |ct, _| async move {
                if LstPriceFetchGuard::try_acquire(ct).is_some() {
                    LstPriceRefreshOutcome::Published
                } else {
                    LstPriceRefreshOutcome::InFlight
                }
            },
            || Some(11),
        ));
        assert_eq!(t1_result, LstPriceRefreshOutcome::InFlight);

        // The old response completes after t1 was accepted. Its result is
        // discarded, releasing the guard; the common wrapper retries once
        // using t1 and publishes, regardless of whether the original caller
        // was Timer A, the per-asset timer, or an on-demand refresh.
        drop(old_refresh);
        let results = RefCell::new(VecDeque::from([
            LstPriceRefreshOutcome::IcpTimestampChanged,
            LstPriceRefreshOutcome::Published,
        ]));
        let attempted_timestamps = RefCell::new(Vec::new());
        let result = futures::executor::block_on(refresh_lst_with_one_catch_up(
            collateral,
            10,
            |_, timestamp| {
                attempted_timestamps.borrow_mut().push(timestamp);
                let result = results.borrow_mut().pop_front().unwrap();
                async move { result }
            },
            || Some(11),
        ));
        assert_eq!(result, LstPriceRefreshOutcome::Published);
        assert_eq!(*attempted_timestamps.borrow(), vec![10, 11]);
        assert!(results.borrow().is_empty());
    }

    #[test]
    fn accepted_icp_timestamp_is_published_to_lst_and_stale_callback_preserves_price() {
        let mut state = configured_state();
        let collateral = Principal::from_slice(&[249, 2]);
        let source = add_lst_config(&mut state, collateral);
        assert_eq!(
            commit_lst_wrapped_price(&mut state, &collateral, 100, &source, f64::NAN),
            LstPriceRefreshOutcome::SanityRejected
        );
        let config = state.get_collateral_config(&collateral).unwrap();
        assert_eq!(config.last_price_timestamp, Some(90));
        assert_eq!(config.last_price, Some(1.0));

        assert_eq!(
            commit_lst_wrapped_price(&mut state, &collateral, 100, &source, 1.25),
            LstPriceRefreshOutcome::Published
        );
        let config = state.get_collateral_config(&collateral).unwrap();
        assert_eq!(config.last_price_timestamp, Some(100));
        assert_eq!(config.last_price, Some(1.25));

        state.set_icp_rate(UsdIcp::from(Decimal::from(2)), Some(101));
        assert_eq!(
            commit_lst_wrapped_price(&mut state, &collateral, 100, &source, 1.5),
            LstPriceRefreshOutcome::IcpTimestampChanged
        );
        let config = state.get_collateral_config(&collateral).unwrap();
        assert_eq!(config.last_price_timestamp, Some(100));
        assert_eq!(config.last_price, Some(1.25));
    }
}
use std::fmt;
use crate::log;
use crate::DEBUG;

// ─── Wave-3 ICRC transfer hygiene helpers ───
//
// Audit-driven (`audit-reports/2026-04-22-28e9896` ICRC-001..005). Two pieces:
//
//   1. `transfer_idempotent` / `transfer_from_idempotent` set a deterministic
//      `created_at_time` (derived from `op_nonce`) so the ledger can
//      deduplicate retries, treat `Duplicate { duplicate_of }` as success
//      (the previous attempt landed at that block index — not a failure),
//      and refresh the fee cache on `BadFee`.
//
//   2. `cached_fee_for` / `refresh_fee_cache` give callers a fast read of
//      the most recent ledger fee with a 10-minute TTL. The cache updates
//      automatically when an idempotent transfer comes back BadFee.

const FEE_CACHE_TTL_NS: u64 = 600_000_000_000; // 10 minutes

thread_local! {
    /// `ledger -> (fee, last_refresh_ns)`. Populated by `refresh_fee_cache`,
    /// invalidated on `BadFee`, and queried by callers that need to size a
    /// transfer (e.g., subtract the fee from the gross amount before sending).
    static LEDGER_FEE_CACHE: RefCell<BTreeMap<Principal, (u64, u64)>> =
        RefCell::new(BTreeMap::new());
}

/// Extract the `created_at_time` (nanoseconds since UNIX epoch) embedded in a
/// nonce produced by `crate::state::next_op_nonce`. The upper 64 bits hold
/// the timestamp captured at first issuance; the lower 64 bits hold a
/// monotonic counter for collision resistance.
pub fn nonce_to_created_at_time(op_nonce: u128) -> u64 {
    (op_nonce >> 64) as u64
}

/// Encode an `op_nonce` as a 16-byte big-endian memo. Useful for explorer
/// correlation and as a tie-breaker in the dedup tuple.
pub fn nonce_to_memo(op_nonce: u128) -> Memo {
    Memo::from(op_nonce.to_be_bytes().to_vec())
}

/// Read the cached fee for a ledger if it is fresh; otherwise return None.
pub fn cached_fee_for(ledger: Principal) -> Option<u64> {
    let now = ic_cdk::api::time();
    LEDGER_FEE_CACHE.with(|c| {
        let cache = c.borrow();
        cache.get(&ledger).and_then(|(fee, ts)| {
            if now.saturating_sub(*ts) < FEE_CACHE_TTL_NS {
                Some(*fee)
            } else {
                None
            }
        })
    })
}

/// Force-set the cache for a ledger (used internally on BadFee and by tests).
pub fn set_cached_fee(ledger: Principal, fee: u64) {
    LEDGER_FEE_CACHE.with(|c| {
        c.borrow_mut().insert(ledger, (fee, ic_cdk::api::time()));
    });
}

/// Query `icrc1_fee()` and update the cache. Returns the freshly-fetched fee.
pub async fn refresh_fee_cache(ledger: Principal) -> Result<u64, String> {
    let fee = get_ledger_fee(ledger).await?;
    set_cached_fee(ledger, fee);
    Ok(fee)
}

/// Convenience: return the cached fee if fresh, else fetch and cache it.
pub async fn get_or_refresh_fee(ledger: Principal) -> Result<u64, String> {
    if let Some(fee) = cached_fee_for(ledger) {
        return Ok(fee);
    }
    refresh_fee_cache(ledger).await
}

/// Ledger balance floor for the V2 3USD default account. Only proof-keyed
/// absorbed backing and durable refund reservations count as obligations.
pub fn three_usd_default_account_required_balance(ledger: Principal) -> Option<u128> {
    crate::state::read_state(|s| {
        // Every V2 refund row must still be backed by its parent ingress
        // journal. A missing or mismatched journal makes the balance floor
        // unprovable; never interpret that orphan as free liquidity.
        let refunds_consistent = s.pending_3usd_refunds.values()
            .filter(|refund| refund.ledger == ledger
                && refund.source == crate::state::ThreeUsdRefundSource::DefaultAccount
                && refund.parent_absorb_id.is_some())
            .all(|refund| {
                let key = crate::state::ThreeUsdReserveIngressKey {
                    stability_pool: refund.stability_pool,
                    vault_id: refund.vault_id,
                    absorb_id: refund.parent_absorb_id.unwrap_or_default(),
                };
                s.three_usd_reserve_ingress_journals.get(&key).is_some_and(|journal| {
                    journal.request.ledger == ledger
                        && journal.protocol_refund_fee_reserve_e8s == refund.amount_e8s
                        && journal.refund.as_ref().is_some_and(|child| {
                            child.op_nonce == refund.op_nonce
                                && child.required_net_credit_e8s == refund.amount_e8s
                        })
                })
            });
        if !refunds_consistent {
            return None;
        }
        let backing = s.sp_three_usd_reserve_absorb_results_by_proof.values()
            .filter(|stored| stored.ledger == ledger
                && stored.proof.ledger_kind == crate::icrc3_proof::SpProofLedger::ThreePoolTransferDefault)
            .try_fold(0u128, |sum, stored| {
                let amount = if stored.icusd_debt_covered_e8s == 0
                    || stored.result.liquidated_debt >= stored.icusd_debt_covered_e8s {
                    u128::from(stored.three_usd_amount_e8s)
                } else {
                    u128::from(stored.three_usd_amount_e8s)
                        .checked_mul(u128::from(stored.result.liquidated_debt))?
                        .checked_div(u128::from(stored.icusd_debt_covered_e8s))?
                };
                sum.checked_add(amount)
            })?;
        let reservations = s.three_usd_reserve_ingress_journals.values()
            .filter(|journal| journal.request.ledger == ledger)
            .try_fold(0u128, |sum, journal| {
                sum.checked_add(u128::from(journal.protocol_refund_fee_reserve_e8s))
            })?;
        backing.checked_add(reservations)
    })
}

/// Idempotent ICRC-1 transfer.
///
/// `op_nonce` MUST be stable across retries of the same logical operation
/// (mint via `crate::state::next_op_nonce` once, persist alongside the
/// pending record, reuse on every retry). The `created_at_time` is derived
/// from `op_nonce` so the ledger's dedup tuple matches across retries.
///
/// Behaviour:
///   * `Ok(block)` — transfer landed at `block`.
///   * `Err(TransferError::Duplicate { duplicate_of })` is converted to
///     `Ok(duplicate_of)` — the previous attempt already landed at that
///     block, the operation succeeded (audit ICRC-003).
///   * `Err(TransferError::BadFee { expected_fee })` updates the fee cache
///     for `ledger` and propagates the error so the caller can retry with
///     the fresh fee (audit ICRC-005).
pub async fn transfer_idempotent(
    ledger: Principal,
    from_subaccount: Option<[u8; 32]>,
    to: Account,
    amount: u128,
    op_nonce: u128,
    memo: Option<Memo>,
) -> Result<u64, TransferError> {
    let _default_account_guard = try_acquire_three_usd_default_account_guard(ledger, from_subaccount)?;
    let created_at_time = nonce_to_created_at_time(op_nonce);
    let memo = memo.unwrap_or_else(|| nonce_to_memo(op_nonce));

    let client = ICRC1Client {
        runtime: CdkRuntime,
        ledger_canister_id: ledger,
    };
    let outer = client
        .transfer(TransferArg {
            from_subaccount,
            to,
            fee: None,
            created_at_time: Some(created_at_time),
            memo: Some(memo),
            amount: Nat::from(amount),
        })
        .await;

    handle_transfer_outcome(ledger, outer)
}

/// Idempotent transfer with an explicit, durable fee pin. Use for protocol
/// refunds whose fee is covered by a bounded capital reservation; BadFee is a
/// no-effect response and the caller must hold/reconcile before changing it.
pub async fn transfer_idempotent_pinned_fee(
    ledger: Principal,
    from_subaccount: Option<[u8; 32]>,
    to: Account,
    amount: u128,
    op_nonce: u128,
    fee_e8s: u64,
) -> Result<u64, TransferError> {
    let created_at_time = nonce_to_created_at_time(op_nonce);
    let memo = nonce_to_memo(op_nonce);
    let client = ICRC1Client { runtime: CdkRuntime, ledger_canister_id: ledger };
    let outer = client.transfer(TransferArg {
        from_subaccount,
        to,
        fee: Some(Nat::from(fee_e8s)),
        created_at_time: Some(created_at_time),
        memo: Some(memo),
        amount: Nat::from(amount),
    }).await;
    handle_transfer_outcome(ledger, outer)
}

/// Idempotent ICRC-2 transfer_from. Same semantics as `transfer_idempotent`
/// but for pull-based transfers (pre-approved spend).
pub async fn transfer_from_idempotent(
    ledger: Principal,
    from: Account,
    to: Account,
    amount: u128,
    op_nonce: u128,
    memo: Option<Memo>,
) -> Result<u64, TransferFromError> {
    let created_at_time = nonce_to_created_at_time(op_nonce);
    let memo = memo.unwrap_or_else(|| nonce_to_memo(op_nonce));

    let client = ICRC1Client {
        runtime: CdkRuntime,
        ledger_canister_id: ledger,
    };
    let outer = client
        .transfer_from(TransferFromArgs {
            spender_subaccount: None,
            from,
            to,
            amount: Nat::from(amount),
            fee: None,
            created_at_time: Some(created_at_time),
            memo: Some(memo),
        })
        .await;

    handle_transfer_from_outcome(ledger, outer)
}

/// Submit the exact ICRC-2 arguments durably recorded by a V2 reserve ingress.
/// Callers must persist this tuple before invoking the helper and pass the same
/// value on every retry.
pub async fn transfer_from_three_usd_ingress_tuple(
    ledger: Principal,
    tuple: &crate::state::ThreeUsdReserveIngressTuple,
) -> Result<u64, TransferFromError> {
    use icrc_ledger_types::icrc2::transfer_from::TransferFromArgs;
    let client = ICRC1Client {
        runtime: CdkRuntime,
        ledger_canister_id: ledger,
    };
    let outer = client
        .transfer_from(TransferFromArgs {
            spender_subaccount: tuple.spender_subaccount,
            from: tuple.source.clone(),
            to: tuple.destination.clone(),
            amount: Nat::from(tuple.amount_e8s),
            fee: tuple.fee_e8s.map(Nat::from),
            created_at_time: Some(tuple.created_at_time_ns),
            memo: Some(Memo(tuple.memo.to_vec().into())),
        })
        .await;
    handle_transfer_from_outcome(ledger, outer)
}

pub fn three_usd_reserve_ingress_journal(
    key: &crate::state::ThreeUsdReserveIngressKey,
) -> Option<crate::state::ThreeUsdReserveIngressJournal> {
    crate::state::read_state(|state| state.three_usd_reserve_ingress_journals.get(key).cloned())
}

thread_local! {
    static THREE_USD_INGRESS_IN_FLIGHT:
        std::cell::RefCell<std::collections::BTreeSet<crate::state::ThreeUsdReserveIngressKey>> =
        std::cell::RefCell::new(std::collections::BTreeSet::new());
    static THREE_USD_INGRESS_ADMISSION_IN_FLIGHT: std::cell::Cell<bool> = std::cell::Cell::new(false);
}

/// Serializes V2 pre-pull liquidity checks so two new ingresses cannot both
/// consume the same protocol-funded refund-fee buffer across inter-canister awaits.
pub struct ThreeUsdReserveIngressAdmissionGuard;

impl ThreeUsdReserveIngressAdmissionGuard {
    pub fn try_acquire() -> Option<Self> {
        THREE_USD_INGRESS_ADMISSION_IN_FLIGHT.with(|active| {
            if active.replace(true) { None } else { Some(Self) }
        })
    }
}

impl Drop for ThreeUsdReserveIngressAdmissionGuard {
    fn drop(&mut self) {
        THREE_USD_INGRESS_ADMISSION_IN_FLIGHT.with(|active| active.set(false));
    }
}

fn try_acquire_three_usd_default_account_guard(
    ledger: Principal,
    from_subaccount: Option<[u8; 32]>,
) -> Result<Option<ThreeUsdReserveIngressAdmissionGuard>, TransferError> {
    let protects_three_usd_default = from_subaccount.is_none()
        && crate::state::read_state(|s| s.three_pool_canister == Some(ledger));
    if protects_three_usd_default {
        ThreeUsdReserveIngressAdmissionGuard::try_acquire()
            .map(Some)
            .ok_or_else(|| TransferError::GenericError {
                error_code: Nat::from(0u8),
                message: "3USD default-account transfer is held by an active reserve admission/refund".into(),
            })
    } else {
        Ok(None)
    }
}

pub struct ThreeUsdReserveIngressGuard(crate::state::ThreeUsdReserveIngressKey);

impl ThreeUsdReserveIngressGuard {
    pub fn try_acquire(key: &crate::state::ThreeUsdReserveIngressKey) -> Option<Self> {
        THREE_USD_INGRESS_IN_FLIGHT.with(|keys| {
            keys.borrow_mut().insert(key.clone()).then(|| Self(key.clone()))
        })
    }
}

impl Drop for ThreeUsdReserveIngressGuard {
    fn drop(&mut self) {
        THREE_USD_INGRESS_IN_FLIGHT.with(|keys| {
            keys.borrow_mut().remove(&self.0);
        });
    }
}

fn handle_transfer_outcome(
    ledger: Principal,
    outer: Result<Result<Nat, TransferError>, (i32, String)>,
) -> Result<u64, TransferError> {
    match outer {
        Ok(Ok(block)) => Ok(block.0.to_u64().unwrap_or(0)),
        Ok(Err(TransferError::Duplicate { duplicate_of })) => {
            let block = duplicate_of.0.to_u64().unwrap_or(0);
            log!(DEBUG,
                "[transfer_idempotent] ledger {} reported Duplicate; treating as success (block {})",
                ledger, block
            );
            Ok(block)
        }
        Ok(Err(TransferError::BadFee { expected_fee })) => {
            let fee = expected_fee.0.to_u64().unwrap_or(0);
            log!(DEBUG,
                "[transfer_idempotent] ledger {} returned BadFee (expected {}), refreshing cache",
                ledger, fee
            );
            set_cached_fee(ledger, fee);
            Err(TransferError::BadFee { expected_fee })
        }
        Ok(Err(other)) => Err(other),
        Err((code, msg)) => Err(TransferError::GenericError {
            error_code: Nat::from(code.max(0) as u64),
            message: msg,
        }),
    }
}

fn handle_transfer_from_outcome(
    ledger: Principal,
    outer: Result<Result<Nat, TransferFromError>, (i32, String)>,
) -> Result<u64, TransferFromError> {
    match outer {
        Ok(Ok(block)) => Ok(block.0.to_u64().unwrap_or(0)),
        Ok(Err(TransferFromError::Duplicate { duplicate_of })) => {
            let block = duplicate_of.0.to_u64().unwrap_or(0);
            log!(DEBUG,
                "[transfer_from_idempotent] ledger {} reported Duplicate; treating as success (block {})",
                ledger, block
            );
            Ok(block)
        }
        Ok(Err(TransferFromError::BadFee { expected_fee })) => {
            let fee = expected_fee.0.to_u64().unwrap_or(0);
            log!(DEBUG,
                "[transfer_from_idempotent] ledger {} returned BadFee (expected {}), refreshing cache",
                ledger, fee
            );
            set_cached_fee(ledger, fee);
            Err(TransferFromError::BadFee { expected_fee })
        }
        Ok(Err(other)) => Err(other),
        Err((code, msg)) => Err(TransferFromError::GenericError {
            error_code: Nat::from(code.max(0) as u64),
            message: msg,
        }),
    }
}

/// Represents an error from a management canister call
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallError {
    method: String,
    reason: Reason,
}

impl CallError {
    /// Returns the name of the method that resulted in this error.
    pub fn method(&self) -> &str {
        &self.method
    }

    /// Returns the failure reason.
    pub fn reason(&self) -> &Reason {
        &self.reason
    }
}

impl fmt::Display for CallError {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            fmt,
            "management call '{}' failed: {}",
            self.method, self.reason
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// The reason for the management call failure.
pub enum Reason {
    /// Failed to send a signature request because the local output queue is
    /// full.
    QueueIsFull,
    /// The canister does not have enough cycles to submit the request.
    OutOfCycles,
    /// The call failed with an error.
    CanisterError(String),
    /// The management canister rejected the signature request (not enough
    /// cycles, the ECDSA subnet is overloaded, etc.).
    Rejected(String),
}

impl fmt::Display for Reason {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::QueueIsFull => write!(fmt, "the canister queue is full"),
            Self::OutOfCycles => write!(fmt, "the canister is out of cycles"),
            Self::CanisterError(msg) => write!(fmt, "canister error: {}", msg),
            Self::Rejected(msg) => {
                write!(fmt, "the management canister rejected the call: {}", msg)
            }
        }
    }
}

/// Query the XRC canister to retrieve the last BTC/USD price.
/// https://github.com/dfinity/exchange-rate-canister
pub async fn fetch_icp_price() -> Result<GetExchangeRateResult, String> {
    const XRC_CALL_COST_CYCLES: u64 = 1_000_000_000;
    const XRC_MARGIN_SEC: u64 = 60;

    let icp = Asset {
        symbol: "ICP".to_string(),
        class: AssetClass::Cryptocurrency,
    };
    let usd = Asset {
        symbol: "USD".to_string(), 
        class: AssetClass::FiatCurrency,
    };

    let timestamp_sec = ic_cdk::api::time() / crate::SEC_NANOS - XRC_MARGIN_SEC;

    let args = GetExchangeRateRequest {
        base_asset: icp,
        quote_asset: usd,
        timestamp: Some(timestamp_sec),
    };

    let xrc_principal = read_state(|s| s.xrc_principal);

    let res_xrc: Result<(GetExchangeRateResult,), _> = ic_cdk::api::call::call_with_payment(
        xrc_principal,
        "get_exchange_rate",
        (args.clone(),),  // Clone args for logging
        XRC_CALL_COST_CYCLES,
    )
    .await;

    // Add detailed logging
    match &res_xrc {
        Ok((xr,)) => {
            log!(DEBUG, "[fetch_icp_price] XRC request args: {:?}", args);
            log!(DEBUG, "[fetch_icp_price] XRC response: {:?}", xr);
            Ok(xr.clone())
        }
        Err((code, msg)) => {
            log!(DEBUG, "[fetch_icp_price] XRC request args: {:?}", args);
            log!(DEBUG, "[fetch_icp_price] XRC error code: {:?}, message: {}", code, msg);  // Changed to {:?}
            Err(format!(
                "Error while calling XRC canister ({:?}): {:?}",  // Changed to {:?}
                code, msg
            ))
        }
    }
}

/// Fetch USDT/USD or USDC/USD price from the XRC canister.
/// Used on-demand for depeg protection on ckstable operations.
pub async fn fetch_stable_price(symbol: &str) -> Result<GetExchangeRateResult, String> {
    const XRC_CALL_COST_CYCLES: u64 = 1_000_000_000;
    const XRC_MARGIN_SEC: u64 = 60;

    let stable = Asset {
        symbol: symbol.to_string(),
        class: AssetClass::Cryptocurrency,
    };
    let usd = Asset {
        symbol: "USD".to_string(),
        class: AssetClass::FiatCurrency,
    };

    let timestamp_sec = ic_cdk::api::time() / crate::SEC_NANOS - XRC_MARGIN_SEC;

    let args = GetExchangeRateRequest {
        base_asset: stable,
        quote_asset: usd,
        timestamp: Some(timestamp_sec),
    };

    let xrc_principal = read_state(|s| s.xrc_principal);

    let res_xrc: Result<(GetExchangeRateResult,), _> = ic_cdk::api::call::call_with_payment(
        xrc_principal,
        "get_exchange_rate",
        (args.clone(),),
        XRC_CALL_COST_CYCLES,
    )
    .await;

    match &res_xrc {
        Ok((xr,)) => {
            log!(DEBUG, "[fetch_stable_price] XRC request for {}: {:?}", symbol, args);
            log!(DEBUG, "[fetch_stable_price] XRC response: {:?}", xr);
            Ok(xr.clone())
        }
        Err((code, msg)) => {
            log!(DEBUG, "[fetch_stable_price] XRC error for {}: {:?}, message: {}", symbol, code, msg);
            Err(format!(
                "Error fetching {} price from XRC ({:?}): {:?}",
                symbol, code, msg
            ))
        }
    }
}

/// Minimal subset of WaterNeuron's CanisterInfo response.
/// Candid deserialization ignores unknown fields, so we only define what we need.
#[derive(candid::CandidType, serde::Deserialize)]
struct LstCanisterInfo {
    exchange_rate: u64,
}

/// Wave-14a follow-up: compute the final price for an `LstWrapped`
/// collateral from already-fetched inputs. Pure function — does not call
/// XRC or the LST canister.
///
/// Previously, `fetch_collateral_price` issued its own XRC
/// `get_exchange_rate` call for the LST's `base_asset` (e.g. ICP/USD for
/// nICP) even though `xrc::fetch_icp_rate` had just cached the same rate.
/// That duplicated the ~1B-cycle XRC round-trip per LST collateral per
/// refresh tick and also doubled every CDP-14 source-count rejection
/// event (one tagged ICP, a paired one tagged nICP). The duplicated call
/// produced no extra information: the price of nICP is mechanically
/// derived from the ICP price, WaterNeuron's redemption rate, and the
/// configured haircut.
///
/// Returns `None` if any input would yield a non-positive / non-finite
/// price:
///   * `wn_exchange_rate == 0` (LST canister unhealthy or not yet
///     initialized — refuse to publish a price; cached value stays in
///     place).
///   * `haircut < 0` or `haircut >= 1` (misconfiguration).
///   * `underlying_rate <= 0` (no upstream price available).
///
/// `wn_exchange_rate` is the LST canister's `exchange_rate` field, scaled
/// by E8S. For WaterNeuron's nICP this represents "nICP minted per ICP
/// staked", so the multiplier `E8S / wn_exchange_rate` converts an
/// underlying ICP price into the equivalent nICP price (1 nICP redeems
/// for `1 / multiplier` ICP).
pub fn compute_lst_wrapped_price(
    underlying_rate: rust_decimal::Decimal,
    wn_exchange_rate: u64,
    haircut: f64,
) -> Option<rust_decimal::Decimal> {
    use rust_decimal::prelude::FromPrimitive;
    use rust_decimal::Decimal;

    if wn_exchange_rate == 0 {
        return None;
    }
    if underlying_rate <= Decimal::ZERO {
        return None;
    }
    let haircut_dec = Decimal::from_f64(haircut)?;
    if haircut_dec < Decimal::ZERO || haircut_dec >= Decimal::ONE {
        return None;
    }
    let multiplier = Decimal::from(crate::E8S) / Decimal::from(wn_exchange_rate);
    let adjusted = underlying_rate * multiplier * (Decimal::ONE - haircut_dec);
    if adjusted <= Decimal::ZERO {
        return None;
    }
    Some(adjusted)
}

/// Refresh one LstWrapped price against a specific accepted ICP source timestamp.
/// Every timer, on-demand caller, and ICP-coupled caller passes through this
/// shared in-flight guard and the same post-await source/timestamp checks.
pub(crate) async fn refresh_lst_wrapped_price_for_icp_timestamp(
    collateral_type: Principal,
    expected_icp_timestamp_ns: u64,
) -> LstPriceRefreshOutcome {
    refresh_lst_with_one_catch_up(
        collateral_type,
        expected_icp_timestamp_ns,
        refresh_lst_wrapped_price_once,
        || read_state(|state| state.last_icp_timestamp),
    )
    .await
}

async fn refresh_lst_wrapped_price_once(
    collateral_type: Principal,
    expected_icp_timestamp_ns: u64,
) -> LstPriceRefreshOutcome {
    use crate::state::{mutate_state, PriceSource};
    use ic_canister_log::log;
    use crate::logs::TRACE_XRC;
    use rust_decimal::prelude::FromPrimitive;

    let Some(_guard) = LstPriceFetchGuard::try_acquire(collateral_type) else {
        return LstPriceRefreshOutcome::InFlight;
    };

    let snapshot = read_state(|state| {
        let Some(config) = state.get_collateral_config(&collateral_type) else {
            return Err(LstPriceRefreshOutcome::NotLstWrapped);
        };
        let icp_rate = state.last_icp_rate.map(|rate| rate.0);
        lst_refresh_preflight(
            state.last_icp_timestamp,
            expected_icp_timestamp_ns,
            icp_rate.is_some(),
            config.last_price_timestamp,
        )?;
        let icp_rate = icp_rate.ok_or(LstPriceRefreshOutcome::NoIcpPrice)?;
        let source = config.price_source.clone();
        let PriceSource::LstWrapped {
            base_asset,
            rate_canister_id,
            rate_method,
            haircut,
            ..
        } = &source
        else {
            return Err(LstPriceRefreshOutcome::NotLstWrapped);
        };
        if base_asset != "ICP" {
            return Err(LstPriceRefreshOutcome::UnsupportedUnderlying);
        }
        Ok((icp_rate, source.clone(), *rate_canister_id, rate_method.clone(), *haircut))
    });
    let (icp_rate, source, rate_canister_id, rate_method, haircut) = match snapshot {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };

    let rate_result: Result<(LstCanisterInfo,), _> =
        ic_cdk::call(rate_canister_id, rate_method.as_str(), ()).await;
    let info = match rate_result {
        Ok((info,)) => info,
        Err((code, msg)) => {
            log!(
                TRACE_XRC,
                "[fetch_collateral_price] LstWrapped rate canister error for {}: {:?} {}",
                collateral_type,
                code,
                msg
            );
            return LstPriceRefreshOutcome::RateCallFailed;
        }
    };

    let Some(final_rate) = compute_lst_wrapped_price(icp_rate, info.exchange_rate, haircut) else {
        log!(
            TRACE_XRC,
            "[fetch_collateral_price] LstWrapped {}: compute returned None (underlying={}, wn_rate={}, haircut={})",
            collateral_type,
            icp_rate,
            info.exchange_rate,
            haircut
        );
        return LstPriceRefreshOutcome::InvalidRate;
    };
    use rust_decimal::prelude::ToPrimitive;
    let Some(final_rate_f64) = final_rate.to_f64().filter(|value| value.is_finite() && *value > 0.0) else {
        return LstPriceRefreshOutcome::InvalidRate;
    };

    // Revalidate the exact underlying sample and config in the same synchronous
    // state mutation that publishes. A delayed callback can never overwrite a
    // newer ICP observation or a changed LST price-source configuration.
    let published = mutate_state(|state| {
        let Some(config) = state.get_collateral_config(&collateral_type) else {
            return false;
        };
        if lst_publication_preflight(
            state.last_icp_timestamp,
            expected_icp_timestamp_ns,
            config.price_source == source,
            config.last_price_timestamp,
        )
        .is_err()
            || !crate::xrc::source_timestamp_is_fresh(
                expected_icp_timestamp_ns,
                ic_cdk::api::time(),
            )
        {
            return false;
        }
        if !crate::xrc::check_price_sanity_band_at_source(
            state,
            &collateral_type,
            Some(&source),
            expected_icp_timestamp_ns,
            final_rate_f64,
        ) {
            return false;
        }
        state.on_collateral_price_change(&collateral_type, final_rate_f64);
        if let Some(config) = state.collateral_configs.get_mut(&collateral_type) {
            config.last_price_timestamp = Some(expected_icp_timestamp_ns);
        }
        true
    });
    if !published {
        return if read_state(|state| {
            state.last_icp_timestamp != Some(expected_icp_timestamp_ns)
        }) {
            LstPriceRefreshOutcome::IcpTimestampChanged
        } else if read_state(|state| {
            state
                .get_collateral_config(&collateral_type)
                .is_some_and(|config| config.price_source != source)
        }) {
            LstPriceRefreshOutcome::ConfigurationChanged
        } else if read_state(|state| {
            state
                .get_collateral_config(&collateral_type)
                .and_then(|config| config.last_price_timestamp)
                .is_some_and(|timestamp| timestamp >= expected_icp_timestamp_ns)
        }) {
            LstPriceRefreshOutcome::AlreadyCurrent
        } else {
            LstPriceRefreshOutcome::SanityRejected
        };
    }

    crate::event::record_price_update(
        collateral_type,
        final_rate,
        expected_icp_timestamp_ns,
    );
    LstPriceRefreshOutcome::Published
}

/// Generic price fetch for any collateral type using its PriceSource config.
/// Routes to XRC, CoinGecko HTTPS outcall, or LstWrapped depending on config.
pub async fn fetch_collateral_price(collateral_type: Principal) {
    use crate::state::{mutate_state, PriceSource, XrcAssetClass};
    use ic_canister_log::log;
    use crate::logs::TRACE_XRC;
    use rust_decimal::prelude::FromPrimitive;

    let price_source = read_state(|s| {
        s.get_collateral_config(&collateral_type).map(|c| c.price_source.clone())
    });

    let price_source = match price_source {
        Some(ps) => ps,
        None => {
            log!(TRACE_XRC, "[fetch_collateral_price] No config for {}", collateral_type);
            return;
        }
    };

    // LstWrapped prices derive from the cached ICP sample. The shared helper
    // skips already-current samples before making the LST rate-canister call.
    if matches!(&price_source, PriceSource::LstWrapped { .. }) {
        if let Some(timestamp_ns) = read_state(|s| s.last_icp_timestamp) {
            let outcome = refresh_lst_wrapped_price_for_icp_timestamp(collateral_type, timestamp_ns).await;
            if !matches!(outcome, LstPriceRefreshOutcome::Published | LstPriceRefreshOutcome::AlreadyCurrent) {
                log!(
                    TRACE_XRC,
                    "[fetch_collateral_price] LstWrapped refresh for {} did not publish: {:?}",
                    collateral_type,
                    outcome
                );
            }
        } else {
            log!(
                TRACE_XRC,
                "[fetch_collateral_price] LstWrapped {} has no cached ICP price yet",
                collateral_type
            );
        }
        return;
    }

    let now_ns = ic_cdk::api::time();
    let Some(_paid_refresh_lease) = crate::xrc::try_acquire_collateral_price_refresh(
        collateral_type,
        &price_source,
        now_ns,
    ) else {
        log!(
            TRACE_XRC,
            "[fetch_collateral_price] suppressing duplicate paid refresh for {} in this source bar",
            collateral_type
        );
        return;
    };

    // CoinGecko variant uses HTTPS outcalls — completely separate path from XRC
    if let PriceSource::CoinGecko { ref coin_id, ref vs_currency } = price_source {
        let result = fetch_coingecko_price(coin_id, vs_currency).await;
        match result {
            Some(sample) => {
                let now_ns = ic_cdk::api::time();
                if !crate::xrc::source_timestamp_is_fresh(sample.updated_at_ns, now_ns) {
                    log!(
                        TRACE_XRC,
                        "[fetch_collateral_price] CoinGecko {} source timestamp {} is stale or future",
                        coin_id,
                        sample.updated_at_ns
                    );
                    return;
                }
                let ts_nanos = sample.updated_at_ns;
                log!(
                    TRACE_XRC,
                    "[fetch_collateral_price] CoinGecko {} price: {} at {}",
                    coin_id, sample.price, ts_nanos
                );
                // Wave-5 LIQ-007: gate every accepted price through the sanity band
                // (rejects single outliers, accepts after N consecutive confirmations).
                // Fresh samples must reach this gate even if the local cache is
                // younger than its reuse window: outlier confirmations require
                // source observations spaced at least five minutes apart.
                let accepted = mutate_state(|s| {
                    if !crate::xrc::accept_price_at_source(
                        s,
                        &collateral_type,
                        Some(&price_source),
                        ts_nanos,
                        sample.price,
                        ic_cdk::api::time(),
                    ) {
                        return false;
                    }
                    s.on_collateral_price_change(&collateral_type, sample.price);
                    if let Some(config) = s.collateral_configs.get_mut(&collateral_type) {
                        config.last_price_timestamp = Some(ts_nanos);
                    }
                    if let Some(price_dec) = rust_decimal::Decimal::from_f64(sample.price) {
                        crate::event::record_price_update(collateral_type, price_dec, ts_nanos);
                    }
                    true
                });
                if !accepted {
                    log!(
                        TRACE_XRC,
                        "[fetch_collateral_price] rejecting outlier CoinGecko price {} for {}; awaiting confirmation",
                        sample.price, coin_id
                    );
                    return;
                }
            }
            None => {
                log!(TRACE_XRC, "[fetch_collateral_price] CoinGecko failed for {}", coin_id);
            }
        }
        return;
    }

    // XRC-based path (only the `Xrc` variant reaches here now —
    // `LstWrapped` is handled above without an XRC call, and `CoinGecko`
    // is handled in its own branch).
    const XRC_CALL_COST_CYCLES: u64 = 1_000_000_000;
    const XRC_MARGIN_SEC: u64 = 60;

    let (base_asset, base_asset_class, quote_asset, quote_asset_class) = match &price_source {
        PriceSource::Xrc { base_asset, base_asset_class, quote_asset, quote_asset_class } => {
            (base_asset.clone(), base_asset_class.clone(), quote_asset.clone(), quote_asset_class.clone())
        }
        PriceSource::LstWrapped { .. } => unreachable!(), // handled above
        PriceSource::CoinGecko { .. } => unreachable!(), // handled above
    };

    let base = Asset {
        symbol: base_asset.clone(),
        class: match base_asset_class {
            XrcAssetClass::Cryptocurrency => AssetClass::Cryptocurrency,
            XrcAssetClass::FiatCurrency => AssetClass::FiatCurrency,
        },
    };
    let quote = Asset {
        symbol: quote_asset.clone(),
        class: match quote_asset_class {
            XrcAssetClass::Cryptocurrency => AssetClass::Cryptocurrency,
            XrcAssetClass::FiatCurrency => AssetClass::FiatCurrency,
        },
    };

    let timestamp_sec = ic_cdk::api::time() / crate::SEC_NANOS - XRC_MARGIN_SEC;

    let args = GetExchangeRateRequest {
        base_asset: base,
        quote_asset: quote,
        timestamp: Some(timestamp_sec),
    };

    let xrc_principal = read_state(|s| s.xrc_principal);

    let res_xrc: Result<(GetExchangeRateResult,), _> = ic_cdk::api::call::call_with_payment(
        xrc_principal,
        "get_exchange_rate",
        (args.clone(),),
        XRC_CALL_COST_CYCLES,
    )
    .await;

    let underlying_rate = match res_xrc {
        Ok((GetExchangeRateResult::Ok(exchange_rate_result),)) => {
            // Wave-14a CDP-14: source-floor gate for non-ICP collaterals.
            // Same rationale as the ICP path: a thin aggregation is cheaper
            // to manipulate. Emits OracleSourceCountInsufficient and skips
            // the price update; cached price stays in place. The CDP-01
            // consecutive-failure counter is intentionally NOT mutated here
            // (it tracks ICP-only). Per-collateral oracle health is observed
            // via the event count rather than a global circuit breaker.
            let num_sources =
                exchange_rate_result.metadata.base_asset_num_received_rates as u32;
            // Wave-14a CDP-14 follow-up: resolve the per-collateral
            // override (defaults to the global floor when unset). For
            // assets with genuinely thin CEX coverage (e.g. XAUT, which
            // only trades on a handful of exchanges), an admin can drop
            // the floor to 2 via `set_collateral_min_xrc_sources`
            // without weakening the gate for other collaterals.
            let floor = read_state(|s| {
                s.get_collateral_config(&collateral_type)
                    .map(|c| c.effective_min_xrc_sources(s.min_xrc_sources_used))
                    .unwrap_or(s.min_xrc_sources_used)
            });
            if !crate::xrc::xrc_metadata_meets_source_floor(num_sources, floor) {
                log!(
                    TRACE_XRC,
                    "[fetch_collateral_price] rejecting {} rate {}: only {} XRC sources (floor {})",
                    base_asset,
                    exchange_rate_result.rate,
                    num_sources,
                    floor
                );
                crate::storage::record_event(&crate::event::Event::OracleSourceCountInsufficient {
                    collateral_type,
                    num_sources,
                    min_required: floor,
                    timestamp: ic_cdk::api::time(),
                });
                None
            } else {
                let Some(rate) = crate::xrc::xrc_rate_to_decimal(
                    exchange_rate_result.rate,
                    exchange_rate_result.metadata.decimals,
                ) else {
                    log!(
                        TRACE_XRC,
                        "[fetch_collateral_price] rejecting invalid rate/decimals for {}: rate={} decimals={}",
                        base_asset,
                        exchange_rate_result.rate,
                        exchange_rate_result.metadata.decimals
                    );
                    return;
                };

                log!(
                    TRACE_XRC,
                    "[fetch_collateral_price] {} rate: {} at timestamp: {}",
                    base_asset, rate, exchange_rate_result.timestamp
                );

                let Some(ts_nanos) = crate::xrc::xrc_timestamp_secs_to_ns(
                    exchange_rate_result.timestamp,
                )
                .filter(|ts| crate::xrc::source_timestamp_is_fresh(*ts, ic_cdk::api::time()))
                else {
                    log!(
                        TRACE_XRC,
                        "[fetch_collateral_price] rejecting stale, future, or overflowing {} timestamp {}",
                        base_asset,
                        exchange_rate_result.timestamp
                    );
                    return;
                };

                Some((rate, ts_nanos))
            }
        }
        Ok((GetExchangeRateResult::Err(error),)) => {
            log!(TRACE_XRC, "[fetch_collateral_price] XRC error for {}: {:?}", base_asset, error);
            None
        }
        Err((code, msg)) => {
            log!(TRACE_XRC, "[fetch_collateral_price] Call error for {}: {:?} {}", base_asset, code, msg);
            None
        }
    };

    let Some((rate, ts_nanos)) = underlying_rate else { return };

    // Only the plain `Xrc` variant reaches here — `LstWrapped` is handled
    // above without re-fetching from XRC, and `CoinGecko` has its own
    // early-return branch.
    let final_rate = rate;

    // Wave-5 LIQ-007: gate every accepted price through the sanity band.
    let final_rate_f64 = match final_rate.to_f64() {
        Some(v) if v.is_finite() && v > 0.0 => v,
        _ => {
            log!(
                TRACE_XRC,
                "[fetch_collateral_price] {}: dropping non-positive/non-finite final rate {}",
                base_asset, final_rate
            );
            return;
        }
    };
    let accepted = mutate_state(|s| {
        let Some(config) = s.get_collateral_config(&collateral_type) else {
            return false;
        };
        if config.price_source != price_source
            || config.last_price_timestamp.is_some_and(|last_ts| last_ts >= ts_nanos)
        {
            return false;
        }
        if !crate::xrc::accept_price_at_source(
            s,
            &collateral_type,
            Some(&price_source),
            ts_nanos,
            final_rate_f64,
            ic_cdk::api::time(),
        ) {
            return false;
        }
        s.on_collateral_price_change(&collateral_type, final_rate_f64);
        if let Some(config) = s.collateral_configs.get_mut(&collateral_type) {
            config.last_price_timestamp = Some(ts_nanos);
        }
        crate::event::record_price_update(collateral_type, final_rate, ts_nanos);
        true
    });
    if !accepted {
        log!(
            TRACE_XRC,
            "[fetch_collateral_price] rejecting outlier {} rate {} for {}; awaiting confirmation",
            base_asset, final_rate_f64, collateral_type
        );
        return;
    }

}

#[derive(Clone, Copy, Debug, PartialEq)]
struct CoinGeckoPriceSample {
    price: f64,
    updated_at_ns: u64,
}

fn parse_coingecko_price_sample(
    body: &str,
    coin_id: &str,
    vs_currency: &str,
) -> Option<CoinGeckoPriceSample> {
    let json: serde_json::Value = serde_json::from_str(body).ok()?;
    let coin = json.get(coin_id)?;
    let price = coin.get(vs_currency)?.as_f64()?;
    if !price.is_finite() || price <= 0.0 {
        return None;
    }
    let updated_at_ns = coin
        .get("last_updated_at")?
        .as_u64()?
        .checked_mul(crate::SEC_NANOS)?;
    Some(CoinGeckoPriceSample { price, updated_at_ns })
}

/// Old CoinGecko snapshots recorded local fetch time, while fresh samples now
/// record provider time. Invalidate only those timestamps on upgrade so the
/// next price-sensitive operation fetches a provider-timestamped sample.
pub fn invalidate_legacy_coingecko_cache_timestamps(
    state: &mut crate::state::State,
) -> usize {
    state
        .collateral_configs
        .values_mut()
        .filter_map(|config| {
            (matches!(
                &config.price_source,
                crate::state::PriceSource::CoinGecko { .. }
            ) && config.last_price_timestamp.take().is_some())
            .then_some(())
        })
        .count()
}

/// Fetch a token price and provider update time from the CoinGecko
/// simple/price API via HTTPS outcall.
async fn fetch_coingecko_price(coin_id: &str, vs_currency: &str) -> Option<CoinGeckoPriceSample> {
    use ic_cdk::api::management_canister::http_request::{
        http_request, CanisterHttpRequestArgument, HttpHeader, HttpMethod,
        TransformContext,
    };
    use ic_canister_log::log;
    use crate::logs::TRACE_XRC;

    // Response body is small (~50 bytes) but headers can be large (~2-3 KB).
    // IC counts headers + body against this limit before transform strips headers.
    const MAX_RESPONSE_BYTES: u64 = 4096;
    // HTTPS outcall cost: base 49_140_000 + 5_200 per request byte + 10_400 per response byte
    // Plus per-node scaling. 100M gives comfortable headroom for 13-node subnets.
    const OUTCALL_CYCLES: u128 = 100_000_000;

    let url = format!(
        "https://api.coingecko.com/api/v3/simple/price?ids={}&vs_currencies={}&include_last_updated_at=true",
        coin_id, vs_currency
    );

    let request = CanisterHttpRequestArgument {
        url,
        max_response_bytes: Some(MAX_RESPONSE_BYTES),
        method: HttpMethod::GET,
        headers: vec![
            HttpHeader {
                name: "Accept".to_string(),
                value: "application/json".to_string(),
            },
        ],
        body: None,
        transform: Some(TransformContext::from_name(
            "coingecko_transform".to_string(),
            vec![],
        )),
    };

    let result = http_request(request, OUTCALL_CYCLES).await;

    match result {
        Ok((response,)) => {
            let status = response.status.0.to_u64().unwrap_or(0);
            if status != 200 {
                log!(TRACE_XRC, "[coingecko] HTTP {} for {}", status, coin_id);
                return None;
            }

            let body = String::from_utf8(response.body).ok()?;
            parse_coingecko_price_sample(&body, coin_id, vs_currency)
        }
        Err((code, msg)) => {
            log!(TRACE_XRC, "[coingecko] Outcall error for {}: {:?} {}", coin_id, code, msg);
            None
        }
    }
}

#[cfg(test)]
mod coingecko_source_timestamp_tests {
    use super::{invalidate_legacy_coingecko_cache_timestamps, parse_coingecko_price_sample};
    use crate::state::{PriceSource, State};
    use crate::InitArg;
    use candid::Principal;

    #[test]
    fn parser_requires_provider_update_time_and_checks_nanosecond_conversion() {
        let sample = parse_coingecko_price_sample(
            r#"{"coin":{"usd":150.0,"last_updated_at":1700000000}}"#,
            "coin",
            "usd",
        )
        .unwrap();
        assert_eq!(sample.price, 150.0);
        assert_eq!(sample.updated_at_ns, 1_700_000_000_000_000_000);

        assert!(parse_coingecko_price_sample(r#"{"coin":{"usd":150.0}}"#, "coin", "usd").is_none());
        assert!(parse_coingecko_price_sample(
            r#"{"coin":{"usd":150.0,"last_updated_at":18446744074}}"#,
            "coin",
            "usd",
        )
        .is_none());
        assert!(parse_coingecko_price_sample(
            r#"{"coin":{"usd":0.0,"last_updated_at":1700000000}}"#,
            "coin",
            "usd",
        )
        .is_none());
    }

    #[test]
    fn upgrade_invalidates_legacy_local_cache_time_that_is_later_than_provider_time() {
        let now_ns = 1_700_000_100_000_000_000;
        let provider_timestamp_ns = now_ns - 10_000_000_000;
        let legacy_local_timestamp_ns = now_ns - 1_000_000_000;
        assert!(legacy_local_timestamp_ns > provider_timestamp_ns);

        let mut state = State::from(InitArg {
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
        let collateral = state.icp_collateral_type();
        let mut config = state.get_collateral_config(&collateral).unwrap().clone();
        config.price_source = PriceSource::CoinGecko {
            coin_id: "example-token".to_string(),
            vs_currency: "usd".to_string(),
        };
        config.last_price = Some(150.0);
        config.last_price_timestamp = Some(legacy_local_timestamp_ns);
        state.collateral_configs.insert(collateral, config);

        assert_eq!(invalidate_legacy_coingecko_cache_timestamps(&mut state), 1);
        let config = state.get_collateral_config(&collateral).unwrap();
        assert_eq!(config.last_price, Some(150.0));
        assert_eq!(config.last_price_timestamp, None);
        let source = config.price_source.clone();
        assert!(crate::xrc::accept_price_at_source(
            &mut state,
            &collateral,
            Some(&source),
            provider_timestamp_ns,
            150.0,
            now_ns,
        ));
        state.on_collateral_price_change(&collateral, 150.0);
        state
            .collateral_configs
            .get_mut(&collateral)
            .unwrap()
            .last_price_timestamp = Some(provider_timestamp_ns);
        assert_eq!(
            state
                .get_collateral_config(&collateral)
                .unwrap()
                .last_price_timestamp,
            Some(provider_timestamp_ns),
        );
    }

}

pub async fn mint_icusd(amount: ICUSD, to: Principal) -> Result<u64, TransferError> {
    let (ledger, op_nonce) = crate::state::mutate_state(|s| (s.icusd_ledger_principal, s.next_op_nonce()));
    transfer_idempotent(
        ledger,
        None,
        Account { owner: to, subaccount: None },
        amount.to_u64() as u128,
        op_nonce,
        None,
    )
    .await
}

/// Mint icUSD using the exact tuple durably reserved by a borrow operation.
/// Retries must call this with the same journal row so ledger deduplication
/// can return `Duplicate` as the original block.
pub async fn mint_icusd_with_borrow_tuple(
    tuple: &crate::state::BorrowMintTuple,
) -> Result<u64, TransferError> {
    let client = ICRC1Client {
        runtime: CdkRuntime,
        ledger_canister_id: tuple.ledger,
    };
    let outer = client
        .transfer(TransferArg {
            from_subaccount: None,
            to: Account {
                owner: tuple.destination,
                subaccount: None,
            },
            fee: None,
            created_at_time: Some(tuple.created_at_time_ns),
            memo: Some(Memo::from(tuple.memo.to_vec())),
            amount: Nat::from(tuple.amount_e8s),
        })
        .await;
    handle_borrow_mint_outcome(tuple.ledger, outer)
}

/// Submit a zero-value, domain-separated operation using the original borrow
/// timestamp. Only an inner typed TooOld response proves that this timestamp
/// has crossed the ledger's deduplication window. Every other outcome remains
/// inconclusive and must leave the borrow journal held.
pub async fn probe_borrow_mint_expiry(
    tuple: &crate::state::BorrowMintTuple,
    vault_id: u64,
    sink: Principal,
) -> Result<(), String> {
    if sink == tuple.destination {
        return Err("borrow expiry probe sink must differ from journal owner".into());
    }
    let mut memo_hash = Sha256::new();
    memo_hash.update(b"RUMI-EXP-PROBE1");
    memo_hash.update(vault_id.to_be_bytes());
    memo_hash.update(tuple.op_nonce.to_be_bytes());
    memo_hash.update(tuple.destination.as_slice());
    let memo = memo_hash.finalize().to_vec();
    let client = ICRC1Client {
        runtime: CdkRuntime,
        ledger_canister_id: tuple.ledger,
    };
    let outcome = client
        .transfer(TransferArg {
            from_subaccount: None,
            to: Account {
                owner: sink,
                subaccount: None,
            },
            fee: Some(Nat::from(0u8)),
            created_at_time: Some(tuple.created_at_time_ns),
            memo: Some(Memo::from(memo)),
            amount: Nat::from(0u8),
        })
        .await;
    classify_borrow_mint_expiry_probe_outcome(outcome)
}

fn classify_borrow_mint_expiry_probe_outcome(
    outcome: Result<Result<Nat, TransferError>, (i32, String)>,
) -> Result<(), String> {
    match outcome {
        Ok(Err(TransferError::TooOld)) => Ok(()),
        Ok(Ok(_)) => Err("zero-value borrow expiry probe succeeded; journal remains held".into()),
        Ok(Err(TransferError::Duplicate { .. })) => {
            Err("zero-value borrow expiry probe was Duplicate; journal remains held".into())
        }
        Ok(Err(error)) => Err(format!(
            "zero-value borrow expiry probe returned {error:?}; journal remains held"
        )),
        Err((code, message)) => Err(format!(
            "zero-value borrow expiry probe call rejected ({code:?}): {message}"
        )),
    }
}

pub enum DurableMintOutcome {
    Confirmed(u64),
    ConfirmedBlockOutOfRange,
    Rejected(TransferError),
}

pub async fn mint_icusd_with_tuple(
    tuple: &crate::state::BorrowMintTuple,
) -> DurableMintOutcome {
    let client = ICRC1Client { runtime: CdkRuntime, ledger_canister_id: tuple.ledger };
    let outer = client.transfer(TransferArg {
        from_subaccount: None,
        to: Account { owner: tuple.destination, subaccount: None },
        fee: None,
        created_at_time: Some(tuple.created_at_time_ns),
        memo: Some(Memo::from(tuple.memo.to_vec())),
        amount: Nat::from(tuple.amount_e8s),
    }).await;
    let confirmed = match &outer {
        Ok(Ok(block)) => Some(block),
        Ok(Err(TransferError::Duplicate { duplicate_of })) => Some(duplicate_of),
        _ => None,
    };
    if confirmed.is_some_and(|block| block.0.to_u64().is_none()) {
        return DurableMintOutcome::ConfirmedBlockOutOfRange;
    }
    match handle_transfer_outcome(tuple.ledger, outer) {
        Ok(block) => DurableMintOutcome::Confirmed(block),
        Err(error) => DurableMintOutcome::Rejected(error),
    }
}

/// Borrow debt may only be committed with a representable, exact mint block.
/// The generic transfer wrapper predates the durable borrow journal and maps
/// oversized ledger Nat indices to zero. Keep the journal unresolved instead
/// of recording a false block identity after a successful external mint.
fn handle_borrow_mint_outcome(
    ledger: Principal,
    outer: Result<Result<Nat, TransferError>, (i32, String)>,
) -> Result<u64, TransferError> {
    let confirmed_block = match &outer {
        Ok(Ok(block)) => Some(block),
        Ok(Err(TransferError::Duplicate { duplicate_of })) => Some(duplicate_of),
        _ => None,
    };
    if confirmed_block.is_some_and(|block| block.0.to_u64().is_none()) {
        return Err(TransferError::GenericError {
            error_code: Nat::from(0u8),
            message: "borrow mint was confirmed at a block index outside the journal's supported range; operator reconciliation is required".to_string(),
        });
    }
    handle_transfer_outcome(ledger, outer)
}

#[cfg(test)]
mod borrow_mint_outcome_tests {
    use super::*;

    #[test]
    fn oversized_success_and_duplicate_blocks_are_held_instead_of_recorded_as_zero() {
        let ledger = Principal::anonymous();
        let oversized = Nat::from(u128::from(u64::MAX) + 1);
        let success = handle_borrow_mint_outcome(ledger, Ok(Ok(oversized.clone())));
        let duplicate = handle_borrow_mint_outcome(
            ledger,
            Ok(Err(TransferError::Duplicate {
                duplicate_of: oversized,
            })),
        );
        assert!(matches!(success, Err(TransferError::GenericError { .. })));
        assert!(matches!(duplicate, Err(TransferError::GenericError { .. })));
        assert_eq!(
            handle_borrow_mint_outcome(ledger, Ok(Ok(Nat::from(42u64)))),
            Ok(42)
        );
    }

    #[test]
    fn expiry_probe_arms_only_on_decoded_typed_too_old() {
        assert_eq!(
            classify_borrow_mint_expiry_probe_outcome(Ok(Err(TransferError::TooOld))),
            Ok(())
        );
        assert!(classify_borrow_mint_expiry_probe_outcome(Ok(Ok(Nat::from(1u8)))).is_err());
        assert!(classify_borrow_mint_expiry_probe_outcome(Ok(Err(
            TransferError::Duplicate {
                duplicate_of: Nat::from(1u8),
            }
        )))
        .is_err());
        assert!(classify_borrow_mint_expiry_probe_outcome(Ok(Err(
            TransferError::BadFee {
                expected_fee: Nat::from(1u8),
            }
        )))
        .is_err());
        // Outer rejects include call failures and Candid decode failures.
        assert!(classify_borrow_mint_expiry_probe_outcome(Err((5, "rejected".into()))).is_err());
    }
}

pub async fn transfer_icusd_from(
    amount: ICUSD,
    caller: Principal,
) -> Result<u64, TransferFromError> {
    let (ledger, op_nonce) =
        crate::state::mutate_state(|s| (s.icusd_ledger_principal, s.next_op_nonce()));
    let protocol_id = ic_cdk::id();
    transfer_from_idempotent(
        ledger,
        Account {
            owner: caller,
            subaccount: None,
        },
        Account {
            owner: protocol_id,
            subaccount: None,
        },
        amount.to_u64() as u128,
        op_nonce,
        None,
    )
    .await
}


/// Thin wrapper around generic transfer_collateral_from for ICP. One-shot
/// callers; retry-loop callers must use `transfer_collateral_from_with_nonce`.
pub async fn transfer_icp_from(amount: ICP, caller: Principal) -> Result<u64, TransferFromError> {
    let ledger = read_state(|s| s.icp_ledger_principal);
    transfer_collateral_from(amount.to_u64(), caller, ledger).await
}

/// Thin wrapper around generic transfer_collateral for ICP. One-shot callers;
/// retry-loop callers must use `transfer_collateral_with_nonce`.
pub async fn transfer_icp(amount: ICP, to: Principal) -> Result<u64, TransferError> {
    let ledger = read_state(|s| s.icp_ledger_principal);
    transfer_collateral(amount.to_u64(), to, ledger).await
}

pub async fn transfer_icusd(amount: ICUSD, to: Principal) -> Result<u64, TransferError> {
    let (ledger, op_nonce) = crate::state::mutate_state(|s| (s.icusd_ledger_principal, s.next_op_nonce()));
    transfer_idempotent(
        ledger,
        None,
        Account { owner: to, subaccount: None },
        amount.to_u64() as u128,
        op_nonce,
        None,
    )
    .await
}

/// Idempotent icUSD transfer with a caller-supplied op_nonce. Wave-4 ICC-007:
/// used by the durable refund queue so retries reuse the same dedup tuple at
/// the icUSD ledger across canister upgrades.
pub async fn transfer_icusd_with_nonce(amount: ICUSD, to: Principal, op_nonce: u128) -> Result<u64, TransferError> {
    let ledger = crate::state::read_state(|s| s.icusd_ledger_principal);
    transfer_idempotent(
        ledger,
        None,
        Account { owner: to, subaccount: None },
        amount.to_u64() as u128,
        op_nonce,
        None,
    )
    .await
}

/// Query the ICRC-1 transfer fee for a given ledger canister.
pub async fn get_ledger_fee(ledger: Principal) -> Result<u64, String> {
    let client = ICRC1Client {
        runtime: CdkRuntime,
        ledger_canister_id: ledger,
    };
    let fee = client.fee().await.map_err(|e| format!("icrc1_fee call failed: {:?}", e))?;
    fee.0
        .to_u64()
        .ok_or_else(|| "icrc1_fee does not fit in u64".to_string())
}

/// Generic collateral transfer: move tokens from the protocol canister to a recipient.
/// The `ledger` parameter is the ICRC-1 ledger canister ID of the collateral token.
///
/// One-shot variant: mints a fresh `op_nonce` per call. Use this for
/// caller-initiated transfers that don't have a persistent retry record.
/// For pending-transfer retry loops, use `transfer_collateral_with_nonce` and
/// pass the nonce stored on the pending entry.
pub async fn transfer_collateral(amount: u64, to: Principal, ledger: Principal) -> Result<u64, TransferError> {
    let op_nonce = crate::state::mutate_state(|s| s.next_op_nonce());
    transfer_collateral_with_nonce(amount, to, ledger, op_nonce).await
}

/// Idempotent collateral transfer with a caller-supplied nonce. Retry-loop
/// callers (process_pending_transfer, try_process_pending_transfers_immediate,
/// schedule_transfer_retry) must persist the nonce alongside the pending entry
/// and pass the same value on every retry so the ledger deduplicates.
pub async fn transfer_collateral_with_nonce(
    amount: u64,
    to: Principal,
    ledger: Principal,
    op_nonce: u128,
) -> Result<u64, TransferError> {
    transfer_idempotent(
        ledger,
        None,
        Account { owner: to, subaccount: None },
        amount as u128,
        op_nonce,
        None,
    )
    .await
}

/// Typed result for journaled collateral dispatch. A returned ledger error is
/// definitive no-effect; an inter-canister reject is ambiguous because the
/// ledger may have committed before the reply was lost.
#[derive(Debug)]
pub enum DurableTransferError {
    LedgerNoEffect(TransferError),
    AmbiguousCall { code: i32, message: String },
    AmbiguousResponse(String),
}

fn classify_durable_transfer_outcome(
    outer: Result<Result<Nat, TransferError>, (i32, String)>,
) -> Result<u64, DurableTransferError> {
    match outer {
        Err((code, message)) => Err(DurableTransferError::AmbiguousCall { code, message }),
        Ok(Err(TransferError::Duplicate { duplicate_of })) => {
            duplicate_of.0.to_u64().ok_or_else(|| DurableTransferError::AmbiguousResponse(
                "duplicate block index does not fit u64".into(),
            ))
        }
        Ok(Err(TransferError::BadFee { expected_fee })) => {
            Err(DurableTransferError::LedgerNoEffect(TransferError::BadFee { expected_fee }))
        }
        Ok(Err(error)) => Err(DurableTransferError::LedgerNoEffect(error)),
        Ok(Ok(block)) => block.0.to_u64().ok_or_else(|| DurableTransferError::AmbiguousResponse(
            "transfer block index does not fit u64".into(),
        )),
    }
}

pub async fn transfer_collateral_with_nonce_status(
    amount: u64,
    to: Principal,
    ledger: Principal,
    op_nonce: u128,
) -> Result<u64, DurableTransferError> {
    transfer_collateral_with_nonce_and_fee_status(amount, None, to, ledger, op_nonce).await
}

async fn transfer_with_nonce_status_and_guard<F, Fut>(
    ledger: Principal,
    from_subaccount: Option<[u8; 32]>,
    dispatch: F,
) -> Result<u64, DurableTransferError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<
        Output = Result<Result<Nat, TransferError>, (i32, String)>,
    >,
{
    let _default_account_guard =
        try_acquire_three_usd_default_account_guard(ledger, from_subaccount).map_err(
            DurableTransferError::LedgerNoEffect,
        )?;
    let result = classify_durable_transfer_outcome(dispatch().await);
    if let Err(DurableTransferError::LedgerNoEffect(TransferError::BadFee { expected_fee })) =
        &result
    {
        if let Some(fee) = expected_fee.0.to_u64() {
            set_cached_fee(ledger, fee);
        }
    }
    result
}

/// Idempotent collateral transfer with a persisted explicit fee. Bot claim
/// transfers use this to make the gross debit (`amount + fee`) equal the
/// collateral removed from the vault. Retries must reuse this exact tuple.
pub async fn transfer_collateral_with_nonce_and_fee_status(
    amount: u64,
    fee_e8s: Option<u64>,
    to: Principal,
    ledger: Principal,
    op_nonce: u128,
) -> Result<u64, DurableTransferError> {
    let client = ICRC1Client {
        runtime: CdkRuntime,
        ledger_canister_id: ledger,
    };
    transfer_with_nonce_status_and_guard(ledger, None, || async move {
        client
            .transfer(TransferArg {
                from_subaccount: None,
                to: Account {
                    owner: to,
                    subaccount: None,
                },
                fee: fee_e8s.map(Nat::from),
                created_at_time: Some(nonce_to_created_at_time(op_nonce)),
                memo: Some(nonce_to_memo(op_nonce)),
                amount: Nat::from(amount),
            })
            .await
    })
    .await
}

/// Generic collateral transfer_from: pull tokens from a user into the protocol canister.
/// The `ledger` parameter is the ICRC-1 ledger canister ID of the collateral token.
pub async fn transfer_collateral_from(amount: u64, from: Principal, ledger: Principal) -> Result<u64, TransferFromError> {
    let op_nonce = crate::state::mutate_state(|s| s.next_op_nonce());
    let protocol_id = ic_cdk::id();
    transfer_from_idempotent(
        ledger,
        Account { owner: from, subaccount: None },
        Account { owner: protocol_id, subaccount: None },
        amount as u128,
        op_nonce,
        None,
    )
    .await
}

/// Transfer ckUSDT or ckUSDC from a user to the protocol (for vault repayment/liquidation)
/// Amount is in e6s (6-decimal stable token units)
pub async fn transfer_stable_from(token_type: StableTokenType, amount_e6s: u64, caller: Principal) -> Result<u64, TransferFromError> {
    let ledger_principal = match token_type {
        StableTokenType::CKUSDT => read_state(|s| s.ckusdt_ledger_principal),
        StableTokenType::CKUSDC => read_state(|s| s.ckusdc_ledger_principal),
    }.ok_or_else(|| TransferFromError::GenericError {
        error_code: Nat::from(0u64),
        message: format!("{:?} ledger not configured", token_type),
    })?;

    let op_nonce = crate::state::mutate_state(|s| s.next_op_nonce());
    let protocol_id = ic_cdk::id();
    transfer_from_idempotent(
        ledger_principal,
        Account { owner: caller, subaccount: None },
        Account { owner: protocol_id, subaccount: None },
        amount_e6s as u128,
        op_nonce,
        None,
    )
    .await
}

/// Query the ICRC-1 balance of the protocol canister on any token ledger.
pub async fn get_token_balance(ledger: Principal) -> Result<u64, String> {
    let protocol_id = ic_cdk::id();
    let result: Result<(Nat,), _> = ic_cdk::call(
        ledger,
        "icrc1_balance_of",
        (Account {
            owner: protocol_id,
            subaccount: None,
        },),
    )
    .await;
    match result {
        Ok((balance,)) => Ok(balance.0.to_u64().unwrap_or(0)),
        Err((code, msg)) => Err(format!("icrc1_balance_of failed: {:?} {}", code, msg)),
    }
}

// ─── Protocol 3USD reserves ───

/// 3pool `virtual_price` is scaled by 1e18; LP token amounts use e8 decimals.
const THREE_POOL_VIRTUAL_PRICE_SCALE: u128 = 1_000_000_000_000_000_000;
/// One e8 unit covers only integer-floor rounding in the value conversion.
const THREE_USD_VALUE_TOLERANCE_E8S: u128 = 1;

/// Minimal `get_pool_status` response. Candid record subtyping lets the
/// backend decode this one field and ignore the rest of the pool status.
#[derive(candid::CandidType, serde::Deserialize)]
struct ThreePoolStatusVirtualPrice {
    virtual_price: Nat,
}

/// Read the virtual price from the configured 3pool canister. Update calls
/// invoke query methods in replicated mode, so callers do not rely on an
/// unauthenticated client-side query result.
pub async fn three_pool_virtual_price(three_pool: Principal) -> Result<u128, String> {
    let result: Result<(ThreePoolStatusVirtualPrice,), _> =
        ic_cdk::call(three_pool, "get_pool_status", ()).await;
    let (status,) = result.map_err(|(code, message)| {
        format!("3pool get_pool_status failed ({:?}): {}", code, message)
    })?;
    let virtual_price = status
        .virtual_price
        .0
        .to_u128()
        .ok_or_else(|| "3pool virtual price exceeds u128".to_string())?;
    if virtual_price == 0 {
        return Err("3pool virtual price is zero".to_string());
    }
    Ok(virtual_price)
}

/// Bind reserves transfers and status reads to the single configured 3pool
/// principal, which is also the 3USD ICRC ledger.
pub fn validate_three_usd_ledger(
    configured_pool: Option<Principal>,
    supplied_ledger: Principal,
) -> Result<Principal, String> {
    match configured_pool {
        Some(pool) if pool == supplied_ledger => Ok(pool),
        Some(_) => Err("3USD ledger does not match the configured 3pool canister".to_string()),
        None => Err("3USD ledger has no configured 3pool canister".to_string()),
    }
}

/// Reject 3USD/LP amounts whose independently read pool value cannot cover
/// the icUSD debt. Arithmetic overflow fails closed. The one-e8-unit margin
/// is only for the integer floor used by the SP's matching conversion.
pub fn validate_three_usd_value(
    three_usd_amount_e8s: u64,
    virtual_price_e18: u128,
    debt_covered_e8s: u64,
) -> Result<(), String> {
    let value_e8s = u128::from(three_usd_amount_e8s)
        .checked_mul(virtual_price_e18)
        .ok_or_else(|| "3USD value calculation overflowed".to_string())?
        / THREE_POOL_VIRTUAL_PRICE_SCALE;
    if value_e8s.saturating_add(THREE_USD_VALUE_TOLERANCE_E8S)
        < u128::from(debt_covered_e8s)
    {
        return Err(format!(
            "3USD value {} e8s is below debt covered {} e8s",
            value_e8s, debt_covered_e8s
        ));
    }
    Ok(())
}

pub fn checked_three_usd_reserves_total(current_e8s: u64, added_e8s: u64) -> Result<u64, String> {
    current_e8s
        .checked_add(added_e8s)
        .ok_or_else(|| "3USD reserves accounting capacity exhausted".to_string())
}

#[cfg(test)]
mod three_usd_value_tests {
    use super::validate_three_usd_value;

    #[test]
    fn value_binding_covers_debt_and_fails_closed() {
        const VP: u128 = 1_000_000_000_000_000_000;
        assert!(validate_three_usd_value(1_000_000_000, VP, 1_000_000_000).is_ok());
        assert!(validate_three_usd_value(1, VP, 10_000_000).is_err());
        assert!(validate_three_usd_value(99, VP, 100).is_ok()); // one-e8 rounding tolerance
        assert!(validate_three_usd_value(u64::MAX, u128::MAX, 1).is_err());

        // A value sufficient at the pre-transfer rate becomes insufficient
        // when the independently re-read pool virtual price falls.
        let amount = 1_000_000_000;
        let debt = 1_000_000_000;
        assert!(validate_three_usd_value(amount, VP, debt).is_ok());
        assert!(validate_three_usd_value(amount, 800_000_000_000_000_000, debt).is_err());
    }

    #[test]
    fn ledger_must_equal_the_configured_pool_principal() {
        use candid::Principal;

        let pool = Principal::from_slice(&[1]);
        let other_ledger = Principal::from_slice(&[2]);
        assert_eq!(super::validate_three_usd_ledger(Some(pool), pool), Ok(pool));
        assert!(super::validate_three_usd_ledger(Some(pool), other_ledger).is_err());
        assert!(super::validate_three_usd_ledger(None, pool).is_err());
    }

    #[test]
    fn reserves_capacity_rejects_overflow() {
        assert_eq!(super::checked_three_usd_reserves_total(7, 5), Ok(12));
        assert!(super::checked_three_usd_reserves_total(u64::MAX, 1).is_err());
    }
}

/// Deterministic subaccount for protocol-held 3USD reserves from SP liquidations.
pub fn protocol_3usd_reserves_subaccount() -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"protocol_3usd_reserves");
    hasher.finalize().into()
}

/// Pull 3USD from the stability pool into the protocol's reserves subaccount via ICRC-2.
/// The SP must have approved this canister to spend `amount` on `ledger` beforehand.
pub async fn transfer_3usd_to_reserves(
    ledger: Principal,
    from: Principal,
    amount: u64,
) -> Result<u64, TransferFromError> {
    let op_nonce = crate::state::mutate_state(|s| s.next_op_nonce());
    let protocol_id = ic_cdk::id();
    transfer_from_idempotent(
        ledger,
        Account { owner: from, subaccount: None },
        Account {
            owner: protocol_id,
            subaccount: Some(protocol_3usd_reserves_subaccount()),
        },
        amount as u128,
        op_nonce,
        None,
    )
    .await
}

// ─── Push-deposit helpers (Oisy wallet integration) ───

/// Compute a deterministic deposit subaccount for a given caller.
/// Subaccount = SHA-256(b"rumi-deposit" || caller.as_slice())
pub fn compute_deposit_subaccount(caller: &Principal) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"rumi-deposit");
    hasher.update(caller.as_slice());
    hasher.finalize().into()
}

/// Return the deposit Account for a caller. The account is owned by the
/// backend canister with a caller-specific subaccount.
pub fn get_deposit_account_for(caller: &Principal) -> Account {
    Account {
        owner: ic_cdk::id(),
        subaccount: Some(compute_deposit_subaccount(caller)),
    }
}

/// Query the ICRC-1 balance of a specific account on a ledger.
pub async fn get_balance_of(account: Account, ledger: Principal) -> Result<u64, String> {
    let result: Result<(Nat,), _> = ic_cdk::call(
        ledger,
        "icrc1_balance_of",
        (account,),
    )
    .await;
    match result {
        Ok((balance,)) => Ok(balance.0.to_u64().unwrap_or(0)),
        Err((code, msg)) => Err(format!("icrc1_balance_of failed: {:?} {}", code, msg)),
    }
}

#[derive(Debug)]
pub(crate) enum SweepDepositError {
    Transfer(String),
    Ledger(TransferError),
    AmbiguousCall { code: i32, message: String },
    AmbiguousResponse(String),
    AmountTooLow { minimum_amount: u64 },
    ExceedsCapacity { amount: u64, maximum_amount: u64 },
    PendingUnknown,
}

#[derive(Clone, Debug)]
pub(crate) struct SweepDepositReceipt {
    pub amount_e8s: u64,
    pub block_index: u64,
    pub journal: PushSweepJournal,
}

impl std::fmt::Display for SweepDepositError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transfer(message) => f.write_str(message),
            Self::Ledger(error) => write!(f, "sweep transfer error: {:?}", error),
            Self::AmbiguousCall { code, message } => {
                write!(f, "sweep transfer call outcome is ambiguous ({code}): {message}")
            }
            Self::AmbiguousResponse(message) => {
                write!(f, "sweep transfer response is ambiguous: {message}")
            }
            Self::AmountTooLow { minimum_amount } => {
                write!(f, "Deposit amount is below minimum {minimum_amount}")
            }
            Self::ExceedsCapacity { amount, maximum_amount } => write!(
                f,
                "Deposit amount ({amount}) exceeds the vault's remaining collateral capacity ({maximum_amount})"
            ),
            Self::PendingUnknown => f.write_str("a prior push sweep may have transferred funds; retry the same operation and prove the exact ledger transfer before starting another sweep"),
        }
    }
}

/// Sweep a pushed deposit only when its net amount fits the vault's inclusive
/// minimum/maximum bounds. Both checks run before the irreversible transfer.
pub(crate) async fn sweep_deposit_for_request(
    caller: &Principal,
    ledger: Principal,
    ledger_fee: u64,
    minimum_transfer_amount: u64,
    max_transfer_amount: u64,
    request: PushSweepRequest,
) -> Result<SweepDepositReceipt, SweepDepositError> {
    let subaccount = compute_deposit_subaccount(caller);
    let mut journal = if let Some(j) = read_state(|s| s.push_sweep_journals.get(caller).cloned()) {
        if j.owner != *caller || j.request != request || j.ledger != ledger {
            return Err(SweepDepositError::Transfer("an unresolved push sweep is bound to another owner/vault operation".into()));
        }
        j
    } else {
        let balance = get_balance_of(Account { owner: ic_cdk::id(), subaccount: Some(subaccount) }, ledger)
            .await.map_err(SweepDepositError::Transfer)?;
        let fee = cached_fee_for(ledger).unwrap_or(ledger_fee);
        if balance == 0 { return Err(SweepDepositError::Transfer("No deposit found in subaccount".into())); }
        if balance <= fee { return Err(SweepDepositError::Transfer(format!("Deposit balance ({balance}) is not enough to cover the ledger fee ({fee})"))); }
        let amount_e8s = balance - fee;
        ensure_sweep_amount_within_bounds(amount_e8s, minimum_transfer_amount, max_transfer_amount)?;
        mutate_state(|s| {
            if let Some(j) = s.push_sweep_journals.get(caller) {
                if j.owner == *caller && j.request == request && j.ledger == ledger { return Ok(j.clone()); }
                return Err(SweepDepositError::Transfer("an unresolved push sweep is bound to another owner/vault operation".into()));
            }
            let vault_id = match &request {
                PushSweepRequest::OpenVault { .. } => s.increment_vault_id(),
                PushSweepRequest::AddMargin { vault_id } => *vault_id,
            };
            let op_nonce = s.next_op_nonce_at(ic_cdk::api::time());
            let j = PushSweepJournal {
                owner: *caller, request: request.clone(), vault_id, ledger,
                from_subaccount: subaccount, to_owner: ic_cdk::id(), amount_e8s,
                fee_e8s: fee, memo: op_nonce.to_be_bytes().to_vec(),
                created_at_time_ns: nonce_to_created_at_time(op_nonce), op_nonce,
                dispatch_attempts: 0,
            };
            s.push_sweep_journals.insert(*caller, j.clone());
            Ok(j)
        })?
    };

    journal = mutate_state(|s| s.mark_push_sweep_dispatched(caller, &journal))
        .map_err(SweepDepositError::Transfer)?;
    let attempt = journal.dispatch_attempts;
    let block = transfer_deposit_idempotent_status(&journal).await.map_err(|error| {
        if attempt == 1 && matches!(&error, DurableTransferError::LedgerNoEffect(_)) {
            mutate_state(|s| s.release_push_sweep_after_first_no_effect(caller, &journal));
        }
        match error {
        DurableTransferError::LedgerNoEffect(TransferError::BadFee { expected_fee }) => {
            if attempt == 1 { SweepDepositError::Ledger(TransferError::BadFee { expected_fee }) } else { SweepDepositError::PendingUnknown }
        }
        DurableTransferError::LedgerNoEffect(error) if attempt == 1 => SweepDepositError::Ledger(error),
        // After an earlier ambiguous attempt, even a no-effect response (most
        // notably TooOld after the ledger's dedup horizon) cannot prove that
        // the original request never committed. Keep the saved tuple held.
        DurableTransferError::LedgerNoEffect(_) => SweepDepositError::PendingUnknown,
        DurableTransferError::AmbiguousCall { code, message } => {
            SweepDepositError::AmbiguousCall { code, message }
        }
        DurableTransferError::AmbiguousResponse(message) => {
            SweepDepositError::AmbiguousResponse(message)
        }
    }})?;

    log!(DEBUG,
        "[sweep_deposit] Swept {} from subaccount for {} on ledger {} (block {})",
        journal.amount_e8s, caller, ledger, block
    );

    Ok(SweepDepositReceipt { amount_e8s: journal.amount_e8s, block_index: block, journal })
}

async fn transfer_deposit_idempotent_status(
    journal: &PushSweepJournal,
) -> Result<u64, DurableTransferError> {
    let client = ICRC1Client { runtime: CdkRuntime, ledger_canister_id: journal.ledger };
    let outcome = client.transfer(push_sweep_transfer_arg(journal)).await;
    let result = classify_push_sweep_outcome(outcome);
    if let Err(DurableTransferError::LedgerNoEffect(TransferError::BadFee { expected_fee })) = &result {
        if let Some(fee) = expected_fee.0.to_u64() {
            set_cached_fee(journal.ledger, fee);
        }
    }
    result
}

/// A ledger `GenericError` is not accepted as proof of no transfer: a ledger
/// can commit and then fail while forming its response. Only a successful
/// block or exact-tuple Duplicate proves positive settlement.
fn classify_push_sweep_outcome(
    outcome: Result<Result<Nat, TransferError>, (i32, String)>,
) -> Result<u64, DurableTransferError> {
    match outcome {
        Ok(Err(TransferError::GenericError { error_code, message })) => {
            Err(DurableTransferError::AmbiguousResponse(format!(
                "ledger GenericError {error_code}: {message}"
            )))
        }
        other => classify_durable_transfer_outcome(other),
    }
}

fn push_sweep_transfer_arg(journal: &PushSweepJournal) -> TransferArg {
    TransferArg {
        from_subaccount: Some(journal.from_subaccount),
        to: Account { owner: journal.to_owner, subaccount: None },
        fee: Some(Nat::from(journal.fee_e8s)),
        created_at_time: Some(journal.created_at_time_ns),
        memo: Some(Memo::from(journal.memo.clone())),
        amount: Nat::from(journal.amount_e8s),
    }
}

async fn transfer_deposit_after_bounds<F, Fut>(
    transfer_amount: u64,
    minimum_transfer_amount: u64,
    max_transfer_amount: u64,
    transfer: F,
) -> Result<Result<u64, DurableTransferError>, SweepDepositError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<u64, DurableTransferError>>,
{
    ensure_sweep_amount_within_bounds(
        transfer_amount,
        minimum_transfer_amount,
        max_transfer_amount,
    )?;
    Ok(transfer().await)
}

fn ensure_sweep_amount_within_bounds(
    transfer_amount: u64,
    minimum_transfer_amount: u64,
    max_transfer_amount: u64,
) -> Result<(), SweepDepositError> {
    if transfer_amount < minimum_transfer_amount {
        return Err(SweepDepositError::AmountTooLow {
            minimum_amount: minimum_transfer_amount,
        });
    }
    if transfer_amount > max_transfer_amount {
        return Err(SweepDepositError::ExceedsCapacity {
            amount: transfer_amount,
            maximum_amount: max_transfer_amount,
        });
    }
    Ok(())
}

/// Approve a spender to transfer icUSD from the protocol canister.
/// Used by interest distribution to approve the 3pool for `donate`.
///
/// Sets `created_at_time` from a fresh nonce so the ledger can dedup, and
/// treats `ApproveError::Duplicate { duplicate_of }` as success (the approve
/// already landed at that block — same effective allowance).
pub async fn approve_icusd(spender: Principal, amount: u64) -> Result<u64, ApproveError> {
    let (ledger, op_nonce) = crate::state::mutate_state(|s| (s.icusd_ledger_principal, s.next_op_nonce()));
    let created_at_time = nonce_to_created_at_time(op_nonce);
    let memo = nonce_to_memo(op_nonce);

    let result: Result<(Result<Nat, ApproveError>,), _> = ic_cdk::call(
        ledger,
        "icrc2_approve",
        (ApproveArgs {
            from_subaccount: None,
            spender: Account { owner: spender, subaccount: None },
            amount: Nat::from(amount),
            expected_allowance: None,
            expires_at: None,
            fee: None,
            created_at_time: Some(created_at_time),
            memo: Some(memo),
        },),
    ).await;
    match result {
        Ok((Ok(block_index),)) => Ok(block_index.0.to_u64().unwrap_or(0)),
        Ok((Err(ApproveError::Duplicate { duplicate_of }),)) => {
            log!(DEBUG,
                "[approve_icusd] ledger {} reported Duplicate; treating as success (block {})",
                ledger, duplicate_of
            );
            Ok(duplicate_of.0.to_u64().unwrap_or(0))
        }
        Ok((Err(ApproveError::BadFee { expected_fee }),)) => {
            let fee = expected_fee.0.to_u64().unwrap_or(0);
            set_cached_fee(ledger, fee);
            Err(ApproveError::BadFee { expected_fee })
        }
        Ok((Err(e),)) => Err(e),
        Err((code, msg)) => Err(ApproveError::GenericError {
            error_code: Nat::from(code as u64),
            message: msg,
        }),
    }
}

#[cfg(test)]
mod sweep_deposit_limit_tests {
    use super::{
        classify_durable_transfer_outcome, classify_push_sweep_outcome, push_sweep_transfer_arg,
        transfer_deposit_after_bounds, transfer_with_nonce_status_and_guard,
        DurableTransferError, SweepDepositError, ThreeUsdReserveIngressAdmissionGuard,
    };
    use crate::state::{PushSweepJournal, PushSweepRequest};
    use candid::Principal;
    use candid::Nat;
    use icrc_ledger_types::icrc1::transfer::TransferError;
    use std::cell::Cell;

    #[test]
    fn sweep_limit_accepts_exact_remaining_capacity() {
        let transferred = Cell::new(false);
        let result = futures::executor::block_on(transfer_deposit_after_bounds(
            12,
            3,
            12,
            || {
                transferred.set(true);
                async { Ok(44) }
            },
        ));
        assert_eq!(result.unwrap().unwrap(), 44);
        assert!(transferred.get());
    }

    #[test]
    fn sweep_limit_rejects_before_transfer_when_capacity_is_exceeded() {
        let transferred = Cell::new(false);
        assert!(matches!(
            futures::executor::block_on(transfer_deposit_after_bounds(
                13,
                3,
                12,
                || {
                    transferred.set(true);
                    async { Ok(44) }
                },
            )),
            Err(SweepDepositError::ExceedsCapacity { .. })
        ));
        assert!(!transferred.get());
    }

    #[test]
    fn sweep_limit_rejects_below_minimum_before_transfer() {
        let transferred = Cell::new(false);
        assert!(matches!(
            futures::executor::block_on(transfer_deposit_after_bounds(
                2,
                3,
                12,
                || {
                    transferred.set(true);
                    async { Ok(44) }
                },
            )),
            Err(SweepDepositError::AmountTooLow { minimum_amount: 3 })
        ));
        assert!(!transferred.get());
    }

    #[test]
    fn sweep_transfer_classifies_ledger_no_effect_separately_from_ambiguous_reject() {
        let no_effect = classify_durable_transfer_outcome(Ok(Err(TransferError::BadFee {
            expected_fee: Nat::from(10u64),
        })));
        assert!(matches!(
            no_effect,
            Err(DurableTransferError::LedgerNoEffect(TransferError::BadFee { .. }))
        ));

        let ambiguous = classify_durable_transfer_outcome(Err((5, "ledger call rejected".into())));
        assert!(matches!(
            ambiguous,
            Err(DurableTransferError::AmbiguousCall { code: 5, .. })
        ));

        let typed_no_effect = classify_durable_transfer_outcome(Ok(Err(
            TransferError::TemporarilyUnavailable,
        )));
        assert!(matches!(
            typed_no_effect,
            Err(DurableTransferError::LedgerNoEffect(
                TransferError::TemporarilyUnavailable
            ))
        ));
    }

    #[test]
    fn durable_transfer_requires_a_representable_block_index() {
        assert_eq!(
            classify_durable_transfer_outcome(Ok(Ok(Nat::from(123u64)))).unwrap(),
            123
        );
        assert_eq!(
            classify_durable_transfer_outcome(Ok(Err(TransferError::Duplicate {
                duplicate_of: Nat::from(456u64),
            })))
            .unwrap(),
            456
        );

        assert!(matches!(
            classify_durable_transfer_outcome(Ok(Ok(Nat::from(u64::MAX as u128 + 1)))),
            Err(DurableTransferError::AmbiguousResponse(_))
        ));
        assert!(matches!(
            classify_durable_transfer_outcome(Ok(Err(TransferError::Duplicate {
                duplicate_of: Nat::from(u64::MAX as u128 + 1),
            }))),
            Err(DurableTransferError::AmbiguousResponse(_))
        ));
    }

    #[test]
    fn concurrent_three_usd_default_account_admission_blocks_status_transfer_before_dispatch() {
        let ledger = Principal::from_slice(&[17]);
        crate::state::replace_state(crate::state::State::default());
        crate::state::mutate_state(|state| state.three_pool_canister = Some(ledger));

        let existing_admission = ThreeUsdReserveIngressAdmissionGuard::try_acquire()
            .expect("first 3USD default-account operation acquires the guard");
        let dispatched = std::cell::Cell::new(false);
        let held = futures::executor::block_on(transfer_with_nonce_status_and_guard(
            ledger,
            None,
            || async {
                dispatched.set(true);
                Ok(Ok(Nat::from(77u64)))
            },
        ));
        assert!(matches!(
            held,
            Err(DurableTransferError::LedgerNoEffect(
                TransferError::GenericError { .. }
            ))
        ));
        assert!(!dispatched.get(), "guard rejection must precede ledger dispatch");

        drop(existing_admission);
        let retried = futures::executor::block_on(transfer_with_nonce_status_and_guard(
            ledger,
            None,
            || async {
                dispatched.set(true);
                Ok(Ok(Nat::from(78u64)))
            },
        ));
        assert_eq!(retried.unwrap(), 78);
        assert!(dispatched.get(), "transfer may dispatch after guard release");
    }

    #[test]
    fn reply_loss_retry_reuses_complete_transfer_tuple_and_duplicate_proves_credit_block() {
        let journal = PushSweepJournal {
            owner: Principal::from_slice(&[1]),
            request: PushSweepRequest::AddMargin { vault_id: 91 },
            vault_id: 91,
            ledger: Principal::from_slice(&[2]),
            from_subaccount: [3; 32],
            to_owner: Principal::from_slice(&[4]),
            amount_e8s: 5_000,
            fee_e8s: 100,
            memo: 77u128.to_be_bytes().to_vec(),
            created_at_time_ns: 12_000,
            op_nonce: 77,
            dispatch_attempts: 1,
        };
        let original = push_sweep_transfer_arg(&journal);
        let retry = push_sweep_transfer_arg(&PushSweepJournal { dispatch_attempts: 2, ..journal });
        assert_eq!(original.from_subaccount, retry.from_subaccount);
        assert_eq!(original.to, retry.to);
        assert_eq!(original.amount, retry.amount);
        assert_eq!(original.fee, retry.fee);
        assert_eq!(original.memo, retry.memo);
        assert_eq!(original.created_at_time, retry.created_at_time);

        assert!(matches!(
            classify_push_sweep_outcome(Ok(Err(TransferError::GenericError {
                error_code: Nat::from(5u64), message: "commit then response error".into(),
            }))),
            Err(DurableTransferError::AmbiguousResponse(_))
        ));
        assert_eq!(
            classify_push_sweep_outcome(Ok(Err(TransferError::Duplicate {
                duplicate_of: Nat::from(123u64),
            }))).unwrap(),
            123
        );
        assert!(matches!(
            classify_push_sweep_outcome(Ok(Err(TransferError::TooOld))),
            Err(DurableTransferError::LedgerNoEffect(TransferError::TooOld))
        ));
    }
}
