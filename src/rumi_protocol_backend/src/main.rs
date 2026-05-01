use candid::{candid_method, Principal};
use ic_canister_log::log;
use ic_canisters_http_types::{HttpRequest, HttpResponse, HttpResponseBuilder};
use ic_cdk_macros::{init, pre_upgrade, post_upgrade, query, update};
use rumi_protocol_backend::{
    event::Event,
    logs::INFO,
    numeric::{ICUSD, ICP, Ratio, UsdIcp},
    state::{read_state, replace_state, Mode, State, RateCurveV2, DEFAULT_INTEREST_RATE_APR},
    vault::{CandidVault, OpenVaultSuccess, VaultArg},
    EventTypeFilter, Fees, GetEventsArg, ProtocolArg, ProtocolError, ProtocolStatus, SuccessWithFee,
    ReserveRedemptionResult, ReserveBalance, CollateralTotals, CollateralInterestInfo, PerCollateralRateCurve,
    VaultArgWithToken, StableTokenType, InterestSplitArg,
    GetSnapshotsArg, ProtocolSnapshot, CollateralSnapshot,
    GetEventsFilteredResponse, StabilityPoolLiquidationResult,
};
use rumi_protocol_backend::logs::DEBUG;
use rumi_protocol_backend::state::mutate_state;
use rumi_protocol_backend::management;
use rumi_protocol_backend::event;
use rumi_protocol_backend::treasury;
use rust_decimal::prelude::FromPrimitive;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use rumi_protocol_backend::storage::events;
use rumi_protocol_backend::LiquidityStatus;
use candid::{CandidType, Deserialize};

/// Stability pool configuration
#[derive(CandidType, Deserialize, Debug)]
pub struct StabilityPoolConfig {
    pub stability_pool_canister: Option<Principal>,
    pub liquidation_discount: u64,
    pub enabled: bool,
}

#[cfg(feature = "self_check")]
fn ok_or_die(result: Result<(), String>) {
    if let Err(msg) = result {
        ic_cdk::println!("{}", msg);
        ic_cdk::trap(&msg);
    }
}

/// Checks that Elliptic Core Canister state is internally consistent.
#[cfg(feature = "self_check")]
fn check_invariants() -> Result<(), String> {
    use protocol_canister::event::replay;

    read_state(|s| {
        s.check_invariants()?;

        let events: Vec<_> = protocol_canister::storage::events().collect();
        let recovered_state = replay(events.clone().into_iter())
            .unwrap_or_else(|e| panic!("failed to replay log {:?}: {:?}", events, e));

        recovered_state.check_invariants()?;

        // A running timer can temporarily violate invariants.
        if (!s.is_timer_running) {
            s.check_semantically_eq(&recovered_state)?;
        }

        Ok(())
    })
}

fn check_postcondition<T>(t: T) -> T {
    #[cfg(feature = "self_check")]
    ok_or_die(check_invariants());
    t
}

/// Validates caller identity and ensures a fresh price is available.
/// If the cached ICP price is older than 30 seconds, triggers an on-demand
/// XRC fetch before proceeding. This allows the background timer to poll
/// lazily (every 300s) while guaranteeing fresh prices for actual operations.
async fn validate_call() -> Result<(), ProtocolError> {
    if ic_cdk::caller() == Principal::anonymous() {
        return Err(ProtocolError::AnonymousCallerNotAllowed);
    }
    // Freeze check — if frozen, reject ALL state-changing operations
    if read_state(|s| s.frozen) {
        return Err(ProtocolError::TemporarilyUnavailable(
            "Protocol is frozen. All operations are suspended pending admin review.".to_string(),
        ));
    }
    rumi_protocol_backend::xrc::ensure_fresh_price().await
}

fn validate_mode() -> Result<(), ProtocolError> {
    match read_state(|s| s.mode) {
        Mode::ReadOnly => {
            Err(ProtocolError::TemporarilyUnavailable(
                "protocol temporarly unavailable, please wait for an upgrade or for total collateral ratio to go above 100%".to_string(),
            ))
        }
        Mode::GeneralAvailability => Ok(()),
        Mode::Recovery => Ok(())
    }
}

/// Validates price freshness for liquidation operations.
/// Liquidations are critical for protocol solvency, so we require fresh prices.
fn validate_price_for_liquidation() -> Result<(), ProtocolError> {
    read_state(|s| s.check_price_not_too_old())
}

/// Wave-5 LIQ-007: emergency brake for liquidations. Decoupled from `validate_mode`
/// because ReadOnly auto-latches on TCR < 100% and liquidations should remain open
/// in that state (they reduce bad debt). `liquidation_frozen` is the explicit
/// admin switch to halt liquidations during a confirmed oracle/dependency outage
/// where liquidating against the cached price would be unsafe.
fn validate_liquidation_not_frozen() -> Result<(), ProtocolError> {
    if read_state(|s| s.liquidation_frozen) {
        return Err(ProtocolError::TemporarilyUnavailable(
            "Liquidations are currently frozen by admin.".to_string(),
        ));
    }
    Ok(())
}

/// Wave-5 LIQ-006: refresh the cached price for a vault's collateral type before
/// a liquidation runs. `validate_price_for_liquidation` only checks the ICP
/// timestamp; for non-ICP vaults the cached `last_price` could be arbitrarily
/// stale if that collateral's background fetch has been failing. Looks up
/// `vault.collateral_type` via a small read_state, then awaits the on-demand
/// fetch. If the vault doesn't exist, we let the downstream call surface its
/// own `Vault not found` error rather than masking it here.
async fn validate_freshness_for_vault(vault_id: u64) -> Result<(), ProtocolError> {
    let collateral_type = read_state(|s| {
        s.vault_id_to_vaults.get(&vault_id).map(|v| v.collateral_type)
    });
    match collateral_type {
        Some(ct) => rumi_protocol_backend::xrc::ensure_fresh_price_for(&ct).await,
        None => Ok(()),
    }
}

/// Pre-filter to reduce cycle waste from anonymous spam.
/// Runs on ONE replica without consensus. Can be bypassed by malicious nodes.
/// NOT a security boundary — all real access control is inside each #[update] method.
#[ic_cdk_macros::inspect_message]
fn inspect_message() {
    let method = ic_cdk::api::call::method_name();
    let caller = ic_cdk::caller();

    match method.as_str() {
        // Query-like reads exposed as update for certification: accept all callers
        "icrc21_canister_call_consent_message" | "icrc10_supported_standards" => {
            ic_cdk::api::call::accept_message();
        }
        // Everything else requires a non-anonymous caller
        _ => {
            if caller != Principal::anonymous() {
                ic_cdk::api::call::accept_message();
            }
            // Anonymous callers silently rejected — saves cycles on Candid decoding
        }
    }
}

fn setup_timers() {
    // ── Immediate price fetch (fire on the very next execution round) ───────
    // Prices are ephemeral and not stored as events, so after an upgrade
    // the collateral configs have stale or missing prices.  An immediate
    // fetch ensures CRs are correct within seconds instead of waiting
    // up to 5 minutes for the first interval tick.
    ic_cdk_timers::set_timer(std::time::Duration::ZERO, || {
        ic_cdk::spawn(rumi_protocol_backend::xrc::fetch_icp_rate())
    });
    let non_icp_collaterals_immediate: Vec<candid::Principal> = read_state(|s| {
        let icp = s.icp_collateral_type();
        s.collateral_configs.keys()
            .filter(|ct| **ct != icp)
            .cloned()
            .collect()
    });
    for ledger_id in non_icp_collaterals_immediate {
        ic_cdk_timers::set_timer(std::time::Duration::ZERO, move || {
            ic_cdk::spawn(rumi_protocol_backend::management::fetch_collateral_price(ledger_id))
        });
    }

    // ── Recurring price fetching timers ─────────────────────────────────────
    // ICP rate fetching timer
    ic_cdk_timers::set_timer_interval(rumi_protocol_backend::xrc::FETCHING_ICP_RATE_INTERVAL, || {
        ic_cdk::spawn(rumi_protocol_backend::xrc::fetch_icp_rate())
    });

    // Price timers for all non-ICP collateral types (timers don't survive upgrades,
    // so we re-register them here for any collateral added via add_collateral_token).
    let non_icp_collaterals: Vec<candid::Principal> = read_state(|s| {
        let icp = s.icp_collateral_type();
        s.collateral_configs.keys()
            .filter(|ct| **ct != icp)
            .cloned()
            .collect()
    });
    for ledger_id in non_icp_collaterals {
        log!(INFO, "[setup_timers] Registering price timer for collateral {}", ledger_id);
        ic_cdk_timers::set_timer_interval(
            rumi_protocol_backend::xrc::FETCHING_ICP_RATE_INTERVAL,
            move || ic_cdk::spawn(rumi_protocol_backend::management::fetch_collateral_price(ledger_id)),
        );
    }

    // clean_stale_operations timer removed — the old implementation dangerously
    // auto-reset Recovery→GA mode based on a timeout. Mode is now managed by
    // update_mode() (automatic) and admin functions (manual).

    // ── Hourly protocol snapshot ────────────────────────────────────────────
    // First snapshot fires after 5 seconds (let prices load first).
    ic_cdk_timers::set_timer(std::time::Duration::from_secs(5), || {
        capture_protocol_snapshot();
    });
    ic_cdk_timers::set_timer_interval(std::time::Duration::from_secs(3600), || {
        capture_protocol_snapshot();
    });
}

fn capture_protocol_snapshot() {
    let snapshot = read_state(|s| {
        let mut total_collateral_value_usd: u64 = 0;
        let mut total_debt: u64 = 0;
        let mut total_vault_count: u64 = 0;
        let mut collateral_snapshots = Vec::new();

        for (ct, config) in s.collateral_configs.iter() {
            let col_total = s.total_collateral_for(ct);
            let debt = s.total_debt_for_collateral(ct).to_u64();
            let vault_count = s.collateral_to_vault_ids
                .get(ct)
                .map(|ids| ids.len() as u64)
                .unwrap_or(0);
            let price = config.last_price.unwrap_or(0.0);

            // Convert collateral to USD value (e8s)
            let col_decimal = Decimal::from(col_total)
                / Decimal::from(10u64.pow(config.decimals as u32));
            let usd_value = (col_decimal * Decimal::try_from(price).unwrap_or_default())
                * Decimal::from(100_000_000u64);
            let usd_e8s = usd_value.to_u64().unwrap_or(0);

            total_collateral_value_usd += usd_e8s;
            total_debt += debt;
            total_vault_count += vault_count;

            collateral_snapshots.push(CollateralSnapshot {
                collateral_type: *ct,
                total_collateral: col_total,
                total_debt: debt,
                vault_count,
                price,
            });
        }

        ProtocolSnapshot {
            timestamp: ic_cdk::api::time(),
            total_collateral_value_usd,
            total_debt,
            total_vault_count,
            collateral_snapshots,
        }
    });

    rumi_protocol_backend::storage::record_snapshot(&snapshot);
}

fn main() {}

#[candid_method(init)]
#[init]
fn init(arg: ProtocolArg) {
    // UPG-006: refuse to init with non-empty stable memory. Catches accidental
    // reinstalls of a canister that already has persisted state. Reinstall mode
    // wipes stable memory before init runs (per IC spec), so this primarily
    // documents intent and guards against future IC behavior changes or hand-
    // crafted install_code calls that do not zero stable memory first.
    assert!(
        ic_cdk::api::stable::stable64_size() == 0,
        "refusing to init: stable memory non-empty; use upgrade mode not reinstall"
    );
    match arg {
        ProtocolArg::Init(init_arg) => {
            log!(
                INFO,
                "[init] initialized Rumi Protocol with args: {:?}",
                init_arg
            );
            rumi_protocol_backend::storage::record_event(&Event::Init(init_arg.clone()));
            replace_state(State::from(init_arg));
        }
        ProtocolArg::Upgrade(_) => ic_cdk::trap("expected Init got Upgrade"),
    }
    setup_timers();
}

#[pre_upgrade]
fn pre_upgrade() {
    use rumi_protocol_backend::storage::save_state_to_stable;

    read_state(|state| {
        save_state_to_stable(state);
    });

    log!(INFO, "[pre_upgrade]: state serialized to stable memory");
}

#[post_upgrade]
fn post_upgrade(arg: ProtocolArg) {
    use rumi_protocol_backend::event::replay;
    use rumi_protocol_backend::storage::{count_events, events, record_event};

    let start = ic_cdk::api::instruction_counter();

    // Extract and record the upgrade event
    let upgrade_args = match arg {
        ProtocolArg::Init(_) => ic_cdk::trap("expected Upgrade got Init"),
        ProtocolArg::Upgrade(args) => {
            log!(
                INFO,
                "[upgrade]: updating configuration with {:?}",
                args
            );
            record_event(&Event::Upgrade(args.clone()));
            args
        }
    };

    // Try to restore from stable memory (fast path, no drift)
    let state = match rumi_protocol_backend::storage::load_state_from_stable() {
        Some(mut state) => {
            log!(INFO, "[upgrade]: restored state from stable memory (skipped event replay of {} events)", count_events());
            // Apply upgrade args to the restored state (the snapshot was taken
            // before this upgrade event, so we must apply it explicitly)
            state.upgrade(upgrade_args);
            state
        }
        None => {
            // Fallback: replay events (first upgrade after this change, or recovery)
            log!(INFO, "[upgrade]: no stable state found, replaying {} events", count_events());
            replay(events()).unwrap_or_else(|e| {
                ic_cdk::trap(&format!(
                    "[upgrade]: failed to replay the event log: {:?}",
                    e
                ))
            })
        }
    };

    // Post-upgrade validation: ensure collateral_configs is consistent
    validate_collateral_state(&state);

    replace_state(state);

    // Migration: set last_accrual_time for any existing vaults that have it at 0.
    // This avoids a massive retroactive accrual on first tick.
    let now = ic_cdk::api::time();
    let migrated = mutate_state(|s| {
        let mut count = 0u64;
        for vault in s.vault_id_to_vaults.values_mut() {
            if vault.last_accrual_time == 0 {
                vault.last_accrual_time = now;
                count += 1;
            }
        }
        count
    });
    if migrated > 0 {
        log!(INFO, "[upgrade]: migrated {} vaults: set last_accrual_time to {}", migrated, now);
    }

    // Safety net: if bot is configured but allowlist is empty, default to ICP
    mutate_state(|s| {
        if s.liquidation_bot_principal.is_some() && s.bot_allowed_collateral_types.is_empty() {
            s.bot_allowed_collateral_types.insert(s.icp_ledger_principal);
            log!(INFO, "[upgrade]: bot_allowed_collateral_types was empty, defaulted to ICP");
        }
    });

    // Wave-3 migration: backfill op_nonce on pending transfers carried over from
    // pre-Wave-3 snapshots so their retries get ledger-side dedup. Without this,
    // legacy entries stay at op_nonce: 0 (TooOld at the ledger) and never finish.
    //
    // Wave-4 LIQ-001: pending_margin_transfers and pending_excess_transfers are now
    // keyed by (vault_id, owner). Legacy entries from pre-Wave-4 snapshots are
    // re-keyed transparently by `state::deserialize_pending_keyed`, so by the time
    // this block runs they already have tuple keys.
    mutate_state(|s| {
        let mut backfilled = 0u64;
        let margin_keys: Vec<(u64, candid::Principal)> = s.pending_margin_transfers.iter()
            .filter(|(_, t)| t.op_nonce == 0)
            .map(|(k, _)| *k)
            .collect();
        for k in margin_keys {
            let nonce = s.next_op_nonce();
            if let Some(t) = s.pending_margin_transfers.get_mut(&k) {
                t.op_nonce = nonce;
                backfilled += 1;
            }
        }
        let excess_keys: Vec<(u64, candid::Principal)> = s.pending_excess_transfers.iter()
            .filter(|(_, t)| t.op_nonce == 0)
            .map(|(k, _)| *k)
            .collect();
        for k in excess_keys {
            let nonce = s.next_op_nonce();
            if let Some(t) = s.pending_excess_transfers.get_mut(&k) {
                t.op_nonce = nonce;
                backfilled += 1;
            }
        }
        let redemption_ids: Vec<u64> = s.pending_redemption_transfer.iter()
            .filter(|(_, t)| t.op_nonce == 0)
            .map(|(id, _)| *id)
            .collect();
        for id in redemption_ids {
            let nonce = s.next_op_nonce();
            if let Some(t) = s.pending_redemption_transfer.get_mut(&id) {
                t.op_nonce = nonce;
                backfilled += 1;
            }
        }
        if backfilled > 0 {
            log!(INFO, "[upgrade]: backfilled op_nonce on {} legacy pending transfers (Wave-3 migration)", backfilled);
        }
    });

    // One-time: remove PHASMA test collateral and clean up empty vaults
    mutate_state(|s| {
        let phasma = candid::Principal::from_text("np5km-uyaaa-aaaaq-aadrq-cai").unwrap();
        if s.collateral_configs.remove(&phasma).is_some() {
            log!(INFO, "[upgrade]: removed PHASMA test collateral from configs");
        }
        // Remove empty vaults (fully liquidated shells with zero debt and zero collateral)
        let empty_vault_ids: Vec<u64> = s.vault_id_to_vaults.iter()
            .filter(|(_, v)| v.borrowed_icusd_amount.0 == 0 && v.collateral_amount == 0)
            .map(|(id, _)| *id)
            .collect();
        for vault_id in &empty_vault_ids {
            if let Some(vault) = s.vault_id_to_vaults.remove(vault_id) {
                if let Some(ids) = s.principal_to_vault_ids.get_mut(&vault.owner) {
                    ids.retain(|id| id != vault_id);
                    if ids.is_empty() {
                        s.principal_to_vault_ids.remove(&vault.owner);
                    }
                }
            }
        }
        if !empty_vault_ids.is_empty() {
            log!(INFO, "[upgrade]: cleaned up {} empty vaults: {:?}", empty_vault_ids.len(), empty_vault_ids);
        }
    });

    // Wave-8b LIQ-002 migration: rebuild `vault_cr_index` from
    // `vault_id_to_vaults`. The index is `serde(skip_serializing)` (kept out
    // of the on-disk snapshot to avoid a state-format migration), so it is
    // empty after `replace_state(state)`. Walking the surviving vaults and
    // re-keying each one converges the index to the post-upgrade CR
    // distribution. O(N log N) one-shot. Empty for fresh installs.
    let reindexed = mutate_state(|s| {
        let vault_ids: Vec<u64> = s.vault_id_to_vaults.keys().copied().collect();
        let count = vault_ids.len();
        for vid in vault_ids {
            s.reindex_vault_cr(vid);
        }
        count
    });
    log!(
        INFO,
        "[upgrade]: Wave-8b LIQ-002 migration rebuilt vault_cr_index for {} vault(s)",
        reindexed,
    );

    let end = ic_cdk::api::instruction_counter();

    log!(
        INFO,
        "[upgrade]: replaying events consumed {} instructions",
        end - start
    );

    // Defense-in-depth: clear transient runtime locks unconditionally on every
    // upgrade. The matching State fields now use `serde(skip_serializing)` so
    // future upgrades won't re-introduce a stuck lock, but snapshots written by
    // the OLD code (before that change shipped) can still carry `true`. Locks
    // guard in-flight async futures that the upgrade has already killed, so
    // resetting them here is always correct.
    mutate_state(|s| {
        s.is_fetching_rate = false;
        s.is_timer_running = false;
    });

    setup_timers();
}

/// Validates that the State has consistent collateral configuration after replay.
/// Logs warnings for any inconsistencies but does not trap — the canister must
/// still upgrade successfully even if data is slightly off.
fn validate_collateral_state(state: &State) {
    // 1. Check that ICP is in collateral_configs
    let icp = state.icp_collateral_type();
    if !state.collateral_configs.contains_key(&icp) {
        log!(INFO, "[post_upgrade_validation] WARNING: ICP ledger {} not found in collateral_configs!", icp);
    } else {
        log!(INFO, "[post_upgrade_validation] ICP collateral config present");
    }

    // 2. Check that all vaults reference a known collateral type
    let mut orphaned_vaults = 0u64;
    for (vault_id, vault) in &state.vault_id_to_vaults {
        if vault.collateral_type == candid::Principal::anonymous() {
            log!(INFO, "[post_upgrade_validation] WARNING: vault #{} still has anonymous collateral_type", vault_id);
            orphaned_vaults += 1;
        } else if !state.collateral_configs.contains_key(&vault.collateral_type) {
            log!(INFO, "[post_upgrade_validation] WARNING: vault #{} references unknown collateral {}", vault_id, vault.collateral_type);
            orphaned_vaults += 1;
        }
    }
    if orphaned_vaults == 0 {
        log!(INFO, "[post_upgrade_validation] All {} vaults have valid collateral_type", state.vault_id_to_vaults.len());
    } else {
        log!(INFO, "[post_upgrade_validation] {} vault(s) with invalid collateral_type!", orphaned_vaults);
    }

    // 3. Log summary of collateral configs
    log!(INFO, "[post_upgrade_validation] {} collateral types configured", state.collateral_configs.len());
    for (ct, config) in &state.collateral_configs {
        log!(INFO, "[post_upgrade_validation]   {} => status={:?}, decimals={}, price={:?}",
            ct, config.status, config.decimals, config.last_price);
    }
}

#[candid_method(query)]
#[query]
fn get_protocol_status() -> ProtocolStatus {
    read_state(|s| ProtocolStatus {
        last_icp_rate: s
            .last_icp_rate
            .unwrap_or(UsdIcp::from(Decimal::ZERO))
            .to_f64(),
        last_icp_timestamp: s.last_icp_timestamp.unwrap_or(0),
        total_icp_margin: s.total_icp_margin_amount().to_u64(),
        total_icusd_borrowed: s.total_borrowed_icusd_amount().to_u64(),
        total_collateral_ratio: s.total_collateral_ratio.to_f64(),
        mode: s.mode,
        liquidation_bonus: s.liquidation_bonus.to_f64(),
        recovery_target_cr: (s.recovery_mode_threshold * s.recovery_cr_multiplier).to_f64(),
        recovery_mode_threshold: s.recovery_mode_threshold.to_f64(),
        recovery_cr_multiplier: s.recovery_cr_multiplier.to_f64(),
        reserve_redemptions_enabled: s.reserve_redemptions_enabled,
        reserve_redemption_fee: s.reserve_redemption_fee.to_f64(),
        ckstable_repay_fee: s.ckstable_repay_fee.to_f64(),
        min_icusd_amount: s.min_icusd_amount.to_u64(),
        global_icusd_mint_cap: s.global_icusd_mint_cap,
        frozen: s.frozen,
        manual_mode_override: s.manual_mode_override,
        interest_pool_share: s.interest_pool_share.to_f64(),
        weighted_average_interest_rate: s.weighted_average_interest_rate().to_f64(),
        borrowing_fee_curve_resolved: match &s.borrowing_fee_curve {
            Some(curve) => s.resolve_curve(curve, None).iter()
                .map(|(cr, mult)| (cr.to_f64(), mult.to_f64()))
                .collect(),
            None => vec![],
        },
        per_collateral_interest: s.collateral_configs.keys()
            .map(|ct| CollateralInterestInfo {
                collateral_type: *ct,
                total_debt_e8s: s.total_debt_for_collateral(ct).to_u64(),
                weighted_interest_rate: s.weighted_interest_rate_for_collateral(ct).to_f64(),
            })
            .collect(),
        per_collateral_rate_curves: s.collateral_configs.keys()
            .map(|ct| {
                let markers = s.resolve_layer1_markers(ct);
                let base = s.collateral_configs.get(ct)
                    .map(|c| c.interest_rate_apr).unwrap_or(DEFAULT_INTEREST_RATE_APR);
                PerCollateralRateCurve {
                    collateral_type: *ct,
                    base_rate: base.to_f64(),
                    markers: markers.iter().map(|(cr, m)| (cr.to_f64(), m.to_f64())).collect(),
                }
            }).collect(),
        interest_split: s.interest_split.iter().map(|r| {
            let dest = match &r.destination {
                rumi_protocol_backend::state::InterestDestination::StabilityPool => "stability_pool".to_string(),
                rumi_protocol_backend::state::InterestDestination::Treasury => "treasury".to_string(),
                rumi_protocol_backend::state::InterestDestination::ThreePool => "three_pool".to_string(),
            };
            InterestSplitArg { destination: dest, bps: r.bps }
        }).collect(),
        // Wave-8e LIQ-005
        protocol_deficit_icusd: s.protocol_deficit_icusd.to_u64(),
        total_deficit_repaid_icusd: s.total_deficit_repaid_icusd.to_u64(),
        deficit_repayment_fraction: s.deficit_repayment_fraction.to_f64(),
        deficit_readonly_threshold_e8s: s.deficit_readonly_threshold_e8s,
        // Wave-10 LIQ-008
        breaker_window_ns: s.breaker_window_ns,
        breaker_window_debt_ceiling_e8s: s.breaker_window_debt_ceiling_e8s,
        windowed_liquidation_total_e8s: s.windowed_liquidation_total(ic_cdk::api::time()),
        liquidation_breaker_tripped: s.liquidation_breaker_tripped,
    })
}

