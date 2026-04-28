import { describe, it, expect } from 'vitest';
import { computeVaultCrPct } from './vaultCr';

const ICP = 'ryjl3-tyaaa-aaaaa-aaaba-cai';

describe('computeVaultCrPct', () => {
  it('returns CR as a percent for a typical ICP vault', () => {
    // 100 ICP @ $5 = $500 collateral, 200 icUSD debt → 250%
    const vault = {
      collateral_amount: 100_00_000_000n, // 100 ICP at e8s
      collateral_type: ICP,
      borrowed_icusd_amount: 200_00_000_000n, // 200 icUSD at e8s
    };
    const priceMap = new Map([[ICP, 5]]);
    const decimalsMap = new Map([[ICP, 8]]);
    expect(computeVaultCrPct(vault, priceMap, decimalsMap)).toBeCloseTo(250, 1);
  });

  it('returns null when debt is zero (debt-free vault)', () => {
    const vault = {
      collateral_amount: 100_00_000_000n,
      collateral_type: ICP,
      borrowed_icusd_amount: 0n,
    };
    const priceMap = new Map([[ICP, 5]]);
    const decimalsMap = new Map([[ICP, 8]]);
    expect(computeVaultCrPct(vault, priceMap, decimalsMap)).toBeNull();
  });

  it('returns null when price is missing for the collateral', () => {
    const vault = {
      collateral_amount: 100_00_000_000n,
      collateral_type: ICP,
      borrowed_icusd_amount: 100_00_000_000n,
    };
    expect(computeVaultCrPct(vault, new Map(), new Map())).toBeNull();
  });

  it('handles principal objects with toText()', () => {
    const vault = {
      collateral_amount: 100_00_000_000n,
      collateral_type: { toText: () => ICP },
      borrowed_icusd_amount: 100_00_000_000n,
    };
    const priceMap = new Map([[ICP, 10]]);
    const decimalsMap = new Map([[ICP, 8]]);
    expect(computeVaultCrPct(vault, priceMap, decimalsMap)).toBeCloseTo(1000, 1);
  });
});
