/**
 * points.ts — pure helpers for the airdrop points UI. No I/O, fully unit-tested.
 *
 * `total_points` from the canister is in usd_e8s-days (USD value x 1e8, held
 * over time in days). Display divides by 1e8 to get human-readable "USD-days".
 */
import type {
  PublicEpochStatus,
  PointsConfig,
  PrincipalState,
  LeaderboardEntry,
  QualifyingAction,
} from '$declarations/rumi_points/rumi_points.did';

const POINTS_FORMATTER = new Intl.NumberFormat('en-US', {
  minimumFractionDigits: 0,
  maximumFractionDigits: 2,
});

/** usd_e8s-days (bigint) -> grouped USD-days string. Reduces via bigint first
 *  so very large whale values never overflow Number's safe-integer range. */
export function formatPoints(raw: bigint): string {
  const hundredthsOfUsdDay = raw / 1_000_000n; // 1e8 / 1e6 = 100
  const usdDays = Number(hundredthsOfUsdDay) / 100;
  return POINTS_FORMATTER.format(usdDays);
}

export function qualifyingActionLabel(a: QualifyingAction): string {
  if ('MintIcUsd' in a) return 'Minted icUSD';
  if ('DepositStabilityPool' in a) return 'Deposited to the stability pool';
  if ('Deposit3Pool' in a) return 'Added 3pool liquidity';
  if ('ProvideAmmLiquidity' in a) return 'Provided AMM liquidity';
  if ('RepayVault' in a) return 'Repaid a vault';
  return 'Qualifying action';
}

export type SeasonPhase = 'unknown' | 'pre' | 'live' | 'ended';

/** Derive the season banner phase. `nowNs` is the current time in ns. */
export function seasonState(
  status: PublicEpochStatus | null,
  config: PointsConfig | null,
  nowNs: bigint,
): SeasonPhase {
  if (!status || !config) return 'unknown';
  if (nowNs >= config.season_end_ns) return 'ended';
  if (status.open_epoch.length > 0 || status.driver_enabled) return 'live';
  return 'pre';
}

export type BodyState = 'disconnected' | 'not_enrolled' | 'enrolled' | 'excluded';

export function bodyState(args: {
  connected: boolean;
  excluded: boolean;
  state: PrincipalState | null;
}): BodyState {
  if (!args.connected) return 'disconnected';
  if (args.excluded) return 'excluded';
  if (args.state) return 'enrolled';
  return 'not_enrolled';
}

/** Find a principal's rank in a bounded leaderboard slice; null if absent. */
export function deriveRank(entries: LeaderboardEntry[], principalText: string): number | null {
  const hit = entries.find((e) => e.principal.toText() === principalText);
  return hit ? hit.rank : null;
}

/**
 * Whether a Next page exists. A partial page (< pageSize) is always the last
 * page; with a known total, stop once the current page reaches it. Prevents
 * paging into an empty trailing page when the last page is exactly pageSize.
 */
export function hasNextPage(
  offset: number,
  rowsLen: number,
  pageSize: number,
  total: number | null,
): boolean {
  if (rowsLen < pageSize) return false;
  if (total !== null && offset + rowsLen >= total) return false;
  return true;
}
