import { describe, it, expect } from 'vitest';
import {
  isDefaultInterestEligible,
  spInterestApr,
  aprToApyPct,
  liveSpApyPct,
} from './liveApy';

/**
 * Fixtures captured from mainnet on 2026-07-22 (`tfesu` get_protocol_status,
 * `tmhzi` get_pool_status). They reproduce the incident that motivated the fix:
 * the advertised APY read 9.96% while a depositor who joined after the BOB
 * sunset earned 7.39%, because sunset BOB carried a 33.90 icUSD opted-in
 * denominator against 22.72 icUSD of debt at 10.06%.
 */
const BOB = '7pail-xaaaa-aaaas-aabmq-cai';
const XRP = '5zjma-7dsov-wwsll-yojyc-23tbo-ruxmz-i';
const ICP = 'ryjl3-tyaaa-aaaaa-aaaba-cai';
const EXE = 'rh2pm-ryaaa-aaaan-qeniq-cai';
const NICP = 'buwm7-7yaaa-aaaar-qagva-cai';
const CKXAUT = 'nza5v-qaaaa-aaaar-qahzq-cai';

const protocolStatus = {
  interestSplit: [
    { destination: 'stability_pool', bps: 3500 },
    { destination: 'three_pool', bps: 6000 },
    { destination: 'treasury', bps: 500 },
  ],
  perCollateralInterest: [
    { collateralType: ICP, totalDebtE8s: 290.06743566, weightedInterestRate: 0.06038732635633318 },
    { collateralType: EXE, totalDebtE8s: 8.40513698, weightedInterestRate: 0.061176829402417636 },
    { collateralType: NICP, totalDebtE8s: 2313.97822355, weightedInterestRate: 0.09094399468784463 },
    { collateralType: CKXAUT, totalDebtE8s: 1721.70029288, weightedInterestRate: 0.009753220361499384 },
    { collateralType: BOB, totalDebtE8s: 22.72320352, weightedInterestRate: 0.10057603299537501 },
    { collateralType: XRP, totalDebtE8s: 1.66813688, weightedInterestRate: 0.03710562864601375 },
  ],
};

const principal = (text: string) => ({ toText: () => text });

const poolStatus = {
  eligible_icusd_per_collateral: [
    [principal(ICP), 120_955_413_570n],
    [principal(EXE), 113_156_562_600n],
    [principal(NICP), 120_955_413_570n],
    [principal(CKXAUT), 113_156_562_600n],
    [principal(BOB), 3_390_420_000n],
    [principal(XRP), 110_767_020_000n],
  ] as Array<[any, bigint]>,
};

describe('isDefaultInterestEligible', () => {
  it('excludes sunset BOB, which every new position is opted out of', () => {
    expect(isDefaultInterestEligible(BOB)).toBe(false);
  });

  it('excludes native XRP, which is gated behind a configured payout address', () => {
    expect(isDefaultInterestEligible(XRP)).toBe(false);
  });

  it('includes ordinary ICRC collateral', () => {
    for (const ct of [ICP, EXE, NICP, CKXAUT]) {
      expect(isDefaultInterestEligible(ct)).toBe(true);
    }
  });
});

describe('liveSpApyPct (advertised rate)', () => {
  it('is the rate a new depositor earns, not the grandfathered maximum', () => {
    expect(liveSpApyPct(protocolStatus, poolStatus)).toBeCloseTo(7.39, 1);
  });

  it('does not include the sunset-BOB term that inflated the old headline', () => {
    const withBob = spInterestApr(protocolStatus, poolStatus, () => true)!;
    expect(aprToApyPct(withBob)).toBeCloseTo(9.96, 1);
    // BOB alone accounted for ~2.4 points of the advertised number.
    expect(aprToApyPct(withBob) - liveSpApyPct(protocolStatus, poolStatus)!).toBeGreaterThan(2);
  });

  it('never exceeds the rate of a fully opted-in position', () => {
    const advertised = liveSpApyPct(protocolStatus, poolStatus)!;
    const maximal = aprToApyPct(spInterestApr(protocolStatus, poolStatus, () => true)!);
    expect(advertised).toBeLessThanOrEqual(maximal);
  });

  it('returns null rather than 0 when inputs are missing', () => {
    expect(liveSpApyPct(null, poolStatus)).toBeNull();
    expect(liveSpApyPct(protocolStatus, null)).toBeNull();
  });
});

describe('spInterestApr with a per-user eligibility set', () => {
  it('reproduces the grandfathered position rate (opted in to everything)', () => {
    const eligible = new Set([ICP, EXE, NICP, CKXAUT, BOB, XRP]);
    const apy = aprToApyPct(spInterestApr(protocolStatus, poolStatus, (ct) => eligible.has(ct))!);
    expect(apy).toBeCloseTo(9.96, 1);
  });

  it('reproduces the post-sunset depositor rate (opted out of BOB only)', () => {
    const eligible = new Set([ICP, EXE, NICP, CKXAUT, XRP]);
    const apy = aprToApyPct(spInterestApr(protocolStatus, poolStatus, (ct) => eligible.has(ct))!);
    expect(apy).toBeCloseTo(7.39, 1);
  });

  it('ignores collateral with no eligible icUSD, avoiding a divide-by-zero term', () => {
    const emptyPool = { eligible_icusd_per_collateral: [[principal(ICP), 0n]] as Array<[any, bigint]> };
    expect(spInterestApr(protocolStatus, emptyPool, () => true)).toBeNull();
  });
});
