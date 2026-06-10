//! Event ingestion (spike 0.2, implementation plan Phase 2 + the auto-registration
//! of Phase 3).
//!
//! PULL-BASED: `rumi_points` polls each source canister's existing
//! `get_*_events(start, length)` query endpoints on a timer, decodes each event
//! into a normalized `IngestedEvent`, and applies it. Auto-registration enrolls a
//! principal on its first qualifying action (spec Section 8). The wire decoding
//! lives in `source_types.rs`; this module holds the normalized model, the
//! apply/registration logic, and the cursor-advancing batch driver.
//!
//! Idempotency: each source has a monotonic ID-based cursor (`state::get_cursor`,
//! the next event id wanted). A poll fetches a window of events at/after the
//! cursor (the forward endpoints fetch by id directly; SP/AMM fetch by array
//! position and filter by id, see the PTS-001 notes in `poll.rs`), applies the
//! batch, and advances the cursor to `max(event_id)+1` in a single post-await
//! message (atomic on the IC, so a trap commits nothing). The `state::PollGuard`
//! prevents two timers from ingesting the same range concurrently. Events below
//! the cursor are filtered out before apply, so no per-event dedup set is needed.
//!
//! PHASE BOUNDARY: this increment ingests events and auto-registers. Position /
//! deposit-value tracking, the repayment 90-day window (which also needs the
//! `repayment_asset` upstream field, a separate branch), 3USD verification, and
//! accrual are Phase 4/5. `IngestKind` already carries the payload those phases
//! need so the normalized model does not have to change later.

use candid::Principal;

use crate::state;
use crate::types::{AssetType, QualifyingAction};
use crate::valuation;

/// The four source canisters polled for events. The `tag` is the stable cursor
/// key (see `state::get_cursor`); never renumber an existing tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceId {
    Backend,
    ThreePool,
    StabilityPool,
    Amm,
}

impl SourceId {
    pub fn tag(self) -> u8 {
        match self {
            SourceId::Backend => 0,
            SourceId::ThreePool => 1,
            SourceId::StabilityPool => 2,
            SourceId::Amm => 3,
        }
    }
}

/// Normalized event, decoded from any source's wire format. Carries enough for
/// Phase 2 (registration) and the payload Phase 4/5 will need (amounts, ledger,
/// vault id) so the model is stable across phases.
#[derive(Clone, Debug, PartialEq)]
pub struct IngestedEvent {
    pub source: SourceId,
    pub event_id: u64,
    /// The acting principal, when the source event carries one. `None` for events
    /// with no attributable caller (e.g. backend `close_vault` / `liquidate_vault`,
    /// which carry only a vault id); those never trigger registration.
    pub caller: Option<Principal>,
    pub timestamp_ns: u64,
    pub kind: IngestKind,
}

/// Source-agnostic event classification.
#[derive(Clone, Debug, PartialEq)]
pub enum IngestKind {
    // ── rumi_protocol_backend (vault) ──
    /// Opened a vault; `borrowed_e8s` is the icUSD minted at open (may be 0).
    VaultOpen { vault_id: u64, borrowed_e8s: u64 },
    /// Borrowed (minted) more icUSD from an existing vault.
    VaultBorrow { vault_id: u64, amount_e8s: u64 },
    /// Repaid vault debt. `repayment_asset` is the ledger the debt was repaid with,
    /// needed to tell a qualifying ckUSDC/ckUSDT repayment (5x window, spec Section
    /// 6) from an icUSD burn or collateral closure. The upstream backend field is
    /// not live yet, so the normalizer sets it `None` and the 5x window is skipped
    /// (logged) until it lands.
    VaultRepay {
        vault_id: u64,
        amount_e8s: u64,
        repayment_asset: Option<candid::Principal>,
    },
    VaultClose { vault_id: u64 },
    VaultLiquidated { vault_id: u64 },
    VaultRedeemed { amount_e8s: u64 },

    // ── rumi_3pool ── amounts are [icUSD, ckUSDT, ckUSDC] (3pool ordering).
    ThreePoolAdd { amounts: [u128; 3] },
    ThreePoolRemove { amounts: [u128; 3] },

    // ── rumi_stability_pool ── `token_ledger` identifies the deposited asset.
    SpDeposit { token_ledger: Principal, amount_e8s: u64 },
    SpWithdraw { token_ledger: Principal, amount_e8s: u64 },

