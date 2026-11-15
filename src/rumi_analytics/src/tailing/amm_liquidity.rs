//! AMM liquidity event tailing. Mirrors AddLiquidity / RemoveLiquidity events
//! into evt_amm_liquidity so the address-value query can reconstruct per-
//! principal LP-share timelines without making inter-canister calls.

use super::{update_cursor_error, update_cursor_source_count, update_cursor_success, BATCH_SIZE};
use crate::{sources, state, storage};
use storage::cursors;
use storage::events::*;

const MAX_CUTOVER_FINGERPRINT_ROWS: u64 = 10_000;

fn nat_to_u64(n: &candid::Nat) -> u64 {
    use num_traits::ToPrimitive;
    n.0.to_u64().unwrap_or(u64::MAX)
}

fn convert_action(action: &sources::amm::AmmLiquidityAction) -> LiquidityAction {
    use sources::amm::AmmLiquidityAction::*;
    match action {
        AddLiquidity => LiquidityAction::Add,
        RemoveLiquidity => LiquidityAction::Remove,
    }
}

fn select_unseen_events<'a>(
    next_wanted_id: u64,
    oldest_available_id: u64,
    events: &'a [sources::amm::AmmLiquidityEvent],
) -> (Vec<&'a sources::amm::AmmLiquidityEvent>, Option<u64>, bool) {
    let gap = next_wanted_id < oldest_available_id;
    let first_wanted_id = next_wanted_id.max(oldest_available_id);
    let unseen: Vec<_> = events
        .iter()
        .filter(|event| event.id >= first_wanted_id)
        .collect();
    let next_cursor = unseen.last().map(|event| event.id.saturating_add(1));
    (unseen, next_cursor, gap)
}

fn normalize_event(evt: &sources::amm::AmmLiquidityEvent) -> AnalyticsAmmLiquidityEvent {
    AnalyticsAmmLiquidityEvent {
        timestamp_ns: evt.timestamp,
        source_event_id: evt.id,
        caller: evt.caller,
        pool_id: evt.pool_id.clone(),
        action: convert_action(&evt.action),
        lp_shares: nat_to_u64(&evt.lp_shares),
    }
}

#[derive(Debug, PartialEq, Eq)]
struct CutoverPlan {
    rows_to_append: Vec<AnalyticsAmmLiquidityEvent>,
    cursor_after: u64,
    target: u64,
    complete: bool,
}

/// Build one bounded, fail-closed page of the one-time AMM generation
/// cutover.  The historical mirror and legacy positional cursor belong to an
/// AMM generation whose ID namespace was reset; neither can seed the new
/// cursor.  Exact normalized-row fingerprints suppress replay of a page while
/// still allowing new-generation events whose IDs collide with legacy rows.
fn prepare_cutover_page<F>(
    expected_cursor: u64,
    expected_target: Option<u64>,
    window: &sources::amm::AmmLiquidityEventWindow,
    mut contains_exact: F,
) -> Result<CutoverPlan, String>
where
    F: FnMut(&AnalyticsAmmLiquidityEvent) -> bool,
{
    let target = expected_target.unwrap_or(window.next_event_id);
    if expected_cursor > target {
        return Err(format!(
            "AMM liquidity generation cutover cursor {} exceeds target {}",
            expected_cursor, target
        ));
    }
    if window.next_event_id < target {
        return Err(format!(
            "AMM liquidity generation cutover source rewound from target {} to {}",
            target, window.next_event_id
        ));
    }
    if expected_cursor < target && window.oldest_available_id > expected_cursor {
        return Err(format!(
            "AMM liquidity generation cutover cannot prove retained prefix: next wanted event ID {} is older than retained ID {}",
            expected_cursor, window.oldest_available_id
        ));
    }
    if window.events.iter().any(|event| event.id < expected_cursor) {
        return Err(format!(
            "AMM liquidity generation cutover source returned an event older than requested ID {}",
            expected_cursor
        ));
    }

    let mut cursor_after = expected_cursor;
    let mut rows_to_append = Vec::new();
    for event in window.events.iter().filter(|event| event.id < target) {
        if event.id != cursor_after {
            return Err(format!(
                "AMM liquidity generation cutover non-contiguous page: expected event ID {}, got {}",
                cursor_after, event.id
            ));
        }
        cursor_after = cursor_after
            .checked_add(1)
            .ok_or_else(|| "AMM liquidity generation cutover cursor overflow".to_string())?;
        let row = normalize_event(event);
        if !contains_exact(&row) {
            rows_to_append.push(row);
        }
    }

    if expected_cursor < target && cursor_after == expected_cursor {
        return Err(format!(
            "AMM liquidity generation cutover made no progress at event ID {} toward target {}",
            expected_cursor, target
        ));
    }

    Ok(CutoverPlan {
        rows_to_append,
        cursor_after,
        target,
        complete: cursor_after == target,
    })
}

