import { describe, it, expect } from 'vitest';
import { Principal } from '@dfinity/principal';
import {
  sourceKey,
  summarizeLedger,
  buildLivePositions,
  epochDateRange,
  epochRangeByIndex,
  SOURCE_META,
  type LiveInputs,
} from './pointsBreakdown';
import type { PointEntry, PointSource, EpochSummary } from '$declarations/rumi_points/rumi_points.did';

const P = Principal.fromText('2vxsx-fae');

function entry(src: string, epoch: number, points: bigint): PointEntry {
  return {
    principal: P,
    epoch_index: BigInt(epoch),
    source: { [src]: null } as unknown as PointSource,
    recorded_at_ns: 0n,
    points_delta: points,
  };
}

/** All-sources-available baseline; spread overrides per case. */
function inputs(over: Partial<LiveInputs> = {}): LiveInputs {
  return {
    vaultDebtUsd: 0,
    spIcusd: 0,
    sp3usd: 0,
    wallet3usd: 0,
    ammShare3usd: 0,
    ammShareIcp: 0,
    recorded3pool: { icusd: 0, ckusdc: 0, ckusdt: 0 },
    icpUsd: 5,
    virtualPrice: 1,
    ...over,
  };
}

describe('sourceKey', () => {
  it('unwraps the candid variant key', () => {
    expect(sourceKey({ IcUsdDebt: null })).toBe('IcUsdDebt');
    expect(sourceKey({ ThreeUsdStabilityPool: null })).toBe('ThreeUsdStabilityPool');
  });
});

describe('summarizeLedger', () => {
  const entries: PointEntry[] = [
    entry('Registration', 6, 0n),
    entry('IcUsdDebt', 7, 100n),
    entry('IcUsdDebt', 8, 100n),
    entry('IcUsdDebt', 9, 100n),
    entry('IcUsdDebt', 10, 100n),
    entry('IcUsdStabilityPool', 7, 50n),
    entry('IcUsdStabilityPool', 10, 50n),
    entry('ThreeUsdStabilityPool', 6, 200n),
    entry('ThreeUsdStabilityPool', 7, 200n),
  ];

  it('totals per source, skipping Registration', () => {
    const s = summarizeLedger(entries, 10);
    expect(s.total).toBe(900n);
    expect(s.sources.map((r) => r.key)).toEqual([
      'IcUsdDebt', // 400
      'ThreeUsdStabilityPool', // 400 — ties keep insertion order after sort
      'IcUsdStabilityPool', // 100
    ]);
    const debt = s.sources.find((r) => r.key === 'IcUsdDebt')!;
    expect(debt.points).toBe(400n);
    expect(debt.sharePct).toBeCloseTo(44.4, 1);
    expect(debt.firstEpoch).toBe(7);
    expect(debt.lastEpoch).toBe(10);
    expect(debt.epochCount).toBe(4);
  });

  it('flags sources not credited in the latest processed epoch as stopped', () => {
    const s = summarizeLedger(entries, 10);
    expect(s.sources.find((r) => r.key === 'IcUsdDebt')!.status).toBe('active');
    expect(s.sources.find((r) => r.key === 'IcUsdStabilityPool')!.status).toBe('active');
    expect(s.sources.find((r) => r.key === 'ThreeUsdStabilityPool')!.status).toBe('stopped');
  });

  it('treats an unknown latest epoch as active (never false-alarms)', () => {
    const s = summarizeLedger(entries, null);
    expect(s.sources.every((r) => r.status === 'active')).toBe(true);
  });

  it('builds a most-recent-first epoch timeline with per-source rows', () => {
    const s = summarizeLedger(entries, 10);
    expect(s.epochs.map((e) => e.epoch)).toEqual([10, 9, 8, 7, 6]);
    const e7 = s.epochs.find((e) => e.epoch === 7)!;
    expect(e7.points).toBe(350n);
    expect(e7.bySource[0]).toEqual({ key: 'ThreeUsdStabilityPool', points: 200n });
  });

  it('handles an empty ledger', () => {
    const s = summarizeLedger([], 10);
    expect(s.total).toBe(0n);
    expect(s.sources).toEqual([]);
    expect(s.epochs).toEqual([]);
  });
});

