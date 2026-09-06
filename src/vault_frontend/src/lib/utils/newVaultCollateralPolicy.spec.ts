import { describe, it, expect } from 'vitest';
import { isHiddenForNewVaults, newVaultCollateralTypes } from './newVaultCollateralPolicy';

const NICP = 'buwm7-7yaaa-aaaar-qagva-cai';
const ICP = 'ryjl3-tyaaa-aaaaa-aaaba-cai';

describe('newVaultCollateralPolicy', () => {
	it('hides nICP from the new-vault selector', () => {
		expect(isHiddenForNewVaults(NICP)).toBe(true);
	});

	it('leaves every other collateral visible', () => {
		expect(isHiddenForNewVaults(ICP)).toBe(false);
	});

	it('filters nICP out of a collateral list while preserving order', () => {
		const list = [
			{ principal: ICP, symbol: 'ICP' },
			{ principal: NICP, symbol: 'nICP' },
			{ principal: 'mxzaz-hqaaa-aaaar-qaada-cai', symbol: 'ckBTC' }
		];
		expect(newVaultCollateralTypes(list).map((c) => c.symbol)).toEqual(['ICP', 'ckBTC']);
	});
});