#[candid_method(query)]
#[query]
fn get_protocol_config() -> rumi_protocol_backend::ProtocolConfig {
    use rumi_protocol_backend::ProtocolConfig;
    read_state(|s| ProtocolConfig {
        mode: s.mode,
        frozen: s.frozen,
        manual_mode_override: s.manual_mode_override,

        borrowing_fee: s.get_borrowing_fee().to_f64(),
        redemption_fee_floor: s.redemption_fee_floor.to_f64(),
        redemption_fee_ceiling: s.redemption_fee_ceiling.to_f64(),
        reserve_redemption_fee: s.reserve_redemption_fee.to_f64(),
        ckstable_repay_fee: s.ckstable_repay_fee.to_f64(),
        liquidation_bonus: s.liquidation_bonus.to_f64(),
        liquidation_protocol_share: s.get_liquidation_protocol_share().to_f64(),

        rmr_floor: s.rmr_floor.to_f64(),
        rmr_ceiling: s.rmr_ceiling.to_f64(),
        rmr_floor_cr: s.rmr_floor_cr.to_f64(),
        rmr_ceiling_cr: s.rmr_ceiling_cr.to_f64(),

        recovery_cr_multiplier: s.recovery_cr_multiplier.to_f64(),
        recovery_mode_threshold: s.recovery_mode_threshold.to_f64(),
        max_partial_liquidation_ratio: s.max_partial_liquidation_ratio.to_f64(),

        min_icusd_amount: s.min_icusd_amount.to_u64(),
        global_icusd_mint_cap: s.global_icusd_mint_cap,
        interest_flush_threshold_e8s: s.interest_flush_threshold_e8s,

        interest_split: s.interest_split.iter().map(|r| {
            let dest = match &r.destination {
                rumi_protocol_backend::state::InterestDestination::StabilityPool => "stability_pool".to_string(),
                rumi_protocol_backend::state::InterestDestination::Treasury => "treasury".to_string(),
                rumi_protocol_backend::state::InterestDestination::ThreePool => "three_pool".to_string(),
            };
            InterestSplitArg { destination: dest, bps: r.bps }
        }).collect(),

        global_rate_curve: s.global_rate_curve.markers.iter()
            .map(|m| (m.cr_level.to_f64(), m.multiplier.to_f64()))
            .collect(),
        recovery_rate_curve: s.recovery_rate_curve.iter()
            .map(|m| (format!("{:?}", m.threshold), m.multiplier.to_f64()))
            .collect(),
        borrowing_fee_curve: match &s.borrowing_fee_curve {
            Some(curve) => s.resolve_curve(curve, None).iter()
                .map(|(cr, mult)| (cr.to_f64(), mult.to_f64()))
                .collect(),
            None => vec![],
        },

        reserve_redemptions_enabled: s.reserve_redemptions_enabled,
        ckusdt_enabled: s.ckusdt_enabled,
        ckusdc_enabled: s.ckusdc_enabled,

        icpswap_routing_enabled: s.icpswap_routing_enabled,

        treasury_principal: s.treasury_principal,
        stability_pool_canister: s.stability_pool_canister,
        three_pool_canister: s.three_pool_canister,
        ckusdt_ledger_principal: s.ckusdt_ledger_principal,
        ckusdc_ledger_principal: s.ckusdc_ledger_principal,

        liquidation_bot_principal: s.liquidation_bot_principal,
        bot_budget_total_e8s: s.bot_budget_total_e8s,
        bot_budget_remaining_e8s: s.bot_budget_remaining_e8s,
        bot_allowed_collateral_types: s.bot_allowed_collateral_types.iter().cloned().collect(),

        collateral_configs: s.collateral_configs.iter()
            .map(|(ct, config)| {
                let mut cfg = config.clone();
                cfg.recovery_target_cr = cfg.borrow_threshold_ratio * s.recovery_cr_multiplier;
                (*ct, cfg)
            })
            .collect(),
    })
}

#[candid_method(query)]
#[query]
fn get_fees(redeemed_amount: u64) -> Fees {
    read_state(|s| Fees {
        borrowing_fee: s.get_borrowing_fee().to_f64(),
        redemption_fee: s.get_redemption_fee(redeemed_amount.into()).to_f64(),
    })
}

#[candid_method(query)]
#[query]
fn get_vault_history(vault_id: u64) -> Vec<(u64, Event)> {
    if ic_cdk::api::data_certificate().is_none() {
        ic_cdk::trap("update call rejected");
    }

    // Iteration order matches the StableLog index, so enumerate() yields the
    // global event-log index alongside each event. The explorer surfaces these
    // ids on per-vault activity rows.
    let mut vault_events: Vec<(u64, Event)> = vec![];
    for (idx, event) in events().enumerate() {
        if event.is_vault_related(&vault_id) {
            vault_events.push((idx as u64, event));
        }
    }
    vault_events
}

#[candid_method(query)]
#[query]
fn get_events(args: GetEventsArg) -> Vec<Event> {
    const MAX_EVENTS_PER_QUERY: usize = 2000;

    events()
        .skip(args.start as usize)
        .take(MAX_EVENTS_PER_QUERY.min(args.length as usize))
        .collect()
}

#[candid_method(query)]
#[query]
fn get_event_count() -> u64 {
    rumi_protocol_backend::storage::count_events()
}

/// Recording-time timestamp for `length` consecutive events starting at
/// `start`. Slots past the end of the side log come back as `0`; the
/// frontend uses these to fill in a real time on admin/upgrade rows whose
/// event payloads have no inline `timestamp` field. Pre-existing events
/// (recorded before this side log shipped) also surface as `0`.
///
/// Cap is high enough (80k) to cover the entire current event log in a
/// single round-trip — at 8 bytes per nat64 that's a 640 KB response,
/// well under the 2 MB IC reply limit. Without that headroom the
/// frontend's mixed-feed admin scope (which spans tens of thousands of
/// indices) misses every event past the first 2k of the requested range.
#[candid_method(query)]
#[query]
fn get_event_timestamps(start: u64, length: u64) -> Vec<u64> {
    if ic_cdk::api::data_certificate().is_none() {
        ic_cdk::trap("update call rejected");
    }
    const MAX: u64 = 80_000;
    rumi_protocol_backend::storage::get_event_timestamps(start, length.min(MAX))
}

/// Server-side filtered event query, paginated newest-first.
/// `start` is the page number (0-indexed) into the *filtered* result set;
/// `length` is page size (capped at `MAX_PAGE_SIZE`).
///
/// Filter semantics (all AND-combined):
/// - `types`: empty/null preserves the legacy behavior of hiding
///   `AccrueInterest`/`PriceUpdate`. When non-empty, only matching variants
///   are included (those two are returnable if explicitly requested).
/// - `principal`: matches via `Event::involves_principal`.
/// - `collateral_token`: matches via `Event::collateral_token` using a
///   per-query `vault_id → collateral_type` lookup built from `OpenVault`.
/// - `time_range`: events with no `timestamp_ns` are excluded.
/// - `min_size_e8s`: events with no `size_e8s_usd` pass through.
///
/// `total` is the matched count across the entire log (not the scanned slice),
/// so the frontend can render accurate result counters.
///
/// Results are cached for `FILTERED_EVENTS_TTL_NS` (10s) keyed on the full
/// filter spec + page, since events append continuously and stale results
/// would hide just-recorded activity.
#[candid_method(query)]
#[query]
fn get_events_filtered(args: GetEventsArg) -> GetEventsFilteredResponse {
    if ic_cdk::api::data_certificate().is_none() {
        ic_cdk::trap("update call rejected");
    }
    const MAX_PAGE_SIZE: usize = 200;
    let page_size = MAX_PAGE_SIZE.min(args.length as usize);
    let page = args.start as usize;

    let now = ic_cdk::api::time();
    let cache_key = filtered_events_cache_key(&args, page, page_size);
    if let Some(cached) = read_filtered_events_cache(cache_key, now) {
        return cached;
    }

    let vault_lookup = if args.collateral_token.is_some() {
        build_vault_collateral_lookup()
    } else {
        std::collections::HashMap::new()
    };

    let icp_price_e8s = read_state(|s| {
        s.collateral_configs
            .get(&s.icp_ledger_principal)
            .and_then(|c| c.last_price)
            .map(|p| (p * 100_000_000.0) as u64)
            .unwrap_or(0)
    });

    let types_set: Option<std::collections::HashSet<EventTypeFilter>> = args.types
        .as_ref()
        .filter(|v| !v.is_empty())
        .map(|v| v.iter().cloned().collect());

    let admin_labels_set: Option<std::collections::HashSet<String>> = args.admin_labels
        .as_ref()
        .filter(|v| !v.is_empty())
        .map(|v| v.iter().cloned().collect());

    let filtered: Vec<(u64, Event)> = events()
        .enumerate()
        .filter(|(_, e)| e.passes_filters(
            types_set.as_ref(),
            args.principal.as_ref(),
            args.collateral_token.as_ref(),
            args.time_range.as_ref(),
            args.min_size_e8s,
            admin_labels_set.as_ref(),
            &vault_lookup,
            icp_price_e8s,
        ))
        .map(|(i, e)| (i as u64, e))
        .collect();

    let total = filtered.len() as u64;
    let start_idx = page * page_size;
    let page_events: Vec<(u64, Event)> = filtered.into_iter()
        .rev()
        .skip(start_idx)
        .take(page_size)
        .collect();

    let resp = GetEventsFilteredResponse {
        total,
        events: page_events,
    };
    write_filtered_events_cache(cache_key, now, &resp);
    resp
}

/// Build `vault_id → collateral_type` by walking `OpenVault` events.
/// Called only when the `collateral_token` filter is active. Cheap relative
/// to the surrounding event scan since `OpenVault` is a small fraction of
/// total events.
fn build_vault_collateral_lookup() -> std::collections::HashMap<u64, Principal> {
    let mut map = std::collections::HashMap::new();
    for event in events() {
        if let Event::OpenVault { vault, .. } = event {
            map.insert(vault.vault_id, vault.collateral_type);
        }
    }
    map
}

const FILTERED_EVENTS_TTL_NS: u64 = 10 * 1_000_000_000;

thread_local! {
    static FILTERED_EVENTS_CACHE: std::cell::RefCell<
        std::collections::HashMap<u64, (u64, GetEventsFilteredResponse)>
    > = std::cell::RefCell::new(std::collections::HashMap::new());
}

fn filtered_events_cache_key(args: &GetEventsArg, page: usize, page_size: usize) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    page.hash(&mut hasher);
    page_size.hash(&mut hasher);
    args.types.hash(&mut hasher);
    args.principal.hash(&mut hasher);
    args.collateral_token.hash(&mut hasher);
    args.time_range.hash(&mut hasher);
    args.min_size_e8s.hash(&mut hasher);
    hasher.finish()
}

fn read_filtered_events_cache(key: u64, now: u64) -> Option<GetEventsFilteredResponse> {
    FILTERED_EVENTS_CACHE.with(|c| {
        c.borrow().get(&key).and_then(|(at, resp)| {
            if now.saturating_sub(*at) < FILTERED_EVENTS_TTL_NS {
                Some(GetEventsFilteredResponse {
                    total: resp.total,
                    events: resp.events.clone(),
                })
            } else {
                None
            }
        })
    })
}

fn write_filtered_events_cache(key: u64, now: u64, resp: &GetEventsFilteredResponse) {
    FILTERED_EVENTS_CACHE.with(|c| {
        let snapshot = GetEventsFilteredResponse {
            total: resp.total,
            events: resp.events.clone(),
        };
        c.borrow_mut().insert(key, (now, snapshot));
    });
}

/// Return all events involving a given principal (as owner, caller, or liquidator).
#[candid_method(query)]
#[query]
fn get_events_by_principal(principal: Principal) -> Vec<(u64, Event)> {
    if ic_cdk::api::data_certificate().is_none() {
        ic_cdk::trap("update call rejected");
    }
    const MAX_RESULTS: usize = 500;

    events()
        .enumerate()
        .filter(|(_, e)| !e.is_accrue_interest() && e.involves_principal(&principal))
        .map(|(i, e)| (i as u64, e))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .take(MAX_RESULTS)
        .collect()
}

#[candid_method(query)]
#[query]
fn get_protocol_snapshots(args: GetSnapshotsArg) -> Vec<ProtocolSnapshot> {
    if ic_cdk::api::data_certificate().is_none() {
        ic_cdk::trap("update call rejected");
    }
    const MAX_SNAPSHOTS_PER_QUERY: usize = 2000;

    rumi_protocol_backend::storage::snapshots()
        .skip(args.start as usize)
        .take(MAX_SNAPSHOTS_PER_QUERY.min(args.length as usize))
        .collect()
}

#[candid_method(query)]
#[query]
fn get_snapshot_count() -> u64 {
    rumi_protocol_backend::storage::count_snapshots()
}

#[candid_method(query)]
#[query]
fn get_liquidity_status(owner: Principal) -> LiquidityStatus {
    let total_liquidity_provided = read_state(|s| s.total_provided_liquidity_amount());
    let liquidity_pool_share = if total_liquidity_provided == 0 {
        0.0
    } else {
        read_state(|s| {
            (s.get_provided_liquidity(owner) / s.total_provided_liquidity_amount()).to_f64()
        })
    };
    read_state(|s| LiquidityStatus {
        liquidity_provided: s.get_provided_liquidity(owner).to_u64(),
        total_liquidity_provided: s.total_provided_liquidity_amount().to_u64(),
        liquidity_pool_share,
        available_liquidity_reward: s.get_liquidity_returns_of(owner).to_u64(),
        total_available_returns: s.total_available_returns().to_u64(),
    })
}

#[candid_method(query)]
#[query]
fn get_vaults(target: Option<Principal>) -> Vec<CandidVault> {
    match target {
        Some(target) => read_state(|s| match s.principal_to_vault_ids.get(&target) {
            Some(vault_ids) => vault_ids
                .iter()
                .map(|id| {
                    let vault = s.vault_id_to_vaults.get(id).cloned().unwrap();
                    CandidVault::from(vault)
                })
                .collect(),
            None => vec![],
        }),
        None => read_state(|s| {
            s.vault_id_to_vaults
                .values()
                .cloned()
                .map(CandidVault::from)
                .collect::<Vec<CandidVault>>()
        }),
    }
}

// Vault related operations
#[candid_method(update)]
#[update]
async fn redeem_icp(icusd_amount: u64) -> Result<SuccessWithFee, ProtocolError> {
    validate_call().await?;
    check_postcondition(rumi_protocol_backend::vault::redeem_icp(icusd_amount).await)
}

/// Generic collateral redemption: burn icUSD and receive any collateral type.
/// `redeem_icp` remains as a convenience wrapper for ICP specifically.
#[candid_method(update)]
#[update]
async fn redeem_collateral(collateral_type: Principal, icusd_amount: u64) -> Result<SuccessWithFee, ProtocolError> {
    validate_call().await?;
    // Wave-5 RED-001: validate_call only refreshes ICP. For non-ICP collaterals
    // (BOB, EXE, ckBTC, ckETH, ckXAUT, nICP) the redeemer would otherwise pay
    // out at whatever last_price is cached, which could be hours stale if the
    // background timer for that asset has been failing. ensure_fresh_price_for
    // delegates to ensure_fresh_price for ICP (already handled), so this is
    // safe to call unconditionally.
    rumi_protocol_backend::xrc::ensure_fresh_price_for(&collateral_type).await?;
    check_postcondition(rumi_protocol_backend::vault::redeem_collateral(collateral_type, icusd_amount).await)
}

#[candid_method(query)]
#[query]
fn get_redemption_rate() -> f64 {
    read_state(|s| {
        s.get_redemption_fee(
            ICUSD::from(100_000_000),
        ).to_f64()
    })
}

#[candid_method(update)]
#[update]
async fn open_vault(collateral_amount: u64, collateral_type: Option<Principal>) -> Result<OpenVaultSuccess, ProtocolError> {
    validate_call().await?;
    check_postcondition(rumi_protocol_backend::vault::open_vault(collateral_amount, collateral_type).await)
}

/// Compound open vault + borrow in a single canister call.
/// Allows Oisy / ICRC-112 wallets to batch approve + this call into one popup.
#[candid_method(update)]
#[update]
async fn open_vault_and_borrow(
    collateral_amount: u64,
    borrow_amount: u64,
    collateral_type: Option<Principal>,
) -> Result<OpenVaultSuccess, ProtocolError> {
    validate_call().await?;
    validate_mode()?;
    check_postcondition(
        rumi_protocol_backend::vault::open_vault_and_borrow(collateral_amount, borrow_amount, collateral_type).await,
    )
}

#[candid_method(update)]
#[update]
async fn borrow_from_vault(arg: VaultArg) -> Result<SuccessWithFee, ProtocolError> {
    validate_call().await?;
    validate_mode()?;
    check_postcondition(rumi_protocol_backend::vault::borrow_from_vault(arg).await)
}

#[candid_method(update)]
#[update]
async fn repay_to_vault(arg: VaultArg) -> Result<u64, ProtocolError> {
    validate_call().await?;
    check_postcondition(rumi_protocol_backend::vault::repay_to_vault(arg).await)
}

/// Repay vault debt using ckUSDT or ckUSDC (1:1 with icUSD)
#[candid_method(update)]
#[update]
async fn repay_to_vault_with_stable(arg: VaultArgWithToken) -> Result<u64, ProtocolError> {
    validate_call().await?;
    check_postcondition(rumi_protocol_backend::vault::repay_to_vault_with_stable(arg).await)
}

#[candid_method(update)]
#[update]
async fn add_margin_to_vault(arg: VaultArg) -> Result<u64, ProtocolError> {
    validate_call().await?;
    check_postcondition(rumi_protocol_backend::vault::add_margin_to_vault(arg).await)
}

// ─── Push-deposit endpoints (Oisy wallet integration) ───

/// Get the deposit account for the caller. The user transfers collateral here,
/// then calls open_vault_with_deposit or add_margin_with_deposit.
#[candid_method(query)]
#[query]
fn get_deposit_account(_collateral_type: Option<Principal>) -> icrc_ledger_types::icrc1::account::Account {
    let caller = ic_cdk::caller();
    rumi_protocol_backend::management::get_deposit_account_for(&caller)
}

/// Open a vault using funds already deposited to the caller's deposit account.
/// Use this instead of open_vault when the wallet cannot do ICRC-2 approve (e.g., Oisy).
#[candid_method(update)]
#[update]
async fn open_vault_with_deposit(borrow_amount: u64, collateral_type: Option<Principal>) -> Result<OpenVaultSuccess, ProtocolError> {
    validate_call().await?;
    validate_mode()?;
    check_postcondition(rumi_protocol_backend::vault::open_vault_with_deposit(borrow_amount, collateral_type).await)
}

/// Add margin to a vault using funds already deposited to the caller's deposit account.
/// Use this instead of add_margin_to_vault when the wallet cannot do ICRC-2 approve.
#[candid_method(update)]
#[update]
async fn add_margin_with_deposit(vault_id: u64) -> Result<u64, ProtocolError> {
    validate_call().await?;
    check_postcondition(rumi_protocol_backend::vault::add_margin_with_deposit(vault_id).await)
}

#[candid_method(update)]
#[update]
async fn close_vault(vault_id: u64) -> Result<Option<u64>, ProtocolError> {
    validate_call().await?;
    check_postcondition(rumi_protocol_backend::vault::close_vault(vault_id).await)
}

// Add the new withdraw collateral endpoint
#[candid_method(update)]
#[update]
async fn withdraw_collateral(vault_id: u64) -> Result<u64, ProtocolError> {
    validate_call().await?;
    check_postcondition(rumi_protocol_backend::vault::withdraw_collateral(vault_id).await)
}

#[candid_method(update)]
#[update]
async fn withdraw_partial_collateral(arg: rumi_protocol_backend::vault::VaultArg) -> Result<u64, ProtocolError> {
    validate_call().await?;
    check_postcondition(rumi_protocol_backend::vault::withdraw_partial_collateral(arg.vault_id, arg.amount).await)
}

#[candid_method(update)]
#[update]
async fn withdraw_and_close_vault(vault_id: u64) -> Result<Option<u64>, ProtocolError> {
    validate_call().await?;
    check_postcondition(rumi_protocol_backend::vault::withdraw_and_close_vault(vault_id).await)
}

// Add the new liquidate vault endpoint
#[candid_method(update)]
#[update]
async fn liquidate_vault(vault_id: u64) -> Result<SuccessWithFee, ProtocolError> {
    validate_call().await?;
    validate_liquidation_not_frozen()?;
    validate_price_for_liquidation()?;
    validate_freshness_for_vault(vault_id).await?;
    check_postcondition(rumi_protocol_backend::vault::liquidate_vault(vault_id).await)
}

// Add the new partial repay vault endpoint
#[candid_method(update)]
#[update]
async fn partial_repay_to_vault(arg: VaultArg) -> Result<u64, ProtocolError> {
    validate_call().await?;
    check_postcondition(rumi_protocol_backend::vault::partial_repay_to_vault(arg).await)
}

// Partial liquidation with icUSD
#[candid_method(update)]
#[update]
async fn liquidate_vault_partial(arg: VaultArg) -> Result<SuccessWithFee, ProtocolError> {
    validate_call().await?;
    validate_liquidation_not_frozen()?;
    validate_price_for_liquidation()?;
    validate_freshness_for_vault(arg.vault_id).await?;
    check_postcondition(rumi_protocol_backend::vault::liquidate_vault_partial(arg.vault_id, arg.amount).await)
}

/// Liquidate a vault using ckUSDT or ckUSDC (1:1 with icUSD)
#[update]
#[candid_method(update)]
async fn liquidate_vault_partial_with_stable(arg: VaultArgWithToken) -> Result<SuccessWithFee, ProtocolError> {
    validate_call().await?;
    validate_liquidation_not_frozen()?;
    validate_price_for_liquidation()?;
    validate_freshness_for_vault(arg.vault_id).await?;
    check_postcondition(rumi_protocol_backend::vault::liquidate_vault_partial_with_stable(arg.vault_id, arg.amount, arg.token_type).await)
}

