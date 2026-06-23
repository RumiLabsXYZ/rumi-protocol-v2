use candid::{candid_method, Principal};
use ic_canister_log::log;
use ic_canisters_http_types::{HttpRequest, HttpResponse, HttpResponseBuilder};
use ic_cdk_macros::{init, pre_upgrade, post_upgrade, query, update};
use rumi_protocol_backend::{
    event::Event,
    logs::INFO,
    numeric::{ICUSD, ICP, Ratio, UsdIcp},
    state::{read_state, replace_state, Mode, State, RateCurveV2},
    vault::{CandidVault, OpenVaultSuccess, VaultArg},
    EventTypeFilter, Fees, GetEventsArg, ProtocolArg, ProtocolError, ProtocolStatus, SuccessWithFee,
    ReserveRedemptionResult, ReserveBalance, CollateralTotals, CollateralInterestInfo, PerCollateralRateCurve,
    VaultArgWithToken, StableTokenType, InterestSplitArg,
    GetSnapshotsArg, ProtocolSnapshot, CollateralSnapshot,
    GetEventsFilteredResponse, ForwardFilteredEventsResponse, StabilityPoolLiquidationResult,
    VaultHistoryPagedResponse, EventsByPrincipalPagedResponse, VaultsPageResponse,
    SupplyAudit, SupplyAuditEntry,
    MAX_VAULT_HISTORY, MAX_EVENTS_BY_PRINCIPAL_LEGACY, MAX_EVENTS_BY_PRINCIPAL_SCAN,
    MAX_EVENTS_BY_PRINCIPAL_OUTPUT, MAX_VAULTS_LEGACY_PAGE, MAX_VAULTS_PAGE_LIMIT,
    PROTOCOL_STATUS_SNAPSHOT_TTL_NANOS, TREASURY_STATS_SNAPSHOT_TTL_NANOS,
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
    use rumi_protocol_backend::event::replay;

    read_state(|s| {
        s.check_invariants()?;

        let events: Vec<_> = rumi_protocol_backend::storage::events().collect();
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
/// If the cached ICP price is older than the freshness threshold, triggers
/// an on-demand XRC fetch before proceeding. This allows the background
/// timer to poll lazily (every 300s) while guaranteeing fresh prices for
/// actual operations.
///
/// Wave-14c CDP-16: this function is `async` because the on-demand XRC
/// fetch (`ensure_fresh_price().await`) is a real `await` suspension point
/// whenever the cached price misses the freshness window. Callers must NOT
/// hold a `read_state` snapshot or any in-flight reference across
/// `validate_call().await`: state can mutate during the suspension (e.g.
/// the price gets refreshed, mode flips, an admin setter runs). Always
/// re-read the fields you need AFTER `validate_call().await` returns.
///
/// The cache-hit path is also async (no work done other than the conditional)
/// but yields once to the executor; in either case, treat the call as a
/// suspension boundary.
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
        // Shared constructor keeps this entry-layer gate byte-identical to the
        // vault-module gates in vault::redeem_collateral / redeem_reserves
        // (audit RED-101).
        Mode::ReadOnly => Err(ProtocolError::read_only_mode()),
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

/// Audit ORACLE-001: refresh a collateral's cached price before a debt-increasing
/// or collateral-decreasing op whose collateral is given directly (the open-*
/// endpoints). `None` means ICP (the default collateral). `ensure_fresh_price_for`
/// delegates to `ensure_fresh_price` for ICP, so this is safe to call
/// unconditionally and mirrors `validate_freshness_for_vault` for the
/// by-vault-id endpoints. Without it, a non-ICP collateral whose background
/// fetch has been failing would let the borrower mint against a stale price.
async fn validate_freshness_for_collateral(
    collateral_type: Option<Principal>,
) -> Result<(), ProtocolError> {
    let ct = collateral_type.unwrap_or_else(|| read_state(|s| s.icp_collateral_type()));
    rumi_protocol_backend::xrc::ensure_fresh_price_for(&ct).await
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
        // M2 EVM-native self-serve: authority is the EIP-712 signature, so the IC
        // caller is irrelevant and ANONYMOUS ingress MUST be accepted (a relayer or
        // a wallet's anonymous agent forwards the signed intent). The in-method
        // signature verification + per-owner nonce/cap are the real boundary; this
        // accept just lets the message reach the method body. inspect_message is a
        // single-replica pre-filter, never a security boundary.
        "open_chain_vault_evm" | "borrow_chain_vault_evm" | "withdraw_chain_collateral_evm"
        | "close_chain_vault_evm" => {
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

// Wave-14b CDP-12 follow-up: track the active TimerId for each of the
// three CDP-12 timers so the developer-gated setters can clear-and-re-
// register in place. Transient (not persisted): every upgrade re-runs
// `setup_timers` which assigns fresh ids.
thread_local! {
    static XRC_FETCH_TIMER_ID: std::cell::Cell<Option<ic_cdk_timers::TimerId>> =
        const { std::cell::Cell::new(None) };
    static INTEREST_TREASURY_TIMER_ID: std::cell::Cell<Option<ic_cdk_timers::TimerId>> =
        const { std::cell::Cell::new(None) };
    static VAULT_CHECK_TIMER_ID: std::cell::Cell<Option<ic_cdk_timers::TimerId>> =
        const { std::cell::Cell::new(None) };
    // Phase 1b Task 15: Monad async loops. Same transient (not-persisted)
    // lifecycle — re-assigned fresh ids by `setup_timers` on every upgrade.
    static SETTLEMENT_TIMER_ID: std::cell::Cell<Option<ic_cdk_timers::TimerId>> =
        const { std::cell::Cell::new(None) };
    static OBSERVER_TIMER_ID: std::cell::Cell<Option<ic_cdk_timers::TimerId>> =
        const { std::cell::Cell::new(None) };
    // Task 12: foreign-chain interest harvest. Same transient lifecycle.
    static CHAIN_INTEREST_TIMER_ID: std::cell::Cell<Option<ic_cdk_timers::TimerId>> =
        const { std::cell::Cell::new(None) };
}

fn register_xrc_fetch_timer() {
    let secs = read_state(|s| s.xrc_fetch_interval_secs);
    XRC_FETCH_TIMER_ID.with(|cell| {
        if let Some(old) = cell.get() {
            ic_cdk_timers::clear_timer(old);
        }
        let new_id = ic_cdk_timers::set_timer_interval(
            std::time::Duration::from_secs(secs),
            || ic_cdk::spawn(rumi_protocol_backend::xrc::fetch_icp_rate()),
        );
        cell.set(Some(new_id));
    });
}

fn register_interest_treasury_timer() {
    let secs = read_state(|s| s.interest_treasury_tick_interval_secs);
    INTEREST_TREASURY_TIMER_ID.with(|cell| {
        if let Some(old) = cell.get() {
            ic_cdk_timers::clear_timer(old);
        }
        let new_id = ic_cdk_timers::set_timer_interval(
            std::time::Duration::from_secs(secs),
            || ic_cdk::spawn(rumi_protocol_backend::xrc::interest_and_treasury_tick()),
        );
        cell.set(Some(new_id));
    });
}

fn register_vault_check_timer() {
    let secs = read_state(|s| s.vault_check_tick_interval_secs);
    VAULT_CHECK_TIMER_ID.with(|cell| {
        if let Some(old) = cell.get() {
            ic_cdk_timers::clear_timer(old);
        }
        let new_id = ic_cdk_timers::set_timer_interval(
            std::time::Duration::from_secs(secs),
            || ic_cdk::spawn(rumi_protocol_backend::xrc::vault_check_tick()),
        );
        cell.set(Some(new_id));
    });
}

/// M2 Task 8: chain-kind timer dispatch. ONE observer fan-out and ONE settlement
/// fan-out run all registered chains, dispatching each chain to its kind's
/// `run_observer` / `run_settlement` by `ChainId` (501 == Solana, everything else
/// Monad). This keeps a SINGLE timer pair total (no per-kind timer proliferation)
/// while letting the two chains run different worker code.
///
/// - Monad chains ALWAYS run (behavior identical to the prior
///   `monad::observer_tick` / `settlement_tick` fan-out).
/// - Solana chains run ONLY when `solana_workers_enabled` is true, so Solana
///   stays DARK by default (no signing-subnet / SOL-RPC cycle burn) until the
///   operator flips the flag via `set_solana_workers_enabled`.
///
/// Borrow discipline: the chain-id list and the `solana_workers_enabled` bool are
/// snapshotted OUT of state under a synchronous `read_state` BEFORE the await
/// loop; no state borrow is held across a `run_observer`/`run_settlement` await
/// (each carries its own re-entrancy + mode/halt guards). No-op when no chain is
/// registered (the Vec is empty), so it is safe to run on a canister before any
/// chain is configured.

const SOLANA_CHAIN_ID: rumi_protocol_backend::chains::config::ChainId =
    rumi_protocol_backend::chains::solana::config::SOLANA_CHAIN_ID;

/// Snapshot the registered chain-id list plus the Solana enable flag (one
/// synchronous read; nothing held across an await).
fn registered_chains_and_solana_flag() -> (Vec<rumi_protocol_backend::chains::config::ChainId>, bool)
{
    read_state(|s| {
        let chains = s
            .multi_chain
            .chain_configs
            .iter()
            .filter(|(_, c)| {
                matches!(c.status, rumi_protocol_backend::chains::config::ChainStatus::Registered)
            })
            .map(|(id, _)| *id)
            .collect();
        (chains, s.solana_workers_enabled)
    })
}

/// Observer fan-out: dispatch each registered chain to its kind's `run_observer`.
async fn run_all_observers() {
    let (chains, solana_enabled) = registered_chains_and_solana_flag();
    for chain in chains {
        if chain == SOLANA_CHAIN_ID {
            if solana_enabled {
                rumi_protocol_backend::chains::solana::deposit_watch::run_observer(chain).await;
            }
            // Solana not enabled: skip (stays dark, no cycle burn).
        } else {
            rumi_protocol_backend::chains::monad::deposit_watch::run_observer(chain).await;
        }
    }
}

/// Settlement fan-out: dispatch each registered chain to its kind's `run_settlement`.
async fn run_all_settlements() {
    let (chains, solana_enabled) = registered_chains_and_solana_flag();
    for chain in chains {
        if chain == SOLANA_CHAIN_ID {
            if solana_enabled {
                rumi_protocol_backend::chains::solana::settlement::run_settlement(chain).await;
            }
            // Solana not enabled: skip (stays dark, no cycle burn).
        } else {
            rumi_protocol_backend::chains::monad::settlement::run_settlement(chain).await;
        }
    }
}

/// Phase 1b Task 15: register Timer D (settlement fan-out). Clears any existing
/// id and re-registers in place so the setter can re-tune live.
///
/// FLOOR: a 0 interval (serde-default-missing on an old snapshot, or a bad
/// setter value that slipped past validation) is forced to 30s, never 0 — a 0s
/// `set_timer_interval` is a busy-loop and the #1 cycle burner (this is the
/// heartbeat-cost regression the protocol removed a heartbeat to avoid).
fn register_settlement_timer() {
    let secs = read_state(|s| s.settlement_tick_interval_secs);
    let secs = if secs == 0 { 30 } else { secs.max(1) };
    SETTLEMENT_TIMER_ID.with(|cell| {
        if let Some(old) = cell.get() {
            ic_cdk_timers::clear_timer(old);
        }
        let new_id = ic_cdk_timers::set_timer_interval(
            std::time::Duration::from_secs(secs),
            || ic_cdk::spawn(run_all_settlements()),
        );
        cell.set(Some(new_id));
    });
}

/// Task 12: interest harvest fan-out — for each registered EVM chain, resolve its
/// interest-treasury recipient + APR and enqueue `InterestMint` ops for every
/// eligible vault. The settlement worker (Timer D) then mints/confirms them.
/// Interest accrual is EVM-only in Phase 1b (Solana has no on-chain mint path
/// here), so Solana is skipped. No-op in ReadOnly mode. No state borrow is held
/// across the treasury-address derive `.await`.
async fn run_all_chain_interest_harvests() {
    if read_state(|s| s.mode == Mode::ReadOnly) {
        return;
    }
    let (chains, _solana_enabled) = registered_chains_and_solana_flag();
    let now = ic_cdk::api::time();
    for chain in chains {
        if chain == SOLANA_CHAIN_ID {
            continue; // interest accrual is EVM-only in Phase 1b
        }
        if let Err(e) = harvest_one_chain_interest(chain, now).await {
            log!(INFO, "[interest harvest chain={:?}] skipped: {}", chain, e);
        }
    }
}

/// Harvest interest for one EVM chain: resolve `apr_bps` + the interest-treasury
/// address (async, cached), then run the in-state harvest. Interest-mint ids are
/// drawn from the SAME `chain_vault_id_counter` as vault opens (guaranteeing
/// global disjointness from real vault ids and prior interest mints): read the
/// counter, hand the harvest a fresh-id closure over a LOCAL copy, write the
/// advanced value back — all inside one `mutate_state`, so the closure never
/// reborrows `s`.
async fn harvest_one_chain_interest(
    chain: rumi_protocol_backend::chains::config::ChainId,
    now: u64,
) -> Result<u64, String> {
    let apr_bps =
        match rumi_protocol_backend::chains::collateral_config::chain_collateral_config(chain) {
            Some(c) if c.interest_apr_bps > 0 => c.interest_apr_bps,
            Some(_) => return Ok(0), // a configured 0-rate chain accrues nothing
            None => return Err(format!("no collateral config for chain {}", chain.0)),
        };
    let treasury =
        rumi_protocol_backend::chains::evm::tecdsa::cached_interest_treasury_address(chain)
            .await
            .map(|(_path, addr)| addr)
            .map_err(|e| format!("interest-treasury address derive failed: {e}"))?;
    let threshold = read_state(|s| s.chain_interest_min_realize_e8s);
    let enqueued = mutate_state(|s| {
        let mut k = s.chain_vault_id_counter;
        let ops = rumi_protocol_backend::chains::interest::harvest_chain_interest_in_state(
            &mut s.multi_chain,
            chain,
            apr_bps,
            threshold,
            &treasury,
            now,
            || {
                k += 1;
                k
            },
        );
        s.chain_vault_id_counter = k;
        ops.len() as u64
    });
    if enqueued > 0 {
        log!(INFO, "[interest harvest chain={:?}] enqueued {} interest mint(s) to treasury {}", chain, enqueued, treasury);
    }
    Ok(enqueued)
}

/// Task 12: register the interest-harvest timer. Clears + re-registers in place
/// so the setter can re-tune live. FLOOR: a 0 interval is forced to the 1-year
/// default (never a busy-loop), mirroring the settlement/observer timers.
fn register_chain_interest_timer() {
    let secs = read_state(|s| s.chain_interest_tick_interval_secs);
    let secs = if secs == 0 { 31_536_000 } else { secs };
    CHAIN_INTEREST_TIMER_ID.with(|cell| {
        if let Some(old) = cell.get() {
            ic_cdk_timers::clear_timer(old);
        }
        let new_id = ic_cdk_timers::set_timer_interval(
            std::time::Duration::from_secs(secs),
            || ic_cdk::spawn(run_all_chain_interest_harvests()),
        );
        cell.set(Some(new_id));
    });
}

/// Phase 1b Task 15: register the inbound observer fan-out. Same
/// clear-and-re-register-in-place + 0-floor protection as the settlement timer.
fn register_observer_timer() {
    let secs = read_state(|s| s.observer_tick_interval_secs);
    let secs = if secs == 0 { 30 } else { secs.max(1) };
    OBSERVER_TIMER_ID.with(|cell| {
        if let Some(old) = cell.get() {
            ic_cdk_timers::clear_timer(old);
        }
        let new_id = ic_cdk_timers::set_timer_interval(
            std::time::Duration::from_secs(secs),
            || ic_cdk::spawn(run_all_observers()),
        );
        cell.set(Some(new_id));
    });
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

    // ── Wave-14b CDP-12: three independent timers ────────────────────────
    // Cadences live in State and are tunable via developer-gated setters
    // (`set_xrc_fetch_interval_secs`, `set_interest_treasury_tick_interval_secs`,
    // `set_vault_check_tick_interval_secs`). The setters call into the
    // `register_*` helpers below to clear and re-register the affected
    // timer in place, so an interval change takes effect immediately
    // without a canister upgrade.
    register_xrc_fetch_timer();
    register_interest_treasury_timer();
    register_vault_check_timer();

    // Price timers for all non-ICP collateral types (timers don't survive upgrades,
    // so we re-register them here for any collateral added via add_collateral_token).
    // Wave-9d DOS-011: register through `xrc::register_collateral_price_timer` so
    // each closure gates the XRC fetch on `CollateralStatus`. Wound-down collateral
    // (Frozen / Sunset / Deprecated) keeps the timer alive but skips the ~1B-cycle
    // XRC call until status flips back to Active or Paused.
    let non_icp_collaterals: Vec<candid::Principal> = read_state(|s| {
        let icp = s.icp_collateral_type();
        s.collateral_configs.keys()
            .filter(|ct| **ct != icp)
            .cloned()
            .collect()
    });
    for ledger_id in non_icp_collaterals {
        log!(INFO, "[setup_timers] Registering price timer for collateral {}", ledger_id);
        rumi_protocol_backend::xrc::register_collateral_price_timer(ledger_id);
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

    // ── Phase 1b Task 15: Monad async loops (Timer D + inbound observer) ─────
    // Both tick fns fan out over registered+enabled chains and are NO-OPS when
    // no chain is registered, so they are safe to run on the staging canister
    // before Monad is configured. Cadences live in State (default 30s) and are
    // tunable via `set_settlement_tick_interval_secs` /
    // `set_observer_tick_interval_secs`, which re-register in place. The register
    // helpers FLOOR a 0 interval to 30s (never a busy-loop). Per-chain
    // re-entrancy guards in run_settlement/run_observer prevent overlapping ticks
    // from double-processing an op.
    register_settlement_timer();
    register_observer_timer();
    register_chain_vault_gc_timer();
    // Task 12: foreign-chain interest harvest. Defaults to a ~1-year interval
    // (effectively OFF) so realization is deliberate; tunable via
    // `set_chain_interest_tick_interval_secs`. No-op when no EVM chain is
    // registered, so it is safe to register on staging before any chain exists.
    register_chain_interest_timer();
}

/// M2 anti-spam backstop: hourly GC of stale `AwaitingDeposit` chain vaults
/// (unfunded opens older than the TTL). Bounds total unfunded state from
/// anonymous `open_chain_vault_evm` spam without the self-DoS of a hard cap.
/// Pruning an `AwaitingDeposit` vault is supply-invariant-safe (no confirmed
/// debt / enqueued mint). Re-registered every upgrade via `setup_timers`.
fn register_chain_vault_gc_timer() {
    ic_cdk_timers::set_timer_interval(std::time::Duration::from_secs(3600), || {
        let now = ic_cdk::api::time();
        let pruned = mutate_state(|s| {
            rumi_protocol_backend::chains::vault::prune_stale_awaiting_deposit(
                &mut s.multi_chain,
                now,
                rumi_protocol_backend::chains::vault::AWAITING_DEPOSIT_TTL_NS,
            )
        });
        if pruned > 0 {
            log!(INFO, "[chain_vault_gc] pruned {} stale AwaitingDeposit vaults", pruned);
        }
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

    // Task 12: mirror the above for FOREIGN-CHAIN vaults — stamp
    // last_interest_accrual_ns for any that decoded with 0 (a vault from a
    // pre-interest-field snapshot), so the first interest harvest does not bill
    // from the unix epoch. New vaults are stamped at mint-confirm; idempotent.
    mutate_state(|s| {
        rumi_protocol_backend::chains::supply::stamp_chain_interest_accrual_start(
            &mut s.multi_chain,
            now,
        );
    });

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
            s.remove_vault_and_unindex(*vault_id);
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

    // Converge the remaining secondary vault indexes with the primary map.
    // collateral_to_vault_ids accumulated stale ids in the persisted snapshot
    // (mainnet 2026-06-11: 185 indexed ids vs 82 open vaults) because close
    // paths never unindexed; the runtime fix stops new drift and this sweep
    // heals what the snapshot still carries. Idempotent — a no-op once
    // consistent.
    let (stale_ids, missing_ids, empty_principals) = mutate_state(|s| {
        let (stale, missing) = s.rebuild_collateral_index();
        let empties = s.prune_empty_principal_entries();
        (stale, missing, empties)
    });
    log!(
        INFO,
        "[upgrade]: vault-index sweep: dropped {} stale collateral-index id(s), \
         re-added {} missing id(s), pruned {} empty principal entr(y/ies)",
        stale_ids,
        missing_ids,
        empty_principals,
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

    // Phase 1b breadcrumb: confirm the multi_chain root survived the upgrade.
    // The V1->V2 migration happens automatically via the `#[serde(default)]`
    // in-place decode of `State.multi_chain` (four V1 fields map across by
    // name; the five new V2 fields hit serde-default and come up empty). No
    // explicit migrate call is needed here; `migrate_multi_chain_state` exists
    // as the unit-tested template for the NEXT version bump.
    let (chains, chain_vaults) = read_state(|s| {
        (s.multi_chain.chain_configs.len(), s.multi_chain.chain_vaults.len())
    });
    log!(INFO, "[post_upgrade] multi_chain: {} chains, {} chain_vaults", chains, chain_vaults);

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
    let now = ic_cdk::api::time();

    // Wave-9b DOS-006: try the cache first. The cache is filled
    // exclusively by the XRC tick (`xrc::fetch_icp_rate`, ~5 min
    // interval). IC queries cannot reliably persist state mutations,
    // so cache misses recompute inline without writing back — the
    // next XRC tick will repopulate. The TTL is intentionally short
    // (5s); within 5s of the last XRC-tick cache fill, queries hit
    // the cache. Beyond 5s we fall through to live recompute, which
    // protects against serving aggregates that are arbitrarily old
    // if the XRC tick is delayed (network outage, frozen mode, etc.).
    let cached = read_state(|s| {
        s.protocol_status_snapshot.as_ref().and_then(|(ts, snap)| {
            if now.saturating_sub(*ts) < PROTOCOL_STATUS_SNAPSHOT_TTL_NANOS {
                Some((*ts, snap.clone()))
            } else {
                None
            }
        })
    });
    let (snapshot_ts_ns, snapshot) = match cached {
        Some(hit) => hit,
        None => {
            // Cache miss / expired: recompute inline. Do NOT write
            // back from a query — IC query state mutations are not
            // persisted across calls. The next XRC tick will refresh
            // the cache.
            let snap = read_state(|s| s.compute_protocol_status_snapshot());
            (now, snap)
        }
    };

    // Live fields: read fresh on every call. The cache MUST NOT mask
    // an admin kill switch (`frozen`, `manual_mode_override`,
    // `liquidation_breaker_tripped`), the current `mode`, the latest
    // XRC price (`last_icp_rate`/`last_icp_timestamp`), the running
    // breaker window total, or the deficit accounting fields. All
    // other fields below are config-shaped (admin-set, change rarely)
    // but cheap to read so we read them live too.
    read_state(|s| ProtocolStatus {
        last_icp_rate: s
            .last_icp_rate
            .unwrap_or(UsdIcp::from(Decimal::ZERO))
            .to_f64(),
        last_icp_timestamp: s.last_icp_timestamp.unwrap_or(0),
        total_icp_margin: snapshot.total_icp_margin,
        total_icusd_borrowed: snapshot.total_icusd_borrowed,
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
        weighted_average_interest_rate: snapshot.weighted_average_interest_rate,
        borrowing_fee_curve_resolved: snapshot.borrowing_fee_curve_resolved.clone(),
        per_collateral_interest: snapshot.per_collateral_interest.iter()
            .map(|p| CollateralInterestInfo {
                collateral_type: p.collateral_type,
                total_debt_e8s: p.total_debt_e8s,
                weighted_interest_rate: p.weighted_interest_rate,
            })
            .collect(),
        per_collateral_rate_curves: snapshot.per_collateral_rate_curves.iter()
            .map(|p| PerCollateralRateCurve {
                collateral_type: p.collateral_type,
                base_rate: p.base_rate,
                markers: p.markers.clone(),
            })
            .collect(),
        interest_split: s.interest_split.iter().map(|r| {
            let dest = match &r.destination {
                rumi_protocol_backend::state::InterestDestination::StabilityPool => "stability_pool".to_string(),
                rumi_protocol_backend::state::InterestDestination::Treasury => "treasury".to_string(),
                rumi_protocol_backend::state::InterestDestination::ThreePool => "three_pool".to_string(),
                rumi_protocol_backend::state::InterestDestination::Amm1 => "amm1".to_string(),
            };
            InterestSplitArg { destination: dest, bps: r.bps }
        }).collect(),
        // Wave-8e LIQ-005 — live (changes on liquidation/redemption + admin)
        protocol_deficit_icusd: s.protocol_deficit_icusd.to_u64(),
        total_deficit_repaid_icusd: s.total_deficit_repaid_icusd.to_u64(),
        deficit_repayment_fraction: s.deficit_repayment_fraction.to_f64(),
        deficit_readonly_threshold_e8s: s.deficit_readonly_threshold_e8s,
        // Wave-10 LIQ-008 — live (windowed total depends on `now`,
        // breaker_tripped is an admin/auto kill switch).
        breaker_window_ns: s.breaker_window_ns,
        breaker_window_debt_ceiling_e8s: s.breaker_window_debt_ceiling_e8s,
        windowed_liquidation_total_e8s: s.windowed_liquidation_total(now),
        liquidation_breaker_tripped: s.liquidation_breaker_tripped,
        // Wave-9b DOS-006
        snapshot_ts_ns,
    })
}

/// Phase 1a: canonical multi-chain icUSD supply (sum across all chains).
/// Equals `sum(state.multi_chain.chain_supplies.values())`. Returns 0 when
/// no chains are registered (the Phase 1a default state).
///
/// Note: this query is read-only and does NOT exercise the invariant
/// check. Operators investigating drift should call `get_supply_audit`
/// for the per-chain breakdown.
#[candid_method(query)]
#[query]
fn get_global_icusd_supply() -> u128 {
    read_state(|s| s.multi_chain.total_supply_all_chains_e8s())
}

/// Phase 1a: per-chain breakdown for external auditors. Iterates
/// `multi_chain.chain_configs` in chain-id order so the response shape is
/// deterministic.
#[candid_method(query)]
#[query]
fn get_supply_audit() -> SupplyAudit {
    read_state(|s| {
        let mut per_chain = Vec::with_capacity(s.multi_chain.chain_configs.len());
        for (chain_id, cfg) in s.multi_chain.chain_configs.iter() {
            let supply = s.multi_chain.chain_supplies.get(chain_id).copied().unwrap_or(0);
            per_chain.push(SupplyAuditEntry {
                chain_id: *chain_id,
                display_name: cfg.display_name.clone(),
                supply_e8s: supply,
            });
        }
        SupplyAudit {
            total_e8s: per_chain.iter().map(|e| e.supply_e8s).sum(),
            per_chain,
        }
    })
}

// Phase 1a: developer-gated chain-registry admin endpoints.

#[candid_method(update)]
#[update]
fn register_chain(arg: rumi_protocol_backend::chains::config::RegisterChainArg) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::ChainAdmin("not developer".into()));
    }
    let now = ic_cdk::api::time();
    let chain_id = arg.chain_id;
    let display_name = arg.display_name.clone();
    let result = mutate_state(|s| rumi_protocol_backend::chains::admin::register_chain_in_state(&mut s.multi_chain, arg, now));
    match result {
        Ok(_) => {
            rumi_protocol_backend::storage::record_event(&rumi_protocol_backend::event::Event::ChainRegistered {
                chain_id,
                display_name,
                timestamp: now,
            });
            log!(INFO, "[register_chain] chain_id={:?} registered", chain_id);
            Ok(())
        }
        Err(e) => Err(ProtocolError::ChainAdmin(format!("{:?}", e))),
    }
}

#[candid_method(update)]
#[update]
fn disable_chain(chain_id: rumi_protocol_backend::chains::config::ChainId) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::ChainAdmin("not developer".into()));
    }
    let result = mutate_state(|s| rumi_protocol_backend::chains::admin::disable_chain_in_state(&mut s.multi_chain, chain_id));
    match result {
        Ok(()) => {
            let now = ic_cdk::api::time();
            rumi_protocol_backend::storage::record_event(&rumi_protocol_backend::event::Event::ChainDisabled {
                chain_id,
                timestamp: now,
            });
            log!(INFO, "[disable_chain] chain_id={:?} disabled", chain_id);
            Ok(())
        }
        Err(e) => Err(ProtocolError::ChainAdmin(format!("{:?}", e))),
    }
}

#[candid_method(update)]
#[update]
fn set_chain_config(
    chain_id: rumi_protocol_backend::chains::config::ChainId,
    update: rumi_protocol_backend::chains::config::UpdateChainConfigArg,
) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::ChainAdmin("not developer".into()));
    }
    let result = mutate_state(|s| rumi_protocol_backend::chains::admin::update_chain_config_in_state(&mut s.multi_chain, chain_id, update));
    match result {
        Ok(()) => {
            let now = ic_cdk::api::time();
            rumi_protocol_backend::storage::record_event(&rumi_protocol_backend::event::Event::ChainConfigUpdated {
                chain_id,
                timestamp: now,
            });
            log!(INFO, "[set_chain_config] chain_id={:?} updated", chain_id);
            Ok(())
        }
        Err(e) => Err(ProtocolError::ChainAdmin(format!("{:?}", e))),
    }
}

