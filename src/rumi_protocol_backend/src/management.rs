use crate::numeric::{ICUSD, ICP};
use crate::state::read_state;
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
    let _default_account_guard = acquire_three_usd_default_account_transfer_guard(
        ledger,
        from_subaccount,
    )?;
    if let Some(guard) = _default_account_guard.as_ref() {
        let fee_before = get_ledger_fee(ledger).await.map_err(default_account_capacity_error)?;
        let balance = get_icrc1_reserve_balance(
            ledger,
            Account { owner: ic_cdk::id(), subaccount: None },
        )
        .await
        .map_err(default_account_capacity_error)?;
        let fee = get_ledger_fee(ledger).await.map_err(default_account_capacity_error)?;
        if !default_fee_is_stable_and_legacy_compatible(fee_before, fee) {
            return Err(default_account_capacity_error(
                "3USD ledger fee is nonzero or changed during preflight; legacy transfer remains held",
            ));
        }
        ensure_default_account_spend_with_balance(guard, ledger, amount, fee, balance)?;
    }
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
            // Preserve the legacy dedup tuple for existing pending operations:
            // this generic path has always omitted the fee field.
            fee: None,
            created_at_time: Some(created_at_time),
            memo: Some(memo),
            amount: Nat::from(amount),
        })
        .await;

    handle_transfer_outcome(ledger, outer)
}

pub async fn transfer_idempotent_exact(
    ledger: Principal,
    from_subaccount: Option<[u8; 32]>,
    to: Account,
    amount: u128,
    fee: u64,
    memo: Memo,
    created_at_time: u64,
) -> Result<u64, TransferError> {
    let _guard = acquire_three_usd_default_account_transfer_guard(ledger, from_subaccount)?;
    if let Some(guard) = _guard.as_ref() {
        ensure_default_account_spend_preserves_refunds(guard, ledger, amount, fee).await?;
    }
    transfer_idempotent_exact_inner(ledger, from_subaccount, to, amount, fee, memo, created_at_time).await
}

pub async fn transfer_idempotent_exact_with_three_usd_guard(
    guard: &crate::ThreeUsdDefaultAccountTransferGuard,
    ledger: Principal,
    from_subaccount: Option<[u8; 32]>,
    to: Account,
    amount: u128,
    fee: u64,
    memo: Memo,
    created_at_time: u64,
) -> Result<u64, TransferError> {
    if from_subaccount.is_some() || !guard.protects(ledger) {
        return Err(TransferError::GenericError {
            error_code: Nat::from(0u64),
            message: "3USD default-account transfer guard does not match the persisted source".into(),
        });
    }
    // The refund worker capacity-checks the initial tuple before persisting it.
    // Retries replay that exact tuple and verify its receipt before clearing it.
    transfer_idempotent_exact_inner(ledger, from_subaccount, to, amount, fee, memo, created_at_time).await
}

async fn transfer_idempotent_exact_inner(
    ledger: Principal,
    from_subaccount: Option<[u8; 32]>,
    to: Account,
    amount: u128,
    fee: u64,
    memo: Memo,
    created_at_time: u64,
) -> Result<u64, TransferError> {
    let client = ICRC1Client { runtime: CdkRuntime, ledger_canister_id: ledger };
    let outer = client.transfer(TransferArg {
        from_subaccount,
        to,
        fee: Some(Nat::from(fee)),
        created_at_time: Some(created_at_time),
        memo: Some(memo),
        amount: Nat::from(amount),
    }).await;
    handle_transfer_outcome(ledger, outer)
}

fn acquire_three_usd_default_account_transfer_guard(
    ledger: Principal,
    from_subaccount: Option<[u8; 32]>,
) -> Result<Option<crate::ThreeUsdDefaultAccountTransferGuard>, TransferError> {
    let is_default_source = from_subaccount.map_or(true, |subaccount| subaccount == [0; 32]);
    if !is_default_source || !crate::state::read_state(|state| state.three_pool_canister == Some(ledger)) {
        return Ok(None);
    }
    crate::ThreeUsdDefaultAccountTransferGuard::try_acquire(ledger).map(Some).ok_or_else(|| {
        TransferError::GenericError {
            error_code: Nat::from(0u64),
            message: "3USD default-account transfer is held by a reserve capacity check".into(),
        }
    })
}

fn default_account_capacity_error(message: impl Into<String>) -> TransferError {
    TransferError::GenericError {
        error_code: Nat::from(0u64),
        message: message.into(),
    }
}