// Stability Pool Integration - allows stability pool to execute liquidations
#[update]
#[candid_method(update)]
async fn stability_pool_liquidate(vault_id: u64, max_debt_to_liquidate: u64) -> Result<StabilityPoolLiquidationResult, ProtocolError> {
    validate_call().await?;
    validate_liquidation_not_frozen()?;
    validate_price_for_liquidation()?;
    validate_freshness_for_vault(vault_id).await?;
    let caller = ic_cdk::api::caller();

    // Authorization: only the registered stability pool canister can call this
    let is_stability_pool = read_state(|s| {
        s.stability_pool_canister.map_or(false, |sp| sp == caller)
    });
    if !is_stability_pool {
        return Err(ProtocolError::GenericError(
            "Caller is not the registered stability pool canister".to_string(),
        ));
    }

    // Get vault info and validate it's liquidatable
    let (vault, collateral_price_usd, liquidatable_debt, collateral_available) = read_state(|s| {
        match s.vault_id_to_vaults.get(&vault_id) {
            Some(vault) => {
                // Per-collateral price lookup
                let price = s.get_collateral_price_decimal(&vault.collateral_type)
                    .ok_or("No price available for this collateral type")?;
                let collateral_price_usd = UsdIcp::from(price);
                let ratio = rumi_protocol_backend::compute_collateral_ratio(vault, collateral_price_usd, s);

                let min_ratio = s.get_min_liquidation_ratio_for(&vault.collateral_type);
                if ratio >= min_ratio {
                    return Err(format!(
                        "Vault #{} is not liquidatable. Current ratio: {:.2}%, minimum: {:.2}%",
                        vault_id,
                        ratio.to_f64() * 100.0,
                        min_ratio.to_f64() * 100.0
                    ));
                }

                // Calculate optimal amount to restore vault to target CR
                let optimal_amount = s.compute_partial_liquidation_cap(vault, collateral_price_usd);
                let actual_liquidatable_debt = optimal_amount.min(vault.borrowed_icusd_amount).min(max_debt_to_liquidate.into());

                // Calculate collateral that will be seized (debt + liquidation bonus)
                let liquidation_bonus = s.get_liquidation_bonus_for(&vault.collateral_type);
                let icp_equivalent = actual_liquidatable_debt / collateral_price_usd;
                let collateral_with_bonus = icp_equivalent * liquidation_bonus;
                let collateral_to_seize = collateral_with_bonus.min(ICP::from(vault.collateral_amount));

                Ok((vault.clone(), collateral_price_usd, actual_liquidatable_debt, collateral_to_seize))
            },
            None => Err(format!("Vault #{} not found", vault_id)),
        }
    }).map_err(|e| ProtocolError::GenericError(e))?;

    if liquidatable_debt == ICUSD::new(0) {
        return Err(ProtocolError::GenericError("No liquidatable debt available".to_string()));
    }

    // Execute the liquidation using existing logic
    let result = rumi_protocol_backend::vault::liquidate_vault_partial(vault_id, liquidatable_debt.to_u64()).await?;

    // Return structured result for stability pool
    Ok(StabilityPoolLiquidationResult {
        success: true,
        vault_id,
        liquidated_debt: liquidatable_debt.to_u64(),
        collateral_received: collateral_available.to_u64(),
        collateral_type: vault.collateral_type.to_string(),
        block_index: result.block_index,
        fee: result.fee_amount_paid,
        collateral_price_e8s: collateral_price_usd.to_e8s(),
    })
}

/// Called by the stability pool after it has already burned icUSD (via 3pool atomic burn).
/// Writes down the vault's debt and releases proportional collateral to the caller.
/// Only callable by the registered stability pool canister.
///
/// Wave-8d LIQ-004 Phase 2: `proof` is required. The SP must pass an
/// ICRC-3 burn block index pointing at a real burn on the icUSD ledger;
/// the backend verifies the block matches the expected memo, amount, and
/// `from` account before accepting the writedown.
#[update]
#[candid_method(update)]
async fn stability_pool_liquidate_debt_burned(
    vault_id: u64,
    icusd_burned_e8s: u64,
    proof: rumi_protocol_backend::icrc3_proof::SpWritedownProof,
) -> Result<StabilityPoolLiquidationResult, ProtocolError> {
    validate_call().await?;
    validate_liquidation_not_frozen()?;
    validate_price_for_liquidation()?;
    validate_freshness_for_vault(vault_id).await?;
    let caller = ic_cdk::api::caller();

    let is_stability_pool = read_state(|s| {
        s.stability_pool_canister.map_or(false, |sp| sp == caller)
    });
    if !is_stability_pool {
        return Err(ProtocolError::GenericError(
            "Caller is not the registered stability pool canister".to_string(),
        ));
    }

    rumi_protocol_backend::vault::liquidate_vault_debt_already_burned(
        vault_id, icusd_burned_e8s, caller, None, proof,
    )
    .await
}

/// Called by the stability pool to liquidate a vault using 3USD reserves.
/// The SP must have approved this canister to spend `three_usd_amount_e8s` on `three_usd_ledger`.
/// Validates vault first, then pulls 3USD, then writes down debt and releases collateral.
/// Only callable by the registered stability pool canister.
///
/// Wave-8d LIQ-004 Phase 2: the backend builds the writedown proof
/// internally from the block index returned by `transfer_3usd_to_reserves`.
/// The SP does not pass a proof on this path (the block does not exist
/// until after the backend's own transfer), so the proof argument has been
/// retired from the entry point's surface; vault binding is enforced by
/// `liquidate_vault_debt_already_burned`'s `vault_id_memo == vault_id`
/// assertion. The 3pool ledger does not persist memos into ICRC-3 blocks,
/// so the verifier skips the memo check on this path; replay defense via
/// `consumed_writedown_proofs` and on-chain account/amount validation
/// remain in force.
#[update]
#[candid_method(update)]
async fn stability_pool_liquidate_with_reserves(
    vault_id: u64,
    icusd_debt_covered_e8s: u64,
    three_usd_amount_e8s: u64,
    three_usd_ledger: Principal,
) -> Result<StabilityPoolLiquidationResult, ProtocolError> {
    validate_call().await?;
    validate_liquidation_not_frozen()?;
    validate_price_for_liquidation()?;
    validate_freshness_for_vault(vault_id).await?;
    let caller = ic_cdk::api::caller();

    let is_stability_pool = read_state(|s| {
        s.stability_pool_canister.map_or(false, |sp| sp == caller)
    });
    if !is_stability_pool {
        return Err(ProtocolError::GenericError(
            "Caller is not the registered stability pool canister".to_string(),
        ));
    }

    // Pre-validate: vault exists, has debt, price available — before pulling any tokens.
    // This prevents pulling 3USD and then failing on a stale/removed vault.
    let liquidation_amount: rumi_protocol_backend::numeric::ICUSD = icusd_debt_covered_e8s.into();
    if liquidation_amount < read_state(|s| s.min_icusd_amount) {
        return Err(ProtocolError::AmountTooLow {
            minimum_amount: read_state(|s| s.min_icusd_amount).to_u64(),
        });
    }
    read_state(|s| {
        match s.vault_id_to_vaults.get(&vault_id) {
            Some(vault) => {
                if let Some(status) = s.get_collateral_status(&vault.collateral_type) {
                    if !status.allows_liquidation() {
                        return Err(ProtocolError::GenericError(
                            "Liquidation is not allowed for this collateral type".to_string(),
                        ));
                    }
                }
                if s.get_collateral_price_decimal(&vault.collateral_type).is_none() {
                    return Err(ProtocolError::GenericError(
                        "No price available for collateral. Price feed may be down.".to_string(),
                    ));
                }
                let capped = liquidation_amount.min(vault.borrowed_icusd_amount);
                if capped == rumi_protocol_backend::numeric::ICUSD::new(0) {
                    return Err(ProtocolError::GenericError(
                        "Cannot liquidate zero amount — vault has no debt".to_string(),
                    ));
                }
                Ok(())
            }
            None => Err(ProtocolError::GenericError(
                format!("Vault #{} not found", vault_id),
            )),
        }
    })?;

    // Pull 3USD from the SP into protocol reserves subaccount (ICRC-2 transfer_from).
    // Only runs after validation passes — no tokens move if vault is stale.
    // The block index returned drives the Phase-2 internal proof below.
    let transfer_block_index = rumi_protocol_backend::management::transfer_3usd_to_reserves(
        three_usd_ledger, caller, three_usd_amount_e8s
    ).await.map_err(|e| ProtocolError::GenericError(
        format!("Failed to pull 3USD from stability pool: {:?}", e)
    ))?;

    // Wave-8d LIQ-004 Phase 2: build the writedown proof from the just-
    // produced transfer block. Vault binding is set here at construction
    // time; `liquidate_vault_debt_already_burned` re-asserts
    // `proof.vault_id_memo == vault_id` before any state mutation, and the
    // verifier checks the on-chain block's accounts and amount match.
    let proof = rumi_protocol_backend::icrc3_proof::SpWritedownProof {
        block_index: transfer_block_index,
        ledger_kind: rumi_protocol_backend::icrc3_proof::SpProofLedger::ThreePoolTransfer,
        vault_id_memo: vault_id,
    };

    // 3USD is now in our reserves subaccount, so write down debt and release collateral.
    // Wave-4 ICC-002: if `liquidate_vault_debt_already_burned` returns Err after the
    // pull above succeeded (vault closed mid-flight, paused, debt hit zero, etc.),
    // the 3USD is stranded in our reserves subaccount and the SP's bookkeeping never
    // got a chance to mark it consumed. Refund it so the SP's ledger balance and
    // bookkeeping stay in sync.
    match rumi_protocol_backend::vault::liquidate_vault_debt_already_burned(
        vault_id, icusd_debt_covered_e8s, caller, Some(three_usd_amount_e8s), proof,
    ).await {
        Ok(success) => Ok(success),
        Err(liq_error) => {
            refund_3usd_to_stability_pool(
                three_usd_ledger, caller, three_usd_amount_e8s, vault_id,
            ).await;
            Err(liq_error)
        }
    }
}

/// Refund a previously pulled 3USD amount from the protocol's reserves
/// subaccount back to the stability pool. Used by `stability_pool_liquidate_with_reserves`
/// when the second-stage backend call fails after `transfer_3usd_to_reserves`
/// already moved tokens. Wave-4 ICC-002.
///
/// On success, logs the refund block index. On any failure (including BadFee or
/// fee-too-large), logs CRITICAL so an operator can manually reconcile via
/// `recover_pending_transfer` or a direct ICRC-1 transfer from the reserves
/// subaccount. The refund itself uses Wave-3's idempotent transfer helper, so
/// retries from the SP side won't double-credit even if the reply is dropped.
async fn refund_3usd_to_stability_pool(
    three_usd_ledger: Principal,
    sp_caller: Principal,
    amount_e8s: u64,
    vault_id: u64,
) {
    use ic_canister_log::log;
    use rumi_protocol_backend::logs::INFO;

    let fee = match rumi_protocol_backend::management::get_or_refresh_fee(three_usd_ledger).await {
        Ok(f) => f,
        Err(e) => {
            log!(INFO,
                "[stability_pool_liquidate_with_reserves] CRITICAL: refund of {} 3USD for vault {} \
                 to SP {} aborted (could not fetch ledger fee: {}). Tokens stranded in reserves; \
                 use admin tools to reconcile.",
                amount_e8s, vault_id, sp_caller, e
            );
            return;
        }
    };
    if amount_e8s <= fee {
        log!(INFO,
            "[stability_pool_liquidate_with_reserves] CRITICAL: refund of {} 3USD for vault {} \
             to SP {} aborted (amount does not cover ledger fee {}). Tokens stranded in reserves.",
            amount_e8s, vault_id, sp_caller, fee
        );
        return;
    }
    let refund_amount = amount_e8s - fee;
    let refund_nonce = mutate_state(|s| s.next_op_nonce());
    let result = rumi_protocol_backend::management::transfer_idempotent(
        three_usd_ledger,
        Some(rumi_protocol_backend::management::protocol_3usd_reserves_subaccount()),
        icrc_ledger_types::icrc1::account::Account { owner: sp_caller, subaccount: None },
        refund_amount as u128,
        refund_nonce,
        None,
    )
    .await;
    match result {
        Ok(block) => {
            log!(INFO,
                "[stability_pool_liquidate_with_reserves] refunded {} 3USD (net of {} fee) to SP {} \
                 for vault {} after liquidation rollback (block {})",
                refund_amount, fee, sp_caller, vault_id, block
            );
        }
        Err(e) => {
            log!(INFO,
                "[stability_pool_liquidate_with_reserves] CRITICAL: refund of {} 3USD for vault {} \
                 to SP {} FAILED: {:?}. Tokens stranded in reserves; reconcile manually.",
                refund_amount, vault_id, sp_caller, e
            );
        }
    }
}

/// Cumulative 3USD held in protocol reserves from stability pool liquidations (e8s).
#[query]
#[candid_method(query)]
fn get_protocol_3usd_reserves() -> u64 {
    read_state(|s| s.protocol_3usd_reserves)
}

// Get stability pool configuration
#[query]
#[candid_method(query)]
fn get_stability_pool_config() -> StabilityPoolConfig {
    read_state(|s| {
        StabilityPoolConfig {
            stability_pool_canister: s.stability_pool_canister,
            liquidation_discount: 10, // 10% discount for stability pool
            enabled: s.stability_pool_canister.is_some(),
        }
    })
}

// Add the new partial liquidate vault endpoint
#[candid_method(update)]
#[update]
async fn partial_liquidate_vault(arg: VaultArg) -> Result<SuccessWithFee, ProtocolError> {
    validate_call().await?;
    validate_liquidation_not_frozen()?;
    validate_price_for_liquidation()?;
    validate_freshness_for_vault(arg.vault_id).await?;
    check_postcondition(rumi_protocol_backend::vault::partial_liquidate_vault(arg).await)
}

// Add the new get liquidatable vaults endpoint
#[candid_method(query)]
#[query]
fn get_liquidatable_vaults() -> Vec<CandidVault> {
    read_state(|s| {
        // Dummy rate for compute_collateral_ratio parameter (it uses per-collateral price internally)
        let dummy_rate = s.last_icp_rate.unwrap_or(UsdIcp::from(dec!(0.0)));

        s.vault_id_to_vaults
            .values()
            .filter(|vault| {
                let ratio = rumi_protocol_backend::compute_collateral_ratio(vault, dummy_rate, s);
                // Zero ratio means no price available — don't mark as liquidatable
                if ratio == Ratio::from(Decimal::ZERO) {
                    return false;
                }
                ratio < s.get_min_liquidation_ratio_for(&vault.collateral_type)
            })
            .cloned()
            .map(CandidVault::from)
            .collect::<Vec<CandidVault>>()
    })
}

#[candid_method(query)]
#[query]
fn get_all_vaults() -> Vec<CandidVault> {
    read_state(|s| {
        s.vault_id_to_vaults
            .values()
            .cloned()
            .map(CandidVault::from)
            .collect::<Vec<CandidVault>>()
    })
}

// Liquidity related operations
#[candid_method(update)]
#[update]
async fn provide_liquidity(amount: u64) -> Result<u64, ProtocolError> {
    validate_call().await?;
    check_postcondition(rumi_protocol_backend::liquidity_pool::provide_liquidity(amount).await)
}

#[candid_method(update)]
#[update]
async fn withdraw_liquidity(amount: u64) -> Result<u64, ProtocolError> {
    validate_call().await?;
    check_postcondition(rumi_protocol_backend::liquidity_pool::withdraw_liquidity(amount).await)
}

#[candid_method(update)]
#[update]
async fn claim_liquidity_returns() -> Result<u64, ProtocolError> {
    validate_call().await?;
    check_postcondition(rumi_protocol_backend::liquidity_pool::claim_liquidity_returns().await)
}

/// Transform function for HTTPS outcalls (CoinGecko price fetches).
/// Strips response headers so all replicas reach consensus on the same payload.
#[query]
fn coingecko_transform(
    args: ic_cdk::api::management_canister::http_request::TransformArgs,
) -> ic_cdk::api::management_canister::http_request::HttpResponse {
    ic_cdk::api::management_canister::http_request::HttpResponse {
        status: args.response.status,
        headers: vec![], // Strip headers — they vary across replicas
        body: args.response.body,
    }
}

#[query]
fn http_request(req: HttpRequest) -> HttpResponse {
    use ic_metrics_encoder::MetricsEncoder;
    if ic_cdk::api::data_certificate().is_none() {
        ic_cdk::trap("update call rejected");
    }

    if req.path() == "/metrics" {
        let mut writer = MetricsEncoder::new(vec![], ic_cdk::api::time() as i64 / 1_000_000);

        fn encode_metrics(w: &mut MetricsEncoder<Vec<u8>>) -> std::io::Result<()> {
            read_state(|s| {
                w.gauge_vec("cycle_balance", "Cycle balance of this canister.")?
                    .value(
                        &[("canister", "rumi-protocol")],
                        ic_cdk::api::canister_balance128() as f64,
                    )?;

                w.encode_gauge(
                    "icusd_active_vault_count",
                    s.vault_id_to_vaults.len() as f64,
                    "Count of active vaults in the system.",
                )?;

                w.encode_gauge(
                    "rumi_vault_owners_count",
                    s.principal_to_vault_ids.keys().len() as f64,
                    "Count of owners of active vaults.",
                )?;

                w.encode_gauge(
                    "rumi_total_provided_liquidity_amount",
                    s.total_provided_liquidity_amount().to_u64() as f64,
                    "Provided amount of liquidity.",
                )?;

                w.encode_gauge(
                    "rumi_liquidity_providers_count",
                    s.liquidity_pool.len() as f64,
                    "Count of liquidity providers.",
                )?;

                w.encode_gauge(
                    "rumi_pending_margin_transfer_count",
                    s.pending_margin_transfers.len() as f64,
                    "Pending margin transfers count.",
                )?;

                w.encode_gauge(
                    "rumi_liquidity_providers_rewards",
                    s.total_available_returns().to_u64() as f64,
                    "Available rewards for liquidity providers.",
                )?;

                w.encode_gauge(
                    "rumi_pending_margin_transfers_count",
                    s.pending_margin_transfers.len() as f64,
                    "Pending margin transfers count.",
                )?;

                w.encode_gauge(
                    "rumi_pending_excess_transfers_count",
                    s.pending_excess_transfers.len() as f64,
                    "Pending excess collateral transfers count.",
                )?;

                w.encode_gauge(
                    "rumi_pending_redemption_transfer_count",
                    s.pending_redemption_transfer.len() as f64,
                    "Pending redemption transfers count.",
                )?;

                w.encode_gauge(
                    "rumi_icp_rate",
                    s.last_icp_rate.unwrap_or(UsdIcp::from(dec!(0))).to_f64(),
                    "ICP rate.",
                )?;

                let total_icp_dec = Decimal::from_u64(s.total_icp_margin_amount().0)
                    .expect("failed to construct decimal from u64")
                    / dec!(100_000_000);

                w.encode_gauge(
                    "icp_total_ICP_margin",
                    total_icp_dec.to_f64().unwrap(),
                    "Total ICP Margin.",
                )?;

                let total_tvl = total_icp_dec * s.last_icp_rate.unwrap_or(UsdIcp::from(dec!(0))).0;

                w.encode_gauge(
                    "total_tvl",
                    total_tvl.to_f64().unwrap(),
                    "Total TVL.",
                )?;

                let total_borrowed_icusd_amount = Decimal::from_u64(s.total_borrowed_icusd_amount().0)
                    .expect("failed to construct decimal from u64")
                    / dec!(100_000_000);

                w.encode_gauge(
                    "icusd_total_borrowed_amount",
                    total_borrowed_icusd_amount.to_f64().unwrap(),
                    "Total borrowed icusd.",
                )?;

                w.encode_gauge(
                    "total_collateral_ratio",
                    s.total_collateral_ratio.to_f64(),
                    "TCR.",
                )?;

                Ok(())
            })
        }

        match encode_metrics(&mut writer) {
            Ok(()) => HttpResponseBuilder::ok()
                .header("Content-Type", "text/plain; version=0.0.4")
                .with_body_and_content_length(writer.into_inner())
                .build(),
            Err(err) => {
                HttpResponseBuilder::server_error(format!("Failed to encode metrics: {}", err))
                    .build()
            }
        }
    } else if req.path() == "/logs" {
        use rumi_protocol_backend::logs::{Log, Priority};
        use serde_json;
        use std::str::FromStr;

        let max_skip_timestamp = match req.raw_query_param("time") {
            Some(arg) => match u64::from_str(arg) {
                Ok(value) => value,
                Err(_) => {
                    return HttpResponseBuilder::bad_request()
                        .with_body_and_content_length("failed to parse the 'time' parameter")
                        .build()
                }
            },
            None => 0,
        };

        let mut entries: Log = Default::default();

        match req.raw_query_param("priority") {
            Some(priority_str) => match Priority::from_str(priority_str) {
                Ok(priority) => match priority {
                    Priority::Info => entries.push_logs(Priority::Info),
                    Priority::TraceXrc => entries.push_logs(Priority::TraceXrc),
                    Priority::Debug => entries.push_logs(Priority::Debug),
                },
                Err(_) => entries.push_all(),
            },
            None => entries.push_all(),
        }

        entries
            .entries
            .retain(|entry| entry.timestamp >= max_skip_timestamp);
        let mut entries_bytes: Vec<u8> = serde_json::to_string(&entries)
            .unwrap_or_default()
            .into_bytes();

        // Truncate bytes to avoid having more than 2MB response.
        let max_size_bytes: usize = 1_900_000;
        entries_bytes.truncate(max_size_bytes);

        HttpResponseBuilder::ok()
            .header("Content-Type", "application/json; charset=utf-8")
            .with_body_and_content_length(entries_bytes)
            .build()
    } else if req.path() == "/dashboard" {
        use rumi_protocol_backend::dashboard::build_dashboard;

        let dashboard = build_dashboard();
        HttpResponseBuilder::ok()
            .header("Content-Type", "text/html; charset=utf-8")
            .with_body_and_content_length(dashboard)
            .build()
    } else {
        HttpResponseBuilder::not_found().build()
    }
}


#[candid_method(update)]
#[update]
async fn recover_pending_transfer(vault_id: u64) -> Result<bool, ProtocolError> {
    let caller = ic_cdk::caller();

    // Wave-4 LIQ-001: pending_margin_transfers and pending_excess_transfers are
    // keyed by (vault_id, owner). Look up the entry that belongs to the caller.
    let key = (vault_id, caller);
    let transfer_info = read_state(|s| {
        if let Some(t) = s.pending_margin_transfers.get(&key).cloned() {
            Some(("margin", t))
        } else {
            s.pending_excess_transfers.get(&key).cloned().map(|t| ("excess", t))
        }
    });

    if let Some((source, transfer)) = transfer_info {
        // Look up per-collateral config for ledger and fee; fall back to global ICP defaults
        let (ledger, transfer_fee) = read_state(|s| {
            match s.get_collateral_config(&transfer.collateral_type) {
                Some(config) => (config.ledger_canister_id, ICP::from(config.ledger_fee)),
                None => (s.icp_ledger_principal, s.icp_ledger_fee),
            }
        });

        if transfer.margin <= transfer_fee {
            // Margin too small to cover fee — clean it up
            mutate_state(|s| {
                match source {
                    "margin" => { s.pending_margin_transfers.remove(&key); },
                    _ => { s.pending_excess_transfers.remove(&key); },
                }
            });
            return Err(ProtocolError::GenericError(
                "Pending transfer margin is too small to cover the ledger fee".to_string()
            ));
        }

        let result = management::transfer_collateral(
            (transfer.margin - transfer_fee).to_u64(),
            transfer.owner,
            ledger,
        ).await;

        match result {
            Ok(block_index) => {
                mutate_state(|s| {
                    match source {
                        "margin" => { event::record_margin_transfer(s, vault_id, caller, block_index); },
                        _ => { s.pending_excess_transfers.remove(&key); },
                    }
                });
                Ok(true)
            }
            Err(error) => {
                log!(
                    DEBUG,
                    "[recover_pending_transfer] failed to transfer margin: {}, via ledger: {}, with error: {}",
                    transfer.margin,
                    ledger,
                    error
                );
                Err(ProtocolError::TransferError(error))
            }
        }
    } else {
        // No pending transfer found for this caller + vault
        Err(ProtocolError::GenericError("No pending transfer found for this vault".to_string()))
    }
}

