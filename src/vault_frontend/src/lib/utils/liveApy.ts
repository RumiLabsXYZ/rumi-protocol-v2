/**
 * Live APY math for the explorer lenses. The analytics canister exposes a 7-day
 * rolling APY (`get_apys`) that's stale when conditions change recently or when
 * a window has zero swap volume (then it's 0% even though LP holders are still
 * earning from interest-split). These helpers compute the *current* APY from
 * protocol + pool state so the lenses display what a depositor would actually
 * earn right now. Use the analytics value as a fallback only.
 *
 * Formula reference: see `calculateTheoreticalApy` in `threePoolService.ts`
 * (LP side) and `liveSpApy` in `StabilityPoolLens.svelte` (SP side). This file
 * unifies both so every lens stays in sync.
 */

interface ProtocolStatusLike {
  interestSplit?: { destination: string; bps: number }[];
  perCollateralInterest?: {
    collateralType: string;
    totalDebtE8s: number;
    weightedInterestRate: number;
  }[];
}

interface PoolStatusLike {
  eligible_icusd_per_collateral?: Array<[any, bigint]>;
}

function principalText(p: any): string {
  if (typeof p === 'string') return p;
  if (typeof p?.toText === 'function') return p.toText();
  return String(p);
}

/**
 * Live SP APY as a percentage (e.g. 6.29 for 6.29%). Mirrors the formula
 * in the /liquidity tab. Returns null if any input is missing.
 *
 * Note: `protocolStatus.perCollateralInterest[i].totalDebtE8s` is already
 * normalized to icUSD (the upstream `QueryOperations.getProtocolStatus` divides
 * by 1e8). The eligible map here also normalizes the bigint e8s to icUSD, so
 * the ratio is in matching units.
 */
export function liveSpApyPct(
  protocolStatus: ProtocolStatusLike | null | undefined,
  poolStatus: PoolStatusLike | null | undefined,
): number | null {
  if (!protocolStatus || !poolStatus) return null;

  const split = protocolStatus.interestSplit ?? [];
  const poolShare =
    (split.find((e) => e.destination === 'stability_pool')?.bps ?? 0) / 10000;
  const perC = protocolStatus.perCollateralInterest;
  if (!perC || perC.length === 0 || poolShare === 0) return null;

  const eligibleMap = new Map<string, number>(
    (poolStatus.eligible_icusd_per_collateral ?? []).map(([p, v]) => [
      principalText(p),
      Number(v) / 1e8,
    ]),
  );

  let totalApr = 0;
  for (const info of perC) {
    const eligible = eligibleMap.get(info.collateralType) ?? 0;
    if (
      eligible === 0 ||
      info.totalDebtE8s === 0 ||
      info.weightedInterestRate === 0
    ) {
      continue;
    }
    totalApr += (info.weightedInterestRate * poolShare * info.totalDebtE8s) / eligible;
  }
  if (totalApr === 0) return null;
  return (Math.pow(1 + totalApr / 365, 365) - 1) * 100;
}

/**
 * Live 3pool LP APY as a percentage (e.g. 4.50 for 4.50%). Mirrors the
 * `calculateTheoreticalApy` math but takes raw 3pool balances and the protocol
 * `interest_split` directly so a single call site doesn't need to plumb both
 * shapes. Returns null if inputs are missing.
 *
 * `threePoolBalances` is the raw `pool.balances` vec keyed by token index
 * (0=icUSD/8d, 1=ckUSDT/6d, 2=ckUSDC/6d). Decimals are normalized so the TVL
 * sum is in icUSD-equivalent.
 */
export function liveLpApyPct(
  protocolStatus: ProtocolStatusLike | null | undefined,
  threePoolBalances: bigint[] | undefined | null,
): number | null {
  if (!protocolStatus) return null;
  if (!threePoolBalances || threePoolBalances.length === 0) return null;

  const split = protocolStatus.interestSplit ?? [];
  const threePoolBps =
    split.find((e) => e.destination === 'three_pool')?.bps ?? 0;
  const perC = protocolStatus.perCollateralInterest;
  if (!perC || perC.length === 0 || threePoolBps === 0) return null;

  // 3pool token order: [icUSD (8d), ckUSDT (6d), ckUSDC (6d)]
  const decimalsByIdx = [8, 6, 6];
  let poolTvlIcusd = 0;
  for (let i = 0; i < threePoolBalances.length; i++) {
    const dec = decimalsByIdx[i] ?? 8;
    poolTvlIcusd += Number(threePoolBalances[i]) / 10 ** dec;
  }
  if (poolTvlIcusd <= 0) return null;

  const threePoolShare = threePoolBps / 10000;
  let totalApr = 0;
  for (const info of perC) {
    if (info.totalDebtE8s === 0 || info.weightedInterestRate === 0) continue;
    totalApr +=
      (info.weightedInterestRate * threePoolShare * info.totalDebtE8s) / poolTvlIcusd;
  }
  if (totalApr === 0) return null;
  return (Math.pow(1 + totalApr / 365, 365) - 1) * 100;
}
