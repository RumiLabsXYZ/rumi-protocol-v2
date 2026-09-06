import { describe, expect, it } from 'vitest';
import type { SwapRoute, RouteType } from '../../services/swapRouter';
import type { ProviderId, ProviderQuote } from '../../services/providers/types';
import { routeVenueLabel } from './routePresentation';

function quote(provider: ProviderId): ProviderQuote {
	return {
		provider,
		label: provider,
		amountOut: 1n,
		feeDisplay: '0.30%',
		priceImpactBps: 0,
		meta: {}
	};
}

function route(type: RouteType, provider?: ProviderId, hopProvider?: ProviderId): SwapRoute {
	return {
		type,
		pathDisplay: 'A → B',
		hops: hopProvider ? 2 : 1,
		estimatedOutput: 1n,
		grossOutput: 1n,
		feeDisplay: '0.30%',
		providerQuote: provider ? quote(provider) : undefined,
		hopProviderQuote: hopProvider ? quote(hopProvider) : undefined
	};
}

describe('routeVenueLabel', () => {
	it.each(['three_pool_swap', 'three_pool_deposit', 'three_pool_redeem'] satisfies RouteType[])(
		'labels direct %s routes as Rumi 3pool',
		(type) => {
			expect(routeVenueLabel(route(type))).toBe('Rumi 3pool');
		}
	);

	it('labels direct provider routes with the selected venue', () => {
		expect(routeVenueLabel(route('icpswap_stable_direct', 'icpswap_ckusdt_icusd'))).toBe(
			'ICPswap ckUSDT/icUSD'
		);
		expect(routeVenueLabel(route('amm_swap', 'rumi_amm'))).toBe('Rumi AMM');
		expect(routeVenueLabel(route('icusd_icp_direct', 'icpswap_icusd_icp'))).toBe(
			'ICPswap icUSD/ICP'
		);
	});

	it('lists multi-hop venues in execution order', () => {
		expect(routeVenueLabel(route('stable_to_icp', undefined, 'icpswap_3usd_icp'))).toBe(
			'Rumi 3pool → ICPswap 3USD/ICP'
		);
		expect(routeVenueLabel(route('icp_to_stable', undefined, 'rumi_amm'))).toBe(
			'Rumi AMM → Rumi 3pool'
		);
		expect(routeVenueLabel(route('stable_to_icp_via_icusd', undefined, 'icpswap_icusd_icp'))).toBe(
			'Rumi 3pool → ICPswap icUSD/ICP'
		);
		expect(routeVenueLabel(route('icp_to_stable_via_icusd', undefined, 'icpswap_icusd_icp'))).toBe(
			'ICPswap icUSD/ICP → Rumi 3pool'
		);
	});

	it('does not guess a venue when provider metadata is missing', () => {
		expect(routeVenueLabel(route('amm_swap'))).toBe('Venue unavailable');
		expect(routeVenueLabel(route('icpswap_stable_direct'))).toBe('Venue unavailable');
		expect(routeVenueLabel(route('stable_to_icp'))).toBe('Rumi 3pool → Venue unavailable');
		expect(routeVenueLabel(route('icp_to_stable'))).toBe('Venue unavailable → Rumi 3pool');
	});

	it('returns a non-empty venue label for every route type', () => {
		const routes: SwapRoute[] = [
			route('three_pool_swap'),
			route('icpswap_stable_direct', 'icpswap_ckusdt_ckusdc'),
			route('three_pool_deposit'),
			route('three_pool_redeem'),
			route('amm_swap', 'rumi_amm'),
			route('stable_to_icp', undefined, 'icpswap_3usd_icp'),
			route('icp_to_stable', undefined, 'icpswap_3usd_icp'),
			route('stable_to_icp_via_icusd', undefined, 'icpswap_icusd_icp'),
			route('icp_to_stable_via_icusd', undefined, 'icpswap_icusd_icp'),
			route('icusd_icp_direct', 'icpswap_icusd_icp')
		];

		expect(routes.map(routeVenueLabel).every((label) => label.length > 0)).toBe(true);
	});
});
