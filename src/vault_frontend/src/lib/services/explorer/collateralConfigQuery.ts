import type { Principal } from '@dfinity/principal';

export async function fetchDiscoveredCollateralConfigs<T>(
	supported: Array<[Principal, unknown]>,
	getConfig: (principal: Principal) => Promise<[] | [T]>
): Promise<T[]> {
	return Promise.all(
		supported.map(async ([principal]) => {
			const config = await getConfig(principal);
			if (config.length === 0) {
				throw new Error(`Missing collateral config for ${principal.toText()}`);
			}
			return config[0];
		})
	);
}

export async function fetchLegacyCollateralConfigs<T>(
	supported: Array<[Principal, unknown]>,
	getConfig: (principal: Principal) => Promise<[] | [T]>
): Promise<T[]> {
	const configs = await Promise.all(
		supported.map(async ([principal]) => {
			try {
				const config = await getConfig(principal);
				return config[0] ?? null;
			} catch {
				return null;
			}
		})
	);
	return configs.filter((config): config is T => config !== null);
}
