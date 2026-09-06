import { describe, expect, it } from 'vitest';
import { Principal } from '@dfinity/principal';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import {
	gainCollaterals,
	liquidationPreferenceCollaterals,
	isSunsetCollateral
} from './sunsetCollateralPolicy';
import type { CollateralInfo } from '../../services/stabilityPoolService';

const componentPath = resolve(__dirname, 'EarnInfoCard.svelte');

const principal = (text: string) => Principal.fromText(text);
const bob: CollateralInfo = {
	ledger_id: principal('7pail-xaaaa-aaaas-aabmq-cai'),
	symbol: 'BOB',
	decimals: 8,
	status: { Sunset: null }
};
const exe: CollateralInfo = {
	ledger_id: principal('rh2pm-ryaaa-aaaan-qeniq-cai'),
	symbol: 'EXE',
	decimals: 8,
	status: { Sunset: null }
};
const icp: CollateralInfo = {
	ledger_id: principal('ryjl3-tyaaa-aaaaa-aaaba-cai'),
	symbol: 'ICP',
	decimals: 8,
	status: { Active: null }
};
const phasma: CollateralInfo = {
	ledger_id: principal('aaaaa-aa'),
	symbol: 'PHASMA',
	decimals: 8,
	status: { Deprecated: null }
};

describe('EarnInfoCard sunset collateral policy', () => {
	it('preserves the existing native XRP payout-routing surface', () => {
		const source = readFileSync(componentPath, 'utf8');

		expect(source).toContain('XrpPayoutRouting.svelte');
		expect(source).toContain('<XrpPayoutRouting');
	});

	it('keeps sunset collateral visible when an existing position has a gain', () => {
		expect(gainCollaterals([icp, bob, exe, phasma]).map((c) => c.symbol)).toEqual([
			'ICP',
			'BOB',
			'EXE'
		]);
	});

	it('offers sunset collateral only as an exit for an existing receiving position', () => {
		const activeRegistryBob = { ...bob, status: { Active: null } } as CollateralInfo;
		expect(liquidationPreferenceCollaterals([icp, bob], new Set()).map((c) => c.symbol)).toEqual([
			'ICP',
			'BOB'
		]);
		expect(
			liquidationPreferenceCollaterals([icp, bob], new Set([bob.ledger_id.toText()])).map(
				(c) => c.symbol
			)
		).toEqual(['ICP']);
		expect(
			liquidationPreferenceCollaterals(
				[icp, activeRegistryBob],
				new Set([activeRegistryBob.ledger_id.toText()])
			).map((c) => c.symbol)
		).toEqual(['ICP']);
		expect(
			liquidationPreferenceCollaterals([icp, exe], new Set([exe.ledger_id.toText()])).map(
				(c) => c.symbol
			)
		).toEqual(['ICP']);
	});

	it('recognizes sunset collateral by principal even before SP registry status synchronization', () => {
		expect(isSunsetCollateral(bob)).toBe(true);
		expect(isSunsetCollateral({ ...bob, status: { Active: null } })).toBe(true);
		expect(isSunsetCollateral(exe)).toBe(true);
		expect(isSunsetCollateral(icp)).toBe(false);
	});
});