// Add treasury configuration endpoint (developer only)
#[candid_method(update)]
#[update]
async fn set_treasury_principal(treasury_principal: Principal) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    
    // Only developer can set treasury principal
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError("Only developer can set treasury principal".to_string()));
    }
    
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_treasury_principal(s, treasury_principal);
    });

    log!(INFO, "[set_treasury_principal] Treasury principal set to: {}", treasury_principal);
    Ok(())
}

#[candid_method(query)]
#[query]
fn get_treasury_principal() -> Option<Principal> {
    read_state(|s| s.treasury_principal)
}

// Add stability pool configuration endpoint (developer only)
#[candid_method(update)]
#[update]
async fn set_stability_pool_principal(stability_pool_principal: Principal) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    
    // Only developer can set stability pool principal
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError("Only developer can set stability pool principal".to_string()));
    }
    
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_stability_pool_principal(s, stability_pool_principal);
    });

    log!(INFO, "[set_stability_pool_principal] Stability pool principal set to: {}", stability_pool_principal);
    Ok(())
}

#[candid_method(query)]
#[query]
fn get_stability_pool_principal() -> Option<Principal> {
    read_state(|s| s.stability_pool_canister)
}

// ---- Liquidation bot admin functions ----

/// Result returned to the bot after a credit-based liquidation.
#[derive(CandidType, Deserialize, Debug)]
pub struct BotLiquidationResult {
    pub vault_id: u64,
    pub collateral_amount: u64,
    pub debt_covered: u64,
    pub collateral_price_e8s: u64,
}

/// Bot stats exposed to the frontend.
#[derive(CandidType, Deserialize, Debug)]
pub struct BotStatsResponse {
    pub liquidation_bot_principal: Option<Principal>,
    pub budget_total_e8s: u64,
    pub budget_remaining_e8s: u64,
    pub budget_start_timestamp: u64,
    pub total_debt_covered_e8s: u64,
}

#[candid_method(update)]
#[update]
async fn set_liquidation_bot_config(bot_principal: Principal, monthly_budget_e8s: u64) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError("Only developer can set liquidation bot config".to_string()));
    }
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_liquidation_bot_principal(s, bot_principal);
        rumi_protocol_backend::event::record_set_bot_budget(s, monthly_budget_e8s, ic_cdk::api::time());
        // Default: allow ICP if the allowlist is empty (first-time setup)
        if s.bot_allowed_collateral_types.is_empty() {
            s.bot_allowed_collateral_types.insert(s.icp_ledger_principal);
        }
    });
    log!(INFO, "[set_liquidation_bot_config] Bot principal: {}, budget: {} e8s", bot_principal, monthly_budget_e8s);
    Ok(())
}

#[candid_method(update)]
#[update]
async fn reset_bot_budget(new_budget_e8s: u64) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError("Only developer can reset bot budget".to_string()));
    }
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_bot_budget(s, new_budget_e8s, ic_cdk::api::time());
    });
    log!(INFO, "[reset_bot_budget] Budget reset to {} e8s", new_budget_e8s);
    Ok(())
}

/// Set which collateral types the bot is allowed to liquidate (developer only).
/// Pass an empty vec to disable bot liquidations entirely.
#[candid_method(update)]
#[update]
async fn set_bot_allowed_collateral_types(collateral_types: Vec<Principal>) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only developer can set bot allowed collateral types".to_string(),
        ));
    }
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_bot_allowed_collateral_types(s, collateral_types.clone());
    });
    log!(INFO, "[set_bot_allowed_collateral_types] Set {} allowed types: {:?}",
        collateral_types.len(), collateral_types);
    Ok(())
}

#[candid_method(query)]
#[query]
fn get_bot_allowed_collateral_types() -> Vec<Principal> {
    read_state(|s| s.bot_allowed_collateral_types.iter().copied().collect())
}

/// Bot calls this to CLAIM a vault for liquidation (phase 1 of 2).
/// Transfers collateral to the bot and locks the vault (`bot_processing = true`).
/// Vault debt and collateral amounts are NOT modified yet.
/// Bot must call `bot_confirm_liquidation` after successful swap, or
/// `bot_cancel_liquidation` if the swap fails (returns collateral).
#[candid_method(update)]
#[update]
async fn bot_claim_liquidation(vault_id: u64) -> Result<BotLiquidationResult, ProtocolError> {
    validate_call().await?;
    validate_price_for_liquidation()?;
    let caller = ic_cdk::api::caller();

    let is_bot = read_state(|s| {
        s.liquidation_bot_principal.map_or(false, |bp| bp == caller)
    });
    if !is_bot {
        return Err(ProtocolError::GenericError(
            "Caller is not the registered liquidation bot canister".to_string(),
        ));
    }

    // Check no existing claim on this vault
    let existing_claim = read_state(|s| s.bot_claims.contains_key(&vault_id));
    if existing_claim {
        return Err(ProtocolError::GenericError(format!(
            "Vault #{} already has an active bot claim", vault_id
        )));
    }

    // Get vault info, validate collateral type, compute amounts, check budget
    let (collateral_price_usd, liquidatable_debt, collateral_to_seize, collateral_type) = read_state(|s| {
        let vault = s.vault_id_to_vaults.get(&vault_id)
            .ok_or_else(|| ProtocolError::GenericError(format!("Vault #{} not found", vault_id)))?;

        if vault.bot_processing {
            return Err(ProtocolError::GenericError(format!(
                "Vault #{} is already being processed", vault_id
            )));
        }

        // Guard: reject collateral types the bot isn't configured to handle
        if !s.bot_allowed_collateral_types.contains(&vault.collateral_type) {
            return Err(ProtocolError::GenericError(format!(
                "Collateral type {} is not in the bot's allowed list.", vault.collateral_type
            )));
        }

        let price = s.get_collateral_price_decimal(&vault.collateral_type)
            .ok_or_else(|| ProtocolError::GenericError("No price available".to_string()))?;
        let collateral_price_usd = UsdIcp::from(price);
        let ratio = rumi_protocol_backend::compute_collateral_ratio(vault, collateral_price_usd, s);
        let min_ratio = s.get_min_liquidation_ratio_for(&vault.collateral_type);

        if ratio >= min_ratio {
            return Err(ProtocolError::GenericError(format!(
                "Vault #{} is not liquidatable (CR {:.2}% >= {:.2}%)",
                vault_id, ratio.to_f64() * 100.0, min_ratio.to_f64() * 100.0
            )));
        }

        let actual = s.compute_partial_liquidation_cap(vault, collateral_price_usd);

        if s.bot_budget_remaining_e8s < actual.to_u64() {
            return Err(ProtocolError::GenericError(format!(
                "Bot budget insufficient: {} remaining, need {}",
                s.bot_budget_remaining_e8s, actual.to_u64()
            )));
        }

        let decimals = s.get_collateral_config(&vault.collateral_type)
            .map(|c| c.decimals)
            .unwrap_or(8);
        let liq_bonus = s.get_liquidation_bonus_for(&vault.collateral_type);
        let collateral_raw = rumi_protocol_backend::numeric::icusd_to_collateral_amount(actual, price, decimals);
        let collateral_with_bonus = ICP::from(collateral_raw) * liq_bonus;
        let collateral_to_seize = collateral_with_bonus.min(ICP::from(vault.collateral_amount));

        Ok((collateral_price_usd, actual, collateral_to_seize, vault.collateral_type))
    })?;

    // Transfer collateral to bot
    match rumi_protocol_backend::management::transfer_collateral(
        collateral_to_seize.to_u64(), caller, collateral_type
    ).await {
        Ok(block) => {
            log!(INFO, "[bot_claim_liquidation] Transferred {} collateral ({}) to bot for vault #{}, block {}",
                collateral_to_seize.to_u64(), collateral_type, vault_id, block);
        }
        Err(e) => {
            log!(INFO, "[bot_claim_liquidation] Collateral transfer failed for vault #{}: {:?}", vault_id, e);
            return Err(ProtocolError::GenericError(format!("Collateral transfer failed: {:?}", e)));
        }
    }

    // Lock the vault and record the claim (but do NOT modify debt/collateral)
    let now = ic_cdk::api::time();
    mutate_state(|s| {
        if let Some(vault) = s.vault_id_to_vaults.get_mut(&vault_id) {
            vault.bot_processing = true;
        }
        s.bot_claims.insert(vault_id, rumi_protocol_backend::state::BotClaim {
            vault_id,
            collateral_amount: collateral_to_seize.to_u64(),
            debt_amount: liquidatable_debt.to_u64(),
            collateral_type,
            claimed_at: now,
            collateral_price_e8s: collateral_price_usd.to_e8s(),
        });
        // Deduct from budget immediately to prevent over-claiming
        s.bot_budget_remaining_e8s = s.bot_budget_remaining_e8s.saturating_sub(liquidatable_debt.to_u64());
    });

    log!(INFO, "[bot_claim_liquidation] Claimed vault #{}: debt={}, collateral={}",
        vault_id, liquidatable_debt.to_u64(), collateral_to_seize.to_u64());

    Ok(BotLiquidationResult {
        vault_id,
        collateral_amount: collateral_to_seize.to_u64(),
        debt_covered: liquidatable_debt.to_u64(),
        collateral_price_e8s: collateral_price_usd.to_e8s(),
    })
}

/// Bot calls this after successfully swapping collateral (phase 2 of 2).
/// Finalizes the liquidation: reduces vault debt and collateral, records event.
#[candid_method(update)]
#[update]
async fn bot_confirm_liquidation(vault_id: u64) -> Result<(), ProtocolError> {
    validate_call().await?;
    let caller = ic_cdk::api::caller();

    let is_bot = read_state(|s| {
        s.liquidation_bot_principal.map_or(false, |bp| bp == caller)
    });
    if !is_bot {
        return Err(ProtocolError::GenericError(
            "Caller is not the registered liquidation bot canister".to_string(),
        ));
    }

    let claim = read_state(|s| s.bot_claims.get(&vault_id).cloned())
        .ok_or_else(|| ProtocolError::GenericError(format!(
            "No active claim for vault #{}", vault_id
        )))?;

    mutate_state(|s| {
        if let Some(vault) = s.vault_id_to_vaults.get_mut(&vault_id) {
            vault.borrowed_icusd_amount -= ICUSD::new(claim.debt_amount);
            vault.collateral_amount = vault.collateral_amount.saturating_sub(claim.collateral_amount);
            vault.bot_processing = false;
        }

        let event = rumi_protocol_backend::event::Event::PartialLiquidateVault {
            vault_id,
            liquidator_payment: ICUSD::new(claim.debt_amount),
            icp_to_liquidator: ICP::from(claim.collateral_amount),
            liquidator: Some(caller),
            icp_rate: Some(UsdIcp::from(Decimal::from(claim.collateral_price_e8s) / dec!(100_000_000))),
            protocol_fee_collateral: None,
            timestamp: Some(ic_cdk::api::time()),
            three_usd_reserves_e8s: None,
        };
        rumi_protocol_backend::storage::record_event(&event);

        s.bot_total_debt_covered_e8s += claim.debt_amount;
        s.bot_claims.remove(&vault_id);
        // Wave-8b LIQ-002: bot-confirmed liquidation reduced debt+collateral
        // → re-key the CR index entry. (No removal: bot path never drains to
        // zero in a single confirm.)
        s.reindex_vault_cr(vault_id);
    });

    log!(INFO, "[bot_confirm_liquidation] Confirmed liquidation for vault #{}: debt={}, collateral={}",
        vault_id, claim.debt_amount, claim.collateral_amount);

    Ok(())
}

/// Bot calls this when the swap failed and collateral has been returned (cancel phase).
/// Unlocks the vault, restores budget, and clears the claim.
/// The bot MUST transfer the collateral back to the backend canister BEFORE calling this.
#[candid_method(update)]
#[update]
async fn bot_cancel_liquidation(vault_id: u64) -> Result<(), ProtocolError> {
    validate_call().await?;
    let caller = ic_cdk::api::caller();

    let is_bot = read_state(|s| {
        s.liquidation_bot_principal.map_or(false, |bp| bp == caller)
    });
    if !is_bot {
        return Err(ProtocolError::GenericError(
            "Caller is not the registered liquidation bot canister".to_string(),
        ));
    }

    let claim = read_state(|s| s.bot_claims.get(&vault_id).cloned())
        .ok_or_else(|| ProtocolError::GenericError(format!(
            "No active claim for vault #{}", vault_id
        )))?;

    // Verify the collateral was actually returned by checking the backend's balance
    let backend_id = ic_cdk::id();
    let balance_result: Result<(candid::Nat,), _> = ic_cdk::call(
        claim.collateral_type,
        "icrc1_balance_of",
        (icrc_ledger_types::icrc1::account::Account {
            owner: backend_id,
            subaccount: None,
        },),
    ).await;

    // Wave-12 BOT-001b: gate the explicit cancel on the protocol's collateral
    // balance having returned to (>=) `claim.collateral_amount - ledger_fee`.
    // Mirrors the Wave-11 BOT-001 auto-cancel gate in `lib.rs::check_vaults`.
    // Unlike the auto-cancel (which skips and emits a reconciliation event so
    // operators can intervene), the explicit cancel rejects: the caller is
    // the bot itself, so forcing the bot to retry its collateral transfer or
    // escalate to `admin_resolve_stuck_claim` is the right escape hatch.
    let observed = match balance_result {
        Ok((bal,)) => bal.0.to_u64().unwrap_or(0),
        Err((code, msg)) => {
            log!(INFO, "[BOT-001b] balance query failed for vault #{}: {:?} {}",
                vault_id, code, msg);
            return Err(ProtocolError::TemporarilyUnavailable(format!(
                "Could not verify collateral return for vault #{}: {:?} {}. Retry once the ledger is available.",
                vault_id, code, msg
            )));
        }
    };

    let required = read_state(|s| {
        let fee = s
            .get_collateral_config(&claim.collateral_type)
            .map(|c| c.ledger_fee)
            .unwrap_or(0);
        claim.collateral_amount.saturating_sub(fee)
    });

    if observed < required {
        log!(INFO, "[BOT-001b] cancel rejected for vault #{}: balance {} < required {} (collateral_amount {})",
            vault_id, observed, required, claim.collateral_amount);
        return Err(ProtocolError::GenericError(format!(
            "Cannot cancel claim for vault #{}: protocol collateral balance {} < required {} (bot must return collateral first; if permanently lost, use admin_resolve_stuck_claim)",
            vault_id, observed, required
        )));
    }

    log!(INFO, "[BOT-001b] balance check passed for vault #{}: balance {} >= required {}",
        vault_id, observed, required);

    mutate_state(|s| {
        if let Some(vault) = s.vault_id_to_vaults.get_mut(&vault_id) {
            vault.bot_processing = false;
        }
        // Restore budget since this liquidation didn't go through
        s.bot_budget_remaining_e8s += claim.debt_amount;
        s.bot_claims.remove(&vault_id);
    });

    log!(INFO, "[bot_cancel_liquidation] Cancelled claim for vault #{}: collateral={}, debt={} (budget restored)",
        vault_id, claim.collateral_amount, claim.debt_amount);

    Ok(())
}

/// Developer-only: force the bot to claim a vault for liquidation regardless of health ratio.
/// Bypasses CR checks but still uses the two-phase claim pattern.
///
/// Compiled out of the mainnet wasm via `cfg(feature = "test_endpoints")` (audit
/// 2026-04-22-28e9896 Wave 2, AUTH-002). The runtime caller gate below remains
/// for the test build that does enable the feature.
#[cfg(feature = "test_endpoints")]
#[candid_method(update)]
#[update]
async fn dev_force_bot_liquidate(vault_id: u64) -> Result<BotLiquidationResult, ProtocolError> {
    validate_call().await?;
    let caller = ic_cdk::caller();
    let is_authorized = read_state(|s| {
        s.developer_principal == caller
            || s.liquidation_bot_principal.map_or(false, |bp| bp == caller)
    });
    if !is_authorized {
        return Err(ProtocolError::GenericError("Only developer or bot can force bot liquidation".to_string()));
    }

    let existing_claim = read_state(|s| s.bot_claims.contains_key(&vault_id));
    if existing_claim {
        return Err(ProtocolError::GenericError(format!(
            "Vault #{} already has an active bot claim", vault_id
        )));
    }

    // Get vault info — NO CR check, but still check collateral allowlist
    let (collateral_price_usd, debt_to_cover, collateral_to_seize, collateral_type) = read_state(|s| {
        let vault = s.vault_id_to_vaults.get(&vault_id)
            .ok_or_else(|| ProtocolError::GenericError(format!("Vault #{} not found", vault_id)))?;

        if vault.bot_processing {
            return Err(ProtocolError::GenericError(format!(
                "Vault #{} is already being processed", vault_id
            )));
        }

        if !s.bot_allowed_collateral_types.contains(&vault.collateral_type) {
            return Err(ProtocolError::GenericError(format!(
                "Collateral type {} is not in the bot's allowed list.", vault.collateral_type
            )));
        }

        let price = s.get_collateral_price_decimal(&vault.collateral_type)
            .ok_or_else(|| ProtocolError::GenericError("No price available".to_string()))?;
        let collateral_price_usd = UsdIcp::from(price);
        let decimals = s.get_collateral_config(&vault.collateral_type)
            .map(|c| c.decimals)
            .unwrap_or(8);

        let debt = vault.borrowed_icusd_amount;
        let collateral_raw = rumi_protocol_backend::numeric::icusd_to_collateral_amount(debt, price, decimals);
        let liq_bonus = s.get_liquidation_bonus_for(&vault.collateral_type);
        let collateral_with_bonus = ICP::from(collateral_raw) * liq_bonus;
        let collateral_to_seize = collateral_with_bonus.min(ICP::from(vault.collateral_amount));

        Ok::<_, ProtocolError>((collateral_price_usd, debt, collateral_to_seize, vault.collateral_type))
    })?;

    // Transfer collateral
    match rumi_protocol_backend::management::transfer_collateral(
        collateral_to_seize.to_u64(), caller, collateral_type
    ).await {
        Ok(block) => {
            log!(INFO, "[dev_force_bot_liquidate] Transferred {} collateral to caller, block {}", collateral_to_seize.to_u64(), block);
        }
        Err(e) => {
            return Err(ProtocolError::GenericError(format!("Collateral transfer failed: {:?}", e)));
        }
    }

    // Lock vault and record claim (same as bot_claim_liquidation)
    let now = ic_cdk::api::time();
    mutate_state(|s| {
        if let Some(vault) = s.vault_id_to_vaults.get_mut(&vault_id) {
            vault.bot_processing = true;
        }
        s.bot_claims.insert(vault_id, rumi_protocol_backend::state::BotClaim {
            vault_id,
            collateral_amount: collateral_to_seize.to_u64(),
            debt_amount: debt_to_cover.to_u64(),
            collateral_type,
            claimed_at: now,
            collateral_price_e8s: collateral_price_usd.to_e8s(),
        });
    });

    log!(INFO, "[dev_force_bot_liquidate] Force-claimed vault #{}: debt={}, collateral={}",
        vault_id, debt_to_cover.to_u64(), collateral_to_seize.to_u64());

    Ok(BotLiquidationResult {
        vault_id,
        collateral_amount: collateral_to_seize.to_u64(),
        debt_covered: debt_to_cover.to_u64(),
        collateral_price_e8s: collateral_price_usd.to_e8s(),
    })
}

/// Developer test: force a PARTIAL bot liquidation, bypassing the CR health check.
/// Uses compute_partial_liquidation_cap to determine debt amount (same as bot_claim_liquidation)
/// but skips the requirement that the vault be below the liquidation threshold.
///
/// Compiled out of the mainnet wasm via `cfg(feature = "test_endpoints")` (AUTH-002).
#[cfg(feature = "test_endpoints")]
#[candid_method(update)]
#[update]
async fn dev_force_partial_bot_liquidate(vault_id: u64) -> Result<BotLiquidationResult, ProtocolError> {
    validate_call().await?;
    let caller = ic_cdk::caller();
    let is_authorized = read_state(|s| {
        s.developer_principal == caller
            || s.liquidation_bot_principal.map_or(false, |bp| bp == caller)
    });
    if !is_authorized {
        return Err(ProtocolError::GenericError("Only developer or bot can force partial bot liquidation".to_string()));
    }

    let existing_claim = read_state(|s| s.bot_claims.contains_key(&vault_id));
    if existing_claim {
        return Err(ProtocolError::GenericError(format!(
            "Vault #{} already has an active bot claim", vault_id
        )));
    }

    // Get vault info — NO CR check, uses partial liquidation cap, checks collateral allowlist
    let (collateral_price_usd, debt_to_cover, collateral_to_seize, collateral_type) = read_state(|s| {
        let vault = s.vault_id_to_vaults.get(&vault_id)
            .ok_or_else(|| ProtocolError::GenericError(format!("Vault #{} not found", vault_id)))?;

        if vault.bot_processing {
            return Err(ProtocolError::GenericError(format!(
                "Vault #{} is already being processed", vault_id
            )));
        }

        if !s.bot_allowed_collateral_types.contains(&vault.collateral_type) {
            return Err(ProtocolError::GenericError(format!(
                "Collateral type {} is not in the bot's allowed list.", vault.collateral_type
            )));
        }

        let price = s.get_collateral_price_decimal(&vault.collateral_type)
            .ok_or_else(|| ProtocolError::GenericError("No price available".to_string()))?;
        let collateral_price_usd = UsdIcp::from(price);
        let decimals = s.get_collateral_config(&vault.collateral_type)
            .map(|c| c.decimals)
            .unwrap_or(8);

        // Use partial liquidation cap — same as bot_claim_liquidation
        let actual = s.compute_partial_liquidation_cap(vault, collateral_price_usd);

        let liq_bonus = s.get_liquidation_bonus_for(&vault.collateral_type);
        let collateral_raw = rumi_protocol_backend::numeric::icusd_to_collateral_amount(actual, price, decimals);
        let collateral_with_bonus = ICP::from(collateral_raw) * liq_bonus;
        let collateral_to_seize = collateral_with_bonus.min(ICP::from(vault.collateral_amount));

        Ok::<_, ProtocolError>((collateral_price_usd, actual, collateral_to_seize, vault.collateral_type))
    })?;

    // Transfer collateral
    match rumi_protocol_backend::management::transfer_collateral(
        collateral_to_seize.to_u64(), caller, collateral_type
    ).await {
        Ok(block) => {
            log!(INFO, "[dev_force_partial_bot_liquidate] Transferred {} collateral to caller, block {}", collateral_to_seize.to_u64(), block);
        }
        Err(e) => {
            return Err(ProtocolError::GenericError(format!("Collateral transfer failed: {:?}", e)));
        }
    }

    // Lock vault and record claim
    let now = ic_cdk::api::time();
    mutate_state(|s| {
        if let Some(vault) = s.vault_id_to_vaults.get_mut(&vault_id) {
            vault.bot_processing = true;
        }
        s.bot_claims.insert(vault_id, rumi_protocol_backend::state::BotClaim {
            vault_id,
            collateral_amount: collateral_to_seize.to_u64(),
            debt_amount: debt_to_cover.to_u64(),
            collateral_type,
            claimed_at: now,
            collateral_price_e8s: collateral_price_usd.to_e8s(),
        });
    });

    log!(INFO, "[dev_force_partial_bot_liquidate] Force-partial-claimed vault #{}: debt={}, collateral={}",
        vault_id, debt_to_cover.to_u64(), collateral_to_seize.to_u64());

    Ok(BotLiquidationResult {
        vault_id,
        collateral_amount: collateral_to_seize.to_u64(),
        debt_covered: debt_to_cover.to_u64(),
        collateral_price_e8s: collateral_price_usd.to_e8s(),
    })
}

