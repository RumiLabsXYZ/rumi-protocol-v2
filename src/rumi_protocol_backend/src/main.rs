use candid::{candid_method, Principal};
use ic_canister_log::log;
use ic_canisters_http_types::{HttpRequest, HttpResponse, HttpResponseBuilder};
use ic_cdk_macros::{init, post_upgrade, query, update};
use rumi_protocol_backend::{
    event::Event,
    logs::INFO,
    numeric::{ICUSD, ICP, Ratio, UsdIcp},
    state::{read_state, replace_state, Mode, State},
    vault::{CandidVault, OpenVaultSuccess, VaultArg},
    Fees, GetEventsArg, ProtocolArg, ProtocolError, ProtocolStatus, SuccessWithFee,
    VaultArgWithToken, StableTokenType,
};
use rumi_protocol_backend::logs::DEBUG;
use rumi_protocol_backend::state::mutate_state;
use rumi_protocol_backend::management;
use rumi_protocol_backend::event;
use rust_decimal::prelude::FromPrimitive;
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use rumi_protocol_backend::storage::events;
use rumi_protocol_backend::LiquidityStatus;
use candid_parser::utils::CandidSource;
use candid_parser::utils::service_equal;
use candid::{CandidType, Deserialize};

/// Result from stability pool liquidation
#[derive(CandidType, Deserialize, Debug)]
pub struct StabilityPoolLiquidationResult {
    pub success: bool,
    pub vault_id: u64,
    pub liquidated_debt: u64,
    pub collateral_received: u64,
    pub collateral_type: String,
    pub block_index: u64,
    pub fee: u64,
}

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

fn setup_timers() {
    // Existing ICP rate fetching timer
    ic_cdk_timers::set_timer_interval(rumi_protocol_backend::xrc::FETCHING_ICP_RATE_INTERVAL, || {
        ic_cdk::spawn(rumi_protocol_backend::xrc::fetch_icp_rate())
    });
    
    // Note: ckBTC rate fetching removed (ICP-only collateral)
}

fn main() {}

