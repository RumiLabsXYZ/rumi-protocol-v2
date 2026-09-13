import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const fx = vi.hoisted(() => ({
  getProtocolStatus: vi.fn(),
  getPoolStatus: vi.fn(),
  getThreePoolApy: vi.fn(),
}));

vi.mock('./protocol', () => ({
  ProtocolService: { getProtocolStatus: fx.getProtocolStatus },
}));

vi.mock('./stabilityPoolService', () => ({
  stabilityPoolService: { getPoolStatus: fx.getPoolStatus },
}));

vi.mock('./threePoolApyService', () => ({
  getThreePoolApy: fx.getThreePoolApy,
}));

import {
  loadThreeUsdRate,
  loadSpRate,
  resolveEarnDestination,
  withTimeout,
  freshEarnSnapshot,
  __resetEarnRatesCacheForTests,
  type RateState,
} from './earnRates';

// `totalDebtE8s` already normalized to icUSD; `eligible_icusd_per_collateral`
// values are e8s. Produces a small, realistic, finite APR (~1.25%) via
// liveSpApyPct's math (see liveApy.spec.ts for the from-scratch derivation).
const SP_PROTOCOL_STATUS = {
  interestSplit: [{ destination: 'stability_pool', bps: 5000 }],
  perCollateralInterest: [
    { collateralType: 'icp', totalDebtE8s: 500, weightedInterestRate: 0.05 },
  ],
};
const SP_POOL_STATUS = {
  eligible_icusd_per_collateral: [['icp', 100_000_000_000n]],
};

const READY_3USD: RateState = { status: 'ready', pct: 4.5 };
const READY_SP: RateState = { status: 'ready', pct: 6.2 };
const UNAVAILABLE: RateState = { status: 'unavailable', pct: null };

beforeEach(() => {
  __resetEarnRatesCacheForTests();
  fx.getProtocolStatus.mockReset().mockResolvedValue(SP_PROTOCOL_STATUS);
  fx.getPoolStatus.mockReset().mockResolvedValue(SP_POOL_STATUS);
  fx.getThreePoolApy.mockReset().mockResolvedValue({
    total_apy_pct: 4.5,
    interest_apr_pct: 2,
    swap_fee_apr_pct: 2.5,
    pool_tvl_icusd: 1_000_000,
    three_pool_share_bps: 5000,
    complete: true,
  });
});

afterEach(() => {
  __resetEarnRatesCacheForTests();
});

describe('resolveEarnDestination', () => {
  it('picks 3USD when its rate is higher', () => {
    expect(resolveEarnDestination({ status: 'ready', pct: 5 }, { status: 'ready', pct: 3 })).toBe('/3usd');
  });

  it('picks the stability pool when its rate is higher', () => {
    expect(resolveEarnDestination({ status: 'ready', pct: 3 }, { status: 'ready', pct: 5 })).toBe('/stability-pool');
  });

  it('picks 3USD on an exact tie', () => {
    expect(resolveEarnDestination({ status: 'ready', pct: 5 }, { status: 'ready', pct: 5 })).toBe('/3usd');
  });

  it('picks whichever single rate is ready when the other is unavailable', () => {
    expect(resolveEarnDestination(UNAVAILABLE, READY_SP)).toBe('/stability-pool');
    expect(resolveEarnDestination(READY_3USD, UNAVAILABLE)).toBe('/3usd');
  });

  it('falls back to 3USD when neither rate is ready', () => {
    expect(resolveEarnDestination(UNAVAILABLE, UNAVAILABLE)).toBe('/3usd');
  });
});

