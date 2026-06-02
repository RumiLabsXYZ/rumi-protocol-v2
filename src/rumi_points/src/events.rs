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
//! Idempotency: each source has a monotonic cursor (`state::get_cursor`). A poll
//! fetches `[cursor, cursor+batch)`, applies the batch, and advances the cursor
//! to `max(event_id)+1` in a single post-await message (atomic on the IC, so a
//! trap commits nothing). The `state::try_begin_poll` guard prevents two timers
//! from ingesting the same range concurrently. Events below the cursor are never
//! refetched, so no per-event dedup set is needed.
//!
//! PHASE BOUNDARY: this increment ingests events and auto-registers. Position /
//! deposit-value tracking, the repayment 90-day window (which also needs the
//! `repayment_asset` upstream field, a separate branch), 3USD verification, and
//! accrual are Phase 4/5. `IngestKind` already carries the payload those phases
//! need so the normalized model does not have to change later.

use candid::Principal;

use crate::state;
use crate::types::QualifyingAction;

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
    /// Repaid vault debt. Asset is not yet attributable (the `repayment_asset`
    /// field is a separate upstream branch); the 5x ck-stable window is Phase 5.
    VaultRepay { vault_id: u64, amount_e8s: u64 },
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
    // Auto-registration on first qualifying, in-season action. `register` is
    // idempotent and rejects excluded principals, so calling it for every
    // matching event is safe and cheap.
    if let (Some(action), Some(caller)) = (ev.kind.qualifying_action(), ev.caller) {
        if state::in_season(ev.timestamp_ns) {
            let _ = state::register(caller, ev.timestamp_ns, action);
        }
    }
    // Position-value tracking, the repayment 90-day window, 3USD verification,
    // and accrual are Phase 4/5 and read the payload carried on `ev.kind`.
}

/// Apply a decoded batch of normalized events (auto-register etc.). Does NOT
/// touch any cursor. Each poller advances its source cursor from that source's
/// own resume signal: the forward endpoints (backend, 3pool) return an explicit
/// `next_start`; the index endpoints (SP, AMM) advance by returned count (see
/// `ingest_batch`). The network-free, unit-testable core of ingestion.
pub fn apply_events(events: &[IngestedEvent]) {
    for ev in events {
        apply_ingested_event(ev);
    }
}

/// Apply a batch and advance the source cursor to `max(event_id) + 1`. Valid for
/// the index endpoints (SP, AMM), where `id == index` holds until their logs trim
/// (far beyond Season-1 volume at current TVL). The forward endpoints (backend,
/// 3pool) instead set the cursor from the endpoint's returned `next_start`. An
/// empty batch leaves the cursor unchanged. Returns the number applied.
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
    use crate::types::{InitArgs, QualifyingAction};

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
            IngestKind::VaultRepay { vault_id: 1, amount_e8s: 5 }.qualifying_action(),
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
}

