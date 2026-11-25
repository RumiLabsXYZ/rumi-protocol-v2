import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, tick, unmount } from 'svelte';

const fx = vi.hoisted(() => ({
	fetchFeeSeries: vi.fn(),
	fetchApys: vi.fn(),
	fetchFeeBreakdownWindow: vi.fn(),
	fetchInterestSplit: vi.fn(),
	fetchStabilityPoolStatus: vi.fn(),
	fetchThreePoolStatus: vi.fn(),
	createProtocolFeeScan: vi.fn(),
	fetchEvents: vi.fn(),
	fetchAllVaults: vi.fn(),
	fetchThreePoolSwapEventsCombined: vi.fn(),
	fetch3PoolLiquidityEventsCombined: vi.fn(),
	fetchAmmSwapEvents: vi.fn(),
	fetchAmmSwapEventCount: vi.fn(),
	fetchAmmLiquidityEvents: vi.fn(),
	fetchAmmLiquidityEventCount: vi.fn(),
	fetchAmmAdminEvents: vi.fn(),
	fetchAmmAdminEventCount: vi.fn(),
	fetch3PoolAdminEvents: vi.fn(),
	fetch3PoolAdminEventCount: vi.fn(),
	fetchStabilityPoolEvents: vi.fn(),
	fetchStabilityPoolEventCount: vi.fn(),
	fetchTreasuryHoldings: vi.fn(),
	fetchProtocolReserves: vi.fn(),
	getProtocolStatus: vi.fn(),
}));

vi.mock('$services/explorer/analyticsService', () => ({
	fetchFeeSeries: fx.fetchFeeSeries,
	fetchApys: fx.fetchApys,
	fetchFeeBreakdownWindow: fx.fetchFeeBreakdownWindow,
}));
vi.mock('$services/explorer/explorerService', () => ({
	fetchInterestSplit: fx.fetchInterestSplit,
	fetchStabilityPoolStatus: fx.fetchStabilityPoolStatus,
	fetchThreePoolStatus: fx.fetchThreePoolStatus,
	createProtocolFeeScan: fx.createProtocolFeeScan,
	fetchEvents: fx.fetchEvents,
	fetchAllVaults: fx.fetchAllVaults,
	fetchThreePoolSwapEventsCombined: fx.fetchThreePoolSwapEventsCombined,
	fetch3PoolLiquidityEventsCombined: fx.fetch3PoolLiquidityEventsCombined,
	fetchAmmSwapEvents: fx.fetchAmmSwapEvents,
	fetchAmmSwapEventCount: fx.fetchAmmSwapEventCount,
	fetchAmmLiquidityEvents: fx.fetchAmmLiquidityEvents,
	fetchAmmLiquidityEventCount: fx.fetchAmmLiquidityEventCount,
	fetchAmmAdminEvents: fx.fetchAmmAdminEvents,
	fetchAmmAdminEventCount: fx.fetchAmmAdminEventCount,
	fetch3PoolAdminEvents: fx.fetch3PoolAdminEvents,
	fetch3PoolAdminEventCount: fx.fetch3PoolAdminEventCount,
	fetchStabilityPoolEvents: fx.fetchStabilityPoolEvents,
	fetchStabilityPoolEventCount: fx.fetchStabilityPoolEventCount,
	fetchTreasuryHoldings: fx.fetchTreasuryHoldings,
	fetchProtocolReserves: fx.fetchProtocolReserves,
}));
vi.mock('$services/protocol', () => ({ ProtocolService: { getProtocolStatus: fx.getProtocolStatus } }));
vi.mock('$utils/liveApy', () => ({ liveSpApyPct: () => null, liveLpApyPct: () => null }));

import RevenueLens from './RevenueLens.svelte';
import { parseFacetsFromUrl } from '$utils/eventFacets';
import { newFeeScan, type FeeScanState } from '$services/explorer/feeScan';

let host: HTMLDivElement;
let instance: unknown;
function result(phase: FeeScanState['phase'] = 'complete'): FeeScanState {
	return { ...newFeeScan('backend-test', 1_793_750_400_000_000_000n), end: 50_001n,
		cursor: phase === 'complete' ? 50_001n : 50_000n, phase };
}
function controller(...states: FeeScanState[]) {
	let current = newFeeScan('backend-test', 1_793_750_400_000_000_000n);
	return {
		snapshot: () => current,
		run: vi.fn(async (onProgress: (state: FeeScanState) => void) => {
			current = states.shift() ?? current;
			onProgress(current);
			return current;
		}),
		cancel: vi.fn(),
	};
}
function button(label: string) {
	const node = [...host.querySelectorAll('button')].find(el => el.textContent === label);
	expect(node).toBeTruthy();
	return node!;
}
function coverage() { return host.querySelector('[aria-label="Borrowing and redemption fee coverage"]')!; }


function render() {
	instance = mount(RevenueLens, { target: host });
	flushSync();
}

async function settle() {
	for (let i = 0; i < 5; i++) {
		await Promise.resolve();
		await tick();
		flushSync();
	}
}