/// Resolve the per-chain EVM vault params (native price symbol + min CR) for a
/// dev-gated chain-vault op. Errors if `chain` is not a known EVM chain or has
/// no collateral config. For Monad (10143) this returns ("MON", 13_000) and for
/// Conflux (71) ("CFX", 13_300), so the EVM endpoints are chain-agnostic.
fn evm_vault_params(
    chain: rumi_protocol_backend::chains::config::ChainId,
) -> Result<(&'static str, u64, u128, Option<u128>), ProtocolError> {
    let symbol = rumi_protocol_backend::chains::evm::evm_chain_config(chain)
        .map(|c| c.native_symbol)
        .ok_or_else(|| {
            ProtocolError::ChainAdmin(format!("chain {} is not a known EVM chain", chain.0))
        })?;
    let cfg = rumi_protocol_backend::chains::collateral_config::chain_collateral_config(chain)
        .ok_or_else(|| {
            ProtocolError::ChainAdmin(format!("no collateral config for chain {}", chain.0))
        })?;
    Ok((symbol, cfg.min_cr_e4, cfg.min_vault_debt_e8s, cfg.debt_ceiling_e8s))
}

/// Phase 1b Task 12: open a foreign-chain EVM (Monad or Conflux) vault, OPEN-THEN-VERIFY.
///
/// Creates the vault in `AwaitingDeposit` with the DECLARED collateral and the
/// intended mint in `pending_mint_e8s`; enqueues NO mint. The caller then reads
/// the vault's `custody_address` (Task 14 `get_chain_vault`) and deposits the
/// native token (MON on Monad, CFX on Conflux) there. deposit-watch verifies the
/// on-chain balance covers the declared collateral at finality, flips the vault
/// to `MintPending`, and enqueues the mint. icUSD is only ever minted against a
/// verified on-chain deposit. The native price symbol and min CR are resolved
/// per-chain from the compile-time configs (see `evm_vault_params`).
///
/// Developer-gated for Phase 1b (matches the chain-admin endpoints). Async
/// because deriving the per-user custody address calls tECDSA. Borrow
/// discipline: no `read_state`/`mutate_state` borrow is held across the
/// `derive_evm_address(...).await`.
#[candid_method(update)]
#[update]
async fn open_chain_vault(
    collateral_chain: rumi_protocol_backend::chains::config::ChainId,
    collateral_e18: u128,
    debt_e8s: u128,
    mint_recipient: String,
) -> Result<u64, ProtocolError> {
    let caller = ic_cdk::caller();
    if read_state(|s| s.developer_principal != caller) {
        return Err(ProtocolError::ChainAdmin("not developer".into()));
    }
    // Resolve the per-chain price symbol + min CR early so a non-EVM chain fails
    // fast (before reserving a vault id or paying for the tECDSA derive).
    let (symbol, min_cr, min_debt, ceiling) = evm_vault_params(collateral_chain)?;
    // Reserve the vault id BEFORE the async derive so the derivation path
    // (chain, caller, vault_id) is unique even across concurrent opens.
    let vault_id = mutate_state(|s| {
        s.chain_vault_id_counter += 1;
        s.chain_vault_id_counter
    });
    let path = rumi_protocol_backend::chains::evm::tecdsa::custody_derivation_path(
        collateral_chain,
        caller,
        vault_id,
    );
    let (_pubkey, custody) = rumi_protocol_backend::chains::evm::tecdsa::derive_evm_address(path)
        .await
        .map_err(|e| ProtocolError::ChainAdmin(format!("derive: {e}")))?;
    let now = ic_cdk::api::time();
    let res = mutate_state(|s| {
        rumi_protocol_backend::chains::vault::open_chain_vault_in_state(
            &mut s.multi_chain,
            collateral_chain,
            caller,
            custody,
            collateral_e18,
            debt_e8s,
            mint_recipient,
            rumi_protocol_backend::chains::evm::tecdsa::is_valid_evm_address,
            symbol,
            min_cr,
            min_debt,
            ceiling,
            now,
            vault_id,
        )
    });
    res.map(|()| vault_id)
        .map_err(|e| ProtocolError::ChainAdmin(format!("{e:?}")))
}

/// Phase 1b Task 13: withdraw foreign-chain EVM (Monad or Conflux) collateral.
///
/// Resolves the vault's chain from state, then CR-checks the REMAINING
/// collateral against that chain's `min_cr_e4` (debt-free vaults skip the
/// check), RESERVES the withdrawn amount (decrements `collateral_amount_native`
/// at enqueue), and enqueues a `NativeWithdrawal` op that Timer D signs and
/// broadcasts. A vault that becomes empty AND debt-free flips to `Closing` here
/// and `Closed` once the transfer confirms.
///
/// There is NO repay endpoint: the user burns icUSD on-chain and the burn-watch
/// observer decrements `debt_e8s` + chain supply. This path moves only
/// collateral. Synchronous — `dest_address` is supplied by the caller, so no
/// tECDSA derive is needed; signing happens later in Timer D. Developer-gated.
///
/// CONCURRENCY INVARIANT (audit FLAG-16): this MUST stay synchronous. Its
/// safety against double-withdraw rests on the reserve-at-enqueue happening
/// atomically within this single message (no `GuardPrincipal` is taken, unlike
/// the ICP-native vault ops). Adding ANY `.await` here re-opens a read->await->
/// write race; if you must, acquire a per-vault `GuardPrincipal` first.
#[candid_method(update)]
#[update]
fn withdraw_chain_collateral(
    vault_id: u64,
    amount_e18: u128,
    dest_address: String,
) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::ChainAdmin("not developer".into()));
    }
    let now = ic_cdk::api::time();
    mutate_state(|s| {
        let vault = s
            .multi_chain
            .chain_vaults
            .get(&vault_id)
            .ok_or_else(|| ProtocolError::ChainAdmin("unknown vault".into()))?;
        // M2 review finding E: an EVM-owned (self-serve) vault may ONLY be driven
        // by its EVM signer via the `_evm` methods — the operator must not be able
        // to move a user's collateral. Refuse the dev path for such vaults.
        if vault.owner_evm.is_some() {
            return Err(ProtocolError::ChainAdmin(
                "vault is EVM-owned; use the signed _evm endpoint".into(),
            ));
        }
        let chain = vault.collateral_chain;
        let (symbol, min_cr, _, _) = evm_vault_params(chain)?;
        rumi_protocol_backend::chains::vault::withdraw_collateral_in_state(
            &mut s.multi_chain,
            vault_id,
            amount_e18,
            dest_address,
            rumi_protocol_backend::chains::evm::tecdsa::is_valid_evm_address,
            symbol,
            min_cr,
            now,
        )
        .map_err(|e| ProtocolError::ChainAdmin(format!("{e:?}")))
    })
}

/// Phase 1b Task 13: close a debt-free foreign-chain EVM (Monad or Conflux) vault.
///
/// Requires the vault's `debt_e8s == 0` (repay first by burning icUSD on the
/// foreign chain), then withdraws the FULL remaining collateral to `dest_address`
/// (vault -> `Closing`, then `Closed` on the transfer's confirmation).
/// Synchronous + developer-gated (mirrors `withdraw_chain_collateral`).
/// CONCURRENCY INVARIANT (audit FLAG-16): keep synchronous — see
/// `withdraw_chain_collateral`; adding an `.await` needs a per-vault guard.
#[candid_method(update)]
#[update]
fn close_chain_vault(vault_id: u64, dest_address: String) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::ChainAdmin("not developer".into()));
    }
    let now = ic_cdk::api::time();
    mutate_state(|s| {
        let vault = s
            .multi_chain
            .chain_vaults
            .get(&vault_id)
            .ok_or_else(|| ProtocolError::ChainAdmin("unknown vault".into()))?;
        // M2 review finding E: an EVM-owned (self-serve) vault may ONLY be driven
        // by its EVM signer via the `_evm` methods — the operator must not be able
        // to move a user's collateral. Refuse the dev path for such vaults.
        if vault.owner_evm.is_some() {
            return Err(ProtocolError::ChainAdmin(
                "vault is EVM-owned; use the signed _evm endpoint".into(),
            ));
        }
        let chain = vault.collateral_chain;
        let (symbol, min_cr, _, _) = evm_vault_params(chain)?;
        rumi_protocol_backend::chains::vault::close_chain_vault_in_state(
            &mut s.multi_chain,
            vault_id,
            dest_address,
            rumi_protocol_backend::chains::evm::tecdsa::is_valid_evm_address,
            symbol,
            min_cr,
            now,
        )
        .map_err(|e| ProtocolError::ChainAdmin(format!("{e:?}")))
    })
}

// ─── M2: EVM-native self-serve vault endpoints (EIP-712 signed, anonymous) ────
//
// Authority is the EVM signature, NEVER the IC caller (anonymous ingress is
// accepted; see inspect_message). The vault is owned by a synthetic principal
// derived from the recovered signer. The dev-gated endpoints above stay for
// operator/test use; these are the self-serve variants.

/// Verified, authenticated intent context shared by all four `_evm` methods.
struct VerifiedIntent {
    chain: rumi_protocol_backend::chains::config::ChainId,
    /// Lowercase `0x` recovered signer; the authoritative EVM owner.
    owner_evm: String,
    /// Vault owner key = `synthetic_owner(chain, signer)`.
    synthetic: candid::Principal,
    symbol: &'static str,
    min_cr: u64,
    min_vault_debt_e8s: u128,
    debt_ceiling_e8s: Option<u128>,
}

/// Resolve per-chain params + the deployed IcUSD contract (for the EIP-712
/// domain) + the current time, then delegate to the pure
/// `eip712::verify_intent`. Maps every failure to `ProtocolError::EvmAuth`. No
/// `.await` (callable from the synchronous methods).
fn verify_intent_ctx(
    intent: &rumi_protocol_backend::chains::evm::eip712::VaultIntent,
    signature: &[u8],
    expected_action: rumi_protocol_backend::chains::evm::eip712::IntentAction,
) -> Result<VerifiedIntent, ProtocolError> {
    let chain = rumi_protocol_backend::chains::config::ChainId(intent.chain_id as u32);
    // Resolve symbol + min CR (fails fast for a non-EVM / unregistered chain).
    let (symbol, min_cr, min_vault_debt_e8s, debt_ceiling_e8s) = evm_vault_params(chain)
        .map_err(|e| ProtocolError::EvmAuth(format!("{e:?}")))?;
    // The deployed IcUSD address binds the EIP-712 domain to this chain+deployment.
    let contract = read_state(|s| s.multi_chain.chain_contracts.get(&chain).cloned())
        .ok_or_else(|| ProtocolError::EvmAuth(format!("no contract set for chain {}", chain.0)))?;
    let now_secs = ic_cdk::api::time() / 1_000_000_000;
    let (owner_evm, synthetic) = rumi_protocol_backend::chains::evm::eip712::verify_intent(
        intent,
        signature,
        expected_action,
        &contract,
        now_secs,
    )
    .map_err(|e| ProtocolError::EvmAuth(format!("{e:?}")))?;
    Ok(VerifiedIntent { chain, owner_evm, synthetic, symbol, min_cr, min_vault_debt_e8s, debt_ceiling_e8s })
}

/// Authorize a borrow/withdraw/close `_evm` op against an existing vault: the
/// vault must be owned by `synthetic` AND carry a matching `owner_evm`. Read-only.
fn evm_owns_vault(s: &State, vault_id: u64, v: &VerifiedIntent) -> bool {
    s.multi_chain
        .chain_vaults
        .get(&vault_id)
        .map(|vault| {
            vault.owner == v.synthetic
                && vault
                    .owner_evm
                    .as_deref()
                    .map(|a| a.eq_ignore_ascii_case(&v.owner_evm))
                    .unwrap_or(false)
        })
        .unwrap_or(false)
}