#[candid_method(init)]
#[init]
fn init(arg: ProtocolArg) {
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

#[post_upgrade]
fn post_upgrade(arg: ProtocolArg) {
    use rumi_protocol_backend::event::replay;
    use rumi_protocol_backend::storage::{count_events, events, record_event};

    let start = ic_cdk::api::instruction_counter();

    log!(INFO, "[upgrade]: replaying {} events", count_events());

    match arg {
        ProtocolArg::Init(_) => ic_cdk::trap("expected Upgrade got Init"),
        ProtocolArg::Upgrade(upgrade_args) => {
            log!(
                INFO,
                "[upgrade]: updating configuration with {:?}",
                upgrade_args
            );
            record_event(&Event::Upgrade(upgrade_args));
        }
    }

    let state = replay(events()).unwrap_or_else(|e| {
        ic_cdk::trap(&format!(
            "[upgrade]: failed to replay the event log: {:?}",
            e
        ))
    });

    // Post-upgrade validation: ensure collateral_configs is consistent
    validate_collateral_state(&state);

    replace_state(state);

    let end = ic_cdk::api::instruction_counter();

    log!(
        INFO,
        "[upgrade]: replaying events consumed {} instructions",
        end - start
    );

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
        recovery_target_cr: s.recovery_target_cr.to_f64(),
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
fn get_vault_history(vault_id: u64) -> Vec<Event> {
    if ic_cdk::api::data_certificate().is_none() {
        ic_cdk::trap("update call rejected");
    }

    let mut vault_events: Vec<Event> = vec![];
    for event in events() {
        if event.is_vault_related(&vault_id) {
            vault_events.push(event);
        }
    }
    vault_events
}

#[candid_method(query)]
#[query]
fn get_events(args: GetEventsArg) -> Vec<Event> {
    if ic_cdk::api::data_certificate().is_none() {
        ic_cdk::trap("update call rejected");
    }
    const MAX_EVENTS_PER_QUERY: usize = 2000;

    events()
        .skip(args.start as usize)
        .take(MAX_EVENTS_PER_QUERY.min(args.length as usize))
        .collect()
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
    check_postcondition(rumi_protocol_backend::vault::liquidate_vault_partial(arg.vault_id, arg.amount).await)
}

/// Liquidate a vault using ckUSDT or ckUSDC (1:1 with icUSD)
#[update]
#[candid_method(update)]
async fn liquidate_vault_partial_with_stable(arg: VaultArgWithToken) -> Result<SuccessWithFee, ProtocolError> {
    validate_call().await?;
    check_postcondition(rumi_protocol_backend::vault::liquidate_vault_partial_with_stable(arg.vault_id, arg.amount, arg.token_type).await)
}

// Stability Pool Integration - allows stability pool to execute liquidations
#[update]
#[candid_method(update)]
async fn stability_pool_liquidate(vault_id: u64, max_debt_to_liquidate: u64) -> Result<StabilityPoolLiquidationResult, ProtocolError> {
    validate_call().await?;
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
    let (vault, _collateral_price_usd, liquidatable_debt, collateral_available) = read_state(|s| {
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

                // Calculate how much can be liquidated
                let max_liquidatable = vault.borrowed_icusd_amount * s.max_partial_liquidation_ratio;
                let actual_liquidatable_debt = max_liquidatable.min(vault.borrowed_icusd_amount).min(max_debt_to_liquidate.into());

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
    })
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

// Add a new heartbeat function to routinely clean up stale operations
#[ic_cdk::heartbeat]
fn heartbeat() {
    use rumi_protocol_backend::state::mutate_state;
    log!(INFO, "[heartbeat] Running scheduled cleanup tasks");
    
    // Clean up any stale operations
    mutate_state(|s| s.clean_stale_operations());
}

#[candid_method(update)]
#[update]
async fn recover_pending_transfer(vault_id: u64) -> Result<bool, ProtocolError> {
    let caller = ic_cdk::caller();
    
    // Validate the caller is the owner of this pending transfer (check both maps)
    let is_owner = read_state(|s| {
        s.pending_margin_transfers
            .get(&vault_id)
            .map(|transfer| transfer.owner == caller)
            .unwrap_or(false)
        || s.pending_excess_transfers
            .get(&vault_id)
            .map(|transfer| transfer.owner == caller)
            .unwrap_or(false)
    });

    if !is_owner {
        return Err(ProtocolError::CallerNotOwner);
    }

    // Process the pending transfer immediately (check margin map first, then excess)
    let transfer_info = read_state(|s| {
        if let Some(t) = s.pending_margin_transfers.get(&vault_id).cloned() {
            Some(("margin", t))
        } else {
            s.pending_excess_transfers.get(&vault_id).cloned().map(|t| ("excess", t))
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
                    "margin" => { s.pending_margin_transfers.remove(&vault_id); },
                    _ => { s.pending_excess_transfers.remove(&vault_id); },
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
                        "margin" => { event::record_margin_transfer(s, vault_id, block_index); },
                        _ => { s.pending_excess_transfers.remove(&vault_id); },
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
        // No pending transfer found for this vault
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
    if (!is_developer) {
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

/// Set the max partial liquidation ratio (developer only)
/// Rate is a decimal: 0.5 = 50%, range 0.1–1.0
#[candid_method(update)]
#[update]
async fn set_max_partial_liquidation_ratio(new_rate: f64) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError("Only developer can set max partial liquidation ratio".to_string()));
    }
    if new_rate < 0.1 || new_rate > 1.0 {
        return Err(ProtocolError::GenericError("Max partial liquidation ratio must be between 0.1 and 1.0".to_string()));
    }
    let rate = Ratio::from(rust_decimal::Decimal::try_from(new_rate)
        .map_err(|_| ProtocolError::GenericError("Invalid rate".to_string()))?);
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_max_partial_liquidation_ratio(s, rate);
    });
    log!(INFO, "[set_max_partial_liquidation_ratio] Max partial liquidation ratio set to: {}", new_rate);
    Ok(())
}

/// Get the current max partial liquidation ratio
#[candid_method(query)]
#[query]
fn get_max_partial_liquidation_ratio() -> f64 {
    read_state(|s| s.max_partial_liquidation_ratio.to_f64())
}

/// Set the recovery target collateral ratio (developer only)
#[candid_method(update)]
#[update]
async fn set_recovery_target_cr(new_rate: f64) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError("Only developer can set recovery target CR".to_string()));
    }
    if new_rate < 1.4 || new_rate > 2.0 {
        return Err(ProtocolError::GenericError("Recovery target CR must be between 1.4 and 2.0".to_string()));
    }
    let rate = Ratio::from(rust_decimal::Decimal::try_from(new_rate)
        .map_err(|_| ProtocolError::GenericError("Invalid rate".to_string()))?);
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_recovery_target_cr(s, rate);
    });
    log!(INFO, "[set_recovery_target_cr] Recovery target CR set to: {}", new_rate);
    Ok(())
}

