// 3pool APY service.
//
// Consolidates the fetch-and-compute pipeline for the 3pool's LP APY so
// each consumer (Swap hero, PoolListView, AmmLiquidityPanel pass-through)
// can call a single cached helper instead of wiring up four parallel
// queries and re-deriving the breakdown.
//
// The pure math still lives in `calculateTotalApy` (threePoolService.ts);
// this module is just the orchestrator + cache + breakdown surfaces.

import {
  threePoolService,
  calculateTotalApy,
  POOL_TOKENS,
} from './threePoolService';
import { ProtocolService } from './protocol';
import { publicActor } from './protocol/apiClient';

export interface ThreePoolApyResult {
  total_apy_pct: number;
  interest_apr_pct: number;
  swap_fee_apr_pct: number;
  pool_tvl_icusd: number;
  three_pool_share_bps: number;
  /**
   * True only when every required input (protocol status, an actual
   * `three_pool` interest-split entry, the swap-fee window, and a positive
   * pool TVL) was fetched live and the resulting APY is a finite number.
   * Callers that must not present a fallback/default as a real rate should
   * check this before rendering `total_apy_pct` — a partial failure still
   * returns a numeric (often 0) result for the existing math below, but
   * `complete: false` marks it as not a trustworthy zero.
   */
  complete: boolean;
}

const APY_CACHE_TTL_MS = 30_000;
let _apyCache: { value: ThreePoolApyResult; expires: number } | null = null;

/**
 * Get cached 3pool LP APY. 30s TTL.
 *
 * Components:
 *   - interest_apr  = sum over collaterals of (rate × share × debt / TVL)
 *   - swap_fee_apr  = fees_7d / TVL × (365/7)
 *   - total_apy     = (1 + (interest_apr + swap_fee_apr)/365)^365 - 1
 *
 * Returns 0% APY (with zero components) on any partial failure rather than
 * throwing, since the callers render UI badges that should degrade
 * gracefully. Callers that must not show a fallback default as a real rate
 * should check `result.complete` rather than assuming a fulfilled promise
 * means live data (see `ThreePoolApyResult.complete`).
 */
export async function getThreePoolApy(): Promise<ThreePoolApyResult> {
  if (_apyCache && _apyCache.expires > Date.now()) {
    return _apyCache.value;
  }

  const [status, protocolStatus, interestSplit, swapFees7d] = await Promise.all([
    threePoolService.getPoolStatus(),
    ProtocolService.getProtocolStatus().catch(() => null),
    (publicActor.get_interest_split() as Promise<
      { destination: string; bps: bigint }[]
    >).catch(() => null),
    threePoolService.getSwapFeesOverWindow(7).catch(() => null),
  ]);

  // TVL in icUSD-equivalent (normalize 6-dec stables to 8-dec via ×100).
  let poolTvlE8s = 0;
  for (let i = 0; i < status.balances.length; i++) {
    const token = POOL_TOKENS[i];
    if (!token) continue;
    const normalized =
      token.decimals === 8
        ? Number(status.balances[i])
        : Number(status.balances[i]) * 100;
    poolTvlE8s += normalized;
  }
  const poolTvlIcusd = poolTvlE8s / 1e8;

  const threePoolEntry = interestSplit?.find(
    (e) => e.destination === 'three_pool',
  );
  // Legacy fallback for the *existing* math below only — does not count as
  // an actual split read for `complete` (see below).
  const threePoolShareBps = threePoolEntry ? Number(threePoolEntry.bps) : 5000;
  const swapFees7dValue = swapFees7d ?? 0n;

  let interestAprPct = 0;
  let swapFeeAprPct = 0;
  let totalApyPct = 0;

  if (protocolStatus && poolTvlIcusd > 0) {
    const share = threePoolShareBps / 10_000;
    let interestApr = 0;
    for (const info of protocolStatus.perCollateralInterest) {
      if (info.totalDebtE8s === 0 || info.weightedInterestRate === 0) continue;
      interestApr +=
        (info.weightedInterestRate * share * info.totalDebtE8s) / poolTvlIcusd;
    }
    const fees7dIcusd = Number(swapFees7dValue) / 1e8;
    const swapFeeApr = (fees7dIcusd / poolTvlIcusd) * (365 / 7);
    interestAprPct = interestApr * 100;
    swapFeeAprPct = swapFeeApr * 100;

    const apy = calculateTotalApy(
      threePoolShareBps,
      protocolStatus.perCollateralInterest,
      poolTvlIcusd,
      swapFees7dValue,
    );
    totalApyPct = apy !== null ? apy * 100 : 0;
  }

  const complete =
    protocolStatus !== null &&
    threePoolEntry !== undefined &&
    swapFees7d !== null &&
    poolTvlIcusd > 0 &&
    Number.isFinite(totalApyPct);

  const result: ThreePoolApyResult = {
    total_apy_pct: totalApyPct,
    interest_apr_pct: interestAprPct,
    swap_fee_apr_pct: swapFeeAprPct,
    pool_tvl_icusd: poolTvlIcusd,
    three_pool_share_bps: threePoolShareBps,
    complete,
  };

  _apyCache = { value: result, expires: Date.now() + APY_CACHE_TTL_MS };
  return result;
}