/// EVM-signed `Open` (async — derives the per-vault custody address via tECDSA).
///
/// Saga/TOCTOU: the nonce is consumed + the per-owner cap checked + the vault id
/// reserved in ONE pre-await `mutate_state` (spend-on-attempt), so a same-nonce
/// double-submit is rejected before the costly derive and no duplicate vault can
/// be created. A failed derive leaves the nonce spent — the user re-signs with
/// `nonce + 1` (a spent counter is harmless; no funds move on this path).
#[candid_method(update)]
#[update]
async fn open_chain_vault_evm(
    intent: rumi_protocol_backend::chains::evm::eip712::VaultIntent,
    signature: Vec<u8>,
) -> Result<u64, ProtocolError> {
    use rumi_protocol_backend::chains::evm::eip712::IntentAction;
    let v = verify_intent_ctx(&intent, &signature, IntentAction::Open)?;
    // Pre-await atomic: consume nonce, enforce per-owner cap, reserve vault id.
    let vault_id = mutate_state(|s| {
        s.multi_chain
            .consume_evm_nonce(&v.synthetic, intent.nonce)
            .map_err(|expected| {
                ProtocolError::EvmAuth(format!(
                    "bad nonce: got {}, expected {}",
                    intent.nonce, expected
                ))
            })?;
        if s.multi_chain.count_owner_active_vaults(&v.synthetic)
            >= rumi_protocol_backend::chains::vault::MAX_VAULTS_PER_OWNER
        {
            return Err(ProtocolError::EvmAuth("per-owner vault cap reached".into()));
        }
        s.chain_vault_id_counter += 1;
        Ok(s.chain_vault_id_counter)
    })?;
    // tECDSA derive of the per-vault custody address, keyed by the synthetic owner.
    let path = rumi_protocol_backend::chains::evm::tecdsa::custody_derivation_path(
        v.chain,
        v.synthetic,
        vault_id,
    );
    let (_pubkey, custody) =
        rumi_protocol_backend::chains::evm::tecdsa::derive_evm_address(path)
            .await
            .map_err(|e| ProtocolError::EvmAuth(format!("derive: {e}")))?;
    let now = ic_cdk::api::time();
    let owner_evm = v.owner_evm.clone();
    mutate_state(|s| {
        // Re-check the per-owner cap on the AUTHORITATIVE map before inserting
        // (M2 review finding C). The pre-await check can be raced: several opens
        // for the same owner (distinct nonces) all pass their pre-await count
        // while their vaults are still mid-derive and not yet inserted. Without
        // this re-check, a pipelined burst could blow past MAX_VAULTS_PER_OWNER.
        // The loser forfeits its (already-spent) nonce — consistent with the
        // async open's spend-on-attempt semantics.
        if s.multi_chain.count_owner_active_vaults(&v.synthetic)
            >= rumi_protocol_backend::chains::vault::MAX_VAULTS_PER_OWNER
        {
            return Err(ProtocolError::EvmAuth("per-owner vault cap reached".into()));
        }
        rumi_protocol_backend::chains::vault::open_chain_vault_in_state(
            &mut s.multi_chain,
            v.chain,
            v.synthetic,
            custody,
            intent.collateral_wei,
            intent.debt_e8s,
            owner_evm.clone(), // mint_recipient == owner (recipient forced == owner)
            rumi_protocol_backend::chains::evm::tecdsa::is_valid_evm_address,
            v.symbol,
            v.min_cr,
            v.min_vault_debt_e8s,
            v.debt_ceiling_e8s,
            now,
            vault_id,
        )
        .map_err(|e| ProtocolError::EvmAuth(format!("{e:?}")))?;
        // Stamp the EVM owner so borrow/withdraw/close can re-authorize.
        if let Some(vault) = s.multi_chain.chain_vaults.get_mut(&vault_id) {
            vault.owner_evm = Some(owner_evm.clone());
        }
        Ok(vault_id)
    })
}

/// EVM-signed `Borrow` (synchronous — no `.await`, so the nonce check + op +
/// nonce bump are atomic in one message; preserves the FLAG-16 sync invariant).
/// Spend-on-success: a failed op does not burn the nonce.
#[candid_method(update)]
#[update]
fn borrow_chain_vault_evm(
    intent: rumi_protocol_backend::chains::evm::eip712::VaultIntent,
    signature: Vec<u8>,
) -> Result<(), ProtocolError> {
    use rumi_protocol_backend::chains::evm::eip712::IntentAction;
    let v = verify_intent_ctx(&intent, &signature, IntentAction::Borrow)?;
    let now = ic_cdk::api::time();
    mutate_state(|s| {
        if !evm_owns_vault(s, intent.vault_id, &v) {
            return Err(ProtocolError::EvmAuth("not the vault owner".into()));
        }
        let expected = s.multi_chain.expected_evm_nonce(&v.synthetic);
        if intent.nonce != expected {
            return Err(ProtocolError::EvmAuth(format!(
                "bad nonce: got {}, expected {}",
                intent.nonce, expected
            )));
        }
        rumi_protocol_backend::chains::vault::borrow_chain_vault_in_state(
            &mut s.multi_chain,
            intent.vault_id,
            intent.debt_e8s,
            v.owner_evm.clone(),
            rumi_protocol_backend::chains::evm::tecdsa::is_valid_evm_address,
            v.symbol,
            v.min_cr,
            v.min_vault_debt_e8s,
            v.debt_ceiling_e8s,
            now,
        )
        .map_err(|e| ProtocolError::EvmAuth(format!("{e:?}")))?;
        s.multi_chain
            .evm_owner_nonces
            .insert(v.synthetic, expected.saturating_add(1));
        Ok(())
    })
}

/// EVM-signed `WithdrawCollateral` (synchronous; spend-on-success). The released
/// collateral goes to `owner_evm` (recipient forced == owner).
#[candid_method(update)]
#[update]
fn withdraw_chain_collateral_evm(
    intent: rumi_protocol_backend::chains::evm::eip712::VaultIntent,
    signature: Vec<u8>,
) -> Result<(), ProtocolError> {
    use rumi_protocol_backend::chains::evm::eip712::IntentAction;
    let v = verify_intent_ctx(&intent, &signature, IntentAction::WithdrawCollateral)?;
    let now = ic_cdk::api::time();
    mutate_state(|s| {
        if !evm_owns_vault(s, intent.vault_id, &v) {
            return Err(ProtocolError::EvmAuth("not the vault owner".into()));
        }
        let expected = s.multi_chain.expected_evm_nonce(&v.synthetic);
        if intent.nonce != expected {
            return Err(ProtocolError::EvmAuth(format!(
                "bad nonce: got {}, expected {}",
                intent.nonce, expected
            )));
        }
        rumi_protocol_backend::chains::vault::withdraw_collateral_in_state(
            &mut s.multi_chain,
            intent.vault_id,
            intent.collateral_wei,
            v.owner_evm.clone(),
            rumi_protocol_backend::chains::evm::tecdsa::is_valid_evm_address,
            v.symbol,
            v.min_cr,
            now,
        )
        .map_err(|e| ProtocolError::EvmAuth(format!("{e:?}")))?;
        s.multi_chain
            .evm_owner_nonces
            .insert(v.synthetic, expected.saturating_add(1));
        Ok(())
    })
}

/// EVM-signed `Close` (synchronous; spend-on-success). Requires `debt_e8s == 0`
/// (repay first by burning icUSD on-chain); returns all collateral to `owner_evm`.
#[candid_method(update)]
#[update]
fn close_chain_vault_evm(
    intent: rumi_protocol_backend::chains::evm::eip712::VaultIntent,
    signature: Vec<u8>,
) -> Result<(), ProtocolError> {
    use rumi_protocol_backend::chains::evm::eip712::IntentAction;
    let v = verify_intent_ctx(&intent, &signature, IntentAction::Close)?;
    let now = ic_cdk::api::time();
    mutate_state(|s| {
        if !evm_owns_vault(s, intent.vault_id, &v) {
            return Err(ProtocolError::EvmAuth("not the vault owner".into()));
        }
        let expected = s.multi_chain.expected_evm_nonce(&v.synthetic);
        if intent.nonce != expected {
            return Err(ProtocolError::EvmAuth(format!(
                "bad nonce: got {}, expected {}",
                intent.nonce, expected
            )));
        }
        rumi_protocol_backend::chains::vault::close_chain_vault_in_state(
            &mut s.multi_chain,
            intent.vault_id,
            v.owner_evm.clone(),
            rumi_protocol_backend::chains::evm::tecdsa::is_valid_evm_address,
            v.symbol,
            v.min_cr,
            now,
        )
        .map_err(|e| ProtocolError::EvmAuth(format!("{e:?}")))?;
        s.multi_chain
            .evm_owner_nonces
            .insert(v.synthetic, expected.saturating_add(1));
        Ok(())
    })
}

// ─── Solana M2 vault endpoints (developer-gated) ─────────────────────────────
//
// These mirror the Monad vault endpoints above (`open_chain_vault`,
// `withdraw_chain_collateral`, `close_chain_vault`) one-for-one, swapping the
// Monad primitives for Solana: threshold-Ed25519 custody derivation instead of
// tECDSA, the base58 address validator (`is_valid_solana_address`) instead of
// the EVM one, and the `"SOL"` manual-price key instead of `"MON"`. They call
// the chain-agnostic helpers in `chains::vault` directly (the Monad endpoints go
// through the `chains::monad::chain_vault` wrappers, which bake in the EVM
// validator + `"MON"`). The collateral/amount fields carry LAMPORTS (u128) here;
// the field is generic native base units (`ChainVaultV1.collateral_amount_native`).

/// Open a Solana chain vault, OPEN-THEN-VERIFY (mirrors `open_chain_vault`).
///
/// Creates the vault in `AwaitingDeposit` with the DECLARED collateral
/// (lamports) and the intended mint in `pending_mint_e8s`; enqueues NO mint. The
/// caller then reads the vault's `custody_address` (`get_chain_vault`) and
/// deposits SOL there. deposit-watch (Task 9) verifies the on-chain balance
/// covers the declared collateral at finality, flips the vault to `MintPending`,
/// and enqueues the mint. icUSD is only ever minted against a verified on-chain
/// deposit.
///
/// Developer-gated. Async because deriving the per-user custody address calls
/// threshold Ed25519. Borrow discipline: the `vault_id` is reserved in one
/// `mutate_state`, the custody derive is `.await`ed with NO state borrow held,
/// then the vault is opened + cloned out in a second `mutate_state`.
#[candid_method(update)]
#[update]
async fn open_solana_vault(
    collateral_lamports: u128,
    debt_e8s: u128,
    mint_recipient_base58: String,
) -> Result<rumi_protocol_backend::chains::monad::chain_vault::ChainVaultV1, ProtocolError> {
    use rumi_protocol_backend::chains::solana::{config, ted25519};
    let caller = ic_cdk::caller();
    if read_state(|s| s.developer_principal != caller) {
        return Err(ProtocolError::ChainAdmin("not developer".into()));
    }
    // Reserve the vault id BEFORE the async derive so the derivation path
    // (chain, caller, vault_id) is unique even across concurrent opens.
    let vault_id = mutate_state(|s| {
        s.chain_vault_id_counter += 1;
        s.chain_vault_id_counter
    });
    let path = ted25519::custody_derivation_path(config::SOLANA_CHAIN_ID, caller, vault_id);
    let (_pubkey, custody) = ted25519::derive_solana_address(path)
        .await
        .map_err(|e| ProtocolError::ChainAdmin(format!("derive: {e}")))?;
    let now = ic_cdk::api::time();
    mutate_state(|s| {
        rumi_protocol_backend::chains::vault::open_chain_vault_in_state(
            &mut s.multi_chain,
            config::SOLANA_CHAIN_ID,
            caller,
            custody,
            collateral_lamports,
            debt_e8s,
            mint_recipient_base58,
            ted25519::is_valid_solana_address,
            "SOL",
            config::SOLANA_MIN_CR_E4,
            0,    // min_vault_debt: no floor for Solana (Increment 0 targets Conflux)
            None, // no debt ceiling for Solana yet
            now,
            vault_id,
        )
        .map_err(|e| ProtocolError::ChainAdmin(format!("{e:?}")))?;
        // Return the inserted vault. `open_chain_vault_in_state` inserts it on
        // success, so the lookup cannot miss.
        Ok(s
            .multi_chain
            .chain_vaults
            .get(&vault_id)
            .cloned()
            .expect("vault present: just inserted"))
    })
}

/// Withdraw Solana collateral (mirrors `withdraw_chain_collateral`).
///
/// CONCURRENCY INVARIANT (audit FLAG-16): keep synchronous — see
/// `withdraw_chain_collateral`; adding an `.await` needs a per-vault guard.
///
/// CR-checks the REMAINING collateral against `SOLANA_MIN_CR_E4` (debt-free
/// vaults skip the check), RESERVES the withdrawn lamports, and enqueues a
/// `NativeWithdrawal` op for the Task-8 settlement worker to sign + broadcast. A
/// vault that becomes empty AND debt-free flips to `Closing` here. There is NO
/// repay endpoint: the user burns icUSD on Solana and burn-watch decrements
/// `debt_e8s` + chain supply. Synchronous (the `dest_address` is supplied by the
/// caller; signing happens later in the settlement worker). Developer-gated.
#[candid_method(update)]
#[update]
fn withdraw_solana_collateral(
    vault_id: u64,
    amount_lamports: u128,
    dest_address: String,
) -> Result<(), ProtocolError> {
    use rumi_protocol_backend::chains::solana::{config, ted25519};
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::ChainAdmin("not developer".into()));
    }
    let now = ic_cdk::api::time();
    mutate_state(|s| {
        rumi_protocol_backend::chains::vault::withdraw_collateral_in_state(
            &mut s.multi_chain,
            vault_id,
            amount_lamports,
            dest_address,
            ted25519::is_valid_solana_address,
            "SOL",
            config::SOLANA_MIN_CR_E4,
            now,
        )
    })
    .map_err(|e| ProtocolError::ChainAdmin(format!("{e:?}")))
}

/// Close a debt-free Solana chain vault (mirrors `close_chain_vault`).
///
/// Requires the vault's `debt_e8s == 0` (repay first by burning icUSD on
/// Solana), then withdraws the FULL remaining collateral to `dest_address`
/// (vault -> `Closing`, then `Closed` on the transfer's confirmation).
/// Synchronous + developer-gated. CONCURRENCY INVARIANT (audit FLAG-16): keep
/// synchronous — see `withdraw_chain_collateral`; an `.await` needs a guard.
#[candid_method(update)]
#[update]
fn close_solana_vault(vault_id: u64, dest_address: String) -> Result<(), ProtocolError> {
    use rumi_protocol_backend::chains::solana::{config, ted25519};
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::ChainAdmin("not developer".into()));
    }
    let now = ic_cdk::api::time();
    mutate_state(|s| {
        rumi_protocol_backend::chains::vault::close_chain_vault_in_state(
            &mut s.multi_chain,
            vault_id,
            dest_address,
            ted25519::is_valid_solana_address,
            "SOL",
            config::SOLANA_MIN_CR_E4,
            now,
        )
    })
    .map_err(|e| ProtocolError::ChainAdmin(format!("{e:?}")))
}

// Phase 1b Task 14: query + admin surface for the foreign-chain working set.
//
// NOTE: `get_user_deposit_address` is intentionally omitted. Custody is
// PER-VAULT (the address is derived with nonce = vault_id inside
// `open_chain_vault`), so a user's deposit address is the vault's
// `custody_address`, returned by `get_chain_vault` after open. A nonce=0
// per-user address would be one NO vault ever uses — surfacing it would be
// misleading. Use `get_chain_vault(vault_id)` for the deposit address.

/// Derive the per-chain SETTLEMENT (minter) address — the tECDSA-derived EVM
/// address whose private share the canister controls for that chain. Operators
/// grant `MINTER_ROLE` on `IcUSD.sol` to this address. Async (the derive hits
/// the management/signing subnet). Developer-gated: derivation costs cycles and
/// a signing-subnet call, so it is not exposed to arbitrary callers even though
/// it is read-only. No state borrow is held across the `.await`.
#[candid_method(update)]
#[update]
async fn get_chain_settlement_address(
    chain: rumi_protocol_backend::chains::config::ChainId,
) -> Result<String, ProtocolError> {
    let caller = ic_cdk::caller();
    if read_state(|s| s.developer_principal != caller) {
        return Err(ProtocolError::ChainAdmin("not developer".into()));
    }
    let path = rumi_protocol_backend::chains::monad::tecdsa::settlement_derivation_path(chain);
    rumi_protocol_backend::chains::monad::tecdsa::derive_evm_address(path)
        .await
        .map(|(_pubkey, addr)| addr)
        .map_err(|e| ProtocolError::ChainAdmin(format!("derive: {e}")))
}

/// Return a single foreign-chain vault by id (the `custody_address` field is the
/// user's deposit address). Public read-only query; no gate.
#[candid_method(query)]
#[query]
fn get_chain_vault(
    vault_id: u64,
) -> Option<rumi_protocol_backend::chains::monad::chain_vault::ChainVaultV1> {
    read_state(|s| s.multi_chain.chain_vaults.get(&vault_id).cloned())
}

/// List the foreign-chain vaults whose `collateral_chain == chain`, CLAMPED to
/// at most 500 entries. The clamp follows the Wave-9a DOS-pagination convention:
/// an unbounded query over a growing map is a cycle-DoS vector. A caller needing
/// the full set past 500 must page via a future cursor endpoint. Public query.
#[candid_method(query)]
#[query]
fn list_chain_vaults(
    chain: rumi_protocol_backend::chains::config::ChainId,
) -> Vec<rumi_protocol_backend::chains::monad::chain_vault::ChainVaultV1> {
    const MAX_CHAIN_VAULTS_RETURNED: usize = 500;
    read_state(|s| {
        s.multi_chain
            .chain_vaults
            .values()
            .filter(|v| v.collateral_chain == chain)
            .take(MAX_CHAIN_VAULTS_RETURNED)
            .cloned()
            .collect()
    })
}

/// True iff `chain`'s settlement queue holds any NON-terminal op (`Queued` or
/// `Inflight`), false once the worker has drained it (no op, or only terminal
/// `Succeeded`/`Failed` ops awaiting prune). Lets a caller observe whether a
/// chain's settlement worker has finished its outbound work without inspecting
/// individual ops. Returns false for an unregistered chain (no queue). Public
/// read-only query; no gate (mirrors `get_chain_vault`).
#[candid_method(query)]
#[query]
fn chain_has_active_settlement_op(
    chain: rumi_protocol_backend::chains::config::ChainId,
) -> bool {
    read_state(|s| {
        s.multi_chain
            .settlement_queues
            .get(&chain)
            .map(|q| q.has_active_op())
            .unwrap_or(false)
    })
}

/// Record the deployed `IcUSD.sol` (or equivalent) contract address for a chain.
/// Developer-gated.
#[candid_method(update)]
#[update]
fn set_chain_contract(
    chain: rumi_protocol_backend::chains::config::ChainId,
    address: String,
) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    if read_state(|s| s.developer_principal != caller) {
        return Err(ProtocolError::ChainAdmin("not developer".into()));
    }
    // Fail-fast on a malformed contract address: an unvalidated value flows into
    // the mint calldata's `to` (`tx::abi_word_address`) and panics deep on the
    // settlement worker path, after the re-entrancy guard + awaits, permanently
    // blocking the chain's worker. A deploy-time typo is enough.
    //
    // Chain-aware: Solana's SPL mint is a base58 32-byte address, not an EVM
    // 0x-hex one. Validate it with the Solana base58 check; every other chain
    // (Monad + any EVM chain) keeps the original EVM validation unchanged.
    if chain == rumi_protocol_backend::chains::solana::config::SOLANA_CHAIN_ID {
        if !rumi_protocol_backend::chains::solana::ted25519::is_valid_solana_address(&address) {
            return Err(ProtocolError::ChainAdmin(format!("invalid Solana address: {address}")));
        }
    } else if !rumi_protocol_backend::chains::monad::tecdsa::is_valid_evm_address(&address) {
        return Err(ProtocolError::ChainAdmin(format!("invalid EVM address: {address}")));
    }
    mutate_state(|s| {
        s.multi_chain.chain_contracts.insert(chain, address.clone());
    });
    log!(INFO, "[set_chain_contract] chain={:?} address={}", chain, address);
    Ok(())
}

/// Set (or replace) the per-chain liquidation config (spec 8, Tier B): the DEX
/// wiring + risk knobs the bot path will read. Developer-gated. The config is
/// validated (slippage cap <= 100%, restore target > par, required addresses
/// present when `enabled`) before persisting; a rejected config mutates nothing.
/// Inert until Increment 2+ reads it (Increment 1 ships only this scaffolding).
#[candid_method(update)]
#[update]
fn set_chain_liquidation_config(
    chain: rumi_protocol_backend::chains::config::ChainId,
    config: rumi_protocol_backend::chains::liquidation_config::ChainLiquidationConfigV1,
) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    if read_state(|s| s.developer_principal != caller) {
        return Err(ProtocolError::ChainAdmin("not developer".into()));
    }
    config
        .validate()
        .map_err(|e| ProtocolError::ChainAdmin(format!("invalid liquidation config: {e:?}")))?;
    // Finding #16: the penalty cushion MUST exceed the slippage + oracle-divergence
    // budget, or a swap can structurally deliver less stable than the debt it
    // clears (under-cover). Increment 3 adds the max_dex_oracle_divergence_bps term.
    if config.enabled {
        let penalty_bps = rumi_protocol_backend::chains::collateral_config::chain_collateral_config(chain)
            .map(|c| c.liquidation_penalty_bps)
            .ok_or_else(|| {
                ProtocolError::ChainAdmin(format!("no collateral config for chain {}", chain.0))
            })?;
        let budget_bps =
            (config.slippage_cap_bps as u64).saturating_add(config.max_dex_oracle_divergence_bps as u64);
        if budget_bps >= penalty_bps {
            return Err(ProtocolError::ChainAdmin(format!(
                "slippage_cap_bps {} + max_dex_oracle_divergence_bps {} must be < liquidation_penalty_bps {} (penalty cushion, finding #16)",
                config.slippage_cap_bps, config.max_dex_oracle_divergence_bps, penalty_bps
            )));
        }
    }
    mutate_state(|s| {
        s.multi_chain.chain_liquidation_configs.insert(chain, config.clone());
    });
    log!(
        INFO,
        "[set_chain_liquidation_config] chain={:?} dex={:?} enabled={}",
        chain,
        config.dex,
        config.enabled
    );
    Ok(())
}

/// Return the per-chain liquidation config, or None if unset. Public read-only
/// query (mirrors `get_chain_vault`); the config is operator DEX wiring + risk
/// knobs, no secrets.
#[candid_method(query)]
#[query]
fn get_chain_liquidation_config(
    chain: rumi_protocol_backend::chains::config::ChainId,
) -> Option<rumi_protocol_backend::chains::liquidation_config::ChainLiquidationConfigV1> {
    read_state(|s| s.multi_chain.chain_liquidation_configs.get(&chain).cloned())
}

