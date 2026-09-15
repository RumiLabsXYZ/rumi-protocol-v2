import { describe, expect, it, vi } from 'vitest';
import type {
	PublicAlarm,
	PublicOverview,
	PublicTargetRow
} from '../../../../declarations/rumi_cycle_sentinel/rumi_cycle_sentinel.did';
import {
	PUBLIC_QUERY_METHODS,
	TARGET_STATE_PRESENTATION,
	formatCycles,
	formatIcpE8s,
	loadTelemetry,
	refreshTelemetry,
	sentinelCanisterId,
	targetState,
	type SentinelActor
} from './cycleSentinel';

const overview = {} as PublicOverview;
const target = { state: { Healthy: null } } as PublicTargetRow;
const alarm = {} as PublicAlarm;

describe('Cycle Sentinel public client', () => {
	it('preserves bigint precision above Number.MAX_SAFE_INTEGER', () => {
		expect(formatCycles(9_007_199_254_740_993n)).toBe('9,007.199254740993 T');
		expect(formatIcpE8s(9_007_199_254_740_993n)).toBe('90,071,992.54740993 ICP');
	});

	it('represents all six public target states', () => {
		expect(Object.keys(TARGET_STATE_PRESENTATION).sort()).toEqual(
			['Healthy', 'Low', 'Stopped', 'Uninstalled', 'Unobserved', 'Unreachable'].sort()
		);
		for (const state of Object.keys(TARGET_STATE_PRESENTATION)) {
			expect(targetState({ state: { [state]: null } } as PublicTargetRow)).toBe(state);
		}
	});

	it('paginates targets and alarms until the cursor is exhausted', async () => {
		const actor = {
			get_public_overview: vi.fn().mockResolvedValue(overview),
			list_public_targets: vi
				.fn()
				.mockResolvedValueOnce({ Ok: { items: [target], next_cursor: ['target-2'] } })
				.mockResolvedValueOnce({ Ok: { items: [target], next_cursor: [] } }),
			list_public_alarms: vi
				.fn()
				.mockResolvedValueOnce({ Ok: { items: [alarm], next_cursor: ['alarm-2'] } })
				.mockResolvedValueOnce({ Ok: { items: [], next_cursor: [] } })
		} as unknown as SentinelActor;

		const result = await loadTelemetry(actor);
		expect(result.targets).toHaveLength(2);
		expect(result.alarms).toHaveLength(1);
		expect(actor.list_public_targets).toHaveBeenNthCalledWith(2, ['target-2'], 100);
		expect(actor.list_public_alarms).toHaveBeenNthCalledWith(2, ['alarm-2'], 100);
	});

	it('retains prior data as stale when refresh fails', async () => {
		const previous = { overview, targets: [], alarms: [], refreshedAt: new Date(0) };
		const result = await refreshTelemetry(previous, async () => {
			throw new Error('unreachable');
		});
		expect(result.snapshot).toBe(previous);
		expect(result.stale).toBe(true);
		expect(result.error).toBe('unreachable');
	});

	it('supports empty pages and an explicit not-configured state', async () => {
		const actor = {
			get_public_overview: vi.fn().mockResolvedValue(overview),
			list_public_targets: vi.fn().mockResolvedValue({ Ok: { items: [], next_cursor: [] } }),
			list_public_alarms: vi.fn().mockResolvedValue({ Ok: { items: [], next_cursor: [] } })
		} as unknown as SentinelActor;
		expect((await loadTelemetry(actor)).targets).toEqual([]);
		expect(sentinelCanisterId('')).toBeUndefined();
		expect(sentinelCanisterId('  aaaaa-aa  ')).toBe('aaaaa-aa');
	});

	it('exposes only anonymous read methods', () => {
		expect(PUBLIC_QUERY_METHODS).toEqual([
			'get_public_overview',
			'list_public_targets',
			'list_public_alarms'
		]);
		expect(PUBLIC_QUERY_METHODS.join(' ')).not.toMatch(/top_up|pause|proposal|execute|approve/i);
	});

	it('marks an unreachable row without inventing a zero balance', () => {
		const row = {
			state: { Unreachable: null },
			advisory_balance_cycles: []
		} as unknown as PublicTargetRow;
		expect(targetState(row)).toBe('Unreachable');
		expect(formatCycles(row.advisory_balance_cycles[0])).toBe('Not available');
	});
});