describe('buildLivePositions — mirrors accrual.rs snapshot_weights', () => {
  it('vault debt earns 1x', () => {
    const live = buildLivePositions(inputs({ vaultDebtUsd: 1100.83 }));
    expect(live.rows).toHaveLength(1);
    expect(live.rows[0].key).toBe('IcUsdDebt');
    expect(live.rows[0].weightedUsd).toBeCloseTo(1100.83);
    expect(live.weeklyEstimateUsdDays).toBeCloseTo(1100.83 * 7);
  });

  it('stability pool: icUSD 1x, 3USD 2x at $1 face', () => {
    const live = buildLivePositions(inputs({ spIcusd: 500, sp3usd: 545 }));
    const sp1 = live.rows.find((r) => r.key === 'IcUsdStabilityPool')!;
    const sp2 = live.rows.find((r) => r.key === 'ThreeUsdStabilityPool')!;
    expect(sp1.weightedUsd).toBeCloseTo(500);
    expect(sp2.valueUsd).toBeCloseTo(545);
    expect(sp2.weightedUsd).toBeCloseTo(1090);
  });

  it('AMM LP values 3USD leg at face + ICP leg at the oracle rate, 2x', () => {
    const live = buildLivePositions(inputs({ ammShare3usd: 10, ammShareIcp: 2, icpUsd: 5 }));
    const amm = live.rows.find((r) => r.key === 'AmmLp')!;
    expect(amm.valueUsd).toBeCloseTo(20);
    expect(amm.weightedUsd).toBeCloseTo(40);
  });

  it('3pool fully verified: recorded credit intact, matched/unmatched split', () => {
    // 100 ckUSDC + 40 ckUSDT with plenty of 3USD held: matched 2*40=80 @5x,
    // unmatched 60 @3x (accrual.rs snapshot_weights_apply_each_multiplier).
    const live = buildLivePositions(
      inputs({ wallet3usd: 1000, recorded3pool: { icusd: 0, ckusdc: 100, ckusdt: 40 } }),
    );
    const matched = live.rows.find((r) => r.key === 'CkStable3PoolMatched')!;
    const unmatched = live.rows.find((r) => r.key === 'CkStable3PoolUnmatched')!;
    expect(matched.valueUsd).toBeCloseTo(80);
    expect(matched.weightedUsd).toBeCloseTo(400);
    expect(unmatched.valueUsd).toBeCloseTo(60);
    expect(unmatched.weightedUsd).toBeCloseTo(180);
    expect(live.threePool!.underVerified).toBe(false);
  });

  it('3pool verification counts wallet + SP + AMM 3USD at the virtual price', () => {
    // held 3USD = 200+300+100 = 600 @ vp 1.03 → verified 618, cap 621.09 ≥ 600.
    const live = buildLivePositions(
      inputs({
        wallet3usd: 200,
        sp3usd: 300,
        ammShare3usd: 100,
        virtualPrice: 1.03,
        recorded3pool: { icusd: 600, ckusdc: 0, ckusdt: 0 },
      }),
    );
    expect(live.threePool!.verifiedUsd).toBeCloseTo(618);
    expect(live.threePool!.creditedUsd).toBeCloseTo(600);
    expect(live.threePool!.underVerified).toBe(false);
    expect(live.rows.find((r) => r.key === 'IcUsd3Pool')!.valueUsd).toBeCloseTo(600);
  });

  it('3pool with the 3USD sold: nothing credited, flagged under-verified', () => {
    const live = buildLivePositions(
      inputs({ recorded3pool: { icusd: 0, ckusdc: 1155, ckusdt: 1368 } }),
    );
    expect(live.rows.filter((r) => r.meta.venue === 'threePool')).toEqual([]);
    expect(live.threePool).toMatchObject({
      recordedUsd: 2523,
      verifiedUsd: 0,
      creditedUsd: 0,
      underVerified: true,
      verificationUnknown: false,
    });
  });

  it('3pool partially verified scales all legs uniformly (apply_verification)', () => {
    // recorded 100 icUSD, held 50 3USD @ vp 1 → cap 50.25 → factor 0.5025.
    const live = buildLivePositions(
      inputs({ wallet3usd: 50, recorded3pool: { icusd: 100, ckusdc: 0, ckusdt: 0 } }),
    );
    expect(live.threePool!.creditedUsd).toBeCloseTo(50.25);
    expect(live.threePool!.underVerified).toBe(true);
    expect(live.rows.find((r) => r.key === 'IcUsd3Pool')!.valueUsd).toBeCloseTo(50.25);
  });

  it('a failed verification read shows recorded uncapped, flagged unknown, never alarms', () => {
    const live = buildLivePositions(
      inputs({ wallet3usd: null, recorded3pool: { icusd: 100, ckusdc: 0, ckusdt: 0 } }),
    );
    expect(live.threePool!.verificationUnknown).toBe(true);
    expect(live.threePool!.underVerified).toBe(false);
    expect(live.rows.find((r) => r.key === 'IcUsd3Pool')!.valueUsd).toBeCloseTo(100);
  });

  it('a failed virtual-price read makes verification unknown, never a false alarm', () => {
    const live = buildLivePositions(
      inputs({ wallet3usd: 1000, virtualPrice: null, recorded3pool: { icusd: 100, ckusdc: 0, ckusdt: 0 } }),
    );
    expect(live.threePool!.verificationUnknown).toBe(true);
    expect(live.threePool!.underVerified).toBe(false);
    expect(live.rows.find((r) => r.key === 'IcUsd3Pool')!.valueUsd).toBeCloseTo(100);
  });

  it('a failed source read is reported unavailable, not shown as zero', () => {
    const live = buildLivePositions(inputs({ vaultDebtUsd: null, spIcusd: 500 }));
    expect(live.unavailable).toContain('vault debt');
    expect(live.rows.find((r) => r.key === 'IcUsdDebt')).toBeUndefined();
    expect(live.rows.find((r) => r.key === 'IcUsdStabilityPool')).toBeDefined();
  });

  it('sums the weekly estimate across all weighted rows', () => {
    const live = buildLivePositions(
      inputs({ vaultDebtUsd: 100, spIcusd: 50, sp3usd: 25 }),
    );
    // 100*1 + 50*1 + 25*2 = 200 per day → 1400 per week.
    expect(live.weeklyEstimateUsdDays).toBeCloseTo(1400);
  });
});

