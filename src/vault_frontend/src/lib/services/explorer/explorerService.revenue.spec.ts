import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const { publicActor } = vi.hoisted(() => ({
	publicActor: {
		get_events_filtered: vi.fn(),
		get_event_timestamps: vi.fn(),
		get_event_count: vi.fn(),
		get_events_forward_filtered: vi.fn(),
	},
}));

vi.mock('$services/protocol/apiClient', () => ({ publicActor }));
vi.mock('@dfinity/agent', () => ({
	Actor: { createActor: vi.fn(() => ({})), canisterIdOf: () => ({ toText: () => 'backend-test' }) },
	HttpAgent: vi.fn(),
}));
vi.mock('./collateralConfigQuery', () => ({}));
vi.mock('./spDepositorQuery', () => ({}));
vi.mock('$services/stabilityPoolService', () => ({ stabilityPoolService: {} }));
vi.mock('$services/threePoolService', () => ({ threePoolService: {} }));
vi.mock('$services/ammService', () => ({ ammService: {} }));

import { createProtocolFeeScan, invalidateCache } from './explorerService';
import { FEE_WINDOWS_NS, formatFeeE8s, completeFee } from './feeScan';

const now = 1_793_750_400_000;
const ns = BigInt(now) * 1_000_000n;
const borrow = (fee = 100n, timestamp: bigint[] = [ns]) => ({ borrow_from_vault: { fee_amount: fee, timestamp } });
const redemption = (fee = 200n) => ({ redemption_on_vaults: { fee_amount: fee, timestamp: [ns] } });
const page = (next_start: bigint, events: unknown[] = [], reached_end = false) => ({ next_start, events, reached_end });
const deferred = () => {
	let resolve!: (value: any) => void;
	const promise = new Promise<any>(r => { resolve = r; });
	return { promise, resolve };
};

beforeEach(() => {
	vi.spyOn(Date, 'now').mockReturnValue(now);
	for (const method of Object.values(publicActor)) method.mockReset();
	publicActor.get_event_count.mockResolvedValue(0n);
	invalidateCache();
});
afterEach(() => vi.restoreAllMocks());