    // ── rumi_amm ──
    AmmAddLiquidity { pool_id: String, lp_shares: u128 },
    AmmRemoveLiquidity { pool_id: String, lp_shares: u128 },

    /// Ingested but not acted on in any current phase (e.g. a 3pool `Donate`, an
    /// SP config event). Kept so the cursor still advances past it.
    Other,
}

impl IngestKind {
    /// The qualifying action this event represents, if it is one of the five
    /// registration triggers (spec Section 8). `None` means "ingest but do not
    /// enroll" (withdrawals, closes, liquidations, redemptions, config events).
    pub fn qualifying_action(&self) -> Option<QualifyingAction> {
        match self {
            IngestKind::VaultOpen { borrowed_e8s, .. } if *borrowed_e8s > 0 => {
                Some(QualifyingAction::MintIcUsd)
            }
            IngestKind::VaultBorrow { .. } => Some(QualifyingAction::MintIcUsd),
            IngestKind::VaultRepay { .. } => Some(QualifyingAction::RepayVault),
            IngestKind::ThreePoolAdd { .. } => Some(QualifyingAction::Deposit3Pool),
            IngestKind::SpDeposit { .. } => Some(QualifyingAction::DepositStabilityPool),
            IngestKind::AmmAddLiquidity { .. } => Some(QualifyingAction::ProvideAmmLiquidity),
            _ => None,
        }
    }
}

/// Apply one normalized event. Phase 2/3: auto-register the caller on its first
/// qualifying, in-season action. `register` is idempotent and rejects excluded
/// principals, so this is safe to call for every matching event.
pub fn apply_ingested_event(ev: &IngestedEvent) {
    let caller = match ev.caller {
        Some(c) => c,
        None => return, // close/liquidate carry no principal; nothing to attribute
    };
    // Everything is gated to in-season activity by a non-excluded principal.
    if !state::in_season(ev.timestamp_ns) || state::is_excluded(&caller) {
        return;
    }
    // Auto-register on the first qualifying action (idempotent).
    if let Some(action) = ev.kind.qualifying_action() {
        let _ = state::register(caller, ev.timestamp_ns, action);
    }
    // Position / repayment tracking only applies to already-registered principals
    // (a non-qualifying first event, e.g. a withdraw, never enrolls anyone, and
    // has no recorded position to adjust).
    if !state::is_registered(&caller) {
        return;
    }
    match &ev.kind {
        IngestKind::ThreePoolAdd { amounts } => apply_3pool(caller, amounts, true, ev.timestamp_ns),
        IngestKind::ThreePoolRemove { amounts } => {
            apply_3pool(caller, amounts, false, ev.timestamp_ns)
        }
        IngestKind::VaultRepay { repayment_asset, amount_e8s, .. } => {
            apply_repayment(caller, *repayment_asset, *amount_e8s, ev.timestamp_ns)
        }
        // Vault debt, SP, and AMM positions are read live at each snapshot (the
        // hybrid model), so their events only register; they are not tracked here.
        _ => {}
    }
}

/// Update the recorded 3pool composition from an add/remove. `amounts` ordering is
/// `[icUSD, ckUSDT, ckUSDC]` (3pool wire order), in native decimals; normalized to
/// `usd_e8s` here so the snapshot accrual reads a common scale.
fn apply_3pool(caller: Principal, amounts: &[u128; 3], add: bool, now_ns: u64) {
    let legs = [
        (AssetType::IcUsd, amounts[0]),
        (AssetType::CkUsdt, amounts[1]),
        (AssetType::CkUsdc, amounts[2]),
    ];
    for (asset, native) in legs {
        if native > 0 {
            let usd = valuation::value_stable_usd_e8s(asset, native);
            state::update_3pool_recorded(caller, asset, usd, add, now_ns);
        }
    }
}

/// Open a 90-day window for a qualifying ckUSDC/ckUSDT repayment (spec Section 6).
/// Skips (with a log) when the asset is absent (the upstream `repayment_asset`
/// field is not live) or is not a ck-stable (an icUSD burn / collateral closure
/// does not qualify).
fn apply_repayment(
    caller: Principal,
    repayment_asset: Option<Principal>,
    amount_e8s: u64,
    now_ns: u64,
) {
    let ledger = match repayment_asset {
        Some(l) => l,
        None => {
            log_ingest("vault repay without repayment_asset; skipping 5x window");
            return;
        }
    };
    if let Some(asset @ (AssetType::CkUsdc | AssetType::CkUsdt)) = state::classify_ledger(&ledger) {
        let usd = valuation::value_stable_usd_e8s(asset, amount_e8s as u128);
        state::record_repayment(caller, asset, usd, now_ns);
    }
}