/// Get the current recovery target collateral ratio
#[candid_method(query)]
#[query]
fn get_recovery_target_cr() -> f64 {
    read_state(|s| s.recovery_target_cr.to_f64())
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

    use rumi_protocol_backend::state::{CollateralConfig, CollateralStatus};

    let config = CollateralConfig {
        ledger_canister_id: arg.ledger_canister_id,
        decimals,
        liquidation_ratio: Ratio::from_f64(arg.liquidation_ratio),
        borrow_threshold_ratio: Ratio::from_f64(arg.borrow_threshold_ratio),
        liquidation_bonus: Ratio::from_f64(arg.liquidation_bonus),
        borrowing_fee: Ratio::from_f64(arg.borrowing_fee),
        interest_rate_apr: Ratio::from_f64(0.0),
        debt_ceiling: arg.debt_ceiling,
        min_vault_debt: rumi_protocol_backend::numeric::ICUSD::from(arg.min_vault_debt),
        ledger_fee: arg.ledger_fee,
        price_source: arg.price_source,
        status: CollateralStatus::Active,
        last_price: None,
        last_price_timestamp: None,
        redemption_fee_floor: Ratio::from_f64(0.005),
        redemption_fee_ceiling: Ratio::from_f64(0.05),
        current_base_rate: Ratio::from_f64(0.0),
        last_redemption_time: 0,
        recovery_target_cr: Ratio::from_f64(arg.recovery_target_cr),
    };

    mutate_state(|s| {
        event::record_add_collateral_type(s, arg.ledger_canister_id, config);
    });

    // Register a price-fetching timer for the new collateral type.
    // For now, this is a placeholder — the XRC price source will be polled
    // at the same interval as ICP. When we add more collateral types with
    // actual oracles, this timer will call a per-collateral price fetch.
    let ledger_id = arg.ledger_canister_id;
    let is_icp = read_state(|s| s.icp_collateral_type() == ledger_id);
    if !is_icp {
        log!(INFO, "[add_collateral_token] Registering price timer for new collateral {}", ledger_id);
        // Future: ic_cdk_timers::set_timer_interval(...) calling
        //   xrc::fetch_collateral_price(ledger_id) for the configured PriceSource.
        // For the initial refactor, the per-collateral price is set via
        //   update_collateral_config or on-demand via ensure_fresh_price_for.
    }

    log!(INFO, "[add_collateral_token] Added collateral type: {} (decimals={})", arg.ledger_canister_id, decimals);
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

#[candid_method(query)]
#[query]
fn get_collateral_config(collateral_type: Principal) -> Option<rumi_protocol_backend::state::CollateralConfig> {
    read_state(|s| s.get_collateral_config(&collateral_type).cloned())
}

#[candid_method(query)]
#[query]
fn get_supported_collateral_types() -> Vec<(Principal, rumi_protocol_backend::state::CollateralStatus)> {
    read_state(|s| s.supported_collateral_types())
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

    // check the public interface against the actual one
    let old_interface =
        std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("rumi_protocol_backend.did");

    check_service_compatible(
        "actual Rumi Protocol candid interface",
        CandidSource::Text(&new_interface),
        "declared candid interface in rumi_protocol_backend.did file",
        CandidSource::File(old_interface.as_path()),
    );
}