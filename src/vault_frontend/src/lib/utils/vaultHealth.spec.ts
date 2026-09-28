import { describe, expect, it } from 'vitest';
import { getVaultCrTextColor, getVaultHealthHeadroom, VAULT_CR_DANGER, VAULT_CR_SAFE } from './vaultHealth';

describe('vault CR text health', () => {
	it('ranks per-collateral health by the same headroom that drives the CR tint', () => {
		const icpHeadroom = getVaultHealthHeadroom(1.5, 1.5, 1.33);
		const xautHeadroom = getVaultHealthHeadroom(1.45, 1.18, 1.12);

		expect(icpHeadroom).toBeCloseTo(0.2441, 3);
		expect(xautHeadroom).toBeCloseTo(0.6958, 3);
		expect(icpHeadroom).toBeLessThan(xautHeadroom);
	});

	it('keeps headroom unbounded for ordering but clamps visible colors at the tint endpoints', () => {
		expect(getVaultHealthHeadroom(1.1, 1.5, 1.33)).toBeLessThan(0);
		expect(getVaultHealthHeadroom(3.2, 1.5, 1.33)).toBeGreaterThan(1);
		expect(getVaultCrTextColor(1.1, 1.5, 1.33)).toBe(VAULT_CR_DANGER);
		expect(getVaultCrTextColor(3.2, 1.5, 1.33)).toBe(VAULT_CR_SAFE);
	});

	it('tints intermediate headroom from white toward pink', () => {
		const color = getVaultCrTextColor(1.5, 1.5, 1.33);
		expect(color).not.toBe(VAULT_CR_SAFE);
		expect(color).not.toBe(VAULT_CR_DANGER);
	});
});
