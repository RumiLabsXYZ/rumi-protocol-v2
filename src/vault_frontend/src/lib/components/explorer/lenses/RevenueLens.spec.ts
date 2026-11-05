import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, tick, unmount } from 'svelte';

const fx = vi.hoisted(() => ({
	fetchFeeSeries: vi.fn(),
	fetchApys: vi.fn(),
	fetchFeeBreakdownWindow: vi.fn(),
	fetchInterestSplit: vi.fn(),
	fetchStabilityPoolStatus: vi.fn(),
	fetchThreePoolStatus: vi.fn(),
	fetchProtocolFeeTotalsFromBackend: vi.fn(),
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
	fetchProtocolFeeTotalsFromBackend: fx.fetchProtocolFeeTotalsFromBackend,
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

let host: HTMLDivElement;
let instance: unknown;

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
	fx.fetchProtocolFeeTotalsFromBackend.mockResolvedValue({ borrowIcusd: 0, redemptionIcusd: 0, borrowCount: 0, redemptionCount: 0 });
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
