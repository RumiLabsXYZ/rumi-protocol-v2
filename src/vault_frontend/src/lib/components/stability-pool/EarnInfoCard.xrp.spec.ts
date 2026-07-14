import { describe, expect, it } from 'vitest';
import { Principal } from '@dfinity/principal';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import {
	gainCollaterals,
	liquidationPreferenceCollaterals,
	isSunsetBobCollateral
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

	it('keeps BOB visible when an existing position has a gain', () => {
		expect(gainCollaterals([icp, bob, phasma]).map((c) => c.symbol)).toEqual(['ICP', 'BOB']);
	});

	it('offers BOB only as an exit for an existing receiving position', () => {
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
	});

	it('recognizes sunset BOB by principal even before SP registry status synchronization', () => {
		expect(isSunsetBobCollateral(bob)).toBe(true);
		expect(isSunsetBobCollateral({ ...bob, status: { Active: null } })).toBe(true);
		expect(isSunsetBobCollateral(icp)).toBe(false);
	});
});