fn cutover_snapshot() -> (bool, u64, Option<u64>) {
    state::read_state(|s| {
        (
            s.amm_liquidity_generation_cutover_complete.unwrap_or(false),
            s.amm_liquidity_generation_cutover_cursor.unwrap_or(0),
            s.amm_liquidity_generation_cutover_target,
        )
    })
}

fn cutover_snapshot_matches(
    observed: (bool, u64, Option<u64>),
    expected_cursor: u64,
    expected_target: Option<u64>,
) -> bool {
    observed == (false, expected_cursor, expected_target)
}

async fn run_generation_cutover(expected_cursor: u64, expected_target: Option<u64>) {
    let window = match sources::amm::get_amm_liquidity_events_from_id(
        state::read_state(|s| s.sources.amm),
        expected_cursor,
        BATCH_SIZE,
    )
    .await
    {
        Ok(window) => window,
        Err(e) => {
            ic_cdk::println!("[tail_amm_liq] generation cutover fetch failed: {}", e);
            state::mutate_state(|s| {
                s.error_counters.amm = s.error_counters.amm.saturating_add(1);
                update_cursor_error(s, cursors::CURSOR_ID_AMM_LIQUIDITY, e);
            });
            return;
        }
    };

    // A competing timer may have committed the same page while this call was
    // in flight.  Only the invocation whose expected durable tuple still
    // matches may append or advance.
    if !cutover_snapshot_matches(cutover_snapshot(), expected_cursor, expected_target) {
        return;
    }

    let existing_fingerprints: std::collections::HashSet<_> =
        match storage::events::bounded_amm_liquidity_events(MAX_CUTOVER_FINGERPRINT_ROWS) {
            Ok(rows) => rows.into_iter().collect(),
            Err(error) => {
                ic_cdk::println!("[tail_amm_liq] generation cutover held: {}", error);
                state::mutate_state(|s| {
                    s.error_counters.amm = s.error_counters.amm.saturating_add(1);
                    update_cursor_source_count(
                        s,
                        cursors::CURSOR_ID_AMM_LIQUIDITY,
                        window.next_event_id,
                    );
                    update_cursor_error(s, cursors::CURSOR_ID_AMM_LIQUIDITY, error);
                });
                return;
            }
        };

    let plan = match prepare_cutover_page(expected_cursor, expected_target, &window, |candidate| {
        existing_fingerprints.contains(candidate)
    }) {
        Ok(plan) => plan,
        Err(error) => {
            ic_cdk::println!("[tail_amm_liq] generation cutover held: {}", error);
            state::mutate_state(|s| {
                s.error_counters.amm = s.error_counters.amm.saturating_add(1);
                update_cursor_source_count(
                    s,
                    cursors::CURSOR_ID_AMM_LIQUIDITY,
                    window.next_event_id,
                );
                update_cursor_error(s, cursors::CURSOR_ID_AMM_LIQUIDITY, error);
            });
            return;
        }
    };

    for row in plan.rows_to_append {
        evt_amm_liquidity::push(row);
    }
    if plan.complete {
        cursors::amm_liquidity::set(plan.cursor_after);
    }
    state::mutate_state(|s| {
        s.amm_liquidity_generation_cutover_cursor = Some(plan.cursor_after);
        s.amm_liquidity_generation_cutover_target = Some(plan.target);
        s.amm_liquidity_generation_cutover_complete = Some(plan.complete);
        update_cursor_source_count(s, cursors::CURSOR_ID_AMM_LIQUIDITY, window.next_event_id);
        update_cursor_success(s, cursors::CURSOR_ID_AMM_LIQUIDITY, ic_cdk::api::time());
    });
}

