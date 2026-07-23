import { describe, it, expect } from 'vitest';
import type { VaultDTO, CollateralInfo } from '../../services/types';
import { CANISTER_IDS } from '../../config';
import {
  aggregatePosition,
  healthTierFor,
  resolveCollateralPrincipal,
  getCollateralAmount,
} from './positionSummary';

const ICP = CANISTER_IDS.ICP_LEDGER;
const CKBTC = 'mxzaz-hqaaa-aaaar-qaada-cai';
const CKXAUT = 'nza5v-qaaaa-aaaar-qahzq-cai';

function vault(overrides: Partial<VaultDTO>): VaultDTO {
  return {
    vaultId: 1,
    owner: 'owner',
    icpMargin: 0,
    borrowedIcusd: 0,
    collateralType: ICP,
    collateralAmount: 0,
    ...overrides,
  } as VaultDTO;
}

function collateral(overrides: Partial<CollateralInfo>): CollateralInfo {
  return {
    principal: ICP,
    symbol: 'ICP',
    decimals: 8,
    ledgerCanisterId: ICP,
    price: 0,
    priceTimestamp: 0,
    minimumCr: 1.5,
    liquidationCr: 1.33,
    borrowingFee: 0,
    liquidationBonus: 0.05,
    recoveryTargetCr: 1.5,
    interestRateApr: 0,
    debtCeiling: 0,
    minVaultDebt: 0,
    minCollateralDeposit: 0,
    ledgerFee: 10_000,
    color: '#3b00b9',
    status: 'Active',
    ...overrides,
  };
}

describe('healthTierFor', () => {
  // No cautionCr/safeCr args -> defaults to ICP's own borrow threshold (1.5)
  // and its comfort line (1.5 * 1.234 = 1.851).
  it('returns "safe" at or above the comfort line', () => {
    expect(healthTierFor(1.851)).toBe('safe');
    expect(healthTierFor(2.81)).toBe('safe');
    expect(healthTierFor(100)).toBe('safe');
  });

  it('returns "caution" between the borrow threshold and the comfort line', () => {
    expect(healthTierFor(1.5)).toBe('caution');
    expect(healthTierFor(1.75)).toBe('caution');
    expect(healthTierFor(1.85)).toBe('caution');
  });

  it('returns "danger" below the borrow threshold', () => {
    expect(healthTierFor(1.49)).toBe('danger');
    expect(healthTierFor(1.0)).toBe('danger');
    expect(healthTierFor(0.5)).toBe('danger');
  });

  it('returns "no-debt" for Infinity', () => {
    expect(healthTierFor(Infinity)).toBe('no-debt');
  });

  it('returns "unknown" for NaN', () => {
    expect(healthTierFor(NaN)).toBe('unknown');
  });

  it('honors explicit cautionCr/safeCr cutoffs instead of the ICP defaults', () => {
    // ckXAUT's real on-chain cutoffs: borrow threshold 1.18, comfort line
    // 1.18 * 1.234 = 1.45612.
    expect(healthTierFor(1.2, 1.18, 1.45612)).toBe('caution');
    expect(healthTierFor(1.1, 1.18, 1.45612)).toBe('danger');
    expect(healthTierFor(1.51, 1.18, 1.45612)).toBe('safe');
  });
});

describe('resolveCollateralPrincipal', () => {
  it('returns collateralType when present', () => {
    expect(resolveCollateralPrincipal(vault({ collateralType: CKBTC }))).toBe(CKBTC);
  });

  it('falls back to ICP ledger for legacy vaults with empty collateralType', () => {
    expect(resolveCollateralPrincipal(vault({ collateralType: '' }))).toBe(ICP);
  });
});