/// A chain vault currently liquidatable on `chain` (CR below the liquidation
/// threshold), with its interest-aware CR + sizing surfaced for an operator/SP.
/// The discovery channel for the eventual SP fallback (finding #30; SP-specific
/// bot-failed filtering lands in Increment 4).
#[derive(CandidType, Deserialize, Debug, Clone)]
pub struct ChainLiquidatableVault {
    pub vault_id: u64,
    pub chain_id: rumi_protocol_backend::chains::config::ChainId,
    pub debt_e8s: u128,
    pub effective_debt_e8s: u128,
    pub collateral_native: u128,
    pub cr_e4: u64,
    pub liquidation_threshold_e4: u64,
    pub sized_repay_e8s: u128,
}

/// Resolve the chain's native collateral symbol (the manual-price key) for the
/// liquidation path. `Err` if the chain is not a known EVM chain.
fn liquidation_price_symbol(
    chain: rumi_protocol_backend::chains::config::ChainId,
) -> Result<&'static str, ProtocolError> {
    rumi_protocol_backend::chains::evm::evm_chain_config(chain)
        .map(|c| c.native_symbol)
        .ok_or_else(|| ProtocolError::ChainAdmin(format!("chain {} is not a known EVM chain", chain.0)))
}

/// Dev-gated manual/permissionless liquidation trigger (spec §7). Runs the
/// IDENTICAL gate as the detection tick (so a manual caller can never liquidate a
/// vault the timer wouldn't). Synchronous: reads price from state, sizes, and
/// enqueues in one mutate_state. Bot tier only in Increment 2; the enqueued
/// `LiquidationSwap` op is inert until Increment 3.
#[candid_method(update)]
#[update]
fn liquidate_chain_vault(vault_id: u64) -> Result<u64, ProtocolError> {
    let caller = ic_cdk::caller();
    if read_state(|s| s.developer_principal != caller) {
        return Err(ProtocolError::ChainAdmin("not developer".into()));
    }
    let chain = read_state(|s| s.multi_chain.chain_vaults.get(&vault_id).map(|v| v.collateral_chain))
        .ok_or_else(|| ProtocolError::ChainAdmin(format!("unknown vault {vault_id}")))?;
    let threshold = rumi_protocol_backend::chains::collateral_config::chain_collateral_config(chain)
        .map(|c| c.liquidation_threshold_e4)
        .ok_or_else(|| ProtocolError::ChainAdmin(format!("no collateral config for chain {}", chain.0)))?;
    let symbol = liquidation_price_symbol(chain)?;
    let now = ic_cdk::api::time();
    mutate_state(|s| {
        rumi_protocol_backend::chains::vault::begin_liquidation_in_state(
            &mut s.multi_chain,
            vault_id,
            rumi_protocol_backend::chains::evm::tecdsa::is_valid_evm_address,
            symbol,
            threshold,
            now,
        )
    })
    .map_err(|e| ProtocolError::ChainAdmin(format!("{e:?}")))
}

/// Public read: vaults currently liquidatable on `chain` (CR below the
/// liquidation threshold, Open, not already marked, no in-flight mint). Capped
/// for DOS safety. Returns empty if the chain has no collateral config or no
/// fresh price.
#[candid_method(query)]
#[query]
fn get_chain_liquidatable_vaults(
    chain: rumi_protocol_backend::chains::config::ChainId,
) -> Vec<ChainLiquidatableVault> {
    use rumi_protocol_backend::chains::collateral_config::chain_collateral_config;
    use rumi_protocol_backend::chains::liquidation as liq;
    use rumi_protocol_backend::chains::monad::chain_vault::ChainVaultStatus;
    let symbol = match liquidation_price_symbol(chain) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let now = ic_cdk::api::time();
    read_state(|s| {
        let cc = chain_collateral_config(chain);
        let threshold = match cc.map(|c| c.liquidation_threshold_e4) {
            Some(t) => t,
            None => return Vec::new(),
        };
        let apr_bps = cc.map(|c| c.interest_apr_bps).unwrap_or(0);
        let max_age = s
            .multi_chain
            .chain_liquidation_configs
            .get(&chain)
            .map(|c| c.max_price_age_ns)
            .unwrap_or(0);
        let price = match liq::fresh_chain_price_e8(&s.multi_chain, chain, symbol, now, max_age) {
            Ok(p) => p,
            Err(_) => return Vec::new(),
        };
        let nd = s
            .multi_chain
            .chain_configs
            .get(&chain)
            .map(|c| c.chain_native_decimals)
            .unwrap_or(18);
        let bonus = liq::bonus_e4_from_penalty_bps(cc.map(|c| c.liquidation_penalty_bps).unwrap_or(0));
        let target = cc.map(|c| c.recovery_target_cr_e4).unwrap_or(15_500);
        s.multi_chain
            .chain_vaults
            .values()
            .filter(|v| {
                v.collateral_chain == chain
                    && v.status == ChainVaultStatus::Open
                    && v.pending_mint_e8s == 0
                    && v.pending_interest_mint_e8s == 0
                    && v.pending_liquidation.is_none()
                    && v.debt_e8s > 0
            })
            .filter_map(|v| {
                let eff = liq::effective_debt_e8s(
                    v.debt_e8s,
                    v.pending_interest_mint_e8s,
                    apr_bps,
                    now.saturating_sub(v.last_interest_accrual_ns),
                );
                let cr = rumi_protocol_backend::chains::vault::collateral_ratio_e4(
                    v.collateral_amount_native,
                    nd,
                    price,
                    eff,
                );
                if cr >= threshold {
                    return None;
                }
                let cv = liq::collateral_value_e8s(v.collateral_amount_native, nd, price);
                Some(ChainLiquidatableVault {
                    vault_id: v.vault_id,
                    chain_id: chain,
                    debt_e8s: v.debt_e8s,
                    effective_debt_e8s: eff,
                    collateral_native: v.collateral_amount_native,
                    cr_e4: cr,
                    liquidation_threshold_e4: threshold,
                    sized_repay_e8s: liq::sized_repay_e8s(eff, cv, target, bonus),
                })
            })
            .take(500)
            .collect()
    })
}

/// Manual-price readout for `(chain, symbol)`: the USD e8 price plus the
/// wall-clock nanosecond timestamp of the last write (audit F-01 freshness).
/// `set_at_ns == 0` means the price was set before the V5 upgrade (timestamp
/// not yet recorded); it self-heals on the next refresh.
#[derive(CandidType, Deserialize, Debug, Clone)]
pub struct ManualPriceInfo {
    pub price_e8: u64,
    pub set_at_ns: u64,
}

/// Set a manual collateral price override for `(chain, symbol)` as a USD e8
/// value (e.g. $2.00 == 2_0000_0000). Phase 1b uses manual prices for foreign
/// collateral; a real oracle is a later task. Callable by the developer OR the
/// narrowly-scoped price-pusher principal (audit F-01) so the always-online CFX
/// price monitor can refresh without holding the full developer key. Stamps the
/// write time so `get_manual_collateral_price` can expose freshness.
#[candid_method(update)]
#[update]
fn set_manual_collateral_price(
    chain: rumi_protocol_backend::chains::config::ChainId,
    symbol: String,
    price_e8: u64,
) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    if read_state(|s| !s.is_price_setter_authorized(caller, chain.0, &symbol)) {
        return Err(ProtocolError::ChainAdmin("not authorized to set price".into()));
    }
    // Reject a zero price. A 0 price drives `collateral_ratio_e4` to 0, which
    // fails-closed (every open/withdraw with debt rejects with BelowMinCr), so
    // it cannot mint under-collateralized — but it is never a legitimate value
    // and silently freezes the chain's vaults. Catch the fat-finger explicitly.
    if price_e8 == 0 {
        return Err(ProtocolError::ChainAdmin("price_e8 must be greater than 0".into()));
    }
    let now = ic_cdk::api::time();
    mutate_state(|s| {
        s.multi_chain.set_manual_price(chain, symbol.clone(), price_e8, now);
    });
    log!(INFO, "[set_manual_collateral_price] chain={:?} symbol={} price_e8={} set_at_ns={}", chain, symbol, price_e8, now);
    Ok(())
}

/// Read the on-chain manual collateral price and its freshness timestamp for
/// `(chain, symbol)`. Returns `None` if no price is set. This is the read path
/// the off-chain CFX monitor uses to verify its writes landed and to know how
/// stale the canister's own manual price is (audit F-01). Read-only query.
#[candid_method(query)]
#[query]
fn get_manual_collateral_price(
    chain: rumi_protocol_backend::chains::config::ChainId,
    symbol: String,
) -> Option<ManualPriceInfo> {
    read_state(|s| {
        s.multi_chain
            .get_manual_price(chain, &symbol)
            .map(|(price_e8, set_at_ns)| ManualPriceInfo { price_e8, set_at_ns })
    })
}

/// Grant, rotate, or revoke (`None` principal) the narrowly-scoped price-pusher.
/// `allowed` is the exact set of `(chain_id, symbol)` pairs the pusher may set —
/// it may set NOTHING outside this list (and the developer is never constrained
/// by it). Passing an empty `allowed` with a `Some` principal registers a pusher
/// that can set nothing (fail-closed) until scope is granted. The pusher may ONLY
/// call `set_manual_collateral_price`, never any other endpoint. Developer-gated.
#[candid_method(update)]
#[update]
fn set_price_pusher_principal(
    principal: Option<Principal>,
    allowed: Vec<(u32, String)>,
) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    if read_state(|s| s.developer_principal != caller) {
        return Err(ProtocolError::ChainAdmin("not developer".into()));
    }
    // The anonymous principal must never be the pusher: it would authorize ALL
    // unauthenticated ingress to set prices for the allow-listed pairs.
    if principal == Some(Principal::anonymous()) {
        return Err(ProtocolError::ChainAdmin(
            "price pusher cannot be the anonymous principal".into(),
        ));
    }
    let allowed_set: std::collections::BTreeSet<(u32, String)> = allowed.into_iter().collect();
    let allowed_count = allowed_set.len();
    mutate_state(|s| {
        s.price_pusher_principal = principal;
        s.price_pusher_allowed = allowed_set;
    });
    log!(
        INFO,
        "[set_price_pusher_principal] principal={:?} allowed_pairs={}",
        principal,
        allowed_count
    );
    Ok(())
}

/// Read the currently-authorized price-pusher principal (`None` if unset).
#[candid_method(query)]
#[query]
fn get_price_pusher_principal() -> Option<Principal> {
    read_state(|s| s.price_pusher_principal)
}

/// Read the `(chain_id, symbol)` pairs the price-pusher principal is allowed to set.
#[candid_method(query)]
#[query]
fn get_price_pusher_allowed() -> Vec<(u32, String)> {
    read_state(|s| s.price_pusher_allowed.iter().cloned().collect())
}

/// Override the EVM RPC canister principal the Monad wrapper talks to. This is
/// the PocketIC/staging override read via `State::evm_rpc_override()`; on
/// mainnet it points at the production EVM RPC canister. Developer-gated.
#[candid_method(update)]
#[update]
fn set_evm_rpc_principal(principal: candid::Principal) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    if read_state(|s| s.developer_principal != caller) {
        return Err(ProtocolError::ChainAdmin("not developer".into()));
    }
    mutate_state(|s| {
        s.evm_rpc_principal_override = Some(principal);
    });
    log!(INFO, "[set_evm_rpc_principal] principal={}", principal);
    Ok(())
}

/// Clear the global supply-invariant halt (set by the Timer-B self-check on
/// drift). Use only AFTER manually confirming `total_supply_all_chains_e8s`
/// matches `total_chain_vault_debt_e8s`. Developer-gated.
#[candid_method(update)]
#[update]
fn clear_invariant_halt() -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    if read_state(|s| s.developer_principal != caller) {
        return Err(ProtocolError::ChainAdmin("not developer".into()));
    }
    // Refuse to clear into a still-diverged state. The halt exists because the
    // Timer-B self-check found `sum(chain_supplies) != total_chain_vault_debt`;
    // clearing it blind would re-enable cross-chain supply mutation on top of a
    // known-bad invariant. Re-verify against live state and only clear when the
    // internal invariant actually holds again (the operator is still expected to
    // have reconciled against on-chain reality first).
    let result = mutate_state(|s| {
        let total_debt = s.multi_chain.total_chain_vault_debt_e8s();
        match rumi_protocol_backend::chains::supply::check_invariant(&s.multi_chain, total_debt) {
            Ok(()) => {
                s.multi_chain.invariant_halted = false;
                Ok(())
            }
            Err(e) => Err(ProtocolError::ChainAdmin(format!(
                "refusing to clear invariant halt: supply invariant still diverged ({e:?}); \
                 reconcile chain_supplies vs vault debt first"
            ))),
        }
    });
    if result.is_ok() {
        log!(INFO, "[clear_invariant_halt] global invariant halt cleared (invariant re-verified)");
    }
    result
}

/// Clear a chain's reorg circuit breaker. Resets BOTH `reorg_halted` AND the
/// `reorg_suspect_streak` debounce counter (Task 11): clearing the halt without
/// also zeroing the streak would let the very next suspect observer tick push
/// the streak back over `REORG_CONFIRM_TICKS` and re-halt the chain.
/// Developer-gated.
#[candid_method(update)]
#[update]
fn clear_reorg_halt(
    chain: rumi_protocol_backend::chains::config::ChainId,
) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    if read_state(|s| s.developer_principal != caller) {
        return Err(ProtocolError::ChainAdmin("not developer".into()));
    }
    mutate_state(|s| {
        s.multi_chain.reorg_halted.remove(&chain);
        s.multi_chain.reorg_suspect_streak.remove(&chain);
    });
    log!(INFO, "[clear_reorg_halt] chain={:?} reorg halt + suspect streak cleared", chain);
    Ok(())
}

/// Seed the burn-watch cursor to the current chain tip when activating a chain
/// (Gate-4 prerequisite). Events before the seed are not scanned (none exist
/// pre-activation). 0 = unseeded (burn-watch inert; deposit-watch still runs).
/// Developer-gated.
#[candid_method(update)]
#[update]
fn set_last_observed_block(
    chain: rumi_protocol_backend::chains::config::ChainId,
    block: u64,
) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    if read_state(|s| s.developer_principal != caller) {
        return Err(ProtocolError::ChainAdmin("not developer".into()));
    }
    mutate_state(|s| {
        s.multi_chain.last_observed_block.insert(chain, block);
    });
    log!(INFO, "[set_last_observed_block] chain={:?} block={}", chain, block);
    Ok(())
}

/// Read the burn-watch cursor (`last_observed_block`) for a chain. Returns 0 when
/// unseeded. Ungated query — used by tests and Task-D staging verification to
/// confirm the cursor advances.
#[candid_method(query)]
#[query]
fn get_last_observed_block(
    chain: rumi_protocol_backend::chains::config::ChainId,
) -> u64 {
    read_state(|s| s.multi_chain.last_observed_block.get(&chain).copied().unwrap_or(0))
}

/// Phase 1c notify-then-verify: submit a burn transaction hash for verification.
///
/// PERMISSIONLESS: anyone may submit a REAL on-chain tx hash. The verify path
/// (`verify_and_apply_burn_proof`) fetches the receipt via ONE
/// `eth_getTransactionReceipt`, rejects forgeries (only Burn logs emitted by the
/// configured icUSD contract count, and the amount/vault come FROM the log, never
/// from the caller), requires finality, and dedups on `(tx_hash, log_index)` via
/// the existing `processed_burn_keys` set — so a re-submit of an already-applied
/// burn returns Ok(0) and changes nothing. Returns the number of burns NEWLY
/// applied from the tx. This replaces the continuous `eth_getLogs` burn-scan as
/// the PRIMARY burn-observation path (one outcall per actual burn instead of
/// O(blocks produced)).
///
/// Finality lag is surfaced as `TemporarilyUnavailable` so the caller (the
/// frontend, per plan Task 7) can poll-and-retry until the receipt is final.
///
/// FUTURE ROBUSTNESS (flagged per Rob 2026-05-31): v1 liveness depends on the
/// submitter (the dApp). Proper DoS protection (this is a permissionless
/// endpoint that spends a ~2B-cycle `eth_getTransactionReceipt` outcall per
/// call) needs the deferred relayer / incentivized-submitter design (audit
/// FLAG-7). A naive per-caller wall-clock rate-limit was rejected: it both fails
/// against principal rotation AND wrongly throttles legitimate back-to-back
/// submissions (e.g. two distinct burns in the same second). The endpoint does
/// reject the anonymous principal as basic hygiene (ingress anonymous is also
/// dropped by `inspect_message`; this is belt-and-suspenders, and covers any
/// future non-ingress entry that skips that hook).
#[candid_method(update)]
#[update]
async fn submit_burn_proof(
    chain_id: rumi_protocol_backend::chains::config::ChainId,
    tx_hash: String,
) -> Result<u32, ProtocolError> {
    use rumi_protocol_backend::chains::monad::burn_proof::{
        verify_and_apply_burn_proof, BurnProofError,
    };

    if ic_cdk::caller() == candid::Principal::anonymous() {
        return Err(ProtocolError::ChainAdmin(
            "anonymous caller not allowed for submit_burn_proof".into(),
        ));
    }

    match verify_and_apply_burn_proof(chain_id, &tx_hash).await {
        Ok(n) => {
            if n > 0 {
                log!(
                    INFO,
                    "[submit_burn_proof] chain={:?} tx={} applied {} burn(s)",
                    chain_id, tx_hash, n
                );
            }
            Ok(n)
        }
        // Transient: the receipt is not yet mined or not yet buried under
        // finality_depth confirmations. The caller should retry.
        Err(BurnProofError::Pending) | Err(BurnProofError::NotFinal) => Err(
            ProtocolError::TemporarilyUnavailable("receipt not yet final; retry".into()),
        ),
        // A transport/RPC failure is also retryable.
        Err(BurnProofError::Rpc(e)) => Err(ProtocolError::TemporarilyUnavailable(format!(
            "burn-proof RPC error; retry: {}",
            e
        ))),
        // Terminal: reverted tx, unknown chain/contract, or a halt-class
        // supply-invariant failure. None of these is fixed by retrying.
        Err(e) => Err(ProtocolError::ChainAdmin(format!("burn proof rejected: {:?}", e))),
    }
}

/// Phase 1c: developer-gated toggle for the EMERGENCY continuous `eth_getLogs`
/// burn-watch poll-scan on a chain. Default OFF (notify-then-verify only, via
/// `submit_burn_proof`). Flip ON only for a targeted catch-up — the scan costs
/// O(blocks produced), so leave it OFF in steady state. No-op (Ok) if the chain
/// is not registered. Persisted in `ChainConfigV2.burn_watch_poll_enabled`.
#[candid_method(update)]
#[update]
fn set_burn_watch_poll_enabled(
    chain_id: rumi_protocol_backend::chains::config::ChainId,
    enabled: bool,
) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    if read_state(|s| s.developer_principal != caller) {
        return Err(ProtocolError::ChainAdmin("not developer".into()));
    }
    mutate_state(|s| {
        if let Some(c) = s.multi_chain.chain_configs.get_mut(&chain_id) {
            c.burn_watch_poll_enabled = enabled;
        }
    });
    log!(
        INFO,
        "[set_burn_watch_poll_enabled] chain={:?} burn-watch poll-scan {}",
        chain_id,
        if enabled { "ENABLED" } else { "disabled" }
    );
    Ok(())
}

/// Delete a chain entirely. Permitted only when the chain carries ZERO supply
/// and NO chain_vaults reference it (so deletion cannot orphan debt/collateral);
/// purges every per-chain map. Developer-gated. This DELETES (not disables) a
/// chain, so it does not record `Event::ChainDisabled`.
#[candid_method(update)]
#[update]
fn delete_chain(
    chain: rumi_protocol_backend::chains::config::ChainId,
) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    if read_state(|s| s.developer_principal != caller) {
        return Err(ProtocolError::ChainAdmin("not developer".into()));
    }
    let result = mutate_state(|s| {
        rumi_protocol_backend::chains::admin::delete_chain_in_state(&mut s.multi_chain, chain)
    });
    match result {
        Ok(()) => {
            log!(INFO, "[delete_chain] chain={:?} deleted (all per-chain state purged)", chain);
            Ok(())
        }
        Err(e) => Err(ProtocolError::ChainAdmin(format!("{e:?}"))),
    }
}

/// Developer-gated recovery for a foreign-chain vault wedged in `MintPending`
/// after its mint permanently reverted (audit FLAG-4). When a mint reverts at
/// finality the worker clears `pending_mint_e8s` but leaves the vault
/// `MintPending`; there is no transition out (withdraw/close require `Open`, and
/// the mint can never re-confirm with `pending == 0`), so the deposited
/// collateral would be locked forever. This transitions a genuinely-stuck vault
/// back to `Open` (debt is 0 under Design B — the mint was never confirmed) so
/// the existing dev-gated `close_chain_vault` / `withdraw_chain_collateral` can
/// return the collateral. Rejects unless the vault is on `chain`, is
/// `MintPending`, has `pending_mint_e8s == 0`, and has NO live (Queued/Inflight)
/// Mint op left.
#[candid_method(update)]
#[update]
async fn recover_stuck_chain_vault(
    chain: rumi_protocol_backend::chains::config::ChainId,
    vault_id: u64,
) -> Result<(), ProtocolError> {
    // M-09 (RECOV-02): this used to flip MintPending->Open on the unverified
    // `pending_mint_e8s == 0` alone. It now delegates to the chains recovery
    // module, which RE-VERIFIES on-chain (via the EVM-RPC quorum) that the mint
    // did NOT actually land before releasing collateral. Async because the
    // verification makes inter-canister RPC calls.
    let caller = ic_cdk::caller();
    if read_state(|s| s.developer_principal != caller) {
        return Err(ProtocolError::ChainAdmin("not developer".into()));
    }
    rumi_protocol_backend::chains::recovery::recover_stuck_chain_vault_verified(chain, vault_id)
        .await
        .map_err(|e| ProtocolError::ChainAdmin(format!("{e}")))?;
    log!(INFO, "[recover_stuck_chain_vault] chain={:?} vault={} MintPending->Open (on-chain-verified mint did NOT land; collateral now recoverable via close/withdraw)", chain, vault_id);
    Ok(())
}