/// Developer test: force a vault to be liquidated by the stability pool, bypassing the bot.
/// Calls the stability pool's notify_liquidatable_vaults with just this vault.
///
/// Compiled out of the mainnet wasm via `cfg(feature = "test_endpoints")` (AUTH-002).
#[cfg(feature = "test_endpoints")]
#[candid_method(update)]
#[update]
async fn dev_test_pool_only_liquidation(vault_id: u64) -> Result<String, ProtocolError> {
    validate_call().await?;
    let caller = ic_cdk::caller();
    let is_dev = read_state(|s| s.developer_principal == caller);
    if !is_dev {
        return Err(ProtocolError::GenericError("Developer only".to_string()));
    }

    let pool_canister = read_state(|s| s.stability_pool_canister)
        .ok_or_else(|| ProtocolError::GenericError("No stability pool configured".to_string()))?;

    // Build vault notification (skips CR check — force test)
    let vault_info = read_state(|s| {
        let vault = s.vault_id_to_vaults.get(&vault_id)
            .ok_or_else(|| ProtocolError::GenericError(format!("Vault #{} not found", vault_id)))?;

        if vault.bot_processing {
            return Err(ProtocolError::GenericError(format!(
                "Vault #{} is locked by bot_processing", vault_id
            )));
        }

        let collateral_price_usd = s.get_collateral_price_decimal(&vault.collateral_type)
            .map(|p| UsdIcp::from(p))
            .ok_or(ProtocolError::GenericError("No price available".to_string()))?;
        let price_e8s = collateral_price_usd.to_e8s();
        let optimal_liq = s.compute_partial_liquidation_cap(vault, collateral_price_usd);

        Ok(rumi_protocol_backend::LiquidatableVaultInfo {
            vault_id: vault.vault_id,
            collateral_type: vault.collateral_type,
            debt_amount: vault.borrowed_icusd_amount.to_u64(),
            collateral_amount: vault.collateral_amount,
            recommended_liquidation_amount: optimal_liq.to_u64(),
            collateral_price_e8s: price_e8s,
        })
    })?;

    // Send directly to the stability pool
    let result: Result<(), _> = ic_cdk::call(
        pool_canister,
        "notify_liquidatable_vaults",
        (vec![vault_info],),
    ).await;

    match result {
        Ok(()) => {
            log!(INFO, "[dev_test_pool_only_liquidation] Sent vault #{} to stability pool", vault_id);
            Ok(format!("Vault #{} sent to stability pool for liquidation", vault_id))
        }
        Err((code, msg)) => {
            Err(ProtocolError::GenericError(format!(
                "Stability pool notification failed: {:?} {}", code, msg
            )))
        }
    }
}

/// Developer test: manually set the cached price for any collateral type.
/// Bypasses XRC — useful for testing liquidation flows with synthetic assets.
///
/// Compiled out of the mainnet wasm via `cfg(feature = "test_endpoints")` (AUTH-002).
#[cfg(feature = "test_endpoints")]
#[candid_method(update)]
#[update]
async fn dev_set_collateral_price(collateral_type: Principal, price_usd: f64) -> Result<String, ProtocolError> {
    validate_call().await?;
    let caller = ic_cdk::caller();
    let is_dev = read_state(|s| s.developer_principal == caller);
    if !is_dev {
        return Err(ProtocolError::GenericError("Developer only".to_string()));
    }

    let ts = ic_cdk::api::time();
    let old_price = mutate_state(|s| {
        match s.collateral_configs.get_mut(&collateral_type) {
            Some(config) => {
                let old = config.last_price;
                config.last_price = Some(price_usd);
                config.last_price_timestamp = Some(ts);
                Ok(old)
            }
            None => Err(ProtocolError::GenericError(
                format!("Collateral type {} not found in configs", collateral_type)
            ))
        }
    })?;

    log!(INFO, "[dev_set_collateral_price] {} price set: {:?} → {}", collateral_type, old_price, price_usd);
    Ok(format!("Price for {} set to ${:.6} (was {:?})", collateral_type, price_usd, old_price))
}

#[candid_method(query)]
#[query]
fn get_bot_stats() -> BotStatsResponse {
    read_state(|s| BotStatsResponse {
        liquidation_bot_principal: s.liquidation_bot_principal,
        budget_total_e8s: s.bot_budget_total_e8s,
        budget_remaining_e8s: s.bot_budget_remaining_e8s,
        budget_start_timestamp: s.bot_budget_start_timestamp,
        total_debt_covered_e8s: s.bot_total_debt_covered_e8s,
    })
}

/// Admin-only: force-resolve a stuck bot claim. Used when the bot's ckUSDC transfer
/// or confirm failed and the vault is stuck with bot_processing=true.
///
/// - `apply_debt_reduction = false`: TransferFailed case. ckUSDC never reached the backend,
///   so vault debt stays as-is. Just unlocks vault and restores budget.
/// - `apply_debt_reduction = true`: ConfirmFailed case. ckUSDC DID reach the backend,
///   so also write down the vault's debt and collateral (same as what confirm would do).
#[candid_method(update)]
#[update]
fn admin_resolve_stuck_claim(vault_id: u64, apply_debt_reduction: bool) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_dev = read_state(|s| s.developer_principal == caller);
    if !is_dev {
        return Err(ProtocolError::GenericError("Unauthorized: developer only".to_string()));
    }

    let claim = read_state(|s| s.bot_claims.get(&vault_id).cloned())
        .ok_or_else(|| ProtocolError::GenericError(format!(
            "No active claim for vault #{}", vault_id
        )))?;

    mutate_state(|s| {
        if let Some(vault) = s.vault_id_to_vaults.get_mut(&vault_id) {
            if apply_debt_reduction {
                vault.borrowed_icusd_amount -= ICUSD::new(claim.debt_amount);
                vault.collateral_amount = vault.collateral_amount.saturating_sub(claim.collateral_amount);
                s.bot_total_debt_covered_e8s += claim.debt_amount;
            }
            vault.bot_processing = false;
        }
        if !apply_debt_reduction {
            s.bot_budget_remaining_e8s += claim.debt_amount;
        }
        s.bot_claims.remove(&vault_id);
        // Wave-8b LIQ-002: re-key only when debt/collateral was actually
        // reduced. The pure-cancel branch only flips `bot_processing`, which
        // does not affect CR.
        if apply_debt_reduction {
            s.reindex_vault_cr(vault_id);
        }
    });

    log!(INFO, "[admin_resolve_stuck_claim] Resolved stuck claim for vault #{}: debt={}, collateral={}, debt_reduced={}",
        vault_id, claim.debt_amount, claim.collateral_amount, apply_debt_reduction);

    Ok(())
}

// ---- Stable token repayment admin functions ----

/// Set the fee rate charged on ckUSDT/ckUSDC repayments (developer only)
/// Rate is a decimal: 0.0002 = 0.02%, max 0.05 = 5%
#[candid_method(update)]
#[update]
async fn set_ckstable_repay_fee(new_rate: f64) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError("Only developer can set ckstable repay fee".to_string()));
    }
    if new_rate < 0.0 || new_rate > 0.05 {
        return Err(ProtocolError::GenericError("Fee rate must be between 0 and 0.05 (5%)".to_string()));
    }
    let rate = Ratio::from(rust_decimal::Decimal::try_from(new_rate)
        .map_err(|_| ProtocolError::GenericError("Invalid fee rate".to_string()))?);
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_ckstable_repay_fee(s, rate);
    });
    log!(INFO, "[set_ckstable_repay_fee] Fee rate set to: {}", new_rate);
    Ok(())
}

/// Get the current ckstable repayment fee rate
#[candid_method(query)]
#[query]
fn get_ckstable_repay_fee() -> f64 {
    read_state(|s| s.ckstable_repay_fee.to_f64())
}

/// Set the minimum icUSD amount for borrow/repay/redemption/liquidation operations (developer only).
/// Amount is in e8s. Must be > 0 and <= 10_000_000_000 (100 icUSD).
#[candid_method(update)]
#[update]
async fn set_min_icusd_amount(new_amount_e8s: u64) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError("Only developer can set min icUSD amount".to_string()));
    }
    if new_amount_e8s == 0 || new_amount_e8s > 10_000_000_000 {
        return Err(ProtocolError::GenericError("Amount must be > 0 and <= 100 icUSD (10_000_000_000 e8s)".to_string()));
    }
    let amount = ICUSD::new(new_amount_e8s);
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_min_icusd_amount(s, amount);
    });
    log!(INFO, "[set_min_icusd_amount] Min icUSD amount set to: {} e8s", new_amount_e8s);
    Ok(())
}

/// Get the current minimum icUSD amount (in e8s)
#[candid_method(query)]
#[query]
fn get_min_icusd_amount() -> u64 {
    read_state(|s| s.min_icusd_amount.to_u64())
}

/// Set the global cap on total icUSD that can be minted (developer only).
/// Amount is in e8s. e.g. 3_000_000_000_000 = 30,000 icUSD.
#[candid_method(update)]
#[update]
async fn set_global_icusd_mint_cap(amount_e8s: u64) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError("Only developer can set global icUSD mint cap".to_string()));
    }
    if amount_e8s == 0 {
        return Err(ProtocolError::GenericError("Amount must be > 0".to_string()));
    }
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_global_icusd_mint_cap(s, amount_e8s);
    });
    log!(INFO, "[set_global_icusd_mint_cap] Global icUSD mint cap set to: {} e8s ({} icUSD)", amount_e8s, amount_e8s as f64 / 1e8);
    Ok(())
}

/// Get the current global icUSD mint cap (in e8s). u64::MAX = uncapped.
#[candid_method(query)]
#[query]
fn get_global_icusd_mint_cap() -> u64 {
    read_state(|s| s.global_icusd_mint_cap)
}

/// Enable or disable a specific stable token for repayments/liquidations (developer only)
#[candid_method(update)]
#[update]
async fn set_stable_token_enabled(token_type: StableTokenType, enabled: bool) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError("Only developer can toggle stable token acceptance".to_string()));
    }
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_stable_token_enabled(s, token_type.clone(), enabled);
    });
    log!(INFO, "[set_stable_token_enabled] {:?} enabled: {}", token_type, enabled);
    Ok(())
}

/// Check if a stable token type is currently enabled
#[candid_method(query)]
#[query]
fn get_stable_token_enabled(token_type: StableTokenType) -> bool {
    read_state(|s| match token_type {
        StableTokenType::CKUSDT => s.ckusdt_enabled,
        StableTokenType::CKUSDC => s.ckusdc_enabled,
    })
}

/// Set the ckUSDT or ckUSDC ledger principal (developer only)
#[candid_method(update)]
#[update]
async fn set_stable_ledger_principal(token_type: StableTokenType, principal: Principal) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError("Only developer can set stable ledger principals".to_string()));
    }
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_stable_ledger_principal(s, token_type.clone(), principal);
    });
    log!(INFO, "[set_stable_ledger_principal] {:?} set to {}", token_type, principal);
    Ok(())
}

/// Set the liquidation bonus multiplier (developer only)
/// Rate is a decimal: 1.1 = 110% (10% bonus), range 1.0–1.5
#[candid_method(update)]
#[update]
async fn set_liquidation_bonus(new_rate: f64) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError("Only developer can set liquidation bonus".to_string()));
    }
    if new_rate < 1.0 || new_rate > 1.5 {
        return Err(ProtocolError::GenericError("Liquidation bonus must be between 1.0 and 1.5".to_string()));
    }
    let rate = Ratio::from(rust_decimal::Decimal::try_from(new_rate)
        .map_err(|_| ProtocolError::GenericError("Invalid rate".to_string()))?);
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_liquidation_bonus(s, rate);
    });
    log!(INFO, "[set_liquidation_bonus] Liquidation bonus set to: {}", new_rate);
    Ok(())
}

/// Get the current liquidation bonus multiplier
#[candid_method(query)]
#[query]
fn get_liquidation_bonus() -> f64 {
    read_state(|s| s.liquidation_bonus.to_f64())
}

/// Set the redemption priority tier for a collateral type (developer only).
/// Tier 1 = redeemed first, tier 3 = redeemed last.
#[candid_method(update)]
#[update]
fn set_redemption_tier(ledger_canister_id: Principal, tier: u8) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError("Only developer can set redemption tier".to_string()));
    }
    if tier < 1 || tier > 3 {
        return Err(ProtocolError::GenericError("Tier must be 1, 2, or 3".to_string()));
    }
    mutate_state(|s| {
        match s.collateral_configs.get_mut(&ledger_canister_id) {
            Some(config) => {
                config.redemption_tier = tier;
                log!(INFO, "[set_redemption_tier] {} set to tier {}", ledger_canister_id, tier);
                Ok(())
            }
            None => Err(ProtocolError::GenericError(format!("No collateral config for {}", ledger_canister_id))),
        }
    })
}

/// Get the redemption priority tier for a collateral type.
#[candid_method(query)]
#[query]
fn get_redemption_tier(ledger_canister_id: Principal) -> Result<u8, ProtocolError> {
    read_state(|s| {
        match s.collateral_configs.get(&ledger_canister_id) {
            Some(config) => Ok(config.redemption_tier),
            None => Err(ProtocolError::GenericError(format!("No collateral config for {}", ledger_canister_id))),
        }
    })
}

/// Set the borrowing fee rate (developer only)
/// Rate is a decimal: 0.005 = 0.5%, range 0.0–0.10 (10%)
#[candid_method(update)]
#[update]
async fn set_borrowing_fee(new_rate: f64) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError("Only developer can set borrowing fee".to_string()));
    }
    if new_rate < 0.0 || new_rate > 0.10 {
        return Err(ProtocolError::GenericError("Borrowing fee must be between 0 and 0.10 (10%)".to_string()));
    }
    let rate = Ratio::from(rust_decimal::Decimal::try_from(new_rate)
        .map_err(|_| ProtocolError::GenericError("Invalid rate".to_string()))?);
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_borrowing_fee(s, rate);
    });
    log!(INFO, "[set_borrowing_fee] Borrowing fee set to: {}", new_rate);
    Ok(())
}

/// Get the current borrowing fee rate
#[candid_method(query)]
#[query]
fn get_borrowing_fee() -> f64 {
    read_state(|s| s.fee.to_f64())
}

/// Set the redemption fee floor (developer only)
/// Rate is a decimal: 0.005 = 0.5%, range 0.0–0.10
#[candid_method(update)]
#[update]
async fn set_redemption_fee_floor(new_rate: f64) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError("Only developer can set redemption fee floor".to_string()));
    }
    if new_rate < 0.0 || new_rate > 0.10 {
        return Err(ProtocolError::GenericError("Redemption fee floor must be between 0 and 0.10 (10%)".to_string()));
    }
    let rate = Ratio::from(rust_decimal::Decimal::try_from(new_rate)
        .map_err(|_| ProtocolError::GenericError("Invalid rate".to_string()))?);
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_redemption_fee_floor(s, rate);
    });
    log!(INFO, "[set_redemption_fee_floor] Redemption fee floor set to: {}", new_rate);
    Ok(())
}

/// Get the current redemption fee floor
#[candid_method(query)]
#[query]
fn get_redemption_fee_floor() -> f64 {
    read_state(|s| s.redemption_fee_floor.to_f64())
}

/// Set the redemption fee ceiling (developer only)
/// Rate is a decimal: 0.05 = 5%, range 0.0–0.50
#[candid_method(update)]
#[update]
async fn set_redemption_fee_ceiling(new_rate: f64) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError("Only developer can set redemption fee ceiling".to_string()));
    }
    if new_rate < 0.0 || new_rate > 0.50 {
        return Err(ProtocolError::GenericError("Redemption fee ceiling must be between 0 and 0.50 (50%)".to_string()));
    }
    let rate = Ratio::from(rust_decimal::Decimal::try_from(new_rate)
        .map_err(|_| ProtocolError::GenericError("Invalid rate".to_string()))?);
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_redemption_fee_ceiling(s, rate);
    });
    log!(INFO, "[set_redemption_fee_ceiling] Redemption fee ceiling set to: {}", new_rate);
    Ok(())
}

/// Get the current redemption fee ceiling
#[candid_method(query)]
#[query]
fn get_redemption_fee_ceiling() -> f64 {
    read_state(|s| s.redemption_fee_ceiling.to_f64())
}

// ── Reserve redemption admin functions ──────────────────────────────

/// Enable or disable reserve redemptions (developer only)
#[candid_method(update)]
#[update]
async fn set_reserve_redemptions_enabled(enabled: bool) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError("Only developer can toggle reserve redemptions".to_string()));
    }
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_reserve_redemptions_enabled(s, enabled);
    });
    log!(INFO, "[set_reserve_redemptions_enabled] Reserve redemptions enabled: {}", enabled);
    Ok(())
}

/// Get whether reserve redemptions are enabled
#[candid_method(query)]
#[query]
fn get_reserve_redemptions_enabled() -> bool {
    read_state(|s| s.reserve_redemptions_enabled)
}

// ── ICPswap routing kill switch (developer only) ────────────────────

/// Enable or disable ICPswap-backed swap routing. When disabled, the frontend
/// skips all ICPswap providers and falls back to Rumi AMM + 3pool only.
#[candid_method(update)]
#[update]
async fn set_icpswap_routing_enabled(enabled: bool) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only developer can toggle ICPswap routing".to_string(),
        ));
    }
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_icpswap_routing_enabled(s, enabled);
    });
    log!(INFO, "[set_icpswap_routing_enabled] ICPswap routing enabled: {}", enabled);
    Ok(())
}

/// Get whether ICPswap-backed swap routing is enabled.
#[candid_method(query)]
#[query]
fn get_icpswap_routing_enabled() -> bool {
    read_state(|s| s.icpswap_routing_enabled)
}

/// Set the flat fee for reserve redemptions (developer only)
/// Rate is a decimal: 0.003 = 0.3%, range 0.0–0.10
#[candid_method(update)]
#[update]
async fn set_reserve_redemption_fee(new_rate: f64) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError("Only developer can set reserve redemption fee".to_string()));
    }
    if new_rate < 0.0 || new_rate > 0.10 {
        return Err(ProtocolError::GenericError("Reserve redemption fee must be between 0 and 0.10 (10%)".to_string()));
    }
    let rate = Ratio::from(rust_decimal::Decimal::try_from(new_rate)
        .map_err(|_| ProtocolError::GenericError("Invalid rate".to_string()))?);
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_reserve_redemption_fee(s, rate);
    });
    log!(INFO, "[set_reserve_redemption_fee] Reserve redemption fee set to: {}", new_rate);
    Ok(())
}

/// Get the current reserve redemption fee
#[candid_method(query)]
#[query]
fn get_reserve_redemption_fee() -> f64 {
    read_state(|s| s.reserve_redemption_fee.to_f64())
}

// ── Admin safety functions (controller-only) ──────────────────────────────────

fn require_controller() -> Result<(), ProtocolError> {
    if ic_cdk::api::is_controller(&ic_cdk::caller()) {
        Ok(())
    } else {
        Err(ProtocolError::CallerNotOwner)
    }
}

/// Manually enter Recovery mode. Automatic mode transitions are suppressed
/// until `exit_recovery_mode` is called.
#[candid_method(update)]
#[update]
fn enter_recovery_mode() -> Result<(), ProtocolError> {
    require_controller()?;
    mutate_state(|s| {
        s.mode = Mode::Recovery;
        s.manual_mode_override = true;
        log!(INFO, "[admin] entered Recovery mode (manual override active)");
    });
    Ok(())
}

/// Exit Recovery mode and re-enable automatic mode transitions based on
/// collateral ratio.
#[candid_method(update)]
#[update]
fn exit_recovery_mode() -> Result<(), ProtocolError> {
    require_controller()?;
    mutate_state(|s| {
        s.mode = Mode::GeneralAvailability;
        s.manual_mode_override = false;
        log!(INFO, "[admin] exited Recovery mode, automatic mode management restored");
    });
    Ok(())
}

/// Emergency kill switch — halts ALL state-changing operations.
/// Supersedes mode; even Recovery and GeneralAvailability are irrelevant while frozen.
#[candid_method(update)]
#[update]
fn freeze_protocol() -> Result<(), ProtocolError> {
    require_controller()?;
    mutate_state(|s| {
        s.frozen = true;
        log!(INFO, "[admin] protocol FROZEN — all operations suspended");
    });
    Ok(())
}

/// Lift the freeze. Operations resume under whatever mode is currently active.
#[candid_method(update)]
#[update]
fn unfreeze_protocol() -> Result<(), ProtocolError> {
    require_controller()?;
    mutate_state(|s| {
        s.frozen = false;
        log!(INFO, "[admin] protocol UNFROZEN — operations resumed");
    });
    Ok(())
}

/// Wave-5 LIQ-007: toggle the liquidation kill switch. When true, all
/// liquidation endpoints reject with TemporarilyUnavailable. Independent of
/// `frozen` (which halts everything) and `Mode::ReadOnly` (which auto-latches
/// on TCR < 100% but lets liquidations through). Use during a confirmed oracle
/// outage where liquidating against the cached price would be unsafe.
#[candid_method(update)]
#[update]
fn set_liquidation_frozen(frozen: bool) -> Result<(), ProtocolError> {
    require_controller()?;
    mutate_state(|s| {
        s.liquidation_frozen = frozen;
        log!(
            INFO,
            "[admin] liquidation_frozen set to {}",
            frozen
        );
    });
    Ok(())
}

/// Wave-5 LIQ-007: read the liquidation kill switch state.
#[candid_method(query)]
#[query]
fn get_liquidation_frozen() -> bool {
    read_state(|s| s.liquidation_frozen)
}

/// Wave-8c LIQ-004: toggle the SP-writedown kill switch. When true, both
/// `stability_pool_liquidate_debt_burned` and
/// `stability_pool_liquidate_with_reserves` reject with
/// TemporarilyUnavailable. Independent of `frozen` (global emergency stop)
/// and `liquidation_frozen` (Wave-5 blanket liquidation halt). Use during a
/// confirmed SP compromise or drift event so user-initiated liquidations
/// stay open.
#[candid_method(update)]
#[update]
fn set_sp_writedown_disabled(disabled: bool) -> Result<(), ProtocolError> {
    require_controller()?;
    mutate_state(|s| {
        s.sp_writedown_disabled = disabled;
        log!(
            INFO,
            "[admin] sp_writedown_disabled set to {}",
            disabled
        );
    });
    Ok(())
}

/// Wave-8c LIQ-004: read the SP-writedown kill switch state.
#[candid_method(query)]
#[query]
fn get_sp_writedown_disabled() -> bool {
    read_state(|s| s.sp_writedown_disabled)
}

/// Wave-8d LIQ-004: snapshot of the consumed-writedown-proof set, used by
/// ops monitoring (cross-check on-chain reserves vs sum of writedowns) and
/// by the PocketIC fence for the Phase-2 wave. Returned as a Vec rather
/// than a Set so it round-trips cleanly through Candid.
#[candid_method(query)]
#[query]
fn get_consumed_writedown_proofs(
) -> Vec<(rumi_protocol_backend::icrc3_proof::SpProofLedger, u64)> {
    read_state(|s| s.consumed_writedown_proofs.iter().copied().collect())
}