describe('cursor fee snapshots', () => {
	it('shares one in-flight stream for both windows, resumes past 25 sparse pages and freezes the tail', async () => {
		publicActor.get_event_count.mockResolvedValue(50_001n);
		publicActor.get_events_forward_filtered.mockImplementation(async (start: bigint, scan: bigint) =>
			page(start + scan, start === 50_000n ? [[start, borrow()]] : []));
		const scan = createProtocolFeeScan();
		const first = scan.run();
		expect(scan.run()).toBe(first);
		const incomplete = await first;
		expect(incomplete).toMatchObject({ phase: 'incomplete', cursor: 50_000n, end: 50_001n });
		expect(completeFee(incomplete, 0, 'borrow')).toBeNull();
		expect(publicActor.get_events_forward_filtered).toHaveBeenCalledTimes(25);
		const done = await scan.run();
		expect(done.phase).toBe('complete'); // false reached_end is OK: live log grew past our H.
		expect(done.windows.map(w => w.borrow)).toEqual([100n, 100n]);
		expect(publicActor.get_event_count).toHaveBeenCalledTimes(1);
		expect(publicActor.get_events_forward_filtered).toHaveBeenLastCalledWith(50_000n, 1n, [[{ Borrow: null }, { Redemption: null }]]);
		expect(publicActor.get_events_filtered).not.toHaveBeenCalled();
	});

	it('filters inclusive time boundaries without assuming timestamp order and formats huge totals exactly', async () => {
		const big = 9_007_199_254_740_993n;
		publicActor.get_event_count.mockResolvedValue(8n);
		publicActor.get_events_forward_filtered.mockResolvedValue(page(8n, [
			[0n, borrow(1n, [ns - FEE_WINDOWS_NS[1] - 1n])],
			[1n, borrow(big)], [2n, borrow(3n, [ns - FEE_WINDOWS_NS[0]])],
			[3n, borrow(5n, [ns - FEE_WINDOWS_NS[0] - 1n])],
			[4n, borrow(7n, [ns - FEE_WINDOWS_NS[1]])],
			[5n, borrow(11n, [ns + 1n])], [6n, redemption()],
			[7n, { redemption_transfered: {} }],
		], true));
		const result = await createProtocolFeeScan().run();
		expect(result.windows.map(w => w.borrow)).toEqual([big + 3n, big + 15n]);
		expect(result.windows[0]).toMatchObject({ borrowCount: 2n, redemption: 200n, redemptionCount: 1n });
		expect(formatFeeE8s(result.windows[0].borrow)).toBe('90071992.54740996');
	});

	it('counts distinct IDs with equal timestamps and leaves undated amounts explicitly unknown by category', async () => {
		publicActor.get_event_count.mockResolvedValue(3n);
		publicActor.get_events_forward_filtered.mockResolvedValue(page(3n, [[0n, borrow()], [1n, borrow()], [2n, borrow(300n, [])]], true));
		const result = await createProtocolFeeScan().run();
		expect(result.windows[0]).toMatchObject({ borrow: 200n, borrowCount: 2n });
		expect(result.undated.borrow).toEqual({ count: 1n, amount: 300n });
		expect(completeFee(result, 0, 'borrow')).toBeNull();
		expect(completeFee(result, 0, 'redemption')).toBe(0n);
		await createProtocolFeeScan().run();
		expect(publicActor.get_event_count).toHaveBeenCalledTimes(2); // never cache unknown totals as success
	});

	it.each(['first', 'later'])('retries a %s-page failure at the last committed cursor without double counting', async where => {
		publicActor.get_event_count.mockResolvedValue(2_001n);
		if (where === 'later') publicActor.get_events_forward_filtered.mockResolvedValueOnce(page(2_000n, [[0n, borrow()]]));
		publicActor.get_events_forward_filtered.mockRejectedValueOnce(new Error('unavailable'));
		const scan = createProtocolFeeScan();
		const failed = await scan.run();
		expect(failed.phase).toBe('error');
		expect(failed.cursor).toBe(where === 'later' ? 2_000n : 0n);
		expect(completeFee(failed, 0, 'borrow')).toBeNull();
		publicActor.get_events_forward_filtered.mockImplementation(async (start: bigint, size: bigint) => page(start + size, start === 0n ? [[0n, borrow()]] : [[2_000n, redemption()]], start + size === 2_001n));
		const done = await scan.run();
		expect(done.windows[0]).toMatchObject({ borrow: 100n, redemption: 200n });
		expect(publicActor.get_event_count).toHaveBeenCalledTimes(1);
	});

	it.each([
		['duplicate', page(2_000n, [[0n, borrow()], [0n, borrow()]])],
		['out of range', page(2_000n, [[2_000n, borrow()]])],
		['nonadvancing', page(0n)], ['skipped cursor', page(2_001n)],
		['premature tail', page(2_000n, [], true)],
		['missing fee', page(2_000n, [[0n, borrow()], [1n, { borrow_from_vault: { timestamp: [ns] } }]])],
		['invalid timestamp', page(2_000n, [[0n, borrow(1n, [ns, ns])]])],
		['unexpected variant', page(2_000n, [[0n, { repay_to_vault: {} }]])],
	])('rejects %s atomically', async (_, response) => {
		publicActor.get_event_count.mockResolvedValue(2_001n);
		publicActor.get_events_forward_filtered.mockResolvedValue(response);
		const result = await createProtocolFeeScan().run();
		expect(result).toMatchObject({ phase: 'error', cursor: 0n });
		expect(result.windows[0].borrow).toBe(0n);
	});

	it('returns verified zero only after successful full coverage, caches briefly and refresh starts anew', async () => {
		const zero = await createProtocolFeeScan().run();
		expect(completeFee(zero, 0, 'borrow')).toBe(0n);
		expect(publicActor.get_events_forward_filtered).not.toHaveBeenCalled();
		await createProtocolFeeScan().run();
		expect(publicActor.get_event_count).toHaveBeenCalledTimes(1);
		await createProtocolFeeScan({ refresh: true }).run();
		expect(publicActor.get_event_count).toHaveBeenCalledTimes(2);
		vi.mocked(Date.now).mockReturnValue(now + 10_001);
		await createProtocolFeeScan().run();
		expect(publicActor.get_event_count).toHaveBeenCalledTimes(3);
	});

	it('keeps a failed count unknown and retries instead of caching zero', async () => {
		publicActor.get_event_count.mockRejectedValueOnce(new Error('offline'));
		const scan = createProtocolFeeScan();
		expect(await scan.run()).toMatchObject({ end: null, phase: 'error' });
		expect(await scan.run()).toMatchObject({ end: 0n, phase: 'complete' });
	});

	it.each(['count', 'page'])('cancels during a pending %s and discards late replies', async stage => {
		const pending = deferred();
		publicActor.get_event_count.mockReturnValue(stage === 'count' ? pending.promise : Promise.resolve(3_000n));
		publicActor.get_events_forward_filtered.mockReturnValue(pending.promise);
		const scan = createProtocolFeeScan(), progress = vi.fn();
		const running = scan.run(progress);
		await vi.waitFor(() => expect(stage === 'count' ? publicActor.get_event_count : publicActor.get_events_forward_filtered).toHaveBeenCalledTimes(1));
		scan.cancel(); const updates = progress.mock.calls.length;
		pending.resolve(stage === 'count' ? 3_000n : page(2_000n, [[0n, borrow()]]));
		expect(await running).toMatchObject({ phase: 'cancelled', cursor: 0n });
		expect(progress).toHaveBeenCalledTimes(updates);
		expect(publicActor.get_events_forward_filtered).toHaveBeenCalledTimes(stage === 'count' ? 0 : 1);
	});
});