/// Developer-gated resolution for a settlement op wedged `Inflight` (audit
/// FLAG-10). `select_next_op` is strictly one-in-flight per chain, so an op whose
/// tx never confirms blocks EVERY later op for that chain (the Solana path has no
/// automatic same-bytes rebroadcast yet). This marks the op `Failed` and applies
/// the SAME per-kind reversal as the confirm-reverted path (clear a Mint's
/// `pending_mint_e8s`; restore a NativeWithdrawal's reserved collateral and flip
/// `Closing -> Open`) so the queue can advance.
///
/// DANGER: the operator MUST first verify on-chain that the op's tx did NOT land.
/// Marking a Mint `Failed` after its tx actually minted on-chain would leave
/// icUSD minted with no recorded debt (an unbacked mint). Rejects unless the op
/// is currently `Inflight`.
#[candid_method(update)]
#[update]
async fn resolve_stuck_settlement_op(
    chain: rumi_protocol_backend::chains::config::ChainId,
    op_id: u64,
) -> Result<(), ProtocolError> {
    // M-08 (RECOV-01): this used to reverse the op on pure operator assertion
    // (no `.await`, no on-chain re-read). It now delegates to the chains recovery
    // module, which RE-READS the op's tx receipt through the EVM-RPC quorum and
    // REFUSES the reversal if the tx actually landed (reversing a landed Mint
    // would leave an unbacked mint). Async because the verification makes
    // inter-canister RPC calls.
    let caller = ic_cdk::caller();
    if read_state(|s| s.developer_principal != caller) {
        return Err(ProtocolError::ChainAdmin("not developer".into()));
    }
    let did_reverse =
        rumi_protocol_backend::chains::recovery::resolve_stuck_settlement_op_verified(chain, op_id)
            .await
            .map_err(|e| ProtocolError::ChainAdmin(format!("{e}")))?;
    if did_reverse {
        log!(INFO, "[resolve_stuck_settlement_op] chain={:?} op={} marked Failed + reversed (on-chain-verified NOT landed)", chain, op_id);
    } else {
        log!(INFO, "[resolve_stuck_settlement_op] chain={:?} op={} no longer Inflight at commit (concurrent confirm); no-op", chain, op_id);
    }
    Ok(())
}

/// On-demand reconciliation report for a chain (audit FLAG-2). Compares the real
/// on-chain icUSD `totalSupply()` (read at the finalized cursor, consensus-safe
/// specific block) against the canister's recorded `chain_supplies` plus any
/// in-flight mints.
#[derive(candid::CandidType, serde::Deserialize, Clone, Debug)]
pub struct ChainSupplyReconciliation {
    pub chain_id: rumi_protocol_backend::chains::config::ChainId,
    pub finalized_block: u64,
    pub onchain_total_supply_e8s: u128,
    pub recorded_supply_e8s: u128,
    pub in_flight_mint_e8s: u128,
    /// True iff on-chain supply exceeds recorded + in-flight mints: the
    /// unbacked-mint signature that the periodic self-check (two internal mirror
    /// fields) structurally cannot detect.
    pub unbacked_excess: bool,
    /// Signed gap = onchain - recorded. Positive => excess (possible unbacked
    /// mint); negative => deficit (an unsubmitted burn the backstop handles).
    pub gap_e8s: i128,
    /// INFORMATIONAL breakdown of the unified supply invariant's RHS for this
    /// chain (spec 5.4, findings #17/#20/#29). These are backing RECLASSIFICATION,
    /// NOT on-chain supply, so they MUST NOT enter `unbacked_excess` (which is the
    /// on-chain-vs-recorded truth check) — adding them there would mask a real
    /// unbacked mint. They are surfaced purely so the operator sees the
    /// debt/reserve/pending-burn split. 0 until Increment 2+ populates them.
    pub reserve_backing_e8s: u128,
    pub pending_chain_burn_e8s: u128,
    /// Physical USDC the reserve address holds for this chain (native base units,
    /// 18-dec on eSpace). Bookkeeping of the ASSET; informational only.
    pub reserve_usdc_native: u128,
}

/// Developer-gated, on-demand supply reconciliation against the chain (audit
/// FLAG-2). The Timer-B self-check only compares two INTERNAL mirror fields
/// (`sum(chain_supplies)` vs `total_chain_vault_debt`), which are kept in
/// lockstep and so cannot reveal canister-vs-chain drift. This reads the real
/// on-chain `totalSupply()` at the finalized cursor and reports the gap, so an
/// unbacked mint (on-chain supply ABOVE recorded, with no in-flight mint) is
/// detectable even when recorded debt is 0 (which the observer's per-tick alarm
/// skips on its no-debt fast path).
#[candid_method(update)]
#[update]
async fn reconcile_chain_supply(
    chain: rumi_protocol_backend::chains::config::ChainId,
) -> Result<ChainSupplyReconciliation, ProtocolError> {
    use rumi_protocol_backend::chains::settlement_queue::{SettlementOpKind, SettlementOpStatus};
    let caller = ic_cdk::caller();
    if read_state(|s| s.developer_principal != caller) {
        return Err(ProtocolError::ChainAdmin("not developer".into()));
    }
    let (contract, recorded, in_flight, cursor, reserve_backing, pending_burn, reserve_usdc) =
        read_state(|s| {
            let contract = s.multi_chain.chain_contracts.get(&chain).cloned();
            let recorded = s.multi_chain.chain_supplies.get(&chain).copied().unwrap_or(0);
            let in_flight = s
                .multi_chain
                .settlement_queues
                .get(&chain)
                .map(|q| {
                    q.pending
                        .values()
                        .filter_map(|op| match &op.kind {
                            SettlementOpKind::Mint { amount_e8s, .. }
                                if matches!(
                                    op.status,
                                    SettlementOpStatus::Queued | SettlementOpStatus::Inflight { .. }
                                ) =>
                            {
                                Some(*amount_e8s)
                            }
                            _ => None,
                        })
                        .sum::<u128>()
                })
                .unwrap_or(0);
            let cursor = s.multi_chain.last_observed_block.get(&chain).copied().unwrap_or(0);
            // Informational RHS breakdown for this chain (NOT part of the
            // unbacked_excess check — findings #17/#20/#29).
            let reserve_backing = s.multi_chain.reserve_backing_e8s.get(&chain).copied().unwrap_or(0);
            let pending_burn = s.multi_chain.pending_chain_burn_e8s.get(&chain).copied().unwrap_or(0);
            let reserve_usdc = s.multi_chain.reserve_usdc_native.get(&chain).copied().unwrap_or(0);
            (contract, recorded, in_flight, cursor, reserve_backing, pending_burn, reserve_usdc)
        });
    let contract = contract.ok_or_else(|| {
        ProtocolError::ChainAdmin(format!("no icUSD contract configured for chain {:?}", chain))
    })?;
    if cursor == 0 {
        return Err(ProtocolError::ChainAdmin(
            "finalized cursor unseeded; call set_last_observed_block(chain, <tip>) first".into(),
        ));
    }
    let onchain =
        rumi_protocol_backend::chains::monad::evm_rpc::erc20_total_supply_at(chain, &contract, cursor)
            .await
            .map_err(|e| {
                ProtocolError::TemporarilyUnavailable(format!("totalSupply read failed; retry: {e}"))
            })?;
    let gap_e8s = onchain as i128 - recorded as i128;
    // The unbacked-excess test compares on-chain totalSupply against recorded
    // chain_supplies (+ in-flight mints) ONLY. reserve_backing / pending_chain_burn
    // are internal backing reclassification that never change on-chain totalSupply,
    // so they MUST stay out of this inequality or they would mask a real unbacked
    // mint (findings #17/#20/#29). They are reported below as informational fields.
    let unbacked_excess = onchain > recorded.saturating_add(in_flight);
    if unbacked_excess {
        log!(INFO, "[reconcile_chain_supply] chain={:?} UNBACKED EXCESS: onchain {} > recorded {} + in_flight {} (block {})", chain, onchain, recorded, in_flight, cursor);
    }
    Ok(ChainSupplyReconciliation {
        chain_id: chain,
        finalized_block: cursor,
        onchain_total_supply_e8s: onchain,
        recorded_supply_e8s: recorded,
        in_flight_mint_e8s: in_flight,
        unbacked_excess,
        gap_e8s,
        reserve_backing_e8s: reserve_backing,
        pending_chain_burn_e8s: pending_burn,
        reserve_usdc_native: reserve_usdc,
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
                rumi_protocol_backend::state::InterestDestination::Amm1 => "amm1".to_string(),
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
        bot_cr_tolerance_bps: s.bot_cr_tolerance_bps,

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
    read_state(|s| {
        let icp_ct = s.icp_collateral_type();
        Fees {
            borrowing_fee: s.get_borrowing_fee().to_f64(),
            redemption_fee: s.get_redemption_fee_for(&icp_ct, redeemed_amount.into()).to_f64(),
        }
    })
}

#[candid_method(query)]
#[query]
fn get_fees_for_collateral(collateral_type: Principal, redeemed_amount: u64) -> Fees {
    read_state(|s| Fees {
        borrowing_fee: s.get_borrowing_fee().to_f64(),
        redemption_fee: s.get_redemption_fee_for(&collateral_type, redeemed_amount.into()).to_f64(),
    })
}

/// Legacy entry point for the explorer's per-vault timeline. Kept for
/// backwards-compat with cached frontend bundles, but now bounded:
/// returns at most `MAX_VAULT_HISTORY` matches. When a vault has more
/// than that many events, the newest matches are returned in chronological
/// order (the frontend reverses for newest-first display). For full
/// historical access, use `get_vault_history_paged`.
///
/// Audit Wave 9a (DOS-001): the previous unbounded scan walked every
/// stable-log entry and decoded each one for `is_vault_related`. The
/// per-call cost scaled linearly with total event-log size; this cap
/// bounds the response to the most relevant slice.
#[candid_method(query)]
#[query]
fn get_vault_history(vault_id: u64) -> Vec<(u64, Event)> {
    if ic_cdk::api::data_certificate().is_none() {
        ic_cdk::trap("update call rejected");
    }

    // Forward scan with a bounded ring buffer keeps the response size
    // O(MAX_VAULT_HISTORY) regardless of total log length. Order in
    // the buffer is chronological (oldest of the latest 200 first),
    // matching the previous semantic — the frontend already reverses
    // for newest-first display.
    let mut buf: std::collections::VecDeque<(u64, Event)> =
        std::collections::VecDeque::with_capacity(MAX_VAULT_HISTORY);
    for (idx, event) in events().enumerate() {
        if event.is_vault_related(&vault_id) {
            if buf.len() == MAX_VAULT_HISTORY {
                buf.pop_front();
            }
            buf.push_back((idx as u64, event));
        }
    }
    buf.into_iter().collect()
}

/// Paginated per-vault timeline. `start` indexes into matches sorted
/// newest-first; `length` is the page size (capped at
/// `MAX_VAULT_HISTORY_PAGE`). `total` is the total matched-event count
/// for this vault so the caller can render accurate page indicators.
///
/// Audit Wave 9a (DOS-001): bounds response size and gives the
/// explorer paged access to a vault's full history without forcing a
/// single round-trip.
#[candid_method(query)]
#[query]
fn get_vault_history_paged(vault_id: u64, start: u64, length: u64) -> VaultHistoryPagedResponse {
    if ic_cdk::api::data_certificate().is_none() {
        ic_cdk::trap("update call rejected");
    }

    let length = length.min(MAX_VAULT_HISTORY as u64);

    let all_matches: Vec<(u64, Event)> = events()
        .enumerate()
        .filter(|(_, e)| e.is_vault_related(&vault_id))
        .map(|(i, e)| (i as u64, e))
        .collect();
    let total = all_matches.len() as u64;

    let events_page: Vec<(u64, Event)> = if start >= total {
        Vec::new()
    } else {
        all_matches
            .into_iter()
            .rev()
            .skip(start as usize)
            .take(length as usize)
            .collect()
    };

    VaultHistoryPagedResponse {
        total,
        events: events_page,
    }
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

/// Forward, id-cursored, type-filtered event scan for incremental ingestion.
/// Scans the window `[start, start+max_scan)` of the global event log (oldest
/// first), returns the events passing the `types` filter paired with their
/// GLOBAL log index, and a `next_start` cursor to resume from without gaps or
/// repeats.
///
/// Unlike `get_events_filtered` (newest-first, paged by page number, no stable
/// cursor), this is a forward window keyed on the stable global index. A poller
/// ingests every matching event exactly once by advancing `start := next_start`
/// until `reached_end`, then resumes from the same cursor as new events append.
/// `events().enumerate().skip(start)` seeks in O(1) (`EventIterator::nth`), so
/// per-call cost is O(max_scan) regardless of how deep `start` is. Only the
/// `types` facet is applied (principal/collateral/size/time are not), matching
/// the points-canister ingestion use case. Added for `rumi_points` (airdrop).
const FORWARD_FILTERED_MAX_SCAN: u64 = 2000;

/// Pure forward-scan + type-filter + cursor logic, generic over the event source
/// so it is unit-testable with a `Vec<Event>`. `count` is the total event count
/// (the resume cursor is clamped to it). Production passes `events()` +
/// `count_events()`; the O(1) seek of `EventIterator::nth` keeps the real scan at
/// O(max_scan) regardless of `start`.
fn scan_events_forward_filtered<I: Iterator<Item = Event>>(
    source: I,
    start: u64,
    max_scan: u64,
    count: u64,
    types_set: Option<&std::collections::HashSet<EventTypeFilter>>,
) -> ForwardFilteredEventsResponse {
    let scan = max_scan.min(FORWARD_FILTERED_MAX_SCAN);
    let empty_lookup = std::collections::HashMap::new();
    let matched: Vec<(u64, Event)> = source
        .enumerate()
        .skip(start as usize)
        .take(scan as usize)
        .filter(|(_, e)| {
            e.passes_filters(types_set, None, None, None, None, None, &empty_lookup, 0)
        })
        .map(|(i, e)| (i as u64, e))
        .collect();
    let next_start = start.saturating_add(scan).min(count);
    ForwardFilteredEventsResponse {
        events: matched,
        next_start,
        reached_end: next_start >= count,
    }
}

#[candid_method(query)]
#[query]
fn get_events_forward_filtered(
    start: u64,
    max_scan: u64,
    types: Option<Vec<EventTypeFilter>>,
) -> ForwardFilteredEventsResponse {
    // NOTE: intentionally NO `data_certificate()` "update call rejected" guard
    // here (unlike `get_events_filtered`). This endpoint is designed to be polled
    // by another canister (rumi_points) via an inter-canister call, which runs in
    // replicated context where `data_certificate()` is `None`; the guard would
    // reject every poll. The scan is O(max_scan)-bounded, so replicated execution
    // is acceptable.
    let types_set: Option<std::collections::HashSet<EventTypeFilter>> = types
        .as_ref()
        .filter(|v| !v.is_empty())
        .map(|v| v.iter().cloned().collect());
    let count = rumi_protocol_backend::storage::count_events();
    scan_events_forward_filtered(events(), start, max_scan, count, types_set.as_ref())
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

/// Legacy entry point for the explorer's per-principal activity feed.
/// Returns up to `MAX_RESULTS` matches in newest-first order. The
/// previous implementation materialised every match before slicing;
/// we now use a bounded ring buffer so memory is O(MAX_RESULTS)
/// regardless of total log length.
///
/// Audit Wave 9a (DOS-003): bounds response size and intermediate
/// memory. The full-log scan complexity is unchanged — for callers
/// that need to walk a very large log without paying that O(N) cost
/// per call, use `get_events_by_principal_paged` to scan a bounded
/// window per call.
#[candid_method(query)]
#[query]
fn get_events_by_principal(principal: Principal) -> Vec<(u64, Event)> {
    if ic_cdk::api::data_certificate().is_none() {
        ic_cdk::trap("update call rejected");
    }

    let mut buf: std::collections::VecDeque<(u64, Event)> =
        std::collections::VecDeque::with_capacity(MAX_EVENTS_BY_PRINCIPAL_LEGACY);
    for (idx, event) in events().enumerate() {
        if !event.is_accrue_interest() && event.involves_principal(&principal) {
            if buf.len() == MAX_EVENTS_BY_PRINCIPAL_LEGACY {
                buf.pop_front();
            }
            buf.push_back((idx as u64, event));
        }
    }
    // Newest-first matches the legacy `.rev().take(MAX_RESULTS)` ordering.
    buf.into_iter().rev().collect()
}

/// Cursor-paginated principal activity feed. Caller passes
/// `scan_start` (event-log index to begin scanning at, inclusive) and
/// `scan_length` (number of log entries to walk in this call, capped
/// at `MAX_SCAN_LENGTH`). The response reports the matches found in
/// that window, the `scan_end` index where the next call should
/// resume, and an `exhausted` flag set true once the scan reaches
/// the current end of the event log.
///
/// Audit Wave 9a (DOS-003): bounds the per-call scan window so a
/// caller paging through a very large log can never trigger an
/// unbounded query — the scan stays under the cycle budget for any
/// log size. Output is also capped at `MAX_OUTPUT_PER_CALL` matches
/// to bound the response payload.
#[candid_method(query)]
#[query]
fn get_events_by_principal_paged(
    principal: Principal,
    scan_start: u64,
    scan_length: u64,
) -> EventsByPrincipalPagedResponse {
    if ic_cdk::api::data_certificate().is_none() {
        ic_cdk::trap("update call rejected");
    }

    let total_events = rumi_protocol_backend::storage::count_events();
    let scan_length = scan_length.min(MAX_EVENTS_BY_PRINCIPAL_SCAN);
    let scan_end = scan_start.saturating_add(scan_length).min(total_events);

    let mut events_page: Vec<(u64, Event)> = Vec::new();
    if scan_start < total_events && scan_length > 0 {
        for (offset, event) in events()
            .skip(scan_start as usize)
            .take(scan_length as usize)
            .enumerate()
        {
            if !event.is_accrue_interest() && event.involves_principal(&principal) {
                let idx = scan_start.saturating_add(offset as u64);
                events_page.push((idx, event));
                if events_page.len() == MAX_EVENTS_BY_PRINCIPAL_OUTPUT {
                    break;
                }
            }
        }
    }

    EventsByPrincipalPagedResponse {
        events: events_page,
        scan_end,
        exhausted: scan_end >= total_events,
        total_events,
    }
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

/// Vault lookup. With `target = Some(principal)` returns every vault
/// owned by that principal (naturally bounded — typical principals own
/// 1-2 vaults; the per-principal index walks at most a few entries).
/// With `target = None` returns the first `MAX_VAULTS_LEGACY_PAGE`
/// vaults by ascending `vault_id`. For full enumeration use
/// `get_vaults_page` with cursoring.
///
/// Audit Wave 9a (DOS-004): the previous `target = None` branch cloned
/// every vault and Candid-encoded the full set — at 10k+ vaults that
/// pushed reply sizes into the megabyte range. The cap bounds the
/// legacy reply; new callers paginate.
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
                .take(MAX_VAULTS_LEGACY_PAGE)
                .cloned()
                .map(CandidVault::from)
                .collect::<Vec<CandidVault>>()
        }),
    }
}

/// Paginated vault enumeration. Returns vaults with `vault_id >= start_id`
/// up to `limit` entries (capped at `MAX_VAULTS_PAGE_LIMIT`), ordered
/// ascending by `vault_id`. `next_start_id` is `Some(id)` when more
/// vaults remain past this page, `None` when the end of the map is
/// reached.
///
/// Audit Wave 9a (DOS-004): replaces unbounded `get_all_vaults` /
/// `get_vaults(None)` reads with a cursor-based page so single-call
/// reply size and instructions stay bounded at any TVL.
#[candid_method(query)]
#[query]
fn get_vaults_page(start_id: u64, limit: u64) -> VaultsPageResponse {
    let limit = limit.min(MAX_VAULTS_PAGE_LIMIT) as usize;

    read_state(|s| {
        let mut iter = s.vault_id_to_vaults.range(start_id..);
        let mut vaults = Vec::with_capacity(limit);
        for (_, vault) in iter.by_ref().take(limit) {
            vaults.push(CandidVault::from(vault.clone()));
        }
        let next_start_id = iter.next().map(|(id, _)| *id);
        VaultsPageResponse { vaults, next_start_id }
    })
}

/// Total vault count (open + closed). Used by the explorer to size
/// pagination controls without fetching the full vault list. Audit
/// Wave 9a (DOS-004) companion to `get_vaults_page`.
#[candid_method(query)]
#[query]
fn get_vault_count() -> u64 {
    read_state(|s| s.vault_id_to_vaults.len() as u64)
}

// Vault related operations
#[candid_method(update)]
#[update]
async fn redeem_icp(icusd_amount: u64) -> Result<SuccessWithFee, ProtocolError> {
    validate_call().await?;
    // Wave-9 RED-003 / RED-101: gate the ICP redemption path on protocol mode,
    // matching redeem_collateral. This endpoint was the RED-003 fix's blind spot
    // (it reaches the same collateral-seizing path via vault::redeem_icp ->
    // vault::redeem_collateral). Defense in depth alongside the shared
    // vault-module gate now in vault::redeem_collateral.
    validate_mode()?;
    check_postcondition(rumi_protocol_backend::vault::redeem_icp(icusd_amount).await)
}

/// Generic collateral redemption: burn icUSD and receive any collateral type.
/// `redeem_icp` remains as a convenience wrapper for ICP specifically.
#[candid_method(update)]
#[update]
async fn redeem_collateral(collateral_type: Principal, icusd_amount: u64) -> Result<SuccessWithFee, ProtocolError> {
    validate_call().await?;
    // Wave-9 RED-003: gate redemption on protocol mode. ReadOnly auto-latches
    // when total collateral ratio drops below 100% (Wave-1) or when the
    // deficit account crosses the configured threshold (Wave-8e LIQ-005);
    // both are insolvency signals where further redemption would deepen the
    // bad-debt position by extracting collateral from a protocol that
    // already owes more than it holds.
    validate_mode()?;
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
        let icp_ct = s.icp_collateral_type();
        s.get_redemption_fee_for(&icp_ct, ICUSD::from(100_000_000)).to_f64()
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
    // ORACLE-001: refresh the (possibly non-ICP) collateral price before minting.
    validate_freshness_for_collateral(collateral_type).await?;
    check_postcondition(
        rumi_protocol_backend::vault::open_vault_and_borrow(collateral_amount, borrow_amount, collateral_type).await,
    )
}

#[candid_method(update)]
#[update]
async fn borrow_from_vault(arg: VaultArg) -> Result<SuccessWithFee, ProtocolError> {
    validate_call().await?;
    validate_mode()?;
    // ORACLE-001: refresh this vault's collateral price before minting more debt.
    validate_freshness_for_vault(arg.vault_id).await?;
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
    // ORACLE-001: refresh the (possibly non-ICP) collateral price before minting.
    validate_freshness_for_collateral(collateral_type).await?;
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
    // ORACLE-001: refresh this vault's collateral price before releasing collateral.
    validate_freshness_for_vault(vault_id).await?;
    check_postcondition(rumi_protocol_backend::vault::withdraw_collateral(vault_id).await)
}

#[candid_method(update)]
#[update]
async fn withdraw_partial_collateral(arg: rumi_protocol_backend::vault::VaultArg) -> Result<u64, ProtocolError> {
    validate_call().await?;
    // ORACLE-001: refresh this vault's collateral price before releasing collateral.
    validate_freshness_for_vault(arg.vault_id).await?;
    check_postcondition(rumi_protocol_backend::vault::withdraw_partial_collateral(arg.vault_id, arg.amount).await)
}

#[candid_method(update)]
#[update]
async fn withdraw_and_close_vault(vault_id: u64) -> Result<Option<u64>, ProtocolError> {
    validate_call().await?;
    check_postcondition(rumi_protocol_backend::vault::withdraw_and_close_vault(vault_id).await)
}

/// Compound repay + withdraw + close in a single canister call.
/// Saves one Oisy consent screen for users closing borrowed vaults.
#[candid_method(update)]
#[update]
async fn repay_and_close_vault(arg: VaultArg) -> Result<rumi_protocol_backend::vault::RepayAndCloseSuccess, ProtocolError> {
    validate_call().await?;
    check_postcondition(rumi_protocol_backend::vault::repay_and_close_vault(arg).await)
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
    // P5: native-XRP collateral is liquidated MANUALLY (claim-based) only; automated
    // stability-pool / bot liquidation cannot settle an XrpClaim (would strand the
    // seized XRP and burn SP depositors), so reject native-XRP here.
    if rumi_protocol_backend::vault::vault_is_native_xrp(vault_id) {
        return Err(ProtocolError::GenericError(
            "Native-XRP collateral is liquidated manually (claim-based), not via the stability pool or bot".to_string(),
        ));
    }
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
    // P5: native-XRP collateral is liquidated MANUALLY (claim-based) only; automated
    // stability-pool / bot liquidation cannot settle an XrpClaim (would strand the
    // seized XRP and burn SP depositors), so reject native-XRP here.
    if rumi_protocol_backend::vault::vault_is_native_xrp(vault_id) {
        return Err(ProtocolError::GenericError(
            "Native-XRP collateral is liquidated manually (claim-based), not via the stability pool or bot".to_string(),
        ));
    }
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
    // P5: native-XRP collateral is liquidated MANUALLY (claim-based) only; automated
    // stability-pool / bot liquidation cannot settle an XrpClaim (would strand the
    // seized XRP and burn SP depositors), so reject native-XRP here.
    if rumi_protocol_backend::vault::vault_is_native_xrp(vault_id) {
        return Err(ProtocolError::GenericError(
            "Native-XRP collateral is liquidated manually (claim-based), not via the stability pool or bot".to_string(),
        ));
    }
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
        Ok(success) => {
            // VER-002 (audit 2026-06-05): the full `three_usd_amount_e8s` was
            // pulled above, but the writedown is capped to the vault's current
            // debt, so `success.liquidated_debt` can be < `icusd_debt_covered_e8s`
            // (e.g. the vault shrank between the SP's read and now). Refund the
            // PROPORTIONAL excess 3USD so only the realized portion leaves the
            // SP. The SP records the same realized portion (floor formula) as
            // consumed, so its tracked aggregate and ledger balance both net to
            // exactly `realized_3usd` with no drift.
            if icusd_debt_covered_e8s > 0 && success.liquidated_debt < icusd_debt_covered_e8s {
                let realized_3usd = (three_usd_amount_e8s as u128)
                    .saturating_mul(success.liquidated_debt as u128)
                    / (icusd_debt_covered_e8s as u128);
                let excess = three_usd_amount_e8s.saturating_sub(realized_3usd as u64);
                if excess > 0 {
                    log!(INFO,
                        "[stability_pool_liquidate_with_reserves] vault #{}: writedown capped \
                         ({} of {} icUSD realized); refunding {} excess 3USD to SP",
                        vault_id, success.liquidated_debt, icusd_debt_covered_e8s, excess);
                    refund_3usd_to_stability_pool(three_usd_ledger, caller, excess, vault_id).await;
                }
            }
            Ok(success)
        }
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

/// Legacy entry point used by the layout's at-risk banner and the
/// liquidator hot path. Returns the first `MAX_LIQUIDATABLE_LEGACY_PAGE`
/// liquidatable vaults by ascending `vault_id`. New callers should
/// use `get_liquidatable_vaults_page` for full enumeration.
///
/// Audit Wave 9a (DOS-004): bounds reply size and per-call instructions
/// when a price drop pushes many vaults underwater simultaneously.
#[candid_method(query)]
#[query]
fn get_liquidatable_vaults() -> Vec<CandidVault> {
    // Wave 9a (DOS-004) shares `MAX_VAULTS_LEGACY_PAGE` with the other
    // vault enumeration legacy entry points; the cap is the same.
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
            .take(MAX_VAULTS_LEGACY_PAGE)
            .cloned()
            .map(CandidVault::from)
            .collect::<Vec<CandidVault>>()
    })
}

