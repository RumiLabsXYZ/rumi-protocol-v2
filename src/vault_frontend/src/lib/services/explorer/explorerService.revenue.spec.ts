import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const { publicActor } = vi.hoisted(() => ({
	publicActor: {
		get_events_filtered: vi.fn(),
		get_event_timestamps: vi.fn(),
	},
}));

vi.mock('$services/protocol/apiClient', () => ({ publicActor }));
vi.mock('@dfinity/agent', () => ({
	Actor: { createActor: vi.fn(() => ({})) },
	HttpAgent: vi.fn(),
}));
vi.mock('./collateralConfigQuery', () => ({}));
vi.mock('./spDepositorQuery', () => ({}));
vi.mock('$services/stabilityPoolService', () => ({ stabilityPoolService: {} }));
vi.mock('$services/threePoolService', () => ({ threePoolService: {} }));
vi.mock('$services/ammService', () => ({ ammService: {} }));

import {
	fetchProtocolFeeTotalsFromBackend,
	invalidateCache,
} from './explorerService';

describe('authoritative explorer fee totals', () => {
	beforeEach(() => {
		publicActor.get_events_filtered.mockReset();
		publicActor.get_event_timestamps.mockReset();
		invalidateCache();
		publicActor.get_event_timestamps.mockResolvedValue([]);
		vi.spyOn(console, 'error').mockImplementation(() => {});
	});
	afterEach(() => vi.restoreAllMocks());

	it('pages filtered matches by page number and sums both pages once', async () => {
		publicActor.get_events_filtered
			.mockResolvedValueOnce({ total: 201n, events: Array.from({ length: 200 }, (_, i) => [BigInt(i), { borrow_from_vault: { fee_amount: 100n } }]) })
			.mockResolvedValueOnce({ total: 201n, events: [[200n, { redemption_on_vaults: { fee_amount: 200n } }]] });

		await expect(fetchProtocolFeeTotalsFromBackend(86_400_000)).resolves.toMatchObject({
			borrowIcusd: 0.0002,
			redemptionIcusd: 0.000002,
			borrowCount: 200,
			redemptionCount: 1,
		});
		expect(publicActor.get_events_filtered.mock.calls.map(([arg]) => arg.start)).toEqual([0n, 1n]);
		expect(publicActor.get_events_filtered.mock.calls[0][0].types).toEqual([[{ Borrow: null }, { Redemption: null }]]);
	});

	it.each(['first', 'later'] as const)('rejects a %s page failure without caching a zero or partial total', async (failurePage) => {
		if (failurePage === 'first') {
			publicActor.get_events_filtered.mockRejectedValueOnce(new Error('backend unavailable'));
		} else {
			publicActor.get_events_filtered
				.mockResolvedValueOnce({ total: 2n, events: [[0n, { borrow_from_vault: { fee_amount: 100n } }]] })
				.mockRejectedValueOnce(new Error('second page unavailable'));
		}

		await expect(fetchProtocolFeeTotalsFromBackend(86_400_000)).rejects.toThrow(/unavailable/);
		if (failurePage === 'first') {
			publicActor.get_events_filtered.mockResolvedValue({ total: 0n, events: [] });
			await expect(fetchProtocolFeeTotalsFromBackend(86_400_000)).resolves.toMatchObject({ borrowIcusd: 0, redemptionIcusd: 0 });
		} else {
			publicActor.get_events_filtered.mockResolvedValue({
				total: 2n,
				events: [[200n, { redemption_on_vaults: { fee_amount: 200n } }]],
			});
			await expect(fetchProtocolFeeTotalsFromBackend(86_400_000)).resolves.toMatchObject({ borrowIcusd: 0.000001, redemptionIcusd: 0.000002 });
		}
	});

	it('returns successful zero when the filtered query is empty', async () => {
		publicActor.get_events_filtered.mockResolvedValue({ total: 0n, events: [] });

		await expect(fetchProtocolFeeTotalsFromBackend(86_400_000)).resolves.toMatchObject({
			borrowIcusd: 0,
			redemptionIcusd: 0,
		});
	});

	it('rejects a bounded scan when matching events remain beyond the page cap', async () => {
		publicActor.get_events_filtered.mockResolvedValue({
			total: 5_001n,
			events: Array.from({ length: 200 }, (_, i) => [BigInt(i), { borrow_from_vault: { fee_amount: 100n } }]),
		});

		await expect(fetchProtocolFeeTotalsFromBackend(90 * 86_400_000)).rejects.toThrow(/incomplete/i);
		expect(publicActor.get_events_filtered).toHaveBeenCalledTimes(25);
	});
});