fn default_account_spend_fits(balance: u64, amount: u64, fee: u64, commitment: u64) -> bool {
    amount
        .checked_add(fee)
        .and_then(|debit| balance.checked_sub(debit))
        .is_some_and(|remaining| remaining >= commitment)
}

fn default_fee_is_stable_and_legacy_compatible(before: u64, after: u64) -> bool {
    before == 0 && after == 0
}

async fn ensure_default_account_spend_preserves_refunds(
    guard: &crate::ThreeUsdDefaultAccountTransferGuard,
    ledger: Principal,
    amount: u128,
    fee: u64,
) -> Result<(), TransferError> {
    let balance = get_icrc1_reserve_balance(
        ledger,
        Account { owner: ic_cdk::id(), subaccount: None },
    )
    .await
    .map_err(default_account_capacity_error)?;
    ensure_default_account_spend_with_balance(guard, ledger, amount, fee, balance)
}

fn ensure_default_account_spend_with_balance(
    guard: &crate::ThreeUsdDefaultAccountTransferGuard,
    ledger: Principal,
    amount: u128,
    fee: u64,
    balance: u64,
) -> Result<(), TransferError> {
    if !guard.protects(ledger) {
        return Err(default_account_capacity_error("3USD default-account spend has no matching capacity guard"));
    }
    let amount = u64::try_from(amount)
        .map_err(|_| default_account_capacity_error("3USD default-account debit exceeds u64"))?;
    let commitment = read_state(|state| state.three_usd_default_account_refund_commitment(ledger, fee))
        .ok_or_else(|| default_account_capacity_error(
            "3USD default-account refund obligations are uncertain; debit remains held",
        ))?;
    if !default_account_spend_fits(balance, amount, fee, commitment) {
        return Err(default_account_capacity_error(format!(
            "3USD default-account debit would violate refund commitments (balance {balance}, debit {amount}+{fee}, committed {commitment})",
        )));
    }
    Ok(())
}

#[cfg(test)]
mod three_usd_default_spend_capacity_tests {
    use super::{default_account_spend_fits, default_fee_is_stable_and_legacy_compatible};

    #[test]
    fn default_spend_must_leave_all_refunds_funded() {
        assert!(default_account_spend_fits(150, 20, 10, 120));
        assert!(!default_account_spend_fits(149, 20, 10, 120));
        assert!(!default_account_spend_fits(150, 20, 10, 121));
        assert!(!default_account_spend_fits(u64::MAX, u64::MAX, 1, 0));
    }