describe('forward compatibility', () => {
  it('an unknown PointSource variant renders with fallback meta instead of crashing', () => {
    const s = summarizeLedger(
      [entry('SomeFutureSource', 3, 100n), entry('IcUsdDebt', 3, 50n)],
      3,
    );
    const row = s.sources.find((r) => (r.key as string) === 'SomeFutureSource')!;
    expect(row).toBeDefined();
    expect(row.meta.label).toBe('SomeFutureSource');
    expect(row.meta.short).toBe('SomeFutureSource');
    expect(row.meta.multiplier).toBe(0);
    expect(s.total).toBe(150n);
  });
});

describe('epoch dates', () => {
  const e10: EpochSummary = {
    epoch_index: 10n,
    points_accrued_this_epoch: 0n,
    epoch_start_ns: 1_786_320_000_000_000_000n,
    registered_principals: 0n,
    active_principals: 0n,
    total_points_all: 0n,
    snapshot_a_ns: 0n,
    snapshot_b_ns: 0n,
    epoch_end_ns: 1_786_924_800_000_000_000n,
  };

  it('formats the UTC epoch window', () => {
    expect(epochDateRange(e10.epoch_start_ns, e10.epoch_end_ns)).toBe('Aug 10 – Aug 17');
  });

  it('looks a range up by epoch index', () => {
    expect(epochRangeByIndex([e10], 10)).toBe('Aug 10 – Aug 17');
    expect(epochRangeByIndex([e10], 9)).toBeNull();
  });
});

describe('SOURCE_META', () => {
  it('covers every PointSource variant', () => {
    for (const key of [
      'Registration',
      'IcUsdDebt',
      'IcUsd3Pool',
      'CkStable3PoolMatched',
      'CkStable3PoolUnmatched',
      'IcUsdStabilityPool',
      'ThreeUsdStabilityPool',
      'AmmLp',
      'VaultRepayment',
    ] as const) {
      expect(SOURCE_META[key]).toBeDefined();
    }
  });
});