async fn run_current_generation() {
    let requested_id = cursors::amm_liquidity::get();

    let window = match sources::amm::get_amm_liquidity_events_from_id(
        state::read_state(|s| s.sources.amm),
        requested_id,
        BATCH_SIZE,
    )
    .await
    {
        Ok(window) => window,
        Err(e) => {
            ic_cdk::println!(
                "[tail_amm_liq] get_amm_liquidity_events_from_id failed: {}",
                e
            );
            state::mutate_state(|s| {
                s.error_counters.amm = s.error_counters.amm.saturating_add(1);
                update_cursor_error(s, cursors::CURSOR_ID_AMM_LIQUIDITY, e);
            });
            return;
        }
    };

    // Another timer invocation may have completed while the query was in
    // flight. Re-read durable progress and the mirror before appending.
    let current_id = cursors::amm_liquidity::get();
    let (unseen, next_cursor, gap) =
        select_unseen_events(current_id, window.oldest_available_id, &window.events);

    state::mutate_state(|s| {
        update_cursor_source_count(s, cursors::CURSOR_ID_AMM_LIQUIDITY, window.next_event_id);
        if gap {
            s.error_counters.amm = s.error_counters.amm.saturating_add(1);
            update_cursor_error(
                s,
                cursors::CURSOR_ID_AMM_LIQUIDITY,
                format!(
                    "AMM liquidity history gap: next wanted event ID {} is older than retained ID {}; resuming at oldest retained event",
                    current_id, window.oldest_available_id
                ),
            );
        }
    });

    let processed = unseen.len();
    for evt in unseen {
        evt_amm_liquidity::push(normalize_event(evt));
    }

    let cursor_after = next_cursor.unwrap_or_else(|| {
        if gap {
            window.oldest_available_id
        } else {
            current_id
        }
    });
    if cursor_after > cursors::amm_liquidity::get() {
        cursors::amm_liquidity::set(cursor_after);
    }

    if processed > 0 {
        state::mutate_state(|s| {
            update_cursor_success(s, cursors::CURSOR_ID_AMM_LIQUIDITY, ic_cdk::api::time());
            // Keep the first retained-window loss visible after successfully
            // ingesting the available suffix. A later gap-free poll clears it.
            if gap {
                update_cursor_error(
                    s,
                    cursors::CURSOR_ID_AMM_LIQUIDITY,
                    format!(
                        "AMM liquidity history gap: next wanted event ID {} is older than retained ID {}; resumed at oldest retained event",
                        current_id, window.oldest_available_id
                    ),
                );
            }
        });
    }
}