    #[test]
    fn generic_legacy_fee_tuple_is_only_allowed_for_stable_zero_fee_ledger() {
        assert!(default_fee_is_stable_and_legacy_compatible(0, 0));
        assert!(!default_fee_is_stable_and_legacy_compatible(0, 1));
        assert!(!default_fee_is_stable_and_legacy_compatible(1, 1));
    }
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
                let rate = rust_decimal::Decimal::from_u64(exchange_rate_result.rate).unwrap()
                    / rust_decimal::Decimal::from_u64(10_u64.pow(exchange_rate_result.metadata.decimals)).unwrap();

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

/// Mint to an explicit account using a previously journaled idempotency
/// nonce. Callers must persist `op_nonce` before the first await and reuse it
/// until the exact transfer is confirmed or held for reconciliation.
pub async fn mint_icusd_with_nonce(
    ledger: Principal,
    amount: ICUSD,
    to: Account,
    op_nonce: u128,
) -> Result<u64, TransferError> {
    transfer_idempotent(ledger, None, to, amount.to_u64() as u128, op_nonce, None).await
}

pub async fn transfer_icusd_from(amount: ICUSD, caller: Principal) -> Result<u64, TransferFromError> {
    let (ledger, op_nonce) = crate::state::mutate_state(|s| (s.icusd_ledger_principal, s.next_op_nonce()));
    let protocol_id = ic_cdk::id();
    transfer_from_idempotent(
        ledger,
        Account { owner: caller, subaccount: None },
        Account { owner: protocol_id, subaccount: None },
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
    fee.0.to_u64().ok_or_else(|| "ledger fee exceeds the supported u64 range".to_string())
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

/// Dispatch one exact ICRC-1 collateral tuple for bot claims and claim
/// cancellation. Every argument is supplied by a durable caller journal;
/// this helper never generates a replacement nonce, memo, or timestamp.
pub async fn transfer_collateral_with_exact_tuple(
    ledger: Principal,
    from: Account,
    to: Account,
    amount: u64,
    fee: u64,
    memo: Vec<u8>,
    created_at_time: u64,
) -> Result<u64, TransferError> {
    let _default_account_guard =
        acquire_three_usd_default_account_transfer_guard(ledger, from.subaccount)?;
    if let Some(guard) = _default_account_guard.as_ref() {
        ensure_default_account_spend_preserves_refunds(guard, ledger, amount as u128, fee).await?;
    }
    let args = TransferArg {
        from_subaccount: from.subaccount,
        to,
        amount: Nat::from(amount),
        fee: Some(Nat::from(fee)),
        memo: Some(Memo::from(memo)),
        created_at_time: Some(created_at_time),
    };
    let result: Result<(Result<Nat, TransferError>,), _> =
        ic_cdk::call(ledger, "icrc1_transfer", (args,)).await;
    match result {
        Ok((Ok(block_index),)) => block_index.0.to_u64().ok_or(TransferError::GenericError {
            error_code: Nat::from(0u8),
            message: "ledger block index exceeds u64".into(),
        }),
        Ok((Err(TransferError::Duplicate { duplicate_of }),)) => {
            duplicate_of.0.to_u64().ok_or(TransferError::GenericError {
                error_code: Nat::from(0u8),
                message: "duplicate ledger block index exceeds u64".into(),
            })
        }
        Ok((Err(error),)) => Err(error),
        Err((code, message)) => Err(TransferError::GenericError {
            error_code: Nat::from(code as u64),
            message,
        }),
    }
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

/// Deterministic subaccount for protocol-held 3USD reserves from SP liquidations.
pub fn protocol_3usd_reserves_subaccount() -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"protocol_3usd_reserves");
    hasher.finalize().into()
}

/// Pull 3USD from the stability pool into the protocol's reserves subaccount via ICRC-2.
/// Legacy reserve pull retained for the old Candid entrypoint during the
/// mixed-version window. The strict 3pool account guard rejects its hashed
/// destination, so it cannot be used after the ledger policy cutover.
pub async fn transfer_3usd_to_reserves_legacy(
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

thread_local! {
    static THREE_USD_RESERVE_INGRESS_IN_FLIGHT:
        std::cell::RefCell<std::collections::BTreeSet<crate::state::ThreeUsdReserveIngressKey>> =
        std::cell::RefCell::new(std::collections::BTreeSet::new());
}

/// Excludes concurrent copies of the same reserve absorption while an
/// inter-canister call is suspended.
pub struct ThreeUsdReserveIngressGuard(crate::state::ThreeUsdReserveIngressKey);

impl ThreeUsdReserveIngressGuard {
    pub fn try_acquire(key: &crate::state::ThreeUsdReserveIngressKey) -> Option<Self> {
        THREE_USD_RESERVE_INGRESS_IN_FLIGHT.with(|keys| {
            keys.borrow_mut().insert(key.clone()).then(|| Self(key.clone()))
        })
    }
}

impl Drop for ThreeUsdReserveIngressGuard {
    fn drop(&mut self) {
        THREE_USD_RESERVE_INGRESS_IN_FLIGHT.with(|keys| {
            keys.borrow_mut().remove(&self.0);
        });
    }
}

pub fn three_usd_reserve_ingress_journal(
    key: &crate::state::ThreeUsdReserveIngressKey,
) -> Option<crate::state::ThreeUsdReserveIngressJournal> {
    crate::state::read_state(|state| state.three_usd_reserve_ingress_journals.get(key).cloned())
}

/// Durably claims a V2 absorb identity before the caller's first await.
/// Absence is intentionally not converted into a no-transfer result: only a
/// persisted `PreTransferRejected` row can establish that this ID is terminal.
pub fn admit_three_usd_reserve_ingress(
    key: crate::state::ThreeUsdReserveIngressKey,
    request: crate::state::ThreeUsdReserveIngressRequest,
) -> Result<crate::state::ThreeUsdReserveIngressJournal, String> {
    use crate::state::{ThreeUsdReserveIngressJournal as Journal, ThreeUsdReserveIngressPhase as Phase};
    crate::state::mutate_state(|state| {
        if key.stability_pool == Principal::anonymous() || key.absorb_id == 0 {
            return Err("invalid 3USD reserve ingress identity".into());
        }
        if let Some(existing) = state.three_usd_reserve_ingress_journals.get(&key) {
            return if existing.request == request {
                Ok(existing.clone())
            } else {
                Err("SP absorb ID was reused with different 3USD reserve arguments".into())
            };
        }
        let invalid_request = request.ledger == Principal::anonymous()
            || request.three_usd_amount_e8s == 0
            || request.icusd_debt_covered_e8s == 0;
        let journal = Journal {
            request,
            phase: if invalid_request {
                Phase::PreTransferRejected { reason: "invalid 3USD reserve ingress request".into() }
            } else {
                Phase::AdmissionPending
            },
            refund: None,
            payout: None,
            refund_fee_reserved_e8s: None,
        };
        state.three_usd_reserve_ingress_journals.insert(key, journal.clone());
        Ok(journal)
    })
}

/// New default-account ingress stays closed until fee capacity is reserved
/// atomically against every durable obligation and every backend writer.
/// This is deliberately a source gate: toggling the historical developer flag
/// cannot bypass the missing account-capacity proof.
pub const THREE_USD_INGRESS_FEE_CAPACITY_PREFLIGHT_READY: bool =
    cfg!(feature = "three-usd-reserve-v2-test-admission");

pub fn three_usd_reserve_ingress_is_enabled() -> bool {
    THREE_USD_INGRESS_FEE_CAPACITY_PREFLIGHT_READY
        && crate::state::read_state(|state| state.three_usd_reserve_ingress_enabled)
}

pub fn set_three_usd_reserve_ingress_enabled(enabled: bool) {
    crate::state::mutate_state(|state| state.three_usd_reserve_ingress_enabled = enabled);
}

pub fn record_three_usd_reserve_ingress_absorbed(
    key: &crate::state::ThreeUsdReserveIngressKey,
    block_index: u64,
    result: crate::state::ThreeUsdReserveIngressResult,
) -> Result<(), String> {
    use crate::state::ThreeUsdReserveIngressPhase as Phase;
    crate::state::mutate_state(|state| {
        let Some(journal) = state.three_usd_reserve_ingress_journals.get_mut(key) else {
            return Err("reserve ingress journal disappeared before absorption receipt was stored".into());
        };
        let realized = if journal.request.icusd_debt_covered_e8s == 0 {
            0
        } else {
            ((journal.request.three_usd_amount_e8s as u128)
                .saturating_mul(result.liquidated_debt as u128)
                / journal.request.icusd_debt_covered_e8s as u128) as u64
        };
        let expected_refund = journal.request.three_usd_amount_e8s.saturating_sub(realized);
        match (expected_refund, journal.refund.as_ref()) {
            (0, None) => {
                journal.refund_fee_reserved_e8s = None;
            }
            (amount, Some(refund))
                if amount > 0
                    && refund.gross_amount_e8s == amount
                    && refund.source_subaccount.is_none() => {}
            _ => return Err("V2 reserve refund child is not durably linked before absorption terminalization".into()),
        }
        let tuple = match &journal.phase {
            Phase::TransferConfirmed { tuple, block_index: confirmed }
                if *confirmed == block_index => tuple.clone(),
            Phase::Absorbed { block_index: confirmed, .. } if *confirmed == block_index => return Ok(()),
            _ => return Err("reserve ingress journal is not confirmed for this transfer block".into()),
        };
        journal.phase = Phase::Absorbed { tuple, block_index, result };
        Ok(())
    })
}

pub fn record_three_usd_reserve_ingress_failed(
    key: &crate::state::ThreeUsdReserveIngressKey,
    block_index: u64,
    error: String,
) -> Result<(), String> {
    use crate::state::ThreeUsdReserveIngressPhase as Phase;
    crate::state::mutate_state(|state| {
        let Some(journal) = state.three_usd_reserve_ingress_journals.get_mut(key) else {
            return Err("reserve ingress journal disappeared before failure receipt was stored".into());
        };
        match journal.refund.as_ref() {
            Some(refund)
                if refund.gross_amount_e8s == journal.request.three_usd_amount_e8s
                    && refund.source_subaccount.is_none() => {}
            _ => return Err("V2 full-refund child is not durably linked before failure terminalization".into()),
        }
        let tuple = match &journal.phase {
            Phase::TransferConfirmed { tuple, block_index: confirmed }
                if *confirmed == block_index => tuple.clone(),
            Phase::FailedAfterTransfer { block_index: confirmed, .. } if *confirmed == block_index => return Ok(()),
            _ => return Err("reserve ingress journal is not confirmed for this transfer block".into()),
        };
        journal.phase = Phase::FailedAfterTransfer { tuple, block_index, error };
        Ok(())
    })
}

/// Pull 3USD into the backend's default account via ICRC-2. A complete tuple
/// is journaled before the first ledger await and reused on exact retries.
pub async fn transfer_3usd_to_reserves(
    key: crate::state::ThreeUsdReserveIngressKey,
    request: crate::state::ThreeUsdReserveIngressRequest,
) -> Result<u64, String> {
    use crate::state::{ThreeUsdReserveIngressPhase as Phase, ThreeUsdReserveIngressTuple};
    use icrc_ledger_types::icrc2::transfer_from::TransferFromArgs;

    let tuple = crate::state::mutate_state(|state| -> Result<ThreeUsdReserveIngressTuple, String> {
        if let Some(existing) = state.three_usd_reserve_ingress_journals.get(&key) {
            if existing.request != request {
                return Err("SP absorb ID was reused with different 3USD reserve arguments".into());
            }
            return match &existing.phase {
                Phase::AdmissionPending => {
                    if !THREE_USD_INGRESS_FEE_CAPACITY_PREFLIGHT_READY
                        || !state.three_usd_reserve_ingress_enabled
                    {
                        return Err("new 3USD reserve ingress is held until default-account fee capacity is proven".into());
                    }
                    let op_nonce = state.next_op_nonce();
                    let tuple = ThreeUsdReserveIngressTuple {
                        spender_owner: ic_cdk::id(),
                        spender_subaccount: None,
                        source: Account { owner: key.stability_pool, subaccount: None },
                        destination: Account { owner: ic_cdk::id(), subaccount: None },
                        amount_e8s: request.three_usd_amount_e8s,
                        fee_e8s: None,
                        memo: nonce_to_memo(op_nonce).0.as_slice().try_into()
                            .map_err(|_| "operation memo must be 16 bytes".to_string())?,
                        created_at_time_ns: nonce_to_created_at_time(op_nonce),
                        op_nonce,
                        parent_absorb_id: Some(key.absorb_id),
                    };
                    let journal = state.three_usd_reserve_ingress_journals.get_mut(&key)
                        .expect("admission row was observed above");
                    journal.phase = Phase::SubmittedOrUnknown { tuple: tuple.clone() };
                    Ok(tuple)
                }
                Phase::PreTransferRejected { reason } => Err(format!("SP absorb ID is terminally rejected before transfer: {reason}")),
                Phase::SubmittedOrUnknown { tuple }
                | Phase::TransferConfirmed { tuple, .. }
                | Phase::Absorbed { tuple, .. }
                | Phase::FailedAfterTransfer { tuple, .. } => Ok(tuple.clone()),
            };
        }
        if !THREE_USD_INGRESS_FEE_CAPACITY_PREFLIGHT_READY
            || !state.three_usd_reserve_ingress_enabled
        {
            return Err("new 3USD reserve ingress is held until default-account fee capacity is proven".into());
        }
        if key.stability_pool == Principal::anonymous()
            || request.ledger == Principal::anonymous()
            || request.three_usd_amount_e8s == 0
            || request.icusd_debt_covered_e8s == 0
        {
            return Err("invalid 3USD reserve ingress request".into());
        }
        Err("3USD reserve ingress ID must be durably admitted before transfer preparation".into())
    })?;

    if tuple.spender_owner != ic_cdk::id()
        || tuple.spender_subaccount.is_some()
        || tuple.source != (Account { owner: key.stability_pool, subaccount: None })
        || tuple.destination != (Account { owner: ic_cdk::id(), subaccount: None })
        || tuple.amount_e8s != request.three_usd_amount_e8s
        || tuple.fee_e8s.is_some()
        || tuple.memo.as_slice() != nonce_to_memo(tuple.op_nonce).0.as_slice()
        || tuple.created_at_time_ns != nonce_to_created_at_time(tuple.op_nonce)
    {
        return Err("stored 3USD reserve ingress tuple does not match its request key".into());
    }
    crate::storage::mark_three_usd_reserve_ingress_v2_used()?;

    match crate::state::read_state(|state| {
        state.three_usd_reserve_ingress_journals.get(&key).map(|journal| journal.phase.clone())
    }) {
        Some(Phase::TransferConfirmed { block_index, .. })
        | Some(Phase::Absorbed { block_index, .. })
        | Some(Phase::FailedAfterTransfer { block_index, .. }) => return Ok(block_index),
        Some(Phase::AdmissionPending) => return Err("3USD reserve ingress admission must be transferred to a submitted tuple first".into()),
        Some(Phase::PreTransferRejected { reason }) => return Err(format!("SP absorb ID is terminally rejected before transfer: {reason}")),
        Some(Phase::SubmittedOrUnknown { .. }) => {},
        None => return Err("3USD reserve ingress journal disappeared before dispatch".into()),
    }

    let client = ICRC1Client { runtime: CdkRuntime, ledger_canister_id: request.ledger };
    let outer = client.transfer_from(TransferFromArgs {
        spender_subaccount: tuple.spender_subaccount,
        from: tuple.source,
        to: tuple.destination,
        amount: Nat::from(tuple.amount_e8s),
        fee: tuple.fee_e8s.map(Nat::from),
        created_at_time: Some(tuple.created_at_time_ns),
        memo: Some(Memo::from(tuple.memo.to_vec())),
    }).await;
    let block_index = match outer {
        Ok(Ok(block)) => block.0.to_u64()
            .ok_or_else(|| "3USD reserve transfer block exceeds u64; reconcile stored tuple".to_string())?,
        Ok(Err(TransferFromError::Duplicate { duplicate_of })) => duplicate_of.0.to_u64()
            .ok_or_else(|| "3USD reserve duplicate block exceeds u64; reconcile stored tuple".to_string())?,
        Ok(Err(error)) => return Err(format!("3USD reserve transferFrom failed: {error:?}")),
        Err((code, message)) => return Err(format!("3USD reserve transferFrom call failed ({code:?}): {message}")),
    };

    crate::state::mutate_state(|state| -> Result<(), String> {
        let Some(journal) = state.three_usd_reserve_ingress_journals.get_mut(&key) else {
            return Err("3USD reserve ingress journal disappeared after dispatch".into());
        };
        match &journal.phase {
            Phase::SubmittedOrUnknown { tuple: stored } if stored == &tuple => {
                journal.phase = Phase::TransferConfirmed { tuple, block_index };
                Ok(())
            }
            Phase::TransferConfirmed { block_index: existing, .. }
            | Phase::Absorbed { block_index: existing, .. }
            | Phase::FailedAfterTransfer { block_index: existing, .. } if *existing == block_index => Ok(()),
            _ => Err("3USD reserve ingress journal changed during transfer dispatch".into()),
        }
    })?;
    Ok(block_index)
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

pub async fn get_icrc1_reserve_balance(ledger: Principal, account: Account) -> Result<u64, String> {
    if ledger == Principal::anonymous() || account.owner == Principal::anonymous() {
        return Err("ICRC-1 reserve ledger and account principals must be configured".into());
    }
    let result: Result<(Nat,), _> = ic_cdk::call(ledger, "icrc1_balance_of", (account,)).await;
    match result {
        Ok((balance,)) => balance.0.to_u64()
            .ok_or_else(|| "ICRC-1 reserve balance exceeds u64 range".to_string()),
        Err((code, message)) => Err(format!("icrc1_balance_of failed: {code:?} {message}")),
    }
}

/// Sweep funds from a deposit subaccount into the protocol's main account.
/// Returns (amount_received, sweep_block_index) where amount is balance minus ledger fee.
pub async fn sweep_deposit(
    caller: &Principal,
    ledger: Principal,
    ledger_fee: u64,
) -> Result<(u64, u64), String> {
    let subaccount = compute_deposit_subaccount(caller);
    let deposit_account = Account {
        owner: ic_cdk::id(),
        subaccount: Some(subaccount),
    };

    // Read how much is sitting in the deposit subaccount
    let balance = get_balance_of(deposit_account, ledger).await?;

    if balance == 0 {
        return Err("No deposit found in subaccount".to_string());
    }

    if balance <= ledger_fee {
        return Err(format!(
            "Deposit balance ({}) is not enough to cover the ledger fee ({})",
            balance, ledger_fee
        ));
    }

    let transfer_amount = balance - ledger_fee;
    let op_nonce = crate::state::mutate_state(|s| s.next_op_nonce());

    let block_index_u64 = transfer_idempotent(
        ledger,
        Some(subaccount),
        Account {
            owner: ic_cdk::id(),
            subaccount: None,
        },
        transfer_amount as u128,
        op_nonce,
        None,
    )
    .await
    .map_err(|e| format!("sweep transfer error: {:?}", e))?;

    log!(DEBUG,
        "[sweep_deposit] Swept {} from subaccount for {} on ledger {} (block {})",
        transfer_amount, caller, ledger, block_index_u64
    );

    Ok((transfer_amount, block_index_u64))
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
