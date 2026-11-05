import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const { feeActor } = vi.hoisted(() => ({
	feeActor: { get_fee_series: vi.fn() },
}));

vi.mock('@dfinity/agent', () => ({
	Actor: { createActor: vi.fn(() => feeActor) },
	HttpAgent: vi.fn(),
}));
vi.mock('$lib/config', () => ({ CONFIG: { analyticsIDL: {} }, CANISTER_IDS: { ANALYTICS: 'aaaaa-aa' } }));

import { fetchFeeSeries, invalidateAnalyticsCache } from './analyticsService';

describe('analytics fee series failure mode', () => {
	beforeEach(() => {
		feeActor.get_fee_series.mockReset();
		invalidateAnalyticsCache('analytics:fee:');
		vi.spyOn(console, 'error').mockImplementation(() => {});
	});
	afterEach(() => vi.restoreAllMocks());

	it('preserves fail-soft legacy calls while allowing the Revenue lens to distinguish errors', async () => {
		feeActor.get_fee_series.mockRejectedValueOnce(new Error('analytics unavailable'));
		await expect(fetchFeeSeries(90)).resolves.toEqual([]);

		feeActor.get_fee_series.mockRejectedValueOnce(new Error('analytics unavailable'));
		await expect(fetchFeeSeries(90, { throwOnError: true })).rejects.toThrow('analytics unavailable');

		feeActor.get_fee_series.mockResolvedValueOnce({ rows: [] });
		await expect(fetchFeeSeries(90, { throwOnError: true })).resolves.toEqual([]);
		expect(feeActor.get_fee_series).toHaveBeenCalledTimes(3);
	});

	it('returns successful empty rows as empty data', async () => {
		feeActor.get_fee_series.mockResolvedValueOnce({ rows: [] });

		await expect(fetchFeeSeries(90, { throwOnError: true })).resolves.toEqual([]);
	});
});
