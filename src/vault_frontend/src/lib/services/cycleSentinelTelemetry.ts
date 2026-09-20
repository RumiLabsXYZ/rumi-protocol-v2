import type { PublicTargetRow } from '$declarations/rumi_cycle_sentinel/rumi_cycle_sentinel.did';

function stateName(state: Record<string, unknown>): string {
	return Object.keys(state)[0] ?? 'Unknown';
}

/**
 * The canister uses `Unreachable` as its fail-closed placeholder before an
 * enabled target has received its first scheduled sample. Keep that distinct
 * from an actual unsuccessful observation, which always has an `as_of_secs`.
 */
export function targetStateLabel(row: PublicTargetRow): string {
	const state = stateName(row.state);
	const awaitingFirstSample =
		state === 'Unreachable' && row.as_of_secs === 0n && row.last_success_at_secs.length === 0;
	return awaitingFirstSample ? 'Awaiting first sample' : state;
}