/// Wave-8b LIQ-002: tune the liquidation-ordering tolerance band. The band is
/// expressed in absolute CR units (e.g., 0.01 = 1% CR = 100 bps). Liquidator
/// endpoints accept a vault only if its CR is within `tolerance` of the
/// lowest-CR vault. Widening to 1.0 effectively disables the gate (all
/// indexed vaults are in band); shrinking to 0 forces strict worst-first
/// ordering. Default is `DEFAULT_LIQUIDATION_ORDERING_TOLERANCE` (1%).
#[candid_method(update)]
#[update]
fn set_liquidation_ordering_tolerance(tolerance_e4: u64) -> Result<(), ProtocolError> {
    require_controller()?;
    // Argument is in basis points (10_000 = 1.0 = 100%). Convert to Decimal
    // by dividing by 10_000. This keeps the wire format integer-only and
    // matches `cr_index_key`'s scaling.
    let tolerance = Ratio::from(
        Decimal::from(tolerance_e4) / Decimal::from(10_000u64),
    );
    mutate_state(|s| {
        s.set_liquidation_ordering_tolerance(tolerance);
        log!(
            INFO,
            "[admin] liquidation_ordering_tolerance set to {} bps ({})",
            tolerance_e4,
            tolerance.to_f64()
        );
    });
    Ok(())
}

/// Wave-8b LIQ-002: read the current liquidation-ordering tolerance band, in
/// basis points (e.g., 100 = 1% CR = the default).
#[candid_method(query)]
#[query]
fn get_liquidation_ordering_tolerance_bps() -> u64 {
    read_state(|s| {
        (s.liquidation_ordering_tolerance.0 * Decimal::from(10_000u64))
            .to_u64()
            .unwrap_or(0)
    })
}

// ── End admin safety functions ────────────────────────────────────────────────

/// Redeem icUSD for ckStable tokens from reserves (with vault spillover fallback)
#[candid_method(update)]
#[update]
async fn redeem_reserves(amount: u64, preferred_token: Option<Principal>) -> Result<ReserveRedemptionResult, ProtocolError> {
    validate_call().await?;
    rumi_protocol_backend::vault::redeem_reserves(amount, preferred_token).await
}

/// Query available reserve balances
#[candid_method(query)]
#[query]
fn get_reserve_balances() -> Vec<ReserveBalance> {
    // Note: This returns cached/approximate balances.
    // Actual balances require async inter-canister calls via the update version.
    // For now we return the configured ledgers; actual balances fetched by frontend directly.
    let mut balances = Vec::new();
    read_state(|s| {
        if let Some(ledger) = s.ckusdt_ledger_principal {
            balances.push(ReserveBalance {
                ledger,
                balance: 0, // frontend queries ledger directly for live balance
                symbol: "ckUSDT".to_string(),
            });
        }
        if let Some(ledger) = s.ckusdc_ledger_principal {
            balances.push(ReserveBalance {
                ledger,
                balance: 0,
                symbol: "ckUSDC".to_string(),
            });
        }
    });
    balances
}

/// Admin: mint icUSD to a recipient (developer only).
/// Used for refunding stuck icUSD from failed operations.
/// Capped at 1,500 icUSD per call with a 72-hour cooldown between mints.
/// Every use is recorded as an on-chain event with a stated reason.
#[candid_method(update)]
#[update]
async fn admin_mint_icusd(amount_e8s: u64, to: Principal, reason: String) -> Result<u64, ProtocolError> {
    const ADMIN_MINT_CAP_E8S: u64 = 150_000_000_000; // 1,500 icUSD
    const ADMIN_MINT_COOLDOWN_NS: u64 = 72 * 3600 * 1_000_000_000; // 72 hours

    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError("Only developer can call admin_mint_icusd".to_string()));
    }
    if amount_e8s == 0 {
        return Err(ProtocolError::GenericError("Amount must be > 0".to_string()));
    }
    if amount_e8s > ADMIN_MINT_CAP_E8S {
        return Err(ProtocolError::GenericError(
            format!("Amount exceeds admin mint cap of {} e8s (1,500 icUSD)", ADMIN_MINT_CAP_E8S)
        ));
    }

    // Enforce 72-hour cooldown
    let last_mint_time = read_state(|s| s.last_admin_mint_time);
    let now = ic_cdk::api::time();
    if last_mint_time > 0 && now.saturating_sub(last_mint_time) < ADMIN_MINT_COOLDOWN_NS {
        let remaining_ns = ADMIN_MINT_COOLDOWN_NS - (now - last_mint_time);
        let remaining_hours = remaining_ns / (3600 * 1_000_000_000);
        return Err(ProtocolError::GenericError(
            format!("Admin mint cooldown active. ~{} hours remaining.", remaining_hours)
        ));
    }

    let amount = rumi_protocol_backend::numeric::ICUSD::from(amount_e8s);
    let block_index = rumi_protocol_backend::management::mint_icusd(amount, to).await
        .map_err(|e| ProtocolError::GenericError(format!("Mint failed: {:?}", e)))?;

    // Update cooldown timestamp
    mutate_state(|s| { s.last_admin_mint_time = now; });

    // Record on-chain event for transparency
    rumi_protocol_backend::event::record_admin_mint(amount, to, reason.clone(), block_index);

    log!(INFO, "[admin_mint_icusd] Minted {} e8s icUSD to {} (block {}). Reason: {}",
        amount_e8s, to, block_index, reason);
    Ok(block_index)
}

/// Set the recovery CR multiplier (developer only).
/// recovery_cr = borrow_threshold × multiplier.
/// Example: multiplier = 1.0333, borrow_threshold = 1.50 → recovery_cr = 1.55.
#[candid_method(update)]
#[update]
async fn set_recovery_cr_multiplier(new_multiplier: f64) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set recovery CR multiplier".to_string(),
        ));
    }
    if new_multiplier < 1.001 || new_multiplier > 1.5 {
        return Err(ProtocolError::GenericError(
            "Recovery CR multiplier must be between 1.001 (0.1% buffer) and 1.5 (50% buffer)".to_string(),
        ));
    }
    let multiplier = Ratio::from(rust_decimal::Decimal::try_from(new_multiplier)
        .map_err(|_| ProtocolError::GenericError("Invalid multiplier value".to_string()))?);
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_recovery_cr_multiplier(s, multiplier);
    });
    log!(INFO, "[set_recovery_cr_multiplier] Multiplier set to: {} ({}% buffer)", new_multiplier, (new_multiplier - 1.0) * 100.0);
    Ok(())
}

/// Get the current recovery CR multiplier
#[candid_method(query)]
#[query]
fn get_recovery_cr_multiplier() -> f64 {
    read_state(|s| s.recovery_cr_multiplier.to_f64())
}

/// Set the global liquidation protocol share (fraction of liquidator's bonus profit).
/// Default: 0.03 (3%). Range: 0.0–1.0.
#[candid_method(update)]
#[update]
async fn set_liquidation_protocol_share(new_share: f64) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set liquidation protocol share".to_string(),
        ));
    }
    if !(0.0..=1.0).contains(&new_share) {
        return Err(ProtocolError::GenericError(
            "Liquidation protocol share must be between 0.0 and 1.0".to_string(),
        ));
    }
    let share = Ratio::from(rust_decimal::Decimal::try_from(new_share)
        .map_err(|_| ProtocolError::GenericError("Invalid share value".to_string()))?);
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_liquidation_protocol_share(s, share);
    });
    log!(INFO, "[set_liquidation_protocol_share] Share set to: {} ({}%)", new_share, new_share * 100.0);
    Ok(())
}

/// Get the global liquidation protocol share.
#[candid_method(query)]
#[query]
fn get_liquidation_protocol_share() -> f64 {
    read_state(|s| s.liquidation_protocol_share.to_f64())
}

/// Wave-8e LIQ-005: tune the per-fee fraction routed to deficit repayment.
/// Default 0.5; bounded [0.0, 1.0]. 0.0 disables repayment; 1.0 routes the
/// entire fee until the deficit is cleared.
#[candid_method(update)]
#[update]
async fn set_deficit_repayment_fraction(new_fraction: f64) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set deficit repayment fraction".to_string(),
        ));
    }
    if !new_fraction.is_finite() || !(0.0..=1.0).contains(&new_fraction) {
        return Err(ProtocolError::GenericError(format!(
            "deficit_repayment_fraction must be in [0.0, 1.0]; got {}",
            new_fraction
        )));
    }
    let fraction = Ratio::from(
        rust_decimal::Decimal::try_from(new_fraction)
            .map_err(|_| ProtocolError::GenericError("Invalid fraction value".to_string()))?,
    );
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_deficit_repayment_fraction(s, fraction);
    });
    log!(
        INFO,
        "[set_deficit_repayment_fraction] Fraction set to: {} ({}%)",
        new_fraction,
        new_fraction * 100.0
    );
    Ok(())
}

/// Wave-8e LIQ-005: set the deficit-driven ReadOnly auto-latch threshold (e8s).
/// 0 disables the latch. Operator should leave at 0 for the first 24-48h
/// post-deploy and set after observing baseline deficit accrual.
#[candid_method(update)]
#[update]
async fn set_deficit_readonly_threshold_e8s(new_threshold: u64) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set deficit ReadOnly threshold".to_string(),
        ));
    }
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_deficit_readonly_threshold_e8s(s, new_threshold);
    });
    log!(
        INFO,
        "[set_deficit_readonly_threshold_e8s] Threshold set to: {} e8s ({})",
        new_threshold,
        if new_threshold == 0 { "latch disabled" } else { "latch armed" }
    );
    Ok(())
}

/// Wave-10 LIQ-008: tune the rolling-window length for the mass-liquidation
/// circuit breaker, in nanoseconds. 0 disables the breaker entirely (no
/// recording, no tripping). Admin-only.
#[candid_method(update)]
#[update]
async fn set_breaker_window_ns(new_window_ns: u64) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set breaker window".to_string(),
        ));
    }
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_breaker_window_ns(s, new_window_ns);
    });
    log!(
        INFO,
        "[set_breaker_window_ns] Window set to: {} ns ({})",
        new_window_ns,
        if new_window_ns == 0 { "breaker disabled" } else { "breaker armed" }
    );
    Ok(())
}

/// Wave-10 LIQ-008: tune the cumulative-debt ceiling for the mass-liquidation
/// circuit breaker, in icUSD e8s. 0 disables tripping (operator should leave
/// at 0 for the first 24-48h post-deploy, then set after observing baseline
/// `windowed_liquidation_total_e8s`). Admin-only.
#[candid_method(update)]
#[update]
async fn set_breaker_window_debt_ceiling_e8s(new_ceiling: u64) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set breaker debt ceiling".to_string(),
        ));
    }
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_breaker_window_debt_ceiling_e8s(s, new_ceiling);
    });
    log!(
        INFO,
        "[set_breaker_window_debt_ceiling_e8s] Ceiling set to: {} e8s ({})",
        new_ceiling,
        if new_ceiling == 0 { "breaker disabled" } else { "breaker armed" }
    );
    Ok(())
}

/// Wave-10 LIQ-008: clear the breaker latch so `check_vaults` resumes
/// auto-publishing on the next tick. Admin-only. Emits `BreakerCleared`
/// with the windowed total at clear time so the audit trail captures
/// what state the operator was looking at when they decided to resume.
#[candid_method(update)]
#[update]
async fn clear_liquidation_breaker() -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can clear the liquidation breaker".to_string(),
        ));
    }
    let now = ic_cdk::api::time();
    let remaining = read_state(|s| s.windowed_liquidation_total(now));
    mutate_state(|s| {
        rumi_protocol_backend::event::record_breaker_cleared(s, remaining);
    });
    log!(
        INFO,
        "[clear_liquidation_breaker] Breaker cleared (windowed total at clear: {} e8s)",
        remaining
    );
    Ok(())
}

/// Set the share of interest revenue sent to the stability pool (0.0–1.0).
/// Remainder goes to protocol treasury. Default: 0.75 (75%).
#[candid_method(update)]
#[update]
async fn set_interest_pool_share(new_share: f64) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set interest pool share".to_string(),
        ));
    }
    if !(0.0..=1.0).contains(&new_share) {
        return Err(ProtocolError::GenericError(
            "Interest pool share must be between 0.0 and 1.0".to_string(),
        ));
    }
    let share = Ratio::from(rust_decimal::Decimal::try_from(new_share)
        .map_err(|_| ProtocolError::GenericError("Invalid share value".to_string()))?);
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_interest_pool_share(s, share);
    });
    log!(INFO, "[set_interest_pool_share] Set to: {} ({}% to stability pool)", new_share, new_share * 100.0);
    Ok(())
}

/// Get the current interest pool share (fraction of interest going to stability pool).
#[candid_method(query)]
#[query]
fn get_interest_pool_share() -> f64 {
    read_state(|s| s.interest_pool_share.to_f64())
}

// ── Interest split (N-way) configuration ────────────────────────────────

/// Set the N-way interest revenue split. Each recipient is a (destination, bps) pair.
/// Destination: "stability_pool", "treasury", or "three_pool".
/// All bps must sum to exactly 10,000.
#[candid_method(update)]
#[update]
async fn set_interest_split(recipients: Vec<InterestSplitArg>) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set interest split".to_string(),
        ));
    }

    // Validate bps sum
    let total_bps: u64 = recipients.iter().map(|r| r.bps).sum();
    if total_bps != 10_000 {
        return Err(ProtocolError::GenericError(
            format!("Interest split bps must sum to 10000, got {}", total_bps),
        ));
    }

    // Validate no zero-bps entries and no duplicate destinations
    let mut seen = std::collections::HashSet::new();
    for r in &recipients {
        if r.bps == 0 {
            return Err(ProtocolError::GenericError(
                "Interest split entries must have bps > 0".to_string(),
            ));
        }
        if !seen.insert(r.destination.clone()) {
            return Err(ProtocolError::GenericError(
                format!("Duplicate destination: {}", r.destination),
            ));
        }
    }

    // Convert string destinations to enum
    let split: Vec<rumi_protocol_backend::state::InterestRecipient> = recipients.iter().map(|r| {
        let dest = match r.destination.as_str() {
            "stability_pool" => rumi_protocol_backend::state::InterestDestination::StabilityPool,
            "treasury" => rumi_protocol_backend::state::InterestDestination::Treasury,
            "three_pool" => rumi_protocol_backend::state::InterestDestination::ThreePool,
            _ => rumi_protocol_backend::state::InterestDestination::Treasury, // fallback
        };
        rumi_protocol_backend::state::InterestRecipient { destination: dest, bps: r.bps }
    }).collect();

    // Validate destinations are known
    for r in &recipients {
        if !["stability_pool", "treasury", "three_pool"].contains(&r.destination.as_str()) {
            return Err(ProtocolError::GenericError(
                format!("Unknown destination: '{}'. Valid: stability_pool, treasury, three_pool", r.destination),
            ));
        }
    }

    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_interest_split(s, split);
    });

    log!(INFO, "[set_interest_split] Updated: {:?}", recipients);
    Ok(())
}

/// Get the current interest split configuration.
#[candid_method(query)]
#[query]
fn get_interest_split() -> Vec<InterestSplitArg> {
    read_state(|s| {
        s.interest_split.iter().map(|r| {
            let dest = match &r.destination {
                rumi_protocol_backend::state::InterestDestination::StabilityPool => "stability_pool".to_string(),
                rumi_protocol_backend::state::InterestDestination::Treasury => "treasury".to_string(),
                rumi_protocol_backend::state::InterestDestination::ThreePool => "three_pool".to_string(),
            };
            InterestSplitArg { destination: dest, bps: r.bps }
        }).collect()
    })
}

/// Set the interest flush threshold (developer only).
/// Interest is accumulated per collateral type and flushed to pools/treasury
/// when any bucket reaches this threshold. Default is 10_000_000 (0.1 icUSD).
#[candid_method(update)]
#[update]
async fn set_interest_flush_threshold(threshold_e8s: u64) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set interest flush threshold".to_string(),
        ));
    }
    if threshold_e8s == 0 {
        return Err(ProtocolError::GenericError(
            "Threshold must be greater than 0".to_string(),
        ));
    }
    mutate_state(|s| {
        s.interest_flush_threshold_e8s = threshold_e8s;
    });
    log!(
        INFO,
        "[set_interest_flush_threshold] Set to {} e8s ({} icUSD)",
        threshold_e8s,
        threshold_e8s as f64 / 100_000_000.0
    );
    Ok(())
}

/// Set the 3pool canister principal for interest donations (developer only).
#[candid_method(update)]
#[update]
async fn set_three_pool_canister(canister_id: Principal) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set 3pool canister".to_string(),
        ));
    }
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_three_pool_canister(s, canister_id);
    });
    log!(INFO, "[set_three_pool_canister] Set to: {}", canister_id);
    Ok(())
}

/// Get the configured 3pool canister principal.
#[candid_method(query)]
#[query]
fn get_three_pool_canister() -> Option<Principal> {
    read_state(|s| s.three_pool_canister)
}

// ── RMR (Redemption Margin Ratio) configuration ────────────────────────

/// Get the RMR floor (ratio redeemers receive when system is healthy).
#[candid_method(query)]
#[query]
fn get_rmr_floor() -> f64 {
    read_state(|s| s.rmr_floor.to_f64())
}

/// Get the RMR ceiling (ratio redeemers receive when system is stressed).
#[candid_method(query)]
#[query]
fn get_rmr_ceiling() -> f64 {
    read_state(|s| s.rmr_ceiling.to_f64())
}

/// Get the CR above which the RMR floor applies.
#[candid_method(query)]
#[query]
fn get_rmr_floor_cr() -> f64 {
    read_state(|s| s.rmr_floor_cr.to_f64())
}

/// Get the CR below which the RMR ceiling applies.
#[candid_method(query)]
#[query]
fn get_rmr_ceiling_cr() -> f64 {
    read_state(|s| s.rmr_ceiling_cr.to_f64())
}

/// Set the RMR floor (0.0–1.0). Must be ≤ current rmr_ceiling.
#[candid_method(update)]
#[update]
async fn set_rmr_floor(value: f64) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let (is_dev, ceiling) = read_state(|s| (s.developer_principal == caller, s.rmr_ceiling.to_f64()));
    if !is_dev {
        return Err(ProtocolError::GenericError("Only the developer principal can set RMR floor".to_string()));
    }
    if !(0.0..=1.0).contains(&value) {
        return Err(ProtocolError::GenericError("RMR floor must be between 0.0 and 1.0".to_string()));
    }
    if value > ceiling {
        return Err(ProtocolError::GenericError(format!("RMR floor ({}) must be ≤ RMR ceiling ({})", value, ceiling)));
    }
    let ratio = Ratio::from(rust_decimal::Decimal::try_from(value)
        .map_err(|_| ProtocolError::GenericError("Invalid value".to_string()))?);
    mutate_state(|s| { rumi_protocol_backend::event::record_set_rmr_floor(s, ratio); });
    log!(INFO, "[set_rmr_floor] Set to: {} ({}%)", value, value * 100.0);
    Ok(())
}

/// Set the RMR ceiling (0.0–1.0). Must be ≥ current rmr_floor.
#[candid_method(update)]
#[update]
async fn set_rmr_ceiling(value: f64) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let (is_dev, floor) = read_state(|s| (s.developer_principal == caller, s.rmr_floor.to_f64()));
    if !is_dev {
        return Err(ProtocolError::GenericError("Only the developer principal can set RMR ceiling".to_string()));
    }
    if !(0.0..=1.0).contains(&value) {
        return Err(ProtocolError::GenericError("RMR ceiling must be between 0.0 and 1.0".to_string()));
    }
    if value < floor {
        return Err(ProtocolError::GenericError(format!("RMR ceiling ({}) must be ≥ RMR floor ({})", value, floor)));
    }
    let ratio = Ratio::from(rust_decimal::Decimal::try_from(value)
        .map_err(|_| ProtocolError::GenericError("Invalid value".to_string()))?);
    mutate_state(|s| { rumi_protocol_backend::event::record_set_rmr_ceiling(s, ratio); });
    log!(INFO, "[set_rmr_ceiling] Set to: {} ({}%)", value, value * 100.0);
    Ok(())
}

/// Set the CR above which the RMR floor applies (≥ 1.0). Must be ≥ current rmr_ceiling_cr.
#[candid_method(update)]
#[update]
async fn set_rmr_floor_cr(value: f64) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let (is_dev, ceiling_cr) = read_state(|s| (s.developer_principal == caller, s.rmr_ceiling_cr.to_f64()));
    if !is_dev {
        return Err(ProtocolError::GenericError("Only the developer principal can set RMR floor CR".to_string()));
    }
    if value < 1.0 {
        return Err(ProtocolError::GenericError("RMR floor CR must be ≥ 1.0".to_string()));
    }
    if value < ceiling_cr {
        return Err(ProtocolError::GenericError(format!("RMR floor CR ({}) must be ≥ RMR ceiling CR ({})", value, ceiling_cr)));
    }
    let ratio = Ratio::from(rust_decimal::Decimal::try_from(value)
        .map_err(|_| ProtocolError::GenericError("Invalid value".to_string()))?);
    mutate_state(|s| { rumi_protocol_backend::event::record_set_rmr_floor_cr(s, ratio); });
    log!(INFO, "[set_rmr_floor_cr] Set to: {} ({}%)", value, value * 100.0);
    Ok(())
}

/// Set the CR below which the RMR ceiling applies (≥ 1.0). Must be ≤ current rmr_floor_cr.
#[candid_method(update)]
#[update]
async fn set_rmr_ceiling_cr(value: f64) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let (is_dev, floor_cr) = read_state(|s| (s.developer_principal == caller, s.rmr_floor_cr.to_f64()));
    if !is_dev {
        return Err(ProtocolError::GenericError("Only the developer principal can set RMR ceiling CR".to_string()));
    }
    if value < 1.0 {
        return Err(ProtocolError::GenericError("RMR ceiling CR must be ≥ 1.0".to_string()));
    }
    if value > floor_cr {
        return Err(ProtocolError::GenericError(format!("RMR ceiling CR ({}) must be ≤ RMR floor CR ({})", value, floor_cr)));
    }
    let ratio = Ratio::from(rust_decimal::Decimal::try_from(value)
        .map_err(|_| ProtocolError::GenericError("Invalid value".to_string()))?);
    mutate_state(|s| { rumi_protocol_backend::event::record_set_rmr_ceiling_cr(s, ratio); });
    log!(INFO, "[set_rmr_ceiling_cr] Set to: {} ({}%)", value, value * 100.0);
    Ok(())
}

#[derive(CandidType, Deserialize)]
pub struct TreasuryStats {
    pub treasury_principal: Option<Principal>,
    pub total_accrued_interest_system: u64,
    pub pending_treasury_interest: u64,
    pub pending_treasury_collateral_entries: u64,
    pub liquidation_protocol_share: f64,
    pub pending_interest_for_pools_total: u64,
    pub interest_flush_threshold_e8s: u64,
}

/// Get treasury-related statistics including accrued interest across all vaults.
#[candid_method(query)]
#[query]
fn get_treasury_stats() -> TreasuryStats {
    read_state(|s| TreasuryStats {
        treasury_principal: s.treasury_principal,
        total_accrued_interest_system: s.vault_id_to_vaults.values()
            .map(|v| v.accrued_interest.to_u64()).sum(),
        pending_treasury_interest: s.pending_treasury_interest.to_u64(),
        pending_treasury_collateral_entries: s.pending_treasury_collateral.len() as u64,
        liquidation_protocol_share: s.liquidation_protocol_share.to_f64(),
        pending_interest_for_pools_total: s.pending_interest_for_pools.values().sum(),
        interest_flush_threshold_e8s: s.interest_flush_threshold_e8s,
    })
}