describe('getCollateralAmount', () => {
  it('prefers collateralAmount', () => {
    expect(getCollateralAmount(vault({ collateralAmount: 10, icpMargin: 5 }))).toBe(10);
  });

  it('falls back to icpMargin when collateralAmount is undefined', () => {
    const v = vault({ icpMargin: 5 });
    delete (v as any).collateralAmount;
    expect(getCollateralAmount(v)).toBe(5);
  });

  it('returns 0 when both are undefined', () => {
    const v = vault({});
    delete (v as any).collateralAmount;
    delete (v as any).icpMargin;
    expect(getCollateralAmount(v)).toBe(0);
  });
});

describe('aggregatePosition', () => {
  it('returns zeros and Infinity CR for an empty vault list', () => {
    const result = aggregatePosition([], []);
    expect(result.totalCollateralUsd).toBe(0);
    expect(result.totalBorrowed).toBe(0);
    expect(result.overallCr).toBe(Infinity);
    expect(result.healthTier).toBe('no-debt');
    expect(result.perCollateral).toEqual([]);
    expect(result.hasAnyMissingPrice).toBe(false);
  });

  it('aggregates a single ICP vault', () => {
    const vaults = [vault({ vaultId: 1, collateralAmount: 100, borrowedIcusd: 500 })];
    const collaterals = [collateral({ price: 5 })];
    const result = aggregatePosition(vaults, collaterals);
    expect(result.totalCollateralUsd).toBe(500);
    expect(result.totalBorrowed).toBe(500);
    expect(result.overallCr).toBe(1);
    expect(result.healthTier).toBe('danger');
    expect(result.perCollateral).toEqual([
      { principal: ICP, symbol: 'ICP', nativeAmount: 100, usdValue: 500, hasPrice: true },
    ]);
  });

  it('aggregates multiple vaults on the same collateral type into one breakdown row', () => {
    const vaults = [
      vault({ vaultId: 1, collateralAmount: 100, borrowedIcusd: 200 }),
      vault({ vaultId: 2, collateralAmount: 50, borrowedIcusd: 100 }),
    ];
    const collaterals = [collateral({ price: 5 })];
    const result = aggregatePosition(vaults, collaterals);
    expect(result.totalCollateralUsd).toBe(750);      // 150 ICP * $5
    expect(result.totalBorrowed).toBe(300);
    expect(result.perCollateral).toHaveLength(1);
    expect(result.perCollateral[0]).toMatchObject({ symbol: 'ICP', nativeAmount: 150, usdValue: 750 });
  });

  it('aggregates vaults across multiple collateral types and sorts by USD desc', () => {
    const vaults = [
      vault({ vaultId: 1, collateralType: ICP, collateralAmount: 100, borrowedIcusd: 200 }),
      vault({ vaultId: 2, collateralType: CKBTC, collateralAmount: 0.1, borrowedIcusd: 300 }),
      vault({ vaultId: 3, collateralType: CKXAUT, collateralAmount: 1, borrowedIcusd: 50 }),
    ];
    const collaterals = [
      collateral({ principal: ICP, symbol: 'ICP', price: 5 }),       // 100 * 5  = 500
      collateral({ principal: CKBTC, symbol: 'ckBTC', price: 60000 }),// 0.1 * 60000 = 6000
      collateral({ principal: CKXAUT, symbol: 'ckXAUT', price: 2000 }),// 1 * 2000 = 2000
    ];
    const result = aggregatePosition(vaults, collaterals);
    expect(result.totalCollateralUsd).toBe(8500);
    expect(result.totalBorrowed).toBe(550);
    expect(result.perCollateral.map(p => p.symbol)).toEqual(['ckBTC', 'ckXAUT', 'ICP']);
  });

  it('treats missing prices as $0 for the total and sets hasAnyMissingPrice', () => {
    const vaults = [
      vault({ vaultId: 1, collateralType: ICP, collateralAmount: 100, borrowedIcusd: 200 }),
      vault({ vaultId: 2, collateralType: CKBTC, collateralAmount: 0.1, borrowedIcusd: 100 }),
    ];
    const collaterals = [collateral({ principal: ICP, price: 5 })]; // ckBTC missing
    const result = aggregatePosition(vaults, collaterals);
    expect(result.totalCollateralUsd).toBe(500);
    expect(result.hasAnyMissingPrice).toBe(true);
    const ckbtcRow = result.perCollateral.find(p => p.principal === CKBTC);
    expect(ckbtcRow?.hasPrice).toBe(false);
    expect(ckbtcRow?.usdValue).toBe(0);
  });

  it('omits zero-balance collateral types from the breakdown', () => {
    const vaults = [
      vault({ vaultId: 1, collateralType: ICP, collateralAmount: 100, borrowedIcusd: 200 }),
      vault({ vaultId: 2, collateralType: CKBTC, collateralAmount: 0, borrowedIcusd: 0 }),
    ];
    const collaterals = [
      collateral({ principal: ICP, price: 5 }),
      collateral({ principal: CKBTC, symbol: 'ckBTC', price: 60000 }),
    ];
    const result = aggregatePosition(vaults, collaterals);
    expect(result.perCollateral.map(p => p.symbol)).toEqual(['ICP']);
  });

  it('returns Infinity CR when there is no debt (collateral-only vault)', () => {
    const vaults = [vault({ collateralAmount: 100, borrowedIcusd: 0 })];
    const collaterals = [collateral({ price: 5 })];
    const result = aggregatePosition(vaults, collaterals);
    expect(result.overallCr).toBe(Infinity);
    expect(result.healthTier).toBe('no-debt');
  });

  it('falls back to ICP ledger for legacy vaults with empty collateralType', () => {
    const vaults = [vault({ collateralType: '', icpMargin: 100, collateralAmount: 100, borrowedIcusd: 500 })];
    const collaterals = [collateral({ principal: ICP, symbol: 'ICP', price: 5 })];
    const result = aggregatePosition(vaults, collaterals);
    expect(result.perCollateral[0].principal).toBe(ICP);
    expect(result.totalCollateralUsd).toBe(500);
  });

  it('uses a truncated principal as symbol fallback when CollateralInfo is missing', () => {
    const vaults = [vault({ collateralType: CKBTC, collateralAmount: 0.1, borrowedIcusd: 0 })];
    const result = aggregatePosition(vaults, []);
    expect(result.perCollateral[0].symbol).toBe(CKBTC.slice(0, 5));
  });

  // Regression coverage for the "flat ICP threshold" bug: a position backed by
  // an asset with a lower borrow threshold (ckXAUT at 1.18 vs ICP's 1.5) must
  // be judged against its own threshold, not ICP's.
  //
  // ckXAUT's real on-chain params (verified via get_collateral_config on
  // nza5v-qaaaa-aaaar-qahzq-cai): borrow_threshold_ratio 1.18,
  // liquidation_ratio 1.12. Bands: danger <1.18, caution 1.18-1.45612,
  // safe >=1.45612.
  describe('weighted health tier for non-ICP collateral', () => {
    const ckxaut = (price: number) =>
      collateral({ principal: CKXAUT, symbol: 'ckXAUT', price, minimumCr: 1.18, liquidationCr: 1.12 });

    it('reports a 151% CR ckXAUT-only position as safe, matching its vault card', () => {
      // The live case that prompted this fix: judged against ICP's numbers the
      // badge said "At risk", and against a liquidationCr-derived cutoff it
      // said "Caution", while VaultCard already showed the same position safe.
      const vaults = [vault({ collateralType: CKXAUT, collateralAmount: 1, borrowedIcusd: 1000 })];
      const result = aggregatePosition(vaults, [ckxaut(1510)]);
      expect(result.overallCr).toBeCloseTo(1.51, 2);
      expect(result.healthTier).toBe('safe');
    });

    it('reports caution between ckXAUT\'s borrow threshold and its comfort line', () => {
      const vaults = [vault({ collateralType: CKXAUT, collateralAmount: 1, borrowedIcusd: 1000 })];
      const result = aggregatePosition(vaults, [ckxaut(1300)]);
      expect(result.overallCr).toBeCloseTo(1.3, 2);
      expect(result.healthTier).toBe('caution');
    });

    it('reports danger only once CR falls below ckXAUT\'s own borrow threshold', () => {
      const vaults = [vault({ collateralType: CKXAUT, collateralAmount: 1, borrowedIcusd: 1000 })];
      const result = aggregatePosition(vaults, [ckxaut(1150)]);
      expect(result.overallCr).toBeCloseTo(1.15, 2);
      expect(result.healthTier).toBe('danger');
    });

    it('keeps ICP-only danger/caution boundaries at ICP\'s own borrow threshold', () => {
      const danger = [vault({ collateralType: ICP, collateralAmount: 100, borrowedIcusd: 500 })];
      expect(aggregatePosition(danger, [collateral({ price: 5 })]).healthTier).toBe('danger'); // CR 1.0

      const caution = [vault({ collateralType: ICP, collateralAmount: 100, borrowedIcusd: 300 })];
      expect(aggregatePosition(caution, [collateral({ price: 5 })]).healthTier).toBe('caution'); // CR ≈1.67

      const safe = [vault({ collateralType: ICP, collateralAmount: 100, borrowedIcusd: 200 })];
      expect(aggregatePosition(safe, [collateral({ price: 5 })]).healthTier).toBe('safe'); // CR 2.5
    });

    it('puts ICP\'s safe line at 1.851, matching VaultCard\'s comfort line', () => {
      // Previously a flat 2.0, which disagreed with the per-vault card's
      // minimumCr * 1.234. 1.86 is safe under the new line, caution under old.
      const vaults = [vault({ collateralType: ICP, collateralAmount: 186, borrowedIcusd: 100 })];
      const result = aggregatePosition(vaults, [collateral({ price: 1 })]);
      expect(result.overallCr).toBeCloseTo(1.86, 2);
      expect(result.healthTier).toBe('safe');
    });

    it('blends borrow thresholds by USD share for a mixed ICP + ckXAUT position', () => {
      // 50/50 USD split (725 + 725 = 1450): weighted minimumCr =
      // (1.5 + 1.18)/2 = 1.34 -> danger below 1.34, safe at/above
      // 1.34 * 1.234 = 1.65356. CR = 1450/1000 = 1.45 sits between -> caution.
      const vaults = [
        vault({ vaultId: 1, collateralType: ICP, collateralAmount: 100, borrowedIcusd: 1000 }),
        vault({ vaultId: 2, collateralType: CKXAUT, collateralAmount: 1, borrowedIcusd: 0 }),
      ];
      const collaterals = [
        collateral({ principal: ICP, symbol: 'ICP', price: 7.25 }), // 725 USD
        ckxaut(725),                                                // 725 USD
      ];
      const result = aggregatePosition(vaults, collaterals);
      expect(result.totalCollateralUsd).toBe(1450);
      expect(result.overallCr).toBeCloseTo(1.45, 2);
      expect(result.healthTier).toBe('caution');
    });

    it('weights toward the dominant asset in a lopsided mix', () => {
      // 90% ckXAUT / 10% ICP: weighted minimumCr = 0.9*1.18 + 0.1*1.5 = 1.212
      // -> safe line 1.212 * 1.234 = 1.495608. CR 1.45 -> caution.
      const vaults = [
        vault({ vaultId: 1, collateralType: CKXAUT, collateralAmount: 1, borrowedIcusd: 1000 }),
        vault({ vaultId: 2, collateralType: ICP, collateralAmount: 145, borrowedIcusd: 0 }),
      ];
      const collaterals = [
        ckxaut(1305),                                              // 1305 USD (90%)
        collateral({ principal: ICP, symbol: 'ICP', price: 1 }),   // 145 USD (10%)
      ];
      const result = aggregatePosition(vaults, collaterals);
      expect(result.totalCollateralUsd).toBe(1450);
      expect(result.overallCr).toBeCloseTo(1.45, 2);
      expect(result.healthTier).toBe('caution');
    });
  });
});