/// On-chain log; a no-op in native unit tests (the `ic0` host import is
/// unavailable off-wasm, where `ic_cdk::println!` would panic).
fn log_ingest(msg: &str) {
    #[cfg(target_arch = "wasm32")]
    ic_cdk::println!("[ingest] {}", msg);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = msg;
}

/// Apply a decoded batch of normalized events (auto-register etc.). Does NOT
/// touch any cursor. Each poller advances its source cursor from that source's
/// own resume signal: the forward endpoints (backend, 3pool) return an explicit
/// `next_start`; the position-indexed endpoints (SP, AMM) advance to the max
/// ingested id via `ingest_batch` (PTS-001). The network-free, unit-testable
/// core of ingestion.
pub fn apply_events(events: &[IngestedEvent]) {
    for ev in events {
        apply_ingested_event(ev);
    }
}

/// Apply a batch and advance the source cursor to `max(event_id) + 1`. Used by
/// the position-indexed endpoints (SP, AMM), whose pollers fetch by array
/// position and pre-filter the window to `event_id >= cursor` (PTS-001), so the
/// cursor stays ID-based even after the source log rotates. The forward
/// endpoints (backend, 3pool) instead set the cursor from the endpoint's
/// returned `next_start`. An empty batch leaves the cursor unchanged. Returns
/// the number applied.
pub fn ingest_batch(source: SourceId, events: &[IngestedEvent]) -> usize {
    apply_events(events);
    if let Some(m) = events.iter().map(|e| e.event_id).max() {
        state::set_cursor(source.tag(), m + 1);
    }
    events.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state;
    use crate::types::{AssetType, DepositKey, InitArgs, QualifyingAction, Venue};

    fn ckusdc() -> Principal {
        Principal::from_text("xevnm-gaaaa-aaaar-qafnq-cai").unwrap()
    }
    fn icusd() -> Principal {
        Principal::from_text("t6bor-paaaa-aaaap-qrd5q-cai").unwrap()
    }
    fn recorded_3pool(p: &Principal, asset: AssetType) -> Option<u128> {
        state::get_principal_state(p)?
            .active_deposits
            .get(&DepositKey { venue: Venue::ThreePool, asset })
            .map(|r| r.recorded_value_usd)
    }

    fn tp(n: u8) -> Principal {
        Principal::from_slice(&[n, n, n, n, n])
    }

    fn in_season_ts() -> u64 {
        crate::DEFAULT_SEASON_START_NS + 1_000
    }

    fn init() {
        state::init_state(
            Some(InitArgs {
                admin: Some(tp(99)),
                ..Default::default()
            }),
            Principal::anonymous(),
        );
    }

    fn ev(source: SourceId, id: u64, caller: Option<Principal>, ts: u64, kind: IngestKind) -> IngestedEvent {
        IngestedEvent {
            source,
            event_id: id,
            caller,
            timestamp_ns: ts,
            kind,
        }
    }

    #[test]
    fn qualifying_action_maps_the_five_triggers() {
        assert_eq!(
            IngestKind::VaultBorrow { vault_id: 1, amount_e8s: 5 }.qualifying_action(),
            Some(QualifyingAction::MintIcUsd)
        );
        assert_eq!(
            IngestKind::VaultOpen { vault_id: 1, borrowed_e8s: 5 }.qualifying_action(),
            Some(QualifyingAction::MintIcUsd)
        );
        // Opening a vault without minting is not a qualifying action.
        assert_eq!(
            IngestKind::VaultOpen { vault_id: 1, borrowed_e8s: 0 }.qualifying_action(),
            None
        );
        assert_eq!(
            IngestKind::VaultRepay { vault_id: 1, amount_e8s: 5, repayment_asset: None }.qualifying_action(),
            Some(QualifyingAction::RepayVault)
        );
        assert_eq!(
            IngestKind::ThreePoolAdd { amounts: [0, 1, 0] }.qualifying_action(),
            Some(QualifyingAction::Deposit3Pool)
        );
        assert_eq!(
            IngestKind::SpDeposit { token_ledger: tp(1), amount_e8s: 5 }.qualifying_action(),
            Some(QualifyingAction::DepositStabilityPool)
        );
        assert_eq!(
            IngestKind::AmmAddLiquidity { pool_id: "3usd-icp".into(), lp_shares: 5 }.qualifying_action(),
            Some(QualifyingAction::ProvideAmmLiquidity)
        );
        // Non-triggers.
        assert_eq!(IngestKind::ThreePoolRemove { amounts: [0, 1, 0] }.qualifying_action(), None);
        assert_eq!(IngestKind::VaultClose { vault_id: 1 }.qualifying_action(), None);
        assert_eq!(IngestKind::Other.qualifying_action(), None);
    }

    #[test]
    fn apply_auto_registers_on_first_qualifying_action() {
        init();
        let p = tp(10);
        apply_ingested_event(&ev(
            SourceId::ThreePool,
            0,
            Some(p),
            in_season_ts(),
            IngestKind::ThreePoolAdd { amounts: [100, 0, 0] },
        ));
        assert!(state::is_registered(&p));
        let st = state::get_principal_state(&p).unwrap();
        assert_eq!(st.first_qualifying_action, QualifyingAction::Deposit3Pool);
        assert_eq!(st.registered_at_ns, in_season_ts());
    }

    #[test]
    fn apply_does_not_register_non_qualifying_event() {
        init();
        let p = tp(11);
        apply_ingested_event(&ev(
            SourceId::ThreePool,
            0,
            Some(p),
            in_season_ts(),
            IngestKind::ThreePoolRemove { amounts: [100, 0, 0] },
        ));
        assert!(!state::is_registered(&p));
    }

    #[test]
    fn apply_ignores_out_of_season_events() {
        init();
        let p = tp(12);
        let before_season = crate::DEFAULT_SEASON_START_NS - 1;
        apply_ingested_event(&ev(
            SourceId::Backend,
            0,
            Some(p),
            before_season,
            IngestKind::VaultBorrow { vault_id: 1, amount_e8s: 100 },
        ));
        assert!(!state::is_registered(&p));
    }

    #[test]
    fn apply_does_not_register_excluded_principal() {
        init();
        let backend = Principal::from_text("tfesu-vyaaa-aaaap-qrd7a-cai").unwrap();
        apply_ingested_event(&ev(
            SourceId::Backend,
            0,
            Some(backend),
            in_season_ts(),
            IngestKind::VaultBorrow { vault_id: 1, amount_e8s: 100 },
        ));
        assert!(!state::is_registered(&backend));
    }

    #[test]
    fn apply_with_no_caller_is_a_noop() {
        init();
        // close_vault carries no caller, so there is nobody to register.
        apply_ingested_event(&ev(
            SourceId::Backend,
            0,
            None,
            in_season_ts(),
            IngestKind::VaultClose { vault_id: 1 },
        ));
        assert_eq!(state::registered_count(), 0);
    }

    #[test]
    fn ingest_batch_applies_all_and_advances_cursor() {
        init();
        let evs = vec![
            ev(SourceId::ThreePool, 5, Some(tp(20)), in_season_ts(), IngestKind::ThreePoolAdd { amounts: [1, 0, 0] }),
            ev(SourceId::ThreePool, 6, Some(tp(21)), in_season_ts(), IngestKind::ThreePoolAdd { amounts: [1, 0, 0] }),
            ev(SourceId::ThreePool, 7, Some(tp(22)), in_season_ts(), IngestKind::ThreePoolRemove { amounts: [1, 0, 0] }),
        ];
        let n = ingest_batch(SourceId::ThreePool, &evs);
        assert_eq!(n, 3);
        assert!(state::is_registered(&tp(20)));
        assert!(state::is_registered(&tp(21)));
        assert!(!state::is_registered(&tp(22))); // remove is not a trigger
        assert_eq!(state::get_cursor(SourceId::ThreePool.tag()), 8); // max id (7) + 1
    }

    #[test]
    fn ingest_batch_empty_leaves_cursor_unchanged() {
        init();
        state::set_cursor(SourceId::Amm.tag(), 42);
        let n = ingest_batch(SourceId::Amm, &[]);
        assert_eq!(n, 0);
        assert_eq!(state::get_cursor(SourceId::Amm.tag()), 42);
    }

    #[test]
    fn ingest_batch_is_idempotent_on_reapply() {
        init();
        let evs = vec![ev(
            SourceId::Backend,
            0,
            Some(tp(30)),
            in_season_ts(),
            IngestKind::VaultBorrow { vault_id: 1, amount_e8s: 100 },
        )];
        ingest_batch(SourceId::Backend, &evs);
        let first = state::get_principal_state(&tp(30)).unwrap();
        ingest_batch(SourceId::Backend, &evs); // re-apply same batch
        let second = state::get_principal_state(&tp(30)).unwrap();
        assert_eq!(first, second); // no double-register, no timestamp change
        assert_eq!(state::registered_count(), 1);
        assert_eq!(state::get_cursor(SourceId::Backend.tag()), 1);
    }

    // ── Phase 4/5: position + repayment tracking ──

    #[test]
    fn threepool_add_records_normalized_composition() {
        init();
        let p = tp(40);
        // amounts are [icUSD (8-dec), ckUSDT (6-dec), ckUSDC (6-dec)].
        apply_ingested_event(&ev(
            SourceId::ThreePool,
            0,
            Some(p),
            in_season_ts(),
            IngestKind::ThreePoolAdd { amounts: [100_000_000, 2_000_000, 3_000_000] },
        ));
        assert_eq!(recorded_3pool(&p, AssetType::IcUsd), Some(100_000_000)); // 8-dec, x1
        assert_eq!(recorded_3pool(&p, AssetType::CkUsdt), Some(200_000_000)); // 6-dec, x100
        assert_eq!(recorded_3pool(&p, AssetType::CkUsdc), Some(300_000_000)); // 6-dec, x100
    }

    #[test]
    fn threepool_remove_decrements_recorded_value() {
        init();
        let p = tp(41);
        apply_ingested_event(&ev(
            SourceId::ThreePool,
            0,
            Some(p),
            in_season_ts(),
            IngestKind::ThreePoolAdd { amounts: [0, 0, 5_000_000] }, // $5 ckUSDC
        ));
        apply_ingested_event(&ev(
            SourceId::ThreePool,
            1,
            Some(p),
            in_season_ts(),
            IngestKind::ThreePoolRemove { amounts: [0, 0, 2_000_000] }, // remove $2
        ));
        assert_eq!(recorded_3pool(&p, AssetType::CkUsdc), Some(300_000_000)); // $3 left
    }

    #[test]
    fn vault_repay_with_ckusdc_opens_a_window() {
        init();
        let p = tp(42);
        apply_ingested_event(&ev(
            SourceId::Backend,
            0,
            Some(p),
            in_season_ts(),
            IngestKind::VaultRepay { vault_id: 1, amount_e8s: 1_000_000, repayment_asset: Some(ckusdc()) },
        ));
        let st = state::get_principal_state(&p).unwrap();
        assert_eq!(st.repayment_events.len(), 1);
        assert_eq!(st.repayment_events[0].asset, AssetType::CkUsdc);
        assert_eq!(st.repayment_events[0].amount_usd, 100_000_000); // $1, 6-dec x100
    }

    #[test]
    fn vault_repay_without_asset_registers_but_opens_no_window() {
        init();
        let p = tp(43);
        apply_ingested_event(&ev(
            SourceId::Backend,
            0,
            Some(p),
            in_season_ts(),
            IngestKind::VaultRepay { vault_id: 1, amount_e8s: 1_000_000, repayment_asset: None },
        ));
        assert!(state::is_registered(&p)); // repay is a qualifying action
        assert!(state::get_principal_state(&p).unwrap().repayment_events.is_empty());
    }

    #[test]
    fn vault_repay_with_icusd_opens_no_window() {
        init();
        let p = tp(44);
        apply_ingested_event(&ev(
            SourceId::Backend,
            0,
            Some(p),
            in_season_ts(),
            IngestKind::VaultRepay { vault_id: 1, amount_e8s: 1_000_000, repayment_asset: Some(icusd()) },
        ));
        assert!(state::is_registered(&p));
        // An icUSD-burn repayment does not qualify for the 5x window (spec Section 6).
        assert!(state::get_principal_state(&p).unwrap().repayment_events.is_empty());
    }
}