describe('loadThreeUsdRate', () => {
  it('resolves ready with the finite complete APY', async () => {
    const r = await loadThreeUsdRate();
    expect(r).toEqual({ status: 'ready', pct: 4.5 });
  });

  it('never invents a rate when the result is incomplete', async () => {
    fx.getThreePoolApy.mockResolvedValue({
      total_apy_pct: 0,
      interest_apr_pct: 0,
      swap_fee_apr_pct: 0,
      pool_tvl_icusd: 0,
      three_pool_share_bps: 5000,
      complete: false,
    });
    const r = await loadThreeUsdRate();
    expect(r).toEqual({ status: 'unavailable', pct: null });
  });

  it('treats a non-finite APY as unavailable, not a real rate', async () => {
    fx.getThreePoolApy.mockResolvedValue({
      total_apy_pct: Infinity,
      interest_apr_pct: 0,
      swap_fee_apr_pct: 0,
      pool_tvl_icusd: 1,
      three_pool_share_bps: 5000,
      complete: true,
    });
    const r = await loadThreeUsdRate();
    expect(r).toEqual({ status: 'unavailable', pct: null });
  });

  it('treats a rejected fetch as unavailable', async () => {
    fx.getThreePoolApy.mockRejectedValue(new Error('boom'));
    const r = await loadThreeUsdRate();
    expect(r).toEqual({ status: 'unavailable', pct: null });
  });

  it('dedupes concurrent/near-immediate callers instead of refetching', async () => {
    await Promise.all([loadThreeUsdRate(), loadThreeUsdRate(), loadThreeUsdRate()]);
    expect(fx.getThreePoolApy).toHaveBeenCalledTimes(1);
  });
});

describe('loadSpRate', () => {
  it('resolves ready with a real, positive APY from protocol + pool status', async () => {
    const r = await loadSpRate();
    expect(r.status).toBe('ready');
    expect(r.pct).not.toBeNull();
  });

  it('is unavailable, not an invented 0%, when the pool status fetch rejects', async () => {
    fx.getPoolStatus.mockRejectedValue(new Error('stability pool status unavailable'));
    const r = await loadSpRate();
    expect(r).toEqual({ status: 'unavailable', pct: null });
  });

  it('dedupes concurrent/near-immediate callers instead of refetching', async () => {
    await Promise.all([loadSpRate(), loadSpRate()]);
    expect(fx.getProtocolStatus).toHaveBeenCalledTimes(1);
    expect(fx.getPoolStatus).toHaveBeenCalledTimes(1);
  });
});

describe('freshEarnSnapshot', () => {
  it('fetches fresh instead of reusing a still-valid cached result from an earlier visit', async () => {
    await loadThreeUsdRate();
    await loadSpRate();
    expect(fx.getThreePoolApy).toHaveBeenCalledTimes(1);
    expect(fx.getProtocolStatus).toHaveBeenCalledTimes(1);

    const { loadThreeUsdRate: fresh3usd, loadSpRate: freshSp } = freshEarnSnapshot();
    await fresh3usd();
    await freshSp();

    expect(fx.getThreePoolApy).toHaveBeenCalledTimes(2);
    expect(fx.getProtocolStatus).toHaveBeenCalledTimes(2);
  });

  it('shares its own fresh fetch with any caller that loads the rate right after, instead of duplicating it', async () => {
    const { loadThreeUsdRate: fresh3usd } = freshEarnSnapshot();
    await Promise.all([fresh3usd(), loadThreeUsdRate()]);
    expect(fx.getThreePoolApy).toHaveBeenCalledTimes(1);
  });
});

describe('withTimeout', () => {
  it('resolves with the real value when it arrives before the bound', async () => {
    const result = await withTimeout(Promise.resolve('real'), 50, () => 'fallback');
    expect(result).toBe('real');
  });

  it('falls back after the bound when the source never settles, without throwing', async () => {
    vi.useFakeTimers();
    try {
      const stalled = new Promise<string>(() => {});
      const resultPromise = withTimeout(stalled, 5000, () => 'fallback');
      await vi.advanceTimersByTimeAsync(5000);
      await expect(resultPromise).resolves.toBe('fallback');
    } finally {
      vi.useRealTimers();
    }
  });

  it('falls back (not throws) when the source rejects', async () => {
    const result = await withTimeout(Promise.reject(new Error('boom')), 50, () => 'fallback');
    expect(result).toBe('fallback');
  });
});
