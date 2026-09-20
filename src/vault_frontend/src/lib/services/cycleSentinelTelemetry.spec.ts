import { describe, expect, it } from 'vitest';
import type { PublicTargetRow } from '$declarations/rumi_cycle_sentinel/rumi_cycle_sentinel.did';
import { targetStateLabel } from './cycleSentinelTelemetry';

function targetRow(overrides: Partial<PublicTargetRow> = {}): PublicTargetRow {
	return {
		state: { Unreachable: null },
		as_of_secs: 0n,
		last_success_at_secs: [],
		...overrides
	} as PublicTargetRow;
}

describe('targetStateLabel', () => {
	it('explains that an enabled target is waiting when no observation has run yet', () => {
		expect(targetStateLabel(targetRow())).toBe('Awaiting first sample');
	});

	it('preserves Unreachable after an actual observation attempt', () => {
		expect(targetStateLabel(targetRow({ as_of_secs: 1_789_754_999n }))).toBe('Unreachable');
	});

	it('preserves every non-placeholder target state', () => {
		expect(targetStateLabel(targetRow({ state: { Healthy: null } }))).toBe('Healthy');
		expect(targetStateLabel(targetRow({ state: { Unobserved: null } }))).toBe('Unobserved');
	});
});
