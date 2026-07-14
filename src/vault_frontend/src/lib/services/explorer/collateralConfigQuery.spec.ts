import { Principal } from '@dfinity/principal';
import { describe, expect, it, vi } from 'vitest';
import {
	fetchDiscoveredCollateralConfigs,
	fetchLegacyCollateralConfigs,
} from './collateralConfigQuery';

const collateral = Principal.fromText('ryjl3-tyaaa-aaaaa-aaaba-cai');
const supported: Array<[Principal, unknown]> = [[collateral, { Active: null }]];

describe('discovered collateral config queries', () => {
	it('rejects when an individual config query rejects', async () => {
		const getConfig = vi.fn().mockRejectedValue(new Error('query unavailable'));

		await expect(fetchDiscoveredCollateralConfigs(supported, getConfig)).rejects.toThrow(
			'query unavailable'
		);
	});

	it('rejects an absent config for a principal returned by discovery', async () => {
		const getConfig = vi.fn().mockResolvedValue([]);

		await expect(fetchDiscoveredCollateralConfigs(supported, getConfig)).rejects.toThrow(
			'Missing collateral config'
		);
	});

	it('returns every successfully fetched config', async () => {
		const config = { ledger_canister_id: collateral };
		const getConfig = vi.fn().mockResolvedValue([config]);

		await expect(fetchDiscoveredCollateralConfigs(supported, getConfig)).resolves.toEqual([config]);
	});
});

describe('legacy collateral config queries', () => {
	it('keeps successful peers when one individual config query fails', async () => {
		const peer = Principal.fromText('2vxsx-fae');
		const config = { ledger_canister_id: peer };
		const getConfig = vi.fn()
			.mockRejectedValueOnce(new Error('query unavailable'))
			.mockResolvedValueOnce([config]);

		await expect(
			fetchLegacyCollateralConfigs(
				[[collateral, { Active: null }], [peer, { Active: null }]],
				getConfig
			)
		).resolves.toEqual([config]);
	});
});