/// Paginated enumeration of currently-liquidatable vaults, ordered
/// ascending by `vault_id` starting at `start_id`. `limit` is capped
/// at `MAX_VAULTS_PAGE_LIMIT` (the same cap as `get_vaults_page`).
/// `next_start_id` carries the cursor for the next page, or `None`
/// once the scan has reached the end of the vault map.
///
/// Audit Wave 9a (DOS-004): pairs with `get_vaults_page` to give the
/// liquidations UI bounded-cost paging at any TVL.
#[candid_method(query)]
#[query]
fn get_liquidatable_vaults_page(start_id: u64, limit: u64) -> VaultsPageResponse {
    let limit = limit.min(MAX_VAULTS_PAGE_LIMIT) as usize;

    read_state(|s| {
        let dummy_rate = s.last_icp_rate.unwrap_or(UsdIcp::from(dec!(0.0)));

        let mut vaults = Vec::with_capacity(limit);
        let mut next_start_id: Option<u64> = None;
        for (id, vault) in s.vault_id_to_vaults.range(start_id..) {
            let ratio = rumi_protocol_backend::compute_collateral_ratio(vault, dummy_rate, s);
            if ratio == Ratio::from(Decimal::ZERO) {
                continue;
            }
            if ratio < s.get_min_liquidation_ratio_for(&vault.collateral_type) {
                if vaults.len() == limit {
                    next_start_id = Some(*id);
                    break;
                }
                vaults.push(CandidVault::from(vault.clone()));
            }
        }
        VaultsPageResponse { vaults, next_start_id }
    })
}

/// Legacy bulk vault enumeration. Returns the first
/// `MAX_VAULTS_LEGACY_PAGE` vaults by ascending `vault_id`. New
/// callers should use `get_vaults_page` for full enumeration.
///
/// Audit Wave 9a (DOS-004): the unbounded clone+encode path scaled
/// linearly with `vault_id_to_vaults.len()` — the cap keeps a single
/// call inside the cycle budget at any TVL.
#[candid_method(query)]
#[query]
fn get_all_vaults() -> Vec<CandidVault> {
    read_state(|s| {
        s.vault_id_to_vaults
            .values()
            .take(MAX_VAULTS_LEGACY_PAGE)
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

// ─── Native XRP rail (experimental/dormant chains::xrp) — P1 wiring ───────────
// HTTPS-outcall transforms for the rippled JSON-RPC reads/submit. Each delegates
// to the consensus-safe reducer in `chains::xrp::xrp_rpc` (HTTP status pinned, body
// reduced to only the consumed fields) so the rail's outcalls resolve them by
// name. Registered as plain queries, like `coingecko_transform`.

#[query]
fn xrp_transform_account(
    args: ic_cdk::api::management_canister::http_request::TransformArgs,
) -> ic_cdk::api::management_canister::http_request::HttpResponse {
    rumi_protocol_backend::chains::xrp::xrp_rpc::transform_account(args)
}

#[query]
fn xrp_transform_server(
    args: ic_cdk::api::management_canister::http_request::TransformArgs,
) -> ic_cdk::api::management_canister::http_request::HttpResponse {
    rumi_protocol_backend::chains::xrp::xrp_rpc::transform_server(args)
}

#[query]
fn xrp_transform_submit(
    args: ic_cdk::api::management_canister::http_request::TransformArgs,
) -> ic_cdk::api::management_canister::http_request::HttpResponse {
    rumi_protocol_backend::chains::xrp::xrp_rpc::transform_submit(args)
}

#[query]
fn xrp_transform_tx(
    args: ic_cdk::api::management_canister::http_request::TransformArgs,
) -> ic_cdk::api::management_canister::http_request::HttpResponse {
    rumi_protocol_backend::chains::xrp::xrp_rpc::transform_tx(args)
}

/// Developer-only guard for the experimental XRP observability endpoints below.
fn xrp_require_developer() -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    if read_state(|s| s.developer_principal == caller) {
        Ok(())
    } else {
        Err(ProtocolError::GenericError(
            "Only the developer may call the experimental XRP endpoints".to_string(),
        ))
    }
}

/// P1 observability (developer-gated, no funds touched): derive the protocol's XRP
/// settlement (custody) classic address via threshold Ed25519 and return it, so an
/// operator can see it on XRPL testnet. The chains::xrp module is dormant; this
/// only derives a public address (derivation is free and idempotent).
#[update]
async fn xrp_settlement_address() -> Result<String, ProtocolError> {
    xrp_require_developer()?;
    let path = rumi_protocol_backend::chains::xrp::ted25519::settlement_derivation_path(
        rumi_protocol_backend::chains::xrp::XRP_CHAIN_ID,
    );
    rumi_protocol_backend::chains::xrp::ted25519::derive_xrp_address(path)
        .await
        .map(|(_pubkey, addr)| addr)
        .map_err(|e| ProtocolError::GenericError(format!("xrp settlement derive failed: {e}")))
}

/// P1 observability (developer-gated): derive the per-vault XRP custody address for
/// `(user, nonce)` via threshold Ed25519 — the address a future XRP vault would
/// publish for deposits. No state is written (derivation is idempotent).
#[update]
async fn xrp_custody_address(user: Principal, nonce: u64) -> Result<String, ProtocolError> {
    xrp_require_developer()?;
    let path = rumi_protocol_backend::chains::xrp::ted25519::custody_derivation_path(
        rumi_protocol_backend::chains::xrp::XRP_CHAIN_ID,
        user,
        nonce,
    );
    rumi_protocol_backend::chains::xrp::ted25519::derive_xrp_address(path)
        .await
        .map(|(_pubkey, addr)| addr)
        .map_err(|e| ProtocolError::GenericError(format!("xrp custody derive failed: {e}")))
}

/// P1 observability (developer-gated): read an XRP account's balance in drops from
/// a public rippled node via the rail's consensus-retry-wrapped outcall. Confirms
/// the transform shims above resolve and the outcall path works end to end on
/// testnet. Returns 0 for an unfunded account.
#[update]
async fn xrp_balance(address: String) -> Result<u64, ProtocolError> {
    xrp_require_developer()?;
    let acct = rumi_protocol_backend::chains::xrp::xrp_rpc::fetch_account_info(&address)
        .await
        .map_err(|e| ProtocolError::GenericError(format!("xrp account_info failed: {e}")))?;
    u64::try_from(acct.balance_drops)
        .map_err(|_| ProtocolError::GenericError("xrp balance exceeds u64 drops".to_string()))
}

/// P3 (native-XRP collateral): open a vault in open-then-verify staging and return
/// its XRPL custody address to fund. No collateral credited, no icUSD minted.
/// Errors until native-XRP collateral is registered (P5).
#[update]
async fn open_xrp_vault() -> Result<rumi_protocol_backend::vault::XrpVaultOpenInfo, ProtocolError> {
    validate_call().await?;
    check_postcondition(rumi_protocol_backend::vault::open_xrp_vault().await)
}

/// P3 (native-XRP collateral): verify the deposit to a vault's custody address and
/// credit it as collateral (creating the Vault with zero debt). Owner-only,
/// idempotent. Borrow icUSD afterwards via the normal `borrow_from_vault`.
#[update]
async fn confirm_xrp_deposit(vault_id: u64) -> Result<u64, ProtocolError> {
    validate_call().await?;
    check_postcondition(rumi_protocol_backend::vault::confirm_xrp_deposit(vault_id).await)
}

/// P4 (native-XRP collateral): settle an XRP collateral claim by signing +
/// submitting a Payment from the source vault's custody address to `destination`
/// (claimant bears the fee). Claimant-only. Returns the local tx hash.
#[update]
async fn settle_xrp_claim(claim_id: u64, destination: String) -> Result<String, ProtocolError> {
    validate_call().await?;
    check_postcondition(
        rumi_protocol_backend::vault::settle_xrp_claim(claim_id, destination).await,
    )
}

/// P5 (native-XRP collateral): register XRP as a collateral (developer-gated). XRP
/// has no IC ledger, so decimals (6 / drops) and fee (0) are NOT queried;
/// custody_kind = NativeXrp routes deposits/payouts through the chains::xrp rail +
/// the XrpClaim model. Params: 150% borrow / 133% liquidation / 12% penalty / $200
/// debt ceiling; borrowing-fee + interest inherited from ICP. Calling this is the
/// deliberate act that ACTIVATES the native-XRP rail — do NOT call on mainnet
/// before the security audit.
#[update]
async fn register_xrp_collateral() -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    if !read_state(|s| s.developer_principal == caller) {
        return Err(ProtocolError::GenericError(
            "Only the developer can register XRP collateral".to_string(),
        ));
    }
    let xrp_ct = rumi_protocol_backend::state::xrp_collateral_principal();
    if read_state(|s| s.collateral_configs.contains_key(&xrp_ct)) {
        return Err(ProtocolError::GenericError(
            "XRP collateral already registered".to_string(),
        ));
    }

    // Inherit ICP's borrowing-fee + interest base ("same as ICP"); the dynamic
    // curves are global / inherited (rate_curve = None inside the config builder).
    let (icp_borrowing_fee, icp_interest_apr, recovery_mult) = read_state(|s| {
        let icp_ct = s.icp_collateral_type();
        let icp = s.get_collateral_config(&icp_ct);
        (
            icp.map(|c| c.borrowing_fee).unwrap_or(Ratio::from_f64(0.0)),
            icp.map(|c| c.interest_rate_apr).unwrap_or(Ratio::from_f64(0.0)),
            s.recovery_cr_multiplier,
        )
    });

    let config = rumi_protocol_backend::state::xrp_collateral_config(
        icp_borrowing_fee,
        icp_interest_apr,
        recovery_mult,
    );

    mutate_state(|s| {
        event::record_add_collateral_type(s, xrp_ct, config);
    });
    // Price-fetch timer for XRP (XRC base_asset "XRP"), like any non-ICP collateral.
    rumi_protocol_backend::xrc::register_collateral_price_timer(xrp_ct);
    log!(
        INFO,
        "[register_xrp_collateral] Registered native-XRP collateral {} (150/133/12, $200 ceiling)",
        xrp_ct
    );
    Ok(())
}

/// P5 observability: all native-XRP vaults still awaiting their on-chain deposit
/// (created by `open_xrp_vault`, removed by `confirm_xrp_deposit`). Developer-gated
/// read (returns empty for non-developers) — ops + the e2e harness use it to see
/// the deposit-staging queue. Returns `(reserved_vault_id, pending)` pairs.
#[candid_method(query)]
#[query]
fn get_xrp_pending_deposits() -> Vec<(u64, rumi_protocol_backend::state::XrpPendingDeposit)> {
    let caller = ic_cdk::caller();
    read_state(|s| {
        if s.developer_principal != caller {
            return Vec::new();
        }
        s.xrp_pending_deposits.iter().map(|(id, d)| (*id, d.clone())).collect()
    })
}

/// P5 observability: all outstanding native-XRP claims (XRP owed out of a custody
/// address — created on withdraw/close/liquidation, removed once `settle_xrp_claim`
/// confirms the Payment validated). Developer-gated read (empty for non-developers).
/// Ops + the frontend use this to know which claims still need settling.
#[candid_method(query)]
#[query]
fn get_xrp_claims() -> Vec<(u64, rumi_protocol_backend::state::XrpClaim)> {
    let caller = ic_cdk::caller();
    read_state(|s| {
        if s.developer_principal != caller {
            return Vec::new();
        }
        s.xrp_claims.iter().map(|(id, c)| (*id, c.clone())).collect()
    })
}

/// P5 (frontend): the caller's OWN native-XRP pending deposits (those whose vault
/// the caller opened). Unlike `get_xrp_pending_deposits` (developer-gated, all
/// users), this is the per-user read the UI uses to show "send XRP to this custody
/// address" for a deposit it is still waiting on. Returns `(reserved_vault_id, pending)`.
#[candid_method(query)]
#[query]
fn get_my_xrp_pending_deposits() -> Vec<(u64, rumi_protocol_backend::state::XrpPendingDeposit)> {
    let caller = ic_cdk::caller();
    read_state(|s| {
        s.xrp_pending_deposits
            .iter()
            .filter(|(_, d)| d.owner == caller)
            .map(|(id, d)| (*id, d.clone()))
            .collect()
    })
}

/// P5 (frontend): the caller's OWN outstanding native-XRP claims (those the caller
/// is the `claimant` of — XRP owed to them from a withdraw/close/liquidation). The UI
/// uses this to list claims the user can `settle_xrp_claim` to an XRPL address.
#[candid_method(query)]
#[query]
fn get_my_xrp_claims() -> Vec<(u64, rumi_protocol_backend::state::XrpClaim)> {
    let caller = ic_cdk::caller();
    read_state(|s| {
        s.xrp_claims
            .iter()
            .filter(|(_, c)| c.claimant == caller)
            .map(|(id, c)| (*id, c.clone()))
            .collect()
    })
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
    // ASYNC-003: serialize per-caller so two concurrent manual recoveries cannot
    // both pay out the same pending entry (the entry is only removed AFTER the
    // await below). Defense-in-depth on top of the nonce-dedup fix.
    let _guard = rumi_protocol_backend::guard::GuardPrincipal::new(caller, "recover_pending_transfer")?;

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

        // ASYNC-003: pay with the entry's PERSISTED op_nonce (not a fresh one) so
        // this manual recovery shares the ledger dedup tuple (created_at_time +
        // memo) with process_pending_transfer's timer retry. transfer_idempotent
        // converts the ledger's Duplicate response to Ok, so a concurrent timer
        // retry and this manual recovery can never double-pay the owner.
        let result = management::transfer_collateral_with_nonce(
            (transfer.margin - transfer_fee).to_u64(),
            transfer.owner,
            ledger,
            transfer.op_nonce,
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

/// Tolerance (in basis points) added to per-collateral
/// `min_liquidation_ratio` when the bot calls `bot_claim_liquidation`.
/// Closes the scan→claim TOCTOU window. See `set_bot_cr_tolerance_bps`.
#[candid_method(update)]
#[update]
async fn set_bot_cr_tolerance_bps(bps: u64) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set the bot CR tolerance".to_string(),
        ));
    }
    let max = rumi_protocol_backend::MAX_BOT_CR_TOLERANCE_BPS;
    if bps > max {
        return Err(ProtocolError::GenericError(format!(
            "Bot CR tolerance {} bps exceeds maximum {} bps",
            bps, max
        )));
    }
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_bot_cr_tolerance_bps(s, bps);
    });
    log!(
        INFO,
        "[set_bot_cr_tolerance_bps] Bot CR tolerance set to: {} bps ({:.2}% above min_liquidation_ratio)",
        bps,
        (bps as f64) / 100.0
    );
    Ok(())
}