beforeEach(() => {
	host = document.createElement('div');
	document.body.appendChild(host);
	for (const method of Object.values(fx)) method.mockReset();
	fx.fetchFeeSeries.mockResolvedValue([]);
	fx.fetchApys.mockResolvedValue(null);
	fx.fetchFeeBreakdownWindow.mockResolvedValue({ borrowIcusd: 0, redemptionIcusd: 0, swapIcusd: 0 });
	fx.fetchInterestSplit.mockResolvedValue([]);
	fx.fetchStabilityPoolStatus.mockResolvedValue(null);
	fx.fetchThreePoolStatus.mockResolvedValue(null);
	fx.createProtocolFeeScan.mockReturnValue(controller(result()));
	fx.fetchEvents.mockResolvedValue({ total: 0n, events: [] });
	fx.fetchAllVaults.mockResolvedValue([]);
	fx.fetchThreePoolSwapEventsCombined.mockResolvedValue([]);
	fx.fetch3PoolLiquidityEventsCombined.mockResolvedValue([]);
	fx.fetchAmmSwapEventCount.mockResolvedValue(0n);
	fx.fetchAmmLiquidityEventCount.mockResolvedValue(0n);
	fx.fetchAmmAdminEventCount.mockResolvedValue(0n);
	fx.fetch3PoolAdminEventCount.mockResolvedValue(0n);
	fx.fetchStabilityPoolEventCount.mockResolvedValue(0n);
	fx.fetchTreasuryHoldings.mockResolvedValue([]);
	fx.fetchProtocolReserves.mockResolvedValue([]);
	fx.getProtocolStatus.mockResolvedValue(null);
});

afterEach(() => {
	if (instance) {
		unmount(instance as any);
		instance = undefined;
	}
	host.remove();
	vi.restoreAllMocks();
});

describe('RevenueLens fee chart and activity link', () => {
	it('renders unavailable for a failed fee-series read', async () => {
		fx.fetchFeeSeries.mockRejectedValueOnce(new Error('analytics unavailable'));
		render();
		await settle();

		const chart = host.querySelector('[aria-label="Daily swap fees over 90 days"]');
		expect(chart?.textContent).toContain('Swap fee history unavailable.');
		expect(chart?.textContent).not.toContain('No data');
	});

	it('renders successful empty series as no data and includes the supported Borrow activity filter', async () => {
		fx.fetchFeeSeries.mockResolvedValueOnce([]);
		render();
		await settle();

		const chart = host.querySelector('[aria-label="Daily swap fees over 90 days"]');
		expect(chart?.textContent).toContain('No data');
		expect(chart?.textContent).not.toContain('Swap fee history unavailable.');

		const activityLink = host.querySelector<HTMLAnchorElement>('a[href^="/explorer/activity"]');
		expect(activityLink).toBeTruthy();
		const facets = parseFacetsFromUrl(new URL(activityLink!.href));
		expect(facets.types).toContain('borrow');
		expect(facets.types).toContain('redemption');
		expect(facets.types).toContain('full_liquidation');
		expect(facets.types).toContain('amm_swap');
	});
});


describe('RevenueLens resumable fee totals', () => {
	it('shows partial amounts without claiming zero and continues the same snapshot to exact totals', async () => {
		const partial = result('incomplete'), complete = result();
		partial.windows[0].borrow = 100n;
		complete.windows[0].borrow = 9_007_199_254_740_993n;
		const scan = controller(partial, complete);
		fx.createProtocolFeeScan.mockReturnValue(scan);
		render(); await settle();
		expect(coverage().textContent).toContain('Incomplete — more history remains.');
		expect(coverage().textContent).toContain('Scanned 50000 of 50001 records.');
		expect(coverage().textContent).toContain('Unavailable — partial known-dated subtotal: 0.00000100 icUSD');
		button('Continue').click(); await settle();
		expect(coverage().textContent).toContain('90071992.54740993 icUSD');
		expect(coverage().textContent).toContain('0.00000000 icUSD');
		expect(coverage().textContent).not.toContain('Unavailable');
		expect(fx.createProtocolFeeScan).toHaveBeenCalledTimes(1);
		expect(scan.run).toHaveBeenCalledTimes(2);
	});

	it('keeps failed scans unavailable and retries the existing checkpoint', async () => {
		const scan = controller(result('error'), result());
		fx.createProtocolFeeScan.mockReturnValue(scan);
		render(); await settle();
		expect(coverage().textContent).toContain('Unavailable — scan failed.');
		button('Retry fee scan').click(); await settle();
		expect(coverage().textContent).not.toContain('Unavailable');
		expect(scan.run).toHaveBeenCalledTimes(2);
		expect(fx.createProtocolFeeScan).toHaveBeenCalledTimes(1);
	});

	it('explains undated records after full coverage while keeping unrelated swap data visible', async () => {
		const state = result();
		state.undated.borrow = { count: 2n, amount: 123n };
		fx.createProtocolFeeScan.mockReturnValue(controller(state));
		fx.fetchFeeBreakdownWindow.mockResolvedValue({ borrowIcusd: 0, redemptionIcusd: 0, swapIcusd: 7 });
		render(); await settle();
		expect(coverage().textContent).toContain('Incomplete — historical fee records have no timestamp.');
		expect(coverage().textContent).toContain('2 undated borrowing fee records (0.00000123 icUSD)');
		expect(coverage().textContent).toContain('Retrying does not recover missing timestamps.');
		expect(coverage().textContent).toContain('Unavailable — partial known-dated subtotal');
		expect(host.textContent).toMatch(/Swap\s+≈\$7/);
		expect([...host.querySelectorAll('button')].some(el => el.textContent === 'Continue')).toBe(false);
	});

	it('refreshes into a new snapshot and cancels work on unmount', async () => {
		const first = controller(result()), second = controller(result('scanning'));
		fx.createProtocolFeeScan.mockReturnValueOnce(first).mockReturnValueOnce(second);
		render(); await settle();
		button('Refresh fee snapshot').click(); await settle();
		expect(first.cancel).toHaveBeenCalledTimes(1);
		expect(fx.createProtocolFeeScan).toHaveBeenLastCalledWith({ refresh: true });
		expect(button('Refresh fee snapshot').disabled).toBe(true);
		await unmount(instance as any); instance = undefined;
		expect(second.cancel).toHaveBeenCalledTimes(1);
	});
});