pub async fn run() {
    let (complete, cursor, target) = cutover_snapshot();
    if complete {
        run_current_generation().await;
    } else {
        run_generation_cutover(cursor, target).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use candid::Principal;

    fn event(id: u64) -> sources::amm::AmmLiquidityEvent {
        sources::amm::AmmLiquidityEvent {
            id,
            caller: Principal::anonymous(),
            pool_id: "pool".to_string(),
            action: sources::amm::AmmLiquidityAction::AddLiquidity,
            token_a: Principal::anonymous(),
            amount_a: candid::Nat::from(1u8),
            token_b: Principal::anonymous(),
            amount_b: candid::Nat::from(1u8),
            lp_shares: candid::Nat::from(1u8),
            timestamp: id,
        }
    }

    fn event_in_generation(id: u64, timestamp_base: u64) -> sources::amm::AmmLiquidityEvent {
        let mut event = event(id);
        event.timestamp = timestamp_base + id;
        event
    }

    #[test]
    fn absolute_cursor_continues_across_retention_wrap_without_duplicates() {
        // At the first wrap, the old positional cursor is 50_000 while the
        // source now retains IDs 1..=50_000. The ID page still selects only
        // the newly appended event.
        let window = vec![event(50_000)];
        let (unseen, next, gap) = select_unseen_events(50_000, 1, &window);
        assert!(!gap);
        assert_eq!(
            unseen.iter().map(|event| event.id).collect::<Vec<_>>(),
            [50_000]
        );
        assert_eq!(next, Some(50_001));

        // The following poll continues from the absolute ID, despite the
        // retained vector having shifted again.
        let window = vec![event(50_000), event(50_001)];
        let (unseen, next, gap) = select_unseen_events(50_001, 2, &window);
        assert!(!gap);
        assert_eq!(
            unseen.iter().map(|event| event.id).collect::<Vec<_>>(),
            [50_001]
        );
        assert_eq!(next, Some(50_002));
    }

    #[test]
    fn skipped_retained_ids_are_reported_and_resume_from_oldest() {
        let window = vec![event(100_000), event(100_001)];
        let (unseen, next, gap) = select_unseen_events(50_000, 100_000, &window);
        assert!(gap);
        assert_eq!(
            unseen.iter().map(|event| event.id).collect::<Vec<_>>(),
            [100_000, 100_001]
        );
        assert_eq!(next, Some(100_002));
    }

    #[test]
    fn generation_reset_cutover_preserves_colliding_ids_and_continues_at_five() {
        let mut existing: Vec<_> = (0..7)
            .map(|id| normalize_event(&event_in_generation(id, 100)))
            .collect();
        let current_events: Vec<_> = (0..5).map(|id| event_in_generation(id, 1_000)).collect();
        let window = sources::amm::AmmLiquidityEventWindow {
            oldest_available_id: 0,
            next_event_id: 5,
            events: current_events,
        };

        let plan = prepare_cutover_page(0, None, &window, |candidate| existing.contains(candidate))
            .expect("current generation is fully retained");
        assert_eq!(plan.cursor_after, 5);
        assert_eq!(plan.target, 5);
        assert!(plan.complete);
        assert_eq!(plan.rows_to_append.len(), 5);

        existing.extend(plan.rows_to_append);
        assert_eq!(existing.len(), 12);
        for id in 0..5 {
            assert!(existing.contains(&normalize_event(&event_in_generation(id, 1_000))));
        }

        let future = vec![event_in_generation(5, 1_000), event_in_generation(6, 1_000)];
        let (unseen, next, gap) = select_unseen_events(5, 0, &future);
        assert!(!gap);
        assert_eq!(
            unseen.iter().map(|event| event.id).collect::<Vec<_>>(),
            [5, 6]
        );
        assert_eq!(next, Some(7));
    }

    #[test]
    fn generation_cutover_replay_is_idempotent_by_full_row_fingerprint() {
        let current_events: Vec<_> = (0..5).map(|id| event_in_generation(id, 1_000)).collect();
        let existing: Vec<_> = current_events.iter().map(normalize_event).collect();
        let window = sources::amm::AmmLiquidityEventWindow {
            oldest_available_id: 0,
            next_event_id: 5,
            events: current_events,
        };

        let replay =
            prepare_cutover_page(0, None, &window, |candidate| existing.contains(candidate))
                .expect("an exact replay still advances the one-time cursor");
        assert!(replay.rows_to_append.is_empty());
        assert_eq!(replay.cursor_after, 5);
        assert!(replay.complete);
    }

    #[test]
    fn generation_cutover_fails_closed_when_retained_prefix_is_missing() {
        let window = sources::amm::AmmLiquidityEventWindow {
            oldest_available_id: 2,
            next_event_id: 5,
            events: (2..5).map(|id| event_in_generation(id, 1_000)).collect(),
        };

        let error = prepare_cutover_page(0, None, &window, |_| false)
            .expect_err("missing current-generation IDs 0 and 1 are ambiguous");
        assert!(error.contains("cannot prove retained prefix"));
    }

    #[test]
    fn generation_cutover_keeps_frozen_target_when_source_grows_between_pages() {
        let window = sources::amm::AmmLiquidityEventWindow {
            oldest_available_id: 0,
            next_event_id: 7,
            events: (0..7).map(|id| event_in_generation(id, 1_000)).collect(),
        };

        let plan = prepare_cutover_page(0, Some(5), &window, |_| false)
            .expect("events appended after the frozen target belong to normal tailing");
        assert_eq!(plan.cursor_after, 5);
        assert_eq!(plan.target, 5);
        assert!(plan.complete);
        assert_eq!(
            plan.rows_to_append
                .iter()
                .map(|row| row.source_event_id)
                .collect::<Vec<_>>(),
            [0, 1, 2, 3, 4]
        );
    }

    #[test]
    fn stale_generation_cutover_callback_cannot_commit_after_an_interleaving_winner() {
        assert!(cutover_snapshot_matches((false, 0, None), 0, None));
        assert!(!cutover_snapshot_matches((false, 5, Some(5)), 0, None));
        assert!(!cutover_snapshot_matches((true, 5, Some(5)), 0, None));
    }
}
