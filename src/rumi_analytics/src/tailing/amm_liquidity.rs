//! AMM liquidity event tailing. Mirrors AddLiquidity / RemoveLiquidity events
//! into evt_amm_liquidity so the address-value query can reconstruct per-
//! principal LP-share timelines without making inter-canister calls.

use super::{update_cursor_error, update_cursor_source_count, update_cursor_success, BATCH_SIZE};
use crate::{sources, state, storage};
use storage::cursors;
use storage::events::*;

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

fn last_mirrored_id() -> Option<u64> {
    let len = evt_amm_liquidity::len();
    len.checked_sub(1)
        .and_then(evt_amm_liquidity::get)
        .map(|row| row.source_event_id)
}

pub async fn run() {
    // The old cursor was a retained-vector position. Seed the new absolute-ID
    // cursor from it on upgrade; the mirrored log's last source ID repairs a
    // possible append-before-cursor-write interruption.
    let requested_id = cursors::amm_liquidity::get()
        .max(cursors::amm_liquidity_legacy::get())
        .max(last_mirrored_id().map_or(0, |id| id.saturating_add(1)));

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
    let current_id = cursors::amm_liquidity::get()
        .max(cursors::amm_liquidity_legacy::get())
        .max(last_mirrored_id().map_or(0, |id| id.saturating_add(1)));
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
        evt_amm_liquidity::push(AnalyticsAmmLiquidityEvent {
            timestamp_ns: evt.timestamp,
            source_event_id: evt.id,
            caller: evt.caller,
            pool_id: evt.pool_id.clone(),
            action: convert_action(&evt.action),
            lp_shares: nat_to_u64(&evt.lp_shares),
        });
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
}