/// Get the effective recovery target CR (threshold × multiplier)
#[candid_method(query)]
#[query]
fn get_recovery_target_cr() -> f64 {
    read_state(|s| (s.recovery_mode_threshold * s.recovery_cr_multiplier).to_f64())
}

/// Legacy: set the recovery target CR as an absolute value.
/// Kept for Candid backwards compat. Internally converts to multiplier.
#[candid_method(update)]
#[update]
async fn set_recovery_target_cr(new_rate: f64) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set recovery target CR".to_string(),
        ));
    }
    // Convert absolute target to multiplier: multiplier = target / current threshold
    let threshold = read_state(|s| s.recovery_mode_threshold.to_f64());
    if threshold <= 0.0 {
        return Err(ProtocolError::GenericError(
            "Cannot compute multiplier: recovery_mode_threshold is zero".to_string(),
        ));
    }
    let multiplier_val = new_rate / threshold;
    if multiplier_val < 1.001 || multiplier_val > 1.5 {
        return Err(ProtocolError::GenericError(format!(
            "Computed multiplier {} (target {} / threshold {}) is out of range 1.001..1.5",
            multiplier_val, new_rate, threshold
        )));
    }
    let multiplier = Ratio::from(rust_decimal::Decimal::try_from(multiplier_val)
        .map_err(|_| ProtocolError::GenericError("Invalid rate".to_string()))?);
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_recovery_cr_multiplier(s, multiplier);
    });
    log!(INFO, "[set_recovery_target_cr] (legacy) → multiplier set to: {} ({}% buffer)", multiplier_val, (multiplier_val - 1.0) * 100.0);
    Ok(())
}

/// Set per-collateral recovery mode overrides for borrowing fee and interest rate (developer only).
/// Pass None to clear an override (reverts to normal value during Recovery).
#[candid_method(update)]
#[update]
async fn set_recovery_parameters(
    collateral_type: Principal,
    recovery_borrowing_fee: Option<f64>,
    recovery_interest_rate_apr: Option<f64>,
) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set recovery parameters".to_string(),
        ));
    }
    // Validate collateral type exists
    let exists = read_state(|s| s.collateral_configs.contains_key(&collateral_type));
    if !exists {
        return Err(ProtocolError::GenericError(
            "Unknown collateral type".to_string(),
        ));
    }
    // Validate fee ranges
    if let Some(fee) = recovery_borrowing_fee {
        if fee < 0.0 || fee > 0.10 {
            return Err(ProtocolError::GenericError(
                "Recovery borrowing fee must be between 0 and 0.10 (10%)".to_string(),
            ));
        }
    }
    if let Some(apr) = recovery_interest_rate_apr {
        if apr < 0.0 || apr > 1.0 {
            return Err(ProtocolError::GenericError(
                "Recovery interest rate APR must be between 0 and 1.0 (100%)".to_string(),
            ));
        }
    }
    let fee_ratio = recovery_borrowing_fee
        .map(|f| Decimal::try_from(f))
        .transpose()
        .map_err(|_| ProtocolError::GenericError("Invalid borrowing fee value".to_string()))?
        .map(Ratio::from);
    let apr_ratio = recovery_interest_rate_apr
        .map(|f| Decimal::try_from(f))
        .transpose()
        .map_err(|_| ProtocolError::GenericError("Invalid interest rate value".to_string()))?
        .map(Ratio::from);
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_recovery_parameters(
            s,
            collateral_type,
            fee_ratio,
            apr_ratio,
        );
    });
    log!(
        INFO,
        "[set_recovery_parameters] collateral={}, recovery_borrowing_fee={:?}, recovery_interest_rate_apr={:?}",
        collateral_type,
        recovery_borrowing_fee,
        recovery_interest_rate_apr
    );
    Ok(())
}

/// Set the base interest rate APR for a specific collateral type (developer only).
/// e.g. 0.02 = 2% APR, 0.005 = 0.5% APR.
#[candid_method(update)]
#[update]
async fn set_interest_rate(
    collateral_type: Principal,
    interest_rate_apr: f64,
) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set interest rates".to_string(),
        ));
    }
    let exists = read_state(|s| s.collateral_configs.contains_key(&collateral_type));
    if !exists {
        return Err(ProtocolError::GenericError(
            "Unknown collateral type".to_string(),
        ));
    }
    if interest_rate_apr < 0.0 || interest_rate_apr > 1.0 {
        return Err(ProtocolError::GenericError(
            "Interest rate APR must be between 0 and 1.0 (100%)".to_string(),
        ));
    }
    let rate = Ratio::from(
        Decimal::try_from(interest_rate_apr)
            .map_err(|_| ProtocolError::GenericError("Invalid interest rate value".to_string()))?,
    );
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_interest_rate(s, collateral_type, rate);
    });
    log!(
        INFO,
        "[set_interest_rate] collateral={}, interest_rate_apr={}",
        collateral_type,
        interest_rate_apr
    );
    Ok(())
}

/// Set the borrowing fee for a specific collateral type (developer only).
/// e.g. 0.005 = 0.5%, 0.001 = 0.1%. Range 0.0–0.10.
#[candid_method(update)]
#[update]
async fn set_collateral_borrowing_fee(
    collateral_type: Principal,
    borrowing_fee: f64,
) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set borrowing fees".to_string(),
        ));
    }
    let exists = read_state(|s| s.collateral_configs.contains_key(&collateral_type));
    if !exists {
        return Err(ProtocolError::GenericError(
            "Unknown collateral type".to_string(),
        ));
    }
    if borrowing_fee < 0.0 || borrowing_fee > 0.10 {
        return Err(ProtocolError::GenericError(
            "Borrowing fee must be between 0 and 0.10 (10%)".to_string(),
        ));
    }
    let fee = Ratio::from(
        Decimal::try_from(borrowing_fee)
            .map_err(|_| ProtocolError::GenericError("Invalid fee value".to_string()))?,
    );
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_collateral_borrowing_fee(s, collateral_type, fee);
    });
    log!(
        INFO,
        "[set_collateral_borrowing_fee] collateral={}, borrowing_fee={}",
        collateral_type,
        borrowing_fee
    );
    Ok(())
}

/// Set rate curve markers for a collateral type or the global default.
/// `collateral_type`: None = update global default curve; Some(principal) = per-asset curve.
/// `markers`: Vec of (cr_level, multiplier) pairs, sorted ascending by cr_level.
#[candid_method(update)]
#[update]
async fn set_rate_curve_markers(
    collateral_type: Option<Principal>,
    markers: Vec<(f64, f64)>,
) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set rate curve markers".to_string(),
        ));
    }
    if markers.len() < 2 {
        return Err(ProtocolError::GenericError(
            "Rate curve must have at least 2 markers".to_string(),
        ));
    }
    // Validate sorted ascending and positive multipliers
    for i in 0..markers.len() {
        if markers[i].1 <= 0.0 {
            return Err(ProtocolError::GenericError(
                format!("Multiplier at index {} must be positive", i),
            ));
        }
        if i > 0 && markers[i].0 <= markers[i - 1].0 {
            return Err(ProtocolError::GenericError(
                "Markers must be sorted ascending by cr_level".to_string(),
            ));
        }
    }
    // Validate collateral type exists if specified
    if let Some(ct) = collateral_type {
        let exists = read_state(|s| s.collateral_configs.contains_key(&ct));
        if !exists {
            return Err(ProtocolError::GenericError("Unknown collateral type".to_string()));
        }
    }
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_rate_curve_markers(
            s, collateral_type, markers.clone(),
        );
    });
    log!(INFO, "[set_rate_curve_markers] collateral={:?}, markers={:?}", collateral_type, markers);
    Ok(())
}

/// Set the recovery rate curve (Layer 2 system-wide multipliers).
/// `markers`: Vec of (SystemThreshold variant name, multiplier) pairs.
#[candid_method(update)]
#[update]
async fn set_recovery_rate_curve(
    markers: Vec<(String, f64)>,
) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set recovery rate curve".to_string(),
        ));
    }
    if markers.len() < 2 {
        return Err(ProtocolError::GenericError(
            "Recovery rate curve must have at least 2 markers".to_string(),
        ));
    }
    // Parse and validate threshold names
    use rumi_protocol_backend::state::SystemThreshold;
    let mut parsed: Vec<(SystemThreshold, f64)> = Vec::new();
    for (thresh_str, mult) in &markers {
        if *mult <= 0.0 {
            return Err(ProtocolError::GenericError(
                format!("Multiplier for {} must be positive", thresh_str),
            ));
        }
        let threshold = match thresh_str.as_str() {
            "LiquidationRatio" => SystemThreshold::LiquidationRatio,
            "BorrowThreshold" => SystemThreshold::BorrowThreshold,
            "WarningCr" => SystemThreshold::WarningCr,
            "HealthyCr" => SystemThreshold::HealthyCr,
            "TotalCollateralRatio" => SystemThreshold::TotalCollateralRatio,
            _ => return Err(ProtocolError::GenericError(
                format!("Unknown threshold: {}. Valid: LiquidationRatio, BorrowThreshold, WarningCr, HealthyCr, TotalCollateralRatio", thresh_str),
            )),
        };
        parsed.push((threshold, *mult));
    }
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_recovery_rate_curve(s, parsed);
    });
    log!(INFO, "[set_recovery_rate_curve] markers={:?}", markers);
    Ok(())
}

/// Set the dynamic borrowing fee curve.
/// Pass None to disable (revert to flat fee).
/// Accepts a JSON-serialized RateCurveV2.
#[candid_method(update)]
#[update]
async fn set_borrowing_fee_curve(
    curve_json: Option<String>,
) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set borrowing fee curve".to_string(),
        ));
    }
    let curve: Option<RateCurveV2> = match curve_json {
        None => None,
        Some(json) => {
            let parsed: RateCurveV2 = serde_json::from_str(&json)
                .map_err(|e| ProtocolError::GenericError(format!("Invalid curve JSON: {}", e)))?;
            // INT-003: validate structure and multiplier upper bound. The
            // upper bound prevents a runaway fee from underflowing
            // `amount - fee` in `borrow_from_vault_internal`.
            parsed
                .validate()
                .map_err(ProtocolError::GenericError)?;
            Some(parsed)
        }
    };
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_borrowing_fee_curve(s, curve);
    });
    log!(INFO, "[set_borrowing_fee_curve] Updated borrowing fee curve");
    Ok(())
}

/// Set the healthy CR override for a collateral type.
/// `healthy_cr`: None = reset to default (1.5x borrow threshold).
#[candid_method(update)]
#[update]
async fn set_healthy_cr(
    collateral_type: Principal,
    healthy_cr: Option<f64>,
) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set healthy CR".to_string(),
        ));
    }
    let exists = read_state(|s| s.collateral_configs.contains_key(&collateral_type));
    if !exists {
        return Err(ProtocolError::GenericError("Unknown collateral type".to_string()));
    }
    // Validate healthy_cr > borrow_threshold if set
    if let Some(cr) = healthy_cr {
        let borrow_threshold = read_state(|s| {
            s.collateral_configs.get(&collateral_type)
                .map(|c| c.borrow_threshold_ratio.to_f64())
                .unwrap_or(1.5)
        });
        if cr <= borrow_threshold {
            return Err(ProtocolError::GenericError(
                format!("healthy_cr ({}) must be greater than borrow_threshold_ratio ({})", cr, borrow_threshold),
            ));
        }
    }
    let ratio = healthy_cr
        .map(|f| Decimal::try_from(f))
        .transpose()
        .map_err(|_| ProtocolError::GenericError("Invalid healthy_cr value".to_string()))?
        .map(Ratio::from);
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_healthy_cr(s, collateral_type, ratio);
    });
    log!(INFO, "[set_healthy_cr] collateral={}, healthy_cr={:?}", collateral_type, healthy_cr);
    Ok(())
}

/// Query: get the current dynamic interest rate for a specific vault.
#[candid_method(query)]
#[query]
fn get_vault_interest_rate(vault_id: u64) -> Result<f64, ProtocolError> {
    read_state(|s| {
        let vault = s.vault_id_to_vaults.get(&vault_id)
            .ok_or_else(|| ProtocolError::GenericError(format!("Vault {} not found", vault_id)))?;
        let config = s.get_collateral_config(&vault.collateral_type)
            .ok_or_else(|| ProtocolError::GenericError("Unknown collateral type".to_string()))?;
        // Compute vault CR
        let price = config.last_price
            .ok_or_else(|| ProtocolError::GenericError("No price available for collateral".to_string()))?;
        let price_dec = Decimal::from_f64(price).unwrap_or(Decimal::ZERO);
        let vault_value = rumi_protocol_backend::numeric::collateral_usd_value(
            vault.collateral_amount,
            price_dec,
            config.decimals,
        );
        let vault_cr = if vault.borrowed_icusd_amount == ICUSD::new(0) {
            Ratio::from(Decimal::MAX)
        } else {
            vault_value / vault.borrowed_icusd_amount
        };
        Ok(s.get_dynamic_interest_rate_for(&vault.collateral_type, vault_cr).to_f64())
    })
}

// Add guard cleanup method for developers to resolve stuck operations
#[candid_method(update)]
#[update]
async fn clear_stuck_operations(principal_id: Option<Principal>) -> Result<u64, ProtocolError> {
    let caller = ic_cdk::caller();
    
    // Only developer can clear stuck operations
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError("Only developer can clear stuck operations".to_string()));
    }
    
    let cleared_count = mutate_state(|s| {
        use ic_cdk::api::time;
        let current_time = time();
        let mut principals_to_remove: Vec<Principal> = Vec::new();
        let mut count = 0u64;

        if let Some(target_principal) = principal_id {
            // Clear specific principal's guard
            if s.principal_guards.contains(&target_principal) {
                principals_to_remove.push(target_principal);
                if let Some(op_name) = s.operation_names.get(&target_principal) {
                    log!(INFO,
                        "[clear_stuck_operations] Clearing operation '{}' for principal: {}",
                        op_name, target_principal.to_string()
                    );
                }
                count += 1;
            }
        } else {
            // Clear all operations older than 2 minutes
            for principal in s.principal_guards.iter() {
                let mut should_remove = false;

                if let Some(timestamp) = s.principal_guard_timestamps.get(principal) {
                    let age_seconds = (current_time - timestamp) / 1_000_000_000;
                    if age_seconds > 120 {
                        should_remove = true;
                    }
                }

                if should_remove {
                    principals_to_remove.push(*principal);
                    if let Some(op_name) = s.operation_names.get(principal) {
                        log!(INFO,
                            "[clear_stuck_operations] Clearing stale operation '{}' for principal: {}",
                            op_name, principal.to_string()
                        );
                    }
                    count += 1;
                }
            }
        }

        // Remove the identified operations
        for principal in principals_to_remove {
            s.principal_guards.remove(&principal);
            s.principal_guard_timestamps.remove(&principal);
            s.operation_states.remove(&principal);
            s.operation_names.remove(&principal);
        }

        count
    });
    
    log!(INFO, "[clear_stuck_operations] Cleared {} stuck operations", cleared_count);
    Ok(cleared_count)
}

// ---- Multi-collateral admin endpoints ----

#[candid_method(update)]
#[update]
async fn add_collateral_token(arg: rumi_protocol_backend::AddCollateralArg) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError("Only developer can add collateral types".to_string()));
    }

    // Check it doesn't already exist
    let already_exists = read_state(|s| s.collateral_configs.contains_key(&arg.ledger_canister_id));
    if already_exists {
        return Err(ProtocolError::GenericError("Collateral type already exists".to_string()));
    }

    // Query icrc1_decimals from the ledger
    let decimals_result: Result<(u8,), _> = ic_cdk::call(arg.ledger_canister_id, "icrc1_decimals", ()).await;
    let decimals = match decimals_result {
        Ok((d,)) => d,
        Err((code, msg)) => {
            return Err(ProtocolError::GenericError(format!(
                "Failed to query icrc1_decimals from {}: {:?} {}",
                arg.ledger_canister_id, code, msg
            )));
        }
    };

    // Query icrc1_fee from the ledger
    let fee_result: Result<(candid::Nat,), _> = ic_cdk::call(arg.ledger_canister_id, "icrc1_fee", ()).await;
    let ledger_fee = match fee_result {
        Ok((f,)) => {
            use num_traits::ToPrimitive;
            f.0.to_u64().unwrap_or(0)
        }
        Err((code, msg)) => {
            return Err(ProtocolError::GenericError(format!(
                "Failed to query icrc1_fee from {}: {:?} {}",
                arg.ledger_canister_id, code, msg
            )));
        }
    };

    use rumi_protocol_backend::state::{CollateralConfig, CollateralStatus};

    let config = CollateralConfig {
        ledger_canister_id: arg.ledger_canister_id,
        decimals,
        liquidation_ratio: Ratio::from_f64(arg.liquidation_ratio),
        borrow_threshold_ratio: Ratio::from_f64(arg.borrow_threshold_ratio),
        liquidation_bonus: Ratio::from_f64(arg.liquidation_bonus),
        borrowing_fee: Ratio::from_f64(arg.borrowing_fee),
        interest_rate_apr: Ratio::from_f64(arg.interest_rate_apr),
        debt_ceiling: arg.debt_ceiling,
        min_vault_debt: rumi_protocol_backend::numeric::ICUSD::from(arg.min_vault_debt),
        ledger_fee,
        price_source: arg.price_source,
        status: CollateralStatus::Active,
        last_price: None,
        last_price_timestamp: None,
        redemption_fee_floor: Ratio::from_f64(arg.redemption_fee_floor.unwrap_or(0.005)),
        redemption_fee_ceiling: Ratio::from_f64(arg.redemption_fee_ceiling.unwrap_or(0.05)),
        current_base_rate: Ratio::from_f64(0.0),
        last_redemption_time: 0,
        // Computed from borrow_threshold_ratio × recovery_cr_multiplier; not user-supplied.
        recovery_target_cr: Ratio::from_f64(arg.borrow_threshold_ratio) * read_state(|s| s.recovery_cr_multiplier),
        min_collateral_deposit: arg.min_collateral_deposit,
        recovery_borrowing_fee: None,
        recovery_interest_rate_apr: None,
        display_color: arg.display_color,
        healthy_cr: None,
        rate_curve: None,
        redemption_tier: arg.redemption_tier.unwrap_or(1).clamp(1, 3),
    };

    mutate_state(|s| {
        event::record_add_collateral_type(s, arg.ledger_canister_id, config);
    });

    // Register a price-fetching timer for the new collateral type.
    // ICP has its own dedicated timer in setup_timers(); other collateral
    // types use the generic fetch_collateral_price.
    let ledger_id = arg.ledger_canister_id;
    let is_icp = read_state(|s| s.icp_collateral_type() == ledger_id);
    if !is_icp {
        log!(INFO, "[add_collateral_token] Registering price timer for collateral {}", ledger_id);
        ic_cdk_timers::set_timer_interval(
            rumi_protocol_backend::xrc::FETCHING_ICP_RATE_INTERVAL,
            move || ic_cdk::spawn(rumi_protocol_backend::management::fetch_collateral_price(ledger_id)),
        );
    }

    log!(INFO, "[add_collateral_token] Added collateral type: {} (decimals={})", arg.ledger_canister_id, decimals);

    // Best-effort: register the new collateral on the stability pool so it
    // can accept liquidation proceeds in this token.  If the SP call fails
    // we log a warning but don't fail the overall operation — the admin can
    // always call register_collateral on the SP manually.
    if let Some(sp_canister) = read_state(|s| s.stability_pool_canister) {
        // Query the ledger symbol for the SP registry entry.
        let symbol = match ic_cdk::call::<(), (String,)>(ledger_id, "icrc1_symbol", ()).await {
            Ok((s,)) => s,
            Err((code, msg)) => {
                log!(INFO, "[add_collateral_token] WARNING: Failed to query icrc1_symbol from {}: {:?} {} — skipping SP registration", ledger_id, code, msg);
                return Ok(());
            }
        };

        #[derive(candid::CandidType)]
        struct SpCollateralInfo {
            ledger_id: Principal,
            symbol: String,
            decimals: u8,
            status: SpCollateralStatus,
        }
        #[derive(candid::CandidType)]
        enum SpCollateralStatus { Active }

        let info = SpCollateralInfo {
            ledger_id: ledger_id,
            symbol: symbol.clone(),
            decimals,
            status: SpCollateralStatus::Active,
        };

        // We ignore the SP's Result return value — if the call itself succeeds,
        // registration worked (or the collateral already existed, which is fine).
        match ic_cdk::call::<(SpCollateralInfo,), ()>(sp_canister, "register_collateral", (info,)).await {
            Ok(()) => {
                log!(INFO, "[add_collateral_token] Registered {} ({}) on stability pool {}", symbol, ledger_id, sp_canister);
            }
            Err((code, msg)) => {
                log!(INFO, "[add_collateral_token] WARNING: Failed to register collateral on SP: {:?} {} — register manually", code, msg);
            }
        }
    }

    Ok(())
}

#[candid_method(update)]
#[update]
async fn set_collateral_status(
    collateral_type: Principal,
    status: rumi_protocol_backend::state::CollateralStatus,
) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError("Only developer can change collateral status".to_string()));
    }

    let exists = read_state(|s| s.collateral_configs.contains_key(&collateral_type));
    if !exists {
        return Err(ProtocolError::GenericError("Collateral type not found".to_string()));
    }

    mutate_state(|s| {
        event::record_update_collateral_status(s, collateral_type, status);
    });

    log!(INFO, "[set_collateral_status] Collateral {} status set to {:?}", collateral_type, status);
    Ok(())
}

#[candid_method(update)]
#[update]
async fn set_collateral_debt_ceiling(
    collateral_type: Principal,
    debt_ceiling: u64,
) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError("Only developer can change debt ceiling".to_string()));
    }

    let exists = read_state(|s| s.collateral_configs.contains_key(&collateral_type));
    if !exists {
        return Err(ProtocolError::GenericError("Collateral type not found".to_string()));
    }

    mutate_state(|s| {
        if let Some(config) = s.collateral_configs.get_mut(&collateral_type) {
            config.debt_ceiling = debt_ceiling;
        }
    });

    log!(INFO, "[set_collateral_debt_ceiling] Collateral {} debt ceiling set to {}", collateral_type, debt_ceiling);
    Ok(())
}

/// Set the LST haircut for a collateral type that uses LstWrapped price source.
/// Haircut is a decimal: 0.07 = 7%, range 0.0–0.50.
#[candid_method(update)]
#[update]
async fn set_lst_haircut(
    collateral_type: Principal,
    haircut: f64,
) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError("Only developer can set LST haircut".to_string()));
    }

    if haircut < 0.0 || haircut > 0.50 {
        return Err(ProtocolError::GenericError(
            format!("Haircut must be between 0.0 and 0.50, got {}", haircut),
        ));
    }

    mutate_state(|s| {
        if let Some(config) = s.collateral_configs.get_mut(&collateral_type) {
            match &mut config.price_source {
                rumi_protocol_backend::state::PriceSource::LstWrapped { haircut: h, .. } => {
                    *h = haircut;
                    log!(INFO, "[set_lst_haircut] Collateral {} haircut set to {}", collateral_type, haircut);
                }
                _ => {
                    log!(INFO, "[set_lst_haircut] Collateral {} is not LstWrapped, ignoring", collateral_type);
                }
            }
        }
    });

    Ok(())
}