#[candid_method(query)]
#[query]
fn get_bot_cr_tolerance_bps() -> u64 {
    read_state(|s| s.bot_cr_tolerance_bps)
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
    // ORC-001 (audit 2026-06-09): mirror the gates every manual/SP liquidation
    // entry enforces. Without the per-vault freshness gate the bot computes the
    // CR gate and seizure from a cached non-ICP price with no staleness
    // ceiling (the VER-001 fail-open class); without the freeze gate the bot
    // keeps claiming through an admin liquidation halt. Dormant while the bot
    // allowlist is ICP-only, live the moment a non-ICP collateral is added.
    validate_liquidation_not_frozen()?;
    validate_freshness_for_vault(vault_id).await?;
    // P5: native-XRP collateral is liquidated MANUALLY (claim-based) only; automated
    // stability-pool / bot liquidation cannot settle an XrpClaim (would strand the
    // seized XRP and burn SP depositors), so reject native-XRP here.
    if rumi_protocol_backend::vault::vault_is_native_xrp(vault_id) {
        return Err(ProtocolError::GenericError(
            "Native-XRP collateral is liquidated manually (claim-based), not via the stability pool or bot".to_string(),
        ));
    }
    let caller = ic_cdk::api::caller();

    let is_bot = read_state(|s| {
        s.liquidation_bot_principal.map_or(false, |bp| bp == caller)
    });
    if !is_bot {
        return Err(ProtocolError::GenericError(
            "Caller is not the registered liquidation bot canister".to_string(),
        ));
    }

    // BK-001/002: hold the per-vault liquidation lock across the claim so the
    // bot's collateral-seizing claim cannot interleave with an in-flight manual
    // or SP liquidation of the same vault (which would otherwise both seize from
    // the shared collateral pool). The persistent `bot_processing` flag set
    // below covers the longer claim->confirm window; this guard covers the claim
    // message itself. Released on return (incl. continuation-trap via cleanup).
    let _vault_liq_guard = rumi_protocol_backend::guard::VaultLiquidationGuard::new(vault_id)?;

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
        // Apply scan→claim TOCTOU tolerance. The scan in `check_vaults`
        // flagged this vault as below `min_ratio`; an XRC tick between
        // that scan and this call can recompute CR slightly above
        // `min_ratio`. Allowing up to `min_ratio + tolerance` here
        // closes the race without widening the strict threshold the
        // manual liquidation paths enforce. The `actual > 0` guard
        // below catches the Recovery-mode case where `min_ratio +
        // tolerance` exceeds the partial-cap target CR.
        let bot_max_ratio = s.get_bot_claim_max_ratio_for(&vault.collateral_type);

        if ratio >= bot_max_ratio {
            return Err(ProtocolError::GenericError(format!(
                "Vault #{} is not liquidatable (CR {:.2}% >= {:.2}%, base {:.2}% + tolerance {} bps)",
                vault_id,
                ratio.to_f64() * 100.0,
                bot_max_ratio.to_f64() * 100.0,
                min_ratio.to_f64() * 100.0,
                s.bot_cr_tolerance_bps
            )));
        }

        let actual = s.compute_partial_liquidation_cap(vault, collateral_price_usd);

        // Defense-in-depth: with the tolerance applied, `ratio` may sit
        // above the partial-cap target CR (`borrow_threshold_ratio`),
        // in which case `compute_partial_liquidation_cap` returns 0.
        // Reject explicitly rather than claiming a 0-debt liquidation,
        // which would deduct nothing from the budget and seize 0
        // collateral — pointless work that would still write a noisy
        // BotClaim record.
        if actual.to_u64() == 0 {
            return Err(ProtocolError::GenericError(format!(
                "Vault #{} has no liquidatable debt at current CR {:.2}% (target CR {:.2}%)",
                vault_id,
                ratio.to_f64() * 100.0,
                s.get_min_collateral_ratio_for(&vault.collateral_type).to_f64() * 100.0
            )));
        }

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
            // AR-B-001 (audit 2026-06-09): saturate the debt write-down. A
            // non-saturating `-=` traps if anything reduced the vault's debt
            // during the claim->confirm window, permanently sticking the vault
            // at `bot_processing = true` with the bot's collateral already
            // paid. The redemption skip + user-op rejection make that window
            // race-free today; saturating keeps a residual drift from ever
            // bricking the vault (it degrades to under-reduction instead).
            vault.borrowed_icusd_amount =
                vault.borrowed_icusd_amount.saturating_sub(ICUSD::new(claim.debt_amount));
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
        // Shared drain rule (see state::cleanup_if_drained): a bot confirm
        // normally only reduces debt+collateral (re-key the CR entry), but if
        // the write-down emptied the vault it must be removed like every
        // other PartialLiquidateVault path, or replay diverges.
        if s.cleanup_if_drained(vault_id) {
            log!(INFO, "[bot_confirm_liquidation] Vault #{} fully liquidated — removed", vault_id);
        }
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

/// Vault IDs with an open `BotClaim` on the protocol side.
///
/// Used by the explorer to reconcile the liquidation_bot's
/// `get_stuck_liquidations` (a permanent history log) against the protocol's
/// live claim set. A record can be `TransferFailed`/`ConfirmFailed` in the
/// bot's log forever, but if the matching vault is no longer in `bot_claims`
/// (e.g. resolved by Wave-11 BOT-001 auto-cancel or `admin_resolve_stuck_claim`),
/// the explorer should not flag it as awaiting admin action.
#[candid_method(query)]
#[query]
fn get_bot_claim_vault_ids() -> Vec<u64> {
    read_state(|s| s.bot_claims.keys().copied().collect())
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
                // AR-B-001 (audit 2026-06-09): saturate, same as
                // bot_confirm_liquidation. The non-saturating `-=` made this
                // recovery endpoint trap on exactly the stuck state it exists
                // to resolve (debt already reduced below the claim amount).
                vault.borrowed_icusd_amount =
                    vault.borrowed_icusd_amount.saturating_sub(ICUSD::new(claim.debt_amount));
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
    // Wave-9 RED-003: reserve redemption walks the same vault cr-index as
    // redeem_collateral on its spillover branch, so the same ReadOnly gate
    // applies. See main.rs::redeem_collateral for the rationale.
    validate_mode()?;
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

/// Wave-9c DOS-005: tune the alert-band width (in bps) used by
/// `check_vaults` to bound the sorted-troves walk on band-only ticks.
/// Default 1000 bps (10% headroom above the worst per-collateral
/// `min_liquidation_ratio`). 0 disables the band (only vaults strictly
/// below the worst floor are visited). Wider values trade cycle savings
/// for safety margin against cross-collateral CR-key drift.
#[candid_method(update)]
#[update]
async fn set_check_vaults_alert_band_bps(new_band_bps: u64) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set check_vaults alert band".to_string(),
        ));
    }
    mutate_state(|s| s.set_check_vaults_alert_band_bps(new_band_bps));
    log!(
        INFO,
        "[set_check_vaults_alert_band_bps] Alert band set to: {} bps ({:.2}% headroom)",
        new_band_bps,
        (new_band_bps as f64) / 100.0
    );
    Ok(())
}

/// Wave-14a CDP-14: tune the XRC source-count floor. Default is 3 (set
/// via `xrc::MIN_XRC_SOURCES`); 0 disables the gate entirely. Used by ops
/// to slacken the requirement if XRC's aggregation degrades industry-
/// wide, or to tighten it when more depth is available.
#[candid_method(update)]
#[update]
async fn set_min_xrc_sources_used(value: u32) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set the XRC source-count floor".to_string(),
        ));
    }
    mutate_state(|s| s.min_xrc_sources_used = value);
    log!(
        INFO,
        "[set_min_xrc_sources_used] XRC source-count floor set to: {} ({})",
        value,
        if value == 0 {
            "gate disabled"
        } else {
            "gate active"
        }
    );
    Ok(())
}

/// Wave-14b CDP-12 follow-up: tune the Timer A (XRC fetch) interval in
/// seconds. Default 300. Re-registers the timer in place; no upgrade
/// required for the change to take effect.
///
/// Trade-offs: lowering this means more XRC calls per minute (~1B cycles
/// each for the ICP/USD pair). Most callers hit the cached price via the
/// PRICE_FRESHNESS_THRESHOLD_NANOS gate anyway, so the timer is mainly a
/// defense-in-depth refresh; 60s would 5x XRC cycle cost. Raising means
/// staler cached prices, which the staleness gate (10 min hard) and the
/// CDP-01 consecutive-failure breaker still bound.
#[candid_method(update)]
#[update]
async fn set_xrc_fetch_interval_secs(secs: u64) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set the XRC fetch interval".to_string(),
        ));
    }
    if secs == 0 {
        return Err(ProtocolError::GenericError(
            "XRC fetch interval must be > 0 (a 0s timer would saturate the canister)".to_string(),
        ));
    }
    mutate_state(|s| s.xrc_fetch_interval_secs = secs);
    register_xrc_fetch_timer();
    log!(INFO, "[set_xrc_fetch_interval_secs] Timer A interval set to {}s", secs);
    Ok(())
}

/// Wave-14b CDP-12 follow-up: tune the Timer B (interest accrual +
/// treasury drains) interval in seconds. Default 60. Re-registers in
/// place.
///
/// Trade-offs: cheap in cycles (pure in-memory work + 3 short ICRC calls).
/// Lowering tightens interest precision and treasury drain cadence at
/// negligible cost. Raising makes interest accrual lumpier but doesn't
/// break anything.
#[candid_method(update)]
#[update]
async fn set_interest_treasury_tick_interval_secs(secs: u64) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set the interest/treasury tick interval".to_string(),
        ));
    }
    if secs == 0 {
        return Err(ProtocolError::GenericError(
            "Interest/treasury tick interval must be > 0".to_string(),
        ));
    }
    mutate_state(|s| s.interest_treasury_tick_interval_secs = secs);
    register_interest_treasury_timer();
    log!(INFO, "[set_interest_treasury_tick_interval_secs] Timer B interval set to {}s", secs);
    Ok(())
}

/// Wave-14b CDP-12 follow-up: tune the Timer C (vault health sweep +
/// aggregate snapshot refresh) interval in seconds. Default 300.
/// Re-registers in place.
///
/// Trade-offs: this is the liquidation-latency knob. Lowering means
/// liquidations dispatch faster (bot/SP get notified within seconds of
/// a vault becoming unhealthy) at the cost of running the Wave-9c
/// sharded `check_vaults` walk more often. Raising slows liquidation
/// response but is still bounded by the 10-minute oracle staleness gate.
#[candid_method(update)]
#[update]
async fn set_vault_check_tick_interval_secs(secs: u64) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set the vault check tick interval".to_string(),
        ));
    }
    if secs == 0 {
        return Err(ProtocolError::GenericError(
            "Vault check tick interval must be > 0".to_string(),
        ));
    }
    mutate_state(|s| s.vault_check_tick_interval_secs = secs);
    register_vault_check_timer();
    log!(INFO, "[set_vault_check_tick_interval_secs] Timer C interval set to {}s", secs);
    Ok(())
}

/// Phase 1b Task 15: tune the Timer D (Monad outbound settlement fan-out)
/// interval in seconds. Default 30. Re-registers in place.
///
/// Rejects `secs == 0` (belt-and-suspenders with the hard floor in
/// `register_settlement_timer`, which would coerce a 0 to 30 anyway). Lowering
/// makes mint/withdrawal settlement land faster (one op per chain per tick) at
/// the cost of more frequent EVM RPC polling; raising slows settlement
/// throughput. The per-chain re-entrancy guard means a tick that out-runs the
/// interval is simply skipped, so a low interval never stacks concurrent runs.
#[candid_method(update)]
#[update]
async fn set_settlement_tick_interval_secs(secs: u64) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set the settlement tick interval".to_string(),
        ));
    }
    if secs == 0 {
        return Err(ProtocolError::GenericError(
            "Settlement tick interval must be > 0".to_string(),
        ));
    }
    mutate_state(|s| s.settlement_tick_interval_secs = secs);
    register_settlement_timer();
    log!(INFO, "[set_settlement_tick_interval_secs] Timer D interval set to {}s", secs);
    Ok(())
}

/// Task 12: tune the foreign-chain interest-harvest interval (seconds). Default
/// ~1 year (effectively OFF). Re-registers the timer in place. Rejects `secs ==
/// 0` (the register fn also floors a 0 to the 1-year default). Developer-gated.
#[candid_method(update)]
#[update]
async fn set_chain_interest_tick_interval_secs(secs: u64) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    if read_state(|s| s.developer_principal != caller) {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set the chain interest tick interval".to_string(),
        ));
    }
    if secs == 0 {
        return Err(ProtocolError::GenericError(
            "Chain interest tick interval must be > 0".to_string(),
        ));
    }
    mutate_state(|s| s.chain_interest_tick_interval_secs = secs);
    register_chain_interest_timer();
    log!(INFO, "[set_chain_interest_tick_interval_secs] interest harvest interval set to {}s", secs);
    Ok(())
}

/// Task 12: tune the interest-realization dust floor (e8s) — accrued interest
/// below this is not minted (its gas would dwarf the interest). Default 0.01
/// icUSD. Developer-gated.
#[candid_method(update)]
#[update]
async fn set_chain_interest_min_realize_e8s(e8s: u128) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    if read_state(|s| s.developer_principal != caller) {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set the chain interest dust floor".to_string(),
        ));
    }
    mutate_state(|s| s.chain_interest_min_realize_e8s = e8s);
    log!(INFO, "[set_chain_interest_min_realize_e8s] interest dust floor set to {} e8s", e8s);
    Ok(())
}

/// Pure validation for `set_chains_ecdsa_key_name`: the name must be a supported
/// IC threshold key (`test_key_1` or `key_1`), and the key may change ONLY while
/// no chain vault exists — switching the key re-derives every per-vault custody
/// address, which would orphan already-deposited collateral.
fn validate_ecdsa_key_change(name: &str, has_chain_vaults: bool) -> Result<(), ProtocolError> {
    if name != "test_key_1" && name != "key_1" {
        return Err(ProtocolError::ChainAdmin(format!(
            "unsupported ecdsa key name '{name}' (expected test_key_1 or key_1)"
        )));
    }
    if has_chain_vaults {
        return Err(ProtocolError::ChainAdmin(
            "cannot change the chains ECDSA key while chain vaults exist (it re-derives + orphans every per-vault custody address)".to_string(),
        ));
    }
    Ok(())
}

/// Set the production tECDSA key name for the EVM chains rail (developer-gated).
/// Intended for a FRESH production canister: set `key_1` BEFORE registering any
/// chain, so all custody/settlement/treasury addresses derive from the
/// production threshold key. Rejected once any chain vault exists (orphan guard).
/// kvg63 staging keeps the default `test_key_1`.
#[candid_method(update)]
#[update]
fn set_chains_ecdsa_key_name(name: String) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    if read_state(|s| s.developer_principal != caller) {
        return Err(ProtocolError::ChainAdmin("not developer".into()));
    }
    let has_vaults = read_state(|s| !s.multi_chain.chain_vaults.is_empty());
    validate_ecdsa_key_change(&name, has_vaults)?;
    mutate_state(|s| s.chains_ecdsa_key_name = name.clone());
    log!(INFO, "[set_chains_ecdsa_key_name] chains EVM tECDSA key set to {}", name);
    Ok(())
}

/// The current chains EVM tECDSA key name (`test_key_1` | `key_1`). Public read
/// (non-sensitive config; the derived addresses are already public).
#[candid_method(query)]
#[query]
fn get_chains_ecdsa_key_name() -> String {
    read_state(|s| s.chains_ecdsa_key_name.clone())
}

/// Task 12: manually trigger one interest harvest for `chain` (developer-gated),
/// for testing/ops independent of the off-by-default timer. Resolves the
/// interest-treasury address + APR, enqueues an `InterestMint` for every
/// eligible vault, and returns the number enqueued. The settlement worker then
/// mints/confirms them on-chain.
#[candid_method(update)]
#[update]
async fn harvest_chain_interest(
    chain: rumi_protocol_backend::chains::config::ChainId,
) -> Result<u64, ProtocolError> {
    let caller = ic_cdk::caller();
    if read_state(|s| s.developer_principal != caller) {
        return Err(ProtocolError::ChainAdmin("not developer".into()));
    }
    let now = ic_cdk::api::time();
    harvest_one_chain_interest(chain, now)
        .await
        .map_err(ProtocolError::ChainAdmin)
}

/// Task 12: derive the per-chain interest-treasury (revenue) address — the
/// tECDSA-derived EVM address that receives minted interest, distinct from the
/// settlement (minter) address. Developer-gated (derivation costs cycles + a
/// signing-subnet call). Used by ops + tests to learn where revenue accrues.
#[candid_method(update)]
#[update]
async fn get_chain_interest_treasury_address(
    chain: rumi_protocol_backend::chains::config::ChainId,
) -> Result<String, ProtocolError> {
    let caller = ic_cdk::caller();
    if read_state(|s| s.developer_principal != caller) {
        return Err(ProtocolError::ChainAdmin("not developer".into()));
    }
    rumi_protocol_backend::chains::evm::tecdsa::cached_interest_treasury_address(chain)
        .await
        .map(|(_path, addr)| addr)
        .map_err(|e| ProtocolError::ChainAdmin(format!("derive: {e}")))
}

/// Increment 3: derive the per-chain liquidation-RESERVE address — the
/// tECDSA-derived EVM address bot-liquidation swaps settle USDC into (the PSM
/// sink). Developer-gated. The operator uses this to FIND + FUND the reserve
/// address and to verify books==custody before bridging (spec §4.8/§5.5).
#[candid_method(update)]
#[update]
async fn get_chain_reserve_address(
    chain: rumi_protocol_backend::chains::config::ChainId,
) -> Result<String, ProtocolError> {
    let caller = ic_cdk::caller();
    if read_state(|s| s.developer_principal != caller) {
        return Err(ProtocolError::ChainAdmin("not developer".into()));
    }
    rumi_protocol_backend::chains::evm::tecdsa::cached_reserve_address(chain)
        .await
        .map(|(_path, addr)| addr)
        .map_err(|e| ProtocolError::ChainAdmin(format!("derive reserve: {e}")))
}

/// Phase 1b Task 15: tune the Monad inbound observer fan-out interval in
/// seconds. Default 30. Re-registers in place.
///
/// Rejects `secs == 0` (belt-and-suspenders with the hard floor in
/// `register_observer_timer`). Lowering tightens deposit-detection and
/// burn-observation latency at the cost of more frequent EVM RPC polling;
/// raising slows it. Same per-chain re-entrancy-guard skip behavior as the
/// settlement timer.
#[candid_method(update)]
#[update]
async fn set_observer_tick_interval_secs(secs: u64) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set the observer tick interval".to_string(),
        ));
    }
    if secs == 0 {
        return Err(ProtocolError::GenericError(
            "Observer tick interval must be > 0".to_string(),
        ));
    }
    mutate_state(|s| s.observer_tick_interval_secs = secs);
    register_observer_timer();
    log!(INFO, "[set_observer_tick_interval_secs] Observer interval set to {}s", secs);
    Ok(())
}

// ─── Solana M1 read-seam endpoints (developer-gated) ─────────────────────────

/// M1 read-seam probe: derive and return the Solana settlement (mint-authority)
/// address. Developer-gated. Exercises threshold Ed25519 on devnet/staging.
#[candid_method(update)]
#[update]
async fn solana_settlement_address() -> Result<String, ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can derive the Solana settlement address".to_string(),
        ));
    }
    use rumi_protocol_backend::chains::solana::{config::SOLANA_CHAIN_ID, ted25519};
    let path = ted25519::settlement_derivation_path(SOLANA_CHAIN_ID);
    let (_pk, addr) = ted25519::derive_solana_address(path)
        .await
        .map_err(ProtocolError::GenericError)?;
    Ok(addr)
}

/// M1 read-seam probe: read a SOL balance (lamports) via the SOL RPC canister.
#[candid_method(update)]
#[update]
async fn solana_get_balance(pubkey: String) -> Result<u64, ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can call solana_get_balance".to_string(),
        ));
    }
    use rumi_protocol_backend::chains::solana::{sol_rpc, ted25519};
    if !ted25519::is_valid_solana_address(&pubkey) {
        return Err(ProtocolError::GenericError(format!(
            "invalid Solana address: {pubkey}"
        )));
    }
    sol_rpc::get_balance(&pubkey)
        .await
        .map_err(ProtocolError::GenericError)
}

/// M1 read-seam probe: read the registered icUSD SPL mint's on-chain supply.
#[candid_method(update)]
#[update]
async fn solana_get_mint_supply() -> Result<u64, ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can call solana_get_mint_supply".to_string(),
        ));
    }
    use rumi_protocol_backend::chains::solana::{config::SOLANA_CHAIN_ID, sol_rpc};
    let mint = read_state(|s| s.multi_chain.chain_contracts.get(&SOLANA_CHAIN_ID).cloned())
        .ok_or_else(|| ProtocolError::GenericError("Solana icUSD mint not set".to_string()))?;
    sol_rpc::get_mint_supply(&mint)
        .await
        .map_err(ProtocolError::GenericError)
}

