import type { SwapRoute } from '../../services/swapRouter';
import type { ProviderId } from '../../services/providers/types';

export function providerLabel(id: ProviderId): string {
	switch (id) {
		case 'rumi_amm':
			return 'Rumi AMM';
		case 'icpswap_3usd_icp':
			return 'ICPswap 3USD/ICP';
		case 'icpswap_icusd_icp':
			return 'ICPswap icUSD/ICP';
		case 'icpswap_ckusdt_icusd':
			return 'ICPswap ckUSDT/icUSD';
		case 'icpswap_icusd_ckusdc':
			return 'ICPswap icUSD/ckUSDC';
		case 'icpswap_ckusdt_ckusdc':
			return 'ICPswap ckUSDT/ckUSDC';
	}
}

export function routeVenueLabel(route: SwapRoute): string {
	const provider = route.providerQuote?.provider ?? route.hopProviderQuote?.provider;
	const selectedVenue = provider ? providerLabel(provider) : null;

	switch (route.type) {
		case 'three_pool_swap':
		case 'three_pool_deposit':
		case 'three_pool_redeem':
			return 'Rumi 3pool';
		case 'amm_swap':
			return selectedVenue ?? 'Venue unavailable';
		case 'icpswap_stable_direct':
		case 'icusd_icp_direct':
			return selectedVenue ?? 'Venue unavailable';
		case 'stable_to_icp':
		case 'stable_to_icp_via_icusd':
			return `Rumi 3pool → ${selectedVenue ?? 'Venue unavailable'}`;
		case 'icp_to_stable':
		case 'icp_to_stable_via_icusd':
			return `${selectedVenue ?? 'Venue unavailable'} → Rumi 3pool`;
	}
}