/// Set the liquidation ratio for a specific collateral type (developer only).
/// e.g. 1.25 = 125%. Must be strictly less than borrow_threshold_ratio.
#[candid_method(update)]
#[update]
async fn set_collateral_liquidation_ratio(
    collateral_type: Principal,
    liquidation_ratio: f64,
) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set liquidation ratio".to_string(),
        ));
    }
    if liquidation_ratio <= 1.0 || liquidation_ratio > 5.0 {
        return Err(ProtocolError::GenericError(format!(
            "liquidation_ratio ({}) must be > 1.0 and ≤ 5.0",
            liquidation_ratio
        )));
    }
    let borrow_threshold = read_state(|s| {
        s.collateral_configs
            .get(&collateral_type)
            .map(|c| c.borrow_threshold_ratio.to_f64())
    });
    let borrow_threshold = match borrow_threshold {
        Some(bt) => bt,
        None => return Err(ProtocolError::GenericError("Unknown collateral type".to_string())),
    };
    if liquidation_ratio >= borrow_threshold {
        return Err(ProtocolError::GenericError(format!(
            "liquidation_ratio ({}) must be strictly less than borrow_threshold_ratio ({})",
            liquidation_ratio, borrow_threshold
        )));
    }
    let ratio = Ratio::from(
        Decimal::try_from(liquidation_ratio)
            .map_err(|_| ProtocolError::GenericError("Invalid liquidation_ratio value".to_string()))?,
    );
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_collateral_liquidation_ratio(s, collateral_type, ratio);
    });
    log!(INFO, "[set_collateral_liquidation_ratio] collateral={}, liquidation_ratio={}", collateral_type, liquidation_ratio);
    Ok(())
}

/// Set the borrow threshold ratio for a specific collateral type (developer only).
/// e.g. 1.55 = 155%. Must be strictly greater than liquidation_ratio
/// and strictly less than healthy_cr if healthy_cr is set.
#[candid_method(update)]
#[update]
async fn set_collateral_borrow_threshold(
    collateral_type: Principal,
    borrow_threshold_ratio: f64,
) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set borrow threshold".to_string(),
        ));
    }
    if borrow_threshold_ratio <= 1.0 || borrow_threshold_ratio > 5.0 {
        return Err(ProtocolError::GenericError(format!(
            "borrow_threshold_ratio ({}) must be > 1.0 and ≤ 5.0",
            borrow_threshold_ratio
        )));
    }
    let (liq_ratio, healthy_cr) = read_state(|s| {
        s.collateral_configs
            .get(&collateral_type)
            .map(|c| (
                c.liquidation_ratio.to_f64(),
                c.healthy_cr.map(|r| r.to_f64()),
            ))
            .unwrap_or((0.0, None))
    });
    let exists = read_state(|s| s.collateral_configs.contains_key(&collateral_type));
    if !exists {
        return Err(ProtocolError::GenericError("Unknown collateral type".to_string()));
    }
    if borrow_threshold_ratio <= liq_ratio {
        return Err(ProtocolError::GenericError(format!(
            "borrow_threshold_ratio ({}) must be strictly greater than liquidation_ratio ({})",
            borrow_threshold_ratio, liq_ratio
        )));
    }
    if let Some(hcr) = healthy_cr {
        if borrow_threshold_ratio >= hcr {
            return Err(ProtocolError::GenericError(format!(
                "borrow_threshold_ratio ({}) must be strictly less than healthy_cr ({})",
                borrow_threshold_ratio, hcr
            )));
        }
    }
    let ratio = Ratio::from(
        Decimal::try_from(borrow_threshold_ratio)
            .map_err(|_| ProtocolError::GenericError("Invalid borrow_threshold_ratio value".to_string()))?,
    );
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_collateral_borrow_threshold(s, collateral_type, ratio);
    });
    log!(INFO, "[set_collateral_borrow_threshold] collateral={}, borrow_threshold_ratio={}", collateral_type, borrow_threshold_ratio);
    Ok(())
}

/// Set the liquidation bonus for a specific collateral type (developer only).
/// e.g. 1.10 = 10% bonus. Range 1.0–1.5.
#[candid_method(update)]
#[update]
async fn set_collateral_liquidation_bonus(
    collateral_type: Principal,
    liquidation_bonus: f64,
) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set liquidation bonus".to_string(),
        ));
    }
    if liquidation_bonus < 1.0 || liquidation_bonus > 1.5 {
        return Err(ProtocolError::GenericError(
            "liquidation_bonus must be between 1.0 and 1.5".to_string(),
        ));
    }
    let exists = read_state(|s| s.collateral_configs.contains_key(&collateral_type));
    if !exists {
        return Err(ProtocolError::GenericError("Unknown collateral type".to_string()));
    }
    let ratio = Ratio::from(
        Decimal::try_from(liquidation_bonus)
            .map_err(|_| ProtocolError::GenericError("Invalid liquidation_bonus value".to_string()))?,
    );
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_collateral_liquidation_bonus(s, collateral_type, ratio);
    });
    log!(INFO, "[set_collateral_liquidation_bonus] collateral={}, liquidation_bonus={}", collateral_type, liquidation_bonus);
    Ok(())
}

/// Set the minimum vault debt (dust threshold) for a specific collateral type (developer only).
/// `min_vault_debt` is in icUSD e8s.
#[candid_method(update)]
#[update]
async fn set_collateral_min_vault_debt(
    collateral_type: Principal,
    min_vault_debt: u64,
) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set min vault debt".to_string(),
        ));
    }
    let exists = read_state(|s| s.collateral_configs.contains_key(&collateral_type));
    if !exists {
        return Err(ProtocolError::GenericError("Unknown collateral type".to_string()));
    }
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_collateral_min_vault_debt(s, collateral_type, min_vault_debt);
    });
    log!(INFO, "[set_collateral_min_vault_debt] collateral={}, min_vault_debt={}", collateral_type, min_vault_debt);
    Ok(())
}

/// Set the ledger fee for a specific collateral type (developer only).
/// `ledger_fee` is in the collateral token's native units.
/// Note: the backend also auto-syncs this from BadFee errors during transfers.
#[candid_method(update)]
#[update]
async fn set_collateral_ledger_fee(
    collateral_type: Principal,
    ledger_fee: u64,
) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set ledger fee".to_string(),
        ));
    }
    let exists = read_state(|s| s.collateral_configs.contains_key(&collateral_type));
    if !exists {
        return Err(ProtocolError::GenericError("Unknown collateral type".to_string()));
    }
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_collateral_ledger_fee(s, collateral_type, ledger_fee);
    });
    log!(INFO, "[set_collateral_ledger_fee] collateral={}, ledger_fee={}", collateral_type, ledger_fee);
    Ok(())
}

/// Set the redemption fee floor for a specific collateral type (developer only).
/// e.g. 0.005 = 0.5%. Must be ≤ redemption_fee_ceiling.
#[candid_method(update)]
#[update]
async fn set_collateral_redemption_fee_floor(
    collateral_type: Principal,
    redemption_fee_floor: f64,
) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set redemption fee floor".to_string(),
        ));
    }
    if redemption_fee_floor < 0.0 || redemption_fee_floor > 0.10 {
        return Err(ProtocolError::GenericError(
            "redemption_fee_floor must be between 0 and 0.10 (10%)".to_string(),
        ));
    }
    let ceiling = read_state(|s| {
        s.collateral_configs
            .get(&collateral_type)
            .map(|c| c.redemption_fee_ceiling.to_f64())
    });
    let ceiling = match ceiling {
        Some(c) => c,
        None => return Err(ProtocolError::GenericError("Unknown collateral type".to_string())),
    };
    if redemption_fee_floor > ceiling {
        return Err(ProtocolError::GenericError(format!(
            "redemption_fee_floor ({}) must be ≤ redemption_fee_ceiling ({})",
            redemption_fee_floor, ceiling
        )));
    }
    let ratio = Ratio::from(
        Decimal::try_from(redemption_fee_floor)
            .map_err(|_| ProtocolError::GenericError("Invalid redemption_fee_floor value".to_string()))?,
    );
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_collateral_redemption_fee_floor(s, collateral_type, ratio);
    });
    log!(INFO, "[set_collateral_redemption_fee_floor] collateral={}, redemption_fee_floor={}", collateral_type, redemption_fee_floor);
    Ok(())
}

/// Set the redemption fee ceiling for a specific collateral type (developer only).
/// e.g. 0.05 = 5%. Must be ≥ redemption_fee_floor. Range 0.0–0.50.
#[candid_method(update)]
#[update]
async fn set_collateral_redemption_fee_ceiling(
    collateral_type: Principal,
    redemption_fee_ceiling: f64,
) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set redemption fee ceiling".to_string(),
        ));
    }
    if redemption_fee_ceiling < 0.0 || redemption_fee_ceiling > 0.50 {
        return Err(ProtocolError::GenericError(
            "redemption_fee_ceiling must be between 0 and 0.50 (50%)".to_string(),
        ));
    }
    let floor = read_state(|s| {
        s.collateral_configs
            .get(&collateral_type)
            .map(|c| c.redemption_fee_floor.to_f64())
    });
    let floor = match floor {
        Some(f) => f,
        None => return Err(ProtocolError::GenericError("Unknown collateral type".to_string())),
    };
    if redemption_fee_ceiling < floor {
        return Err(ProtocolError::GenericError(format!(
            "redemption_fee_ceiling ({}) must be ≥ redemption_fee_floor ({})",
            redemption_fee_ceiling, floor
        )));
    }
    let ratio = Ratio::from(
        Decimal::try_from(redemption_fee_ceiling)
            .map_err(|_| ProtocolError::GenericError("Invalid redemption_fee_ceiling value".to_string()))?,
    );
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_collateral_redemption_fee_ceiling(s, collateral_type, ratio);
    });
    log!(INFO, "[set_collateral_redemption_fee_ceiling] collateral={}, redemption_fee_ceiling={}", collateral_type, redemption_fee_ceiling);
    Ok(())
}

/// Set the minimum collateral deposit for a specific collateral type (developer only).
/// `min_collateral_deposit` is in the collateral token's native units.
#[candid_method(update)]
#[update]
async fn set_collateral_min_deposit(
    collateral_type: Principal,
    min_collateral_deposit: u64,
) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set min collateral deposit".to_string(),
        ));
    }
    let exists = read_state(|s| s.collateral_configs.contains_key(&collateral_type));
    if !exists {
        return Err(ProtocolError::GenericError("Unknown collateral type".to_string()));
    }
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_collateral_min_deposit(s, collateral_type, min_collateral_deposit);
    });
    log!(INFO, "[set_collateral_min_deposit] collateral={}, min_collateral_deposit={}", collateral_type, min_collateral_deposit);
    Ok(())
}

/// Set the display color (hex) for a collateral type, used by frontend (developer only).
/// Pass None to clear.
#[candid_method(update)]
#[update]
async fn set_collateral_display_color(
    collateral_type: Principal,
    display_color: Option<String>,
) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set display color".to_string(),
        ));
    }
    if let Some(ref c) = display_color {
        if !c.starts_with('#') || (c.len() != 4 && c.len() != 7 && c.len() != 9) {
            return Err(ProtocolError::GenericError(
                "display_color must be a hex color like #RGB, #RRGGBB, or #RRGGBBAA".to_string(),
            ));
        }
    }
    let exists = read_state(|s| s.collateral_configs.contains_key(&collateral_type));
    if !exists {
        return Err(ProtocolError::GenericError("Unknown collateral type".to_string()));
    }
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_collateral_display_color(s, collateral_type, display_color.clone());
    });
    log!(INFO, "[set_collateral_display_color] collateral={}, display_color={:?}", collateral_type, display_color);
    Ok(())
}

#[candid_method(query)]
#[query]
fn get_collateral_config(collateral_type: Principal) -> Option<rumi_protocol_backend::state::CollateralConfig> {
    read_state(|s| {
        s.get_collateral_config(&collateral_type).cloned().map(|mut config| {
            // Always compute recovery_target_cr from the formula rather than returning
            // the cached value, which may be stale if the multiplier changed after config creation.
            config.recovery_target_cr = config.borrow_threshold_ratio * s.recovery_cr_multiplier;
            config
        })
    })
}

#[candid_method(query)]
#[query]
fn get_supported_collateral_types() -> Vec<(Principal, rumi_protocol_backend::state::CollateralStatus)> {
    read_state(|s| s.supported_collateral_types())
}

/// Returns per-collateral aggregate totals (collateral amount, debt, vault count).
/// O(collateral_types × vaults_per_type) but computed on-canister — returns a tiny response
/// instead of transferring all vault data to the caller.
#[candid_method(query)]
#[query]
fn get_collateral_totals() -> Vec<CollateralTotals> {
    read_state(|s| {
        s.collateral_configs
            .iter()
            .map(|(ct, config)| {
                let vault_count = s
                    .collateral_to_vault_ids
                    .get(ct)
                    .map(|ids| ids.len() as u64)
                    .unwrap_or(0);
                CollateralTotals {
                    collateral_type: *ct,
                    symbol: config
                        .display_color
                        .as_ref()
                        .map(|_| String::new()) // placeholder — symbol fetched from ledger by frontend
                        .unwrap_or_default(),
                    decimals: config.decimals,
                    total_collateral: s.total_collateral_for(ct),
                    total_debt: s.total_debt_for_collateral(ct).to_u64(),
                    vault_count,
                    price: config.last_price.unwrap_or(0.0),
                }
            })
            .collect()
    })
}

/// Update any per-collateral parameter (developer only).
/// Replaces the entire CollateralConfig for the given collateral type.
/// Use `get_collateral_config` to fetch the current config, modify fields, then pass back.
#[candid_method(update)]
#[update]
async fn update_collateral_config(
    collateral_type: Principal,
    config: rumi_protocol_backend::state::CollateralConfig,
) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError("Only developer can update collateral config".to_string()));
    }

    let exists = read_state(|s| s.collateral_configs.contains_key(&collateral_type));
    if !exists {
        return Err(ProtocolError::GenericError("Collateral type not found".to_string()));
    }

    // Ensure the ledger_canister_id in the config matches the collateral_type key
    if config.ledger_canister_id != collateral_type {
        return Err(ProtocolError::GenericError(
            "ledger_canister_id in config must match collateral_type".to_string(),
        ));
    }

    mutate_state(|s| {
        event::record_update_collateral_config(s, collateral_type, config);
    });

    log!(INFO, "[update_collateral_config] Updated config for collateral {}", collateral_type);
    Ok(())
}

/// Admin correction of vault collateral amount (developer only).
/// Used to fix vault state that was inflated/deflated by bugs.
/// Records an on-chain event for full auditability.
#[candid_method(update)]
#[update]
async fn admin_correct_vault_collateral(
    vault_id: u64,
    new_collateral_amount: u64,
    reason: String,
) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError("Only developer can correct vault collateral".to_string()));
    }

    let old_amount = read_state(|s| {
        s.vault_id_to_vaults
            .get(&vault_id)
            .map(|v| v.collateral_amount)
            .ok_or(ProtocolError::GenericError(format!("Vault #{} not found", vault_id)))
    })?;

    // Safety: only allow downward corrections. Reducing collateral is conservative
    // (protects protocol solvency). Increasing collateral could let someone borrow
    // against phantom value — if collateral was under-reported, the safe fix is for
    // the user to deposit more.
    if new_collateral_amount > old_amount {
        return Err(ProtocolError::GenericError(
            format!(
                "Admin corrections can only reduce collateral (current: {}, requested: {}). \
                 To increase collateral, the vault owner should deposit more.",
                old_amount, new_collateral_amount
            )
        ));
    }

    mutate_state(|s| {
        event::record_admin_vault_correction(s, vault_id, old_amount, new_collateral_amount, reason.clone());
    });

    log!(INFO, "[admin_correct_vault_collateral] Vault #{}: {} -> {} raw units. Reason: {}",
        vault_id, old_amount, new_collateral_amount, reason);
    Ok(())
}

/// Sweep untracked ICP surplus from the backend to treasury.
///
/// Auto-calculates the surplus: actual ICP balance minus the sum of all
/// ICP vault collateral, pending margin/excess/redemption transfers, and
/// pending treasury collateral. Only the surplus can be swept — it is
/// physically impossible to touch tracked collateral with this function.
#[update]
async fn admin_sweep_to_treasury(reason: String) -> Result<u64, ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only developer can sweep to treasury".to_string(),
        ));
    }

    let (treasury, icp_ledger, icp_fee) = read_state(|s| {
        (s.treasury_principal, s.icp_ledger_principal, s.icp_ledger_fee)
    });
    let treasury = treasury.ok_or(ProtocolError::GenericError(
        "Treasury principal not configured".to_string(),
    ))?;

    // 1. Query actual ICP balance of this canister
    let actual_balance = management::get_token_balance(icp_ledger)
        .await
        .map_err(|e| ProtocolError::GenericError(format!("Failed to query ICP balance: {}", e)))?;

    // 2. Sum all tracked ICP obligations
    let tracked = read_state(|s| {
        let mut total: u64 = 0;

        // All ICP vault collateral
        for vault in s.vault_id_to_vaults.values() {
            if vault.collateral_type == s.icp_ledger_principal {
                total = total.saturating_add(vault.collateral_amount);
            }
        }

        // Pending margin transfers (ICP only)
        for pmt in s.pending_margin_transfers.values() {
            if pmt.collateral_type == s.icp_ledger_principal
                || pmt.collateral_type == Principal::anonymous()
            {
                total = total.saturating_add(pmt.margin.0);
            }
        }

        // Pending excess transfers (ICP only)
        for pmt in s.pending_excess_transfers.values() {
            if pmt.collateral_type == s.icp_ledger_principal
                || pmt.collateral_type == Principal::anonymous()
            {
                total = total.saturating_add(pmt.margin.0);
            }
        }

        // Pending redemption transfers (ICP only)
        for pmt in s.pending_redemption_transfer.values() {
            if pmt.collateral_type == s.icp_ledger_principal
                || pmt.collateral_type == Principal::anonymous()
            {
                total = total.saturating_add(pmt.margin.0);
            }
        }

        // Pending treasury collateral (ICP only)
        for (amount, ledger) in &s.pending_treasury_collateral {
            if *ledger == s.icp_ledger_principal {
                total = total.saturating_add(*amount);
            }
        }

        total
    });

    // 3. Compute surplus (leave 1 transfer fee as buffer)
    let fee_buffer = icp_fee.0;
    let surplus = actual_balance
        .saturating_sub(tracked)
        .saturating_sub(fee_buffer);

    if surplus == 0 {
        return Err(ProtocolError::GenericError(format!(
            "No surplus to sweep (actual: {}, tracked: {}, fee buffer: {})",
            actual_balance, tracked, fee_buffer
        )));
    }

    // 4. Transfer surplus to treasury
    let block_index = management::transfer_collateral(surplus, treasury, icp_ledger)
        .await
        .map_err(|e| ProtocolError::GenericError(format!("Transfer failed: {:?}", e)))?;

    log!(
        INFO,
        "[admin_sweep_to_treasury] Swept {} e8s ICP to treasury (block {}). Reason: {}",
        surplus,
        block_index,
        reason
    );

    // 5. Record audit event
    event::record_admin_sweep_to_treasury(surplus, treasury, block_index, reason.clone());

    // 6. Notify treasury for bookkeeping (non-critical)
    let _ = treasury::notify_treasury_deposit(
        treasury,
        treasury::DepositType::LiquidationFee, // closest category for recovered funds
        treasury::AssetType::ICP,
        surplus,
        block_index,
    )
    .await;

    Ok(block_index)
}

// ── Admin Debt Correction ─────────────────────────────────────────────────

#[derive(CandidType, Deserialize)]
struct VaultDebtCorrection {
    vault_id: u64,
    correct_borrowed_e8s: u64,
    correct_accrued_interest_e8s: u64,
}

/// Admin-only: correct vault debt amounts that were inflated by replay interest drift.
/// Records an auditable event for each correction.
#[update]
#[candid_method(update)]
fn admin_correct_vault_debts(corrections: Vec<VaultDebtCorrection>) -> Result<String, ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only developer can correct vault debts".to_string(),
        ));
    }

    let now = ic_cdk::api::time();
    let mut results = Vec::new();

    mutate_state(|s| {
        for c in &corrections {
            if let Some(vault) = s.vault_id_to_vaults.get_mut(&c.vault_id) {
                let old_borrowed = vault.borrowed_icusd_amount.0;
                let old_accrued = vault.accrued_interest.0;
                vault.borrowed_icusd_amount = ICUSD::new(c.correct_borrowed_e8s);
                vault.accrued_interest = ICUSD::new(c.correct_accrued_interest_e8s);

                rumi_protocol_backend::storage::record_event(&Event::AdminDebtCorrection {
                    vault_id: c.vault_id,
                    old_borrowed,
                    new_borrowed: c.correct_borrowed_e8s,
                    old_accrued,
                    new_accrued: c.correct_accrued_interest_e8s,
                    timestamp: Some(now),
                });

                results.push(format!(
                    "vault#{}: borrowed {}→{}, accrued {}→{}",
                    c.vault_id, old_borrowed, c.correct_borrowed_e8s,
                    old_accrued, c.correct_accrued_interest_e8s
                ));

                // Wave-8b LIQ-002: admin correction changes debt → re-key.
                s.reindex_vault_cr(c.vault_id);
            } else {
                results.push(format!("vault#{}: NOT FOUND", c.vault_id));
            }
        }
    });

    log!(INFO, "[admin_correct_vault_debts] Applied {} corrections", results.len());
    Ok(results.join("\n"))
}

// ICRC-21 Consent Message (delegates to icrc21 module)
#[update]
fn icrc21_canister_call_consent_message(
    request: rumi_protocol_backend::icrc21::ConsentMessageRequest,
) -> rumi_protocol_backend::icrc21::Icrc21ConsentMessageResult {
    rumi_protocol_backend::icrc21::icrc21_canister_call_consent_message(request)
}

// ICRC-28 Trusted Origins
#[query]
fn icrc28_trusted_origins() -> rumi_protocol_backend::icrc21::Icrc28TrustedOriginsResponse {
    rumi_protocol_backend::icrc21::icrc28_trusted_origins()
}

// ICRC-10 Supported Standards
#[query]
fn icrc10_supported_standards() -> Vec<rumi_protocol_backend::icrc21::StandardRecord> {
    rumi_protocol_backend::icrc21::icrc10_supported_standards()
}

// Checks the real candid interface against the one declared in the did file
#[test]
fn check_candid_interface_compatibility() {
    use candid_parser::utils::{service_equal, CandidSource};

    fn source_to_str(source: &CandidSource) -> String {
        match source {
            CandidSource::File(f) => {
                std::fs::read_to_string(f).unwrap_or_else(|_| "".to_string())
            }
            CandidSource::Text(t) => t.to_string(),
        }
    }
    
    fn check_service_compatible(
        new_name: &str,
        new: CandidSource,
        old_name: &str,
        old: CandidSource,
    ) {
        let new_str = source_to_str(&new);
        let old_str = source_to_str(&old);
        match service_equal(new, old) {
            Ok(_) => {}
            Err(e) => {
                eprintln!(
                    "{} is not compatible with {}!\n\n\
            {}:\n\
            {}\n\n\
            {}:\n\
            {}\n",
                    new_name, old_name, new_name, new_str, old_name, old_str
                );
                panic!("{:?}", e);
            }
        }
    }

    candid::export_service!();

    let new_interface = __export_service();

    // Allow regenerating the .did from the live source: `RUMI_REGEN_DID=1
    // cargo test ... check_candid_interface_compatibility`. Skips the equality
    // assertion and writes the canonical interface back to the file instead.
    let manifest_dir = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let did_path = manifest_dir.join("rumi_protocol_backend.did");
    if std::env::var("RUMI_REGEN_DID").is_ok() {
        std::fs::write(&did_path, &new_interface).expect("failed to write .did");
        eprintln!("Regenerated {}", did_path.display());
        return;
    }

    check_service_compatible(
        "actual Rumi Protocol candid interface",
        CandidSource::Text(&new_interface),
        "declared candid interface in rumi_protocol_backend.did file",
        CandidSource::File(did_path.as_path()),
    );
}