/// M2 sign-seam probe: build, threshold-Ed25519 sign, and return the legacy wire
/// bytes of a System Transfer from the Solana settlement address to `to` for
/// `lamports`. Developer-gated. Uses a dummy (all-zero) blockhash, so the bytes
/// are NOT broadcastable (sign-validity is what we prove here); the real broadcast
/// path fetches a fresh blockhash and calls `sol_rpc::send_transaction`. The
/// returned bytes are `[compact-u16 sig count][64-byte sig][serialized message]`.
#[candid_method(update)]
#[update]
async fn solana_sign_test_transfer(to: String, lamports: u64) -> Result<Vec<u8>, ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can call solana_sign_test_transfer".to_string(),
        ));
    }
    use rumi_protocol_backend::chains::solana::{config::SOLANA_CHAIN_ID, ted25519, tx};
    use solana_message::Hash;
    use solana_pubkey::Pubkey;

    // Decode the recipient once: this both validates (32-byte base58) and yields
    // the raw bytes. A non-32-byte or non-base58 `to` still returns the same
    // "invalid Solana address" rejection as the prior explicit validity check.
    let to_arr = ted25519::decode_solana_address(&to)
        .map_err(|_| ProtocolError::GenericError(format!("invalid Solana address: {to}")))?;
    let to_pk = Pubkey::new_from_array(to_arr);

    // Derive the settlement (mint-authority) signer address + its path.
    let from_path = ted25519::settlement_derivation_path(SOLANA_CHAIN_ID);
    let (from_pubkey, _from_addr) = ted25519::derive_solana_address(from_path.clone())
        .await
        .map_err(ProtocolError::GenericError)?;

    // Dummy blockhash: sign-validity does not depend on blockhash freshness.
    let blockhash = Hash::new_from_array([0u8; 32]);

    tx::sign_transfer(from_path, &from_pubkey, &to_pk, lamports, blockhash)
        .await
        .map_err(ProtocolError::GenericError)
}

/// M2 durable-nonce bootstrap: idempotently create + initialize the settlement
/// key's durable nonce account on Solana devnet. Developer-gated; the operator
/// runs this once per settlement key. If the nonce account already holds an
/// Initialized durable nonce, this is a no-op (returns Ok). Otherwise it obtains a
/// real recent blockhash, builds the 2-instruction create+initialize transaction,
/// multi-signs it (fee payer + new nonce account, both threshold-Ed25519), and
/// broadcasts it. Subsequent settlement transactions reference the durable nonce
/// so build->sign(slow)->broadcast stays valid across async gaps.
///
/// `blockhash` (playbook #4): `getLatestBlockhash` changes every slot, so the
/// DFINITY sol-rpc canister's multi-provider consensus almost never agrees on it
/// and chronically returns `#Inconsistent` - which the canister-side auto-fetch
/// rejects. So on real devnet/mainnet the operator obtains a fresh finalized
/// blockhash out-of-band (e.g. `solana blockhash`, or a single-provider
/// `getLatestBlockhash`) and passes it here as a 32-byte base58 string; it is fed
/// straight into the create-nonce tx, bypassing the broken consensus fetch. Pass it
/// promptly: blockhashes expire (~60s). The no-arg/`None` path auto-fetches and
/// only works where multi-provider consensus on `getLatestBlockhash` is possible
/// (the PocketIC mock / a consensus-capable environment).
#[candid_method(update)]
#[update]
async fn solana_bootstrap_nonce(blockhash: Option<String>) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can bootstrap the Solana nonce account".to_string(),
        ));
    }
    use rumi_protocol_backend::chains::solana::{config::SOLANA_CHAIN_ID, ted25519, tx};
    use solana_message::Hash;

    // Decode the operator-supplied blockhash if present. A Solana blockhash is a
    // 32-byte base58 value, so `decode_solana_address` is the right decoder; a
    // non-32-byte / non-base58 value is rejected with a clear error rather than
    // being fed into the transaction.
    let blockhash_override = match blockhash {
        Some(bh) => {
            let decoded = ted25519::decode_solana_address(&bh).map_err(|e| {
                ProtocolError::GenericError(format!(
                    "invalid blockhash (must be a 32-byte base58 value): {e}"
                ))
            })?;
            Some(Hash::new_from_array(decoded))
        }
        None => None,
    };

    tx::bootstrap_nonce_account(SOLANA_CHAIN_ID, blockhash_override)
        .await
        .map_err(ProtocolError::GenericError)
}

/// Developer-gated: set the SOL RPC canister principal override (mock/staging).
#[candid_method(update)]
#[update]
async fn set_sol_rpc_principal(p: Principal) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set the SOL RPC principal".to_string(),
        ));
    }
    mutate_state(|s| s.sol_rpc_principal_override = Some(p));
    Ok(())
}

/// Developer-gated (M2 Task 8): enable or disable the Solana observer +
/// settlement workers. While `false` (the default on every existing snapshot),
/// the chain-kind timer dispatch SKIPS the Solana `run_observer` /
/// `run_settlement` even when a Solana chain is registered, so Solana burns no
/// signing-subnet / SOL-RPC cycles until the operator flips this on. Monad chains
/// are unaffected (they always run). Takes effect on the NEXT timer tick (the
/// dispatcher reads the flag each tick); no upgrade or timer re-registration
/// needed.
#[candid_method(update)]
#[update]
async fn set_solana_workers_enabled(enabled: bool) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::ChainAdmin("not developer".into()));
    }
    mutate_state(|s| s.solana_workers_enabled = enabled);
    log!(INFO, "[set_solana_workers_enabled] Solana workers {}", if enabled { "ENABLED" } else { "disabled" });
    Ok(())
}

/// Wave-9c DOS-005: tune the cadence of the safety-belt full sweep that
/// walks every vault in `vault_cr_index` regardless of CR band.
/// Default 12 (one full sweep per hour at the 5-minute XRC cadence).
/// 0 or 1 forces full sweep every tick (revert path that mirrors
/// pre-Wave-9c behavior).
#[candid_method(update)]
#[update]
async fn set_check_vaults_full_sweep_every_n_ticks(
    new_n: u64,
) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set check_vaults full-sweep cadence"
                .to_string(),
        ));
    }
    mutate_state(|s| s.set_check_vaults_full_sweep_every_n_ticks(new_n));
    log!(
        INFO,
        "[set_check_vaults_full_sweep_every_n_ticks] Cadence set to: {} ticks ({})",
        new_n,
        if new_n <= 1 {
            "full sweep every tick (Wave-9c effectively disabled)"
        } else {
            "Wave-9c band sharding active"
        }
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
            "amm1" => rumi_protocol_backend::state::InterestDestination::Amm1,
            _ => rumi_protocol_backend::state::InterestDestination::Treasury, // fallback
        };
        rumi_protocol_backend::state::InterestRecipient { destination: dest, bps: r.bps }
    }).collect();

    // Validate destinations are known
    for r in &recipients {
        if !["stability_pool", "treasury", "three_pool", "amm1"].contains(&r.destination.as_str()) {
            return Err(ProtocolError::GenericError(
                format!("Unknown destination: '{}'. Valid: stability_pool, treasury, three_pool, amm1", r.destination),
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
                rumi_protocol_backend::state::InterestDestination::Amm1 => "amm1".to_string(),
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

/// Set the AMM1 canister principal for interest donations (developer only).
///
/// `distribute_interest` routes the `Amm1` arm of the N-way interest split to
/// this principal. Rejects `Principal::anonymous()` defensively so a misformed
/// admin call cannot leak interest mints to the anonymous principal.
#[candid_method(update)]
#[update]
async fn set_amm1_canister(canister: Principal) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set AMM1 canister".to_string(),
        ));
    }
    if canister == Principal::anonymous() {
        return Err(ProtocolError::GenericError(
            "AMM1 canister cannot be the anonymous principal".to_string(),
        ));
    }
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_amm1_canister(s, canister);
    });
    log!(INFO, "[set_amm1_canister] Set to: {}", canister);
    Ok(())
}

/// Get the configured AMM1 canister principal.
#[candid_method(query)]
#[query]
fn get_amm1_canister() -> Option<Principal> {
    read_state(|s| s.amm1_canister)
}

/// Set the canonical AMM1 pool_id used by `donate_icusd_to_amm1`.
/// Must match the AMM's `make_pool_id(token_a, token_b)` output exactly.
/// Developer-only.
#[candid_method(update)]
#[update]
async fn set_amm1_pool_id(pool_id: String) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only the developer principal can set AMM1 pool_id".to_string(),
        ));
    }
    if pool_id.is_empty() || pool_id.len() > 256 {
        return Err(ProtocolError::GenericError(
            "pool_id must be non-empty and <= 256 chars".to_string(),
        ));
    }
    mutate_state(|s| {
        rumi_protocol_backend::event::record_set_amm1_pool_id(s, pool_id.clone());
    });
    log!(INFO, "[set_amm1_pool_id] Updated: {}", pool_id);
    Ok(())
}

/// Read the configured AMM1 pool_id, if any.
#[candid_method(query)]
#[query]
fn get_amm1_pool_id() -> Option<String> {
    read_state(|s| s.amm1_pool_id.clone())
}

/// Diagnostic: return the length of the AMM1 donation retry queue.
#[candid_method(query)]
#[query]
fn get_pending_amm1_donations_count() -> u64 {
    read_state(|s| s.pending_amm1_donations.len() as u64)
}

/// Lightweight payload for the AMM TVL sampler (the latest cached XRC ICP/USD
/// price in e8s). `u128` instead of `u64` so the candid encodes as `nat`,
/// matching the unbounded TVL math the sampler does in USD.
#[derive(CandidType, Deserialize, Debug, Clone)]
pub struct ProtocolStatusLite {
    pub price_e8s: u128,
}

/// Lightweight query exposing the cached XRC ICP/USD price as `u128` in e8s.
///
/// Used by `rumi_amm`'s TVL sampler so it can compute USD-denominated TVL
/// without re-fetching from XRC. Reads the same `last_icp_rate` field that
/// `get_protocol_status` exposes (just narrower payload). Returns `0` when
/// the price has not been cached yet (e.g. before the first XRC tick).
#[candid_method(query)]
#[query]
fn get_icp_usd_price_e8s() -> ProtocolStatusLite {
    read_state(|s| ProtocolStatusLite {
        price_e8s: s
            .last_icp_rate
            .map(|p| p.to_e8s() as u128)
            .unwrap_or(0),
    })
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
    /// Wave-9b DOS-007: nanosecond timestamp at which the cached
    /// `total_accrued_interest_system` was last computed.
    pub snapshot_ts_ns: u64,
}

/// Get treasury-related statistics including accrued interest across all vaults.
///
/// Wave-9b DOS-007: `total_accrued_interest_system` is cached with a
/// 5-second TTL on the read path (refreshed in the existing 5-minute
/// XRC tick). Other fields are O(1)/O(small) and read fresh.
#[candid_method(query)]
#[query]
fn get_treasury_stats() -> TreasuryStats {
    let now = ic_cdk::api::time();

    // See `get_protocol_status` for the full caching rationale. Same
    // shape: cache filled by XRC tick, queries read or recompute
    // inline (no write-back from queries).
    let cached = read_state(|s| {
        s.treasury_stats_snapshot.as_ref().and_then(|(ts, snap)| {
            if now.saturating_sub(*ts) < TREASURY_STATS_SNAPSHOT_TTL_NANOS {
                Some((*ts, snap.clone()))
            } else {
                None
            }
        })
    });
    let (snapshot_ts_ns, snapshot) = match cached {
        Some(hit) => hit,
        None => {
            let snap = read_state(|s| s.compute_treasury_stats_snapshot());
            (now, snap)
        }
    };

    read_state(|s| TreasuryStats {
        treasury_principal: s.treasury_principal,
        // Wave-9b DOS-007: heavy field served from cache.
        total_accrued_interest_system: snapshot.total_accrued_interest_system,
        // Live fields below — change on liquidations / harvest, must
        // never be served stale.
        pending_treasury_interest: s.pending_treasury_interest.to_u64(),
        pending_treasury_collateral_entries: s.pending_treasury_collateral.len() as u64,
        liquidation_protocol_share: s.liquidation_protocol_share.to_f64(),
        pending_interest_for_pools_total: s.pending_interest_for_pools.values().sum(),
        interest_flush_threshold_e8s: s.interest_flush_threshold_e8s,
        snapshot_ts_ns,
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
    rumi_protocol_backend::validate_f64_inclusive("interest_rate_apr", interest_rate_apr, 0.0, 1.0)
        .map_err(ProtocolError::GenericError)?;
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
    rumi_protocol_backend::validate_f64_inclusive("borrowing_fee", borrowing_fee, 0.0, 0.10)
        .map_err(ProtocolError::GenericError)?;
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
    // Validate finite values, sorted ascending, and positive multipliers
    for i in 0..markers.len() {
        if !markers[i].0.is_finite() || !markers[i].1.is_finite() {
            return Err(ProtocolError::GenericError(
                format!("Marker at index {} must contain finite numbers, got ({}, {})", i, markers[i].0, markers[i].1),
            ));
        }
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
        if !mult.is_finite() || *mult <= 0.0 {
            return Err(ProtocolError::GenericError(
                format!("Multiplier for {} must be a finite positive number, got {}", thresh_str, mult),
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
        if !cr.is_finite() {
            return Err(ProtocolError::GenericError(format!(
                "healthy_cr ({}) must be a finite number", cr
            )));
        }
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
        // New collateral types start by inheriting the global XRC source-count
        // floor. Operator can override later via `set_collateral_min_xrc_sources`
        // if the asset has genuinely thin CEX coverage on XRC.
        min_xrc_sources: None,
        // P2: collaterals registered via this admin path are ICRC-custodied.
        // Native-XRP collateral (custody_kind = NativeXrp) is registered through a
        // separate path once its deposit flow is wired (spec P5); not settable here.
        custody_kind: None,
    };

    mutate_state(|s| {
        event::record_add_collateral_type(s, arg.ledger_canister_id, config);
    });

    // Register a price-fetching timer for the new collateral type.
    // ICP has its own dedicated timer in setup_timers(); other collateral
    // types use the generic fetch_collateral_price.
    // Wave-9d DOS-011: registers via `xrc::register_collateral_price_timer`
    // so the closure gates on `CollateralStatus`. The timer is permanent
    // for the canister lifetime; status changes flip the gate at the next
    // tick with no `clear_timer` / `TimerId` bookkeeping (which would not
    // survive upgrade anyway).
    let ledger_id = arg.ledger_canister_id;
    let is_icp = read_state(|s| s.icp_collateral_type() == ledger_id);
    if !is_icp {
        log!(INFO, "[add_collateral_token] Registering price timer for collateral {}", ledger_id);
        rumi_protocol_backend::xrc::register_collateral_price_timer(ledger_id);
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

/// Wave-14a CDP-14 follow-up: set the per-collateral XRC source-count floor
/// override. `None` clears the override and the collateral inherits the
/// global floor (`State.min_xrc_sources_used`, default 3). `Some(0)` is a
/// per-collateral kill switch (matches the global semantics).
///
/// Use case: collaterals whose underlying asset has genuinely thin CEX
/// coverage on XRC (e.g. XAUT, listed on only a handful of exchanges)
/// chronically fall short of the strict floor=3 gate even when XRC is
/// healthy. Dropping the floor to 2 for those specific assets stops the
/// rejection-event flood without weakening the gate for the rest.
#[candid_method(update)]
#[update]
async fn set_collateral_min_xrc_sources(
    collateral_type: Principal,
    min_xrc_sources: Option<u32>,
) -> Result<(), ProtocolError> {
    let caller = ic_cdk::caller();
    let is_developer = read_state(|s| s.developer_principal == caller);
    if !is_developer {
        return Err(ProtocolError::GenericError(
            "Only developer can change collateral min XRC sources".to_string(),
        ));
    }

    // Sanity guard: anything above ~10 is meaningless given XRC's CEX panel size
    // and would just freeze the collateral's price. Refuse rather than let a typo
    // create a denial-of-service on a specific asset.
    if let Some(n) = min_xrc_sources {
        if n > 10 {
            return Err(ProtocolError::GenericError(format!(
                "min_xrc_sources={} exceeds the practical cap of 10",
                n
            )));
        }
    }

    let exists = read_state(|s| s.collateral_configs.contains_key(&collateral_type));
    if !exists {
        return Err(ProtocolError::GenericError(
            "Collateral type not found".to_string(),
        ));
    }

    mutate_state(|s| {
        event::record_set_collateral_min_xrc_sources(s, collateral_type, min_xrc_sources);
    });

    log!(
        INFO,
        "[set_collateral_min_xrc_sources] Collateral {} min_xrc_sources set to {:?} (None=inherit global)",
        collateral_type,
        min_xrc_sources
    );
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

    rumi_protocol_backend::validate_f64_inclusive("haircut", haircut, 0.0, 0.50)
        .map_err(ProtocolError::GenericError)?;

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
    if !liquidation_ratio.is_finite() || liquidation_ratio <= 1.0 || liquidation_ratio > 5.0 {
        return Err(ProtocolError::GenericError(format!(
            "liquidation_ratio ({}) must be a finite number > 1.0 and ≤ 5.0",
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
    if !borrow_threshold_ratio.is_finite() || borrow_threshold_ratio <= 1.0 || borrow_threshold_ratio > 5.0 {
        return Err(ProtocolError::GenericError(format!(
            "borrow_threshold_ratio ({}) must be a finite number > 1.0 and ≤ 5.0",
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
    rumi_protocol_backend::validate_f64_inclusive("liquidation_bonus", liquidation_bonus, 1.0, 1.5)
        .map_err(ProtocolError::GenericError)?;
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
    rumi_protocol_backend::validate_f64_inclusive("redemption_fee_floor", redemption_fee_floor, 0.0, 0.10)
        .map_err(ProtocolError::GenericError)?;
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
    rumi_protocol_backend::validate_f64_inclusive("redemption_fee_ceiling", redemption_fee_ceiling, 0.0, 0.50)
        .map_err(ProtocolError::GenericError)?;
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

// Validates the forward, id-cursored, type-filtered scan that backs
// `get_events_forward_filtered` (the rumi_points ingestion endpoint): window
// bounding, global-index tagging, type filtering, and the resume cursor.
#[test]
fn forward_filtered_scan_windows_filters_and_advances_cursor() {
    use std::collections::HashSet;

    let close = |id: u64| Event::CloseVault {
        vault_id: id,
        block_index: None,
        timestamp: Some(100),
    };
    let evs = vec![close(0), close(1), close(2)];
    let count = evs.len() as u64;
    let close_filter: HashSet<EventTypeFilter> = HashSet::from([EventTypeFilter::CloseVault]);
    let borrow_filter: HashSet<EventTypeFilter> = HashSet::from([EventTypeFilter::Borrow]);

    let ids = |r: &ForwardFilteredEventsResponse| r.events.iter().map(|(i, _)| *i).collect::<Vec<_>>();

    // Full scan: all three match, tagged with their global indices, caught up.
    let r = scan_events_forward_filtered(evs.clone().into_iter(), 0, 10, count, Some(&close_filter));
    assert_eq!(ids(&r), vec![0, 1, 2]);
    assert_eq!(r.next_start, 3);
    assert!(r.reached_end);

    // Bounded window [0,2): only indices 0,1; not yet caught up.
    let r = scan_events_forward_filtered(evs.clone().into_iter(), 0, 2, count, Some(&close_filter));
    assert_eq!(ids(&r), vec![0, 1]);
    assert_eq!(r.next_start, 2);
    assert!(!r.reached_end);

    // Resume from cursor 2: only index 2, then caught up.
    let r = scan_events_forward_filtered(evs.clone().into_iter(), 2, 10, count, Some(&close_filter));
    assert_eq!(ids(&r), vec![2]);
    assert_eq!(r.next_start, 3);
    assert!(r.reached_end);

    // A non-matching filter yields no events but still advances the cursor (so a
    // poller never stalls on a quiet range).
    let r = scan_events_forward_filtered(evs.into_iter(), 0, 10, count, Some(&borrow_filter));
    assert!(r.events.is_empty());
    assert_eq!(r.next_start, 3);
    assert!(r.reached_end);
}

// Proves `evm_vault_params` resolves the native price symbol + min CR per chain
// from the compile-time configs, and that Monad (10143) is behavior-preserving
// (still ("MON", 13_000)) while Conflux (71) mirrors the ICP params ("CFX",
// 13_300). Non-EVM / unknown chains error rather than silently defaulting.
#[cfg(test)]
mod chain_vault_param_tests {
    use super::evm_vault_params;

    #[test]
    fn evm_vault_params_resolves_per_chain() {
        use rumi_protocol_backend::chains::config::ChainId;
        assert_eq!(evm_vault_params(ChainId(10143)).unwrap(), ("MON", 13_000, 10_000_000, None)); // Monad preserved
        assert_eq!(
            evm_vault_params(ChainId(71)).unwrap(),
            ("CFX", 15_000, 10_000_000, Some(500 * 100_000_000))
        ); // Conflux: 150% open gate + 500-icUSD ceiling
        assert!(evm_vault_params(ChainId(999)).is_err());
    }

    #[test]
    fn ecdsa_key_change_rules() {
        use super::validate_ecdsa_key_change;
        // Both supported key names are accepted when no vault exists.
        assert!(validate_ecdsa_key_change("key_1", false).is_ok());
        assert!(validate_ecdsa_key_change("test_key_1", false).is_ok());
        // An unknown key name is rejected (typo/foot-gun guard).
        assert!(validate_ecdsa_key_change("bogus_key", false).is_err());
        // Changing the key while chain vaults exist is blocked (would re-derive +
        // orphan every custody address).
        assert!(validate_ecdsa_key_change("key_1", true).is_err());
    }
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

