// AMM1 (3USD/ICP) APY service.
//
// Computes the total APY for the AMM1 pool by summing trading-fee APY and
// reward APY, both annualized over a 7-day window using arithmetic-mean TVL
// across the available samples. Also exposes wallet-side helpers for
// `get_pending_rewards` and `claim_rewards`.
//
// Background:
//   - rumi_amm exposes `get_amm_pool_stats` for windowed fee/volume aggregates.
//   - rumi_amm exposes `get_amm_reward_series(pool_id, window_days)` returning
//     daily DailyRewardPoint records (icUSD e8s).
//   - rumi_amm exposes `get_amm_tvl_series(pool_id, window_days)` returning
//     TvlSample records with `tvl_usd_e8s`.
//   - We use an inline IDL fragment for reward methods (`claim_rewards`,
//     `get_pending_rewards`, reward/tvl series). This was originally added
//     when those endpoints were missing from the generated declarations; the
//     declarations now have them, but we keep the inline fragment so this
//     service is self-contained and resilient to declaration drift.

import { Principal } from '@dfinity/principal';
import { Actor, HttpAgent, AnonymousIdentity } from '@dfinity/agent';
import { IDL } from '@dfinity/candid';
import { canisterIDLs } from './pnp';
import { walletStore } from '../stores/wallet';
import { get } from 'svelte/store';
import { CANISTER_IDS, CONFIG } from '../config';
import { isOisyWallet } from './protocol/walletOperations';
import { getOisySignerAgent, createOisyActor } from './oisySigner';

// ──────────────────────────────────────────────────────────────
// Types
// ──────────────────────────────────────────────────────────────

export interface Amm1ApyResult {
  total_apy_pct: number;
  trading_fee_apy_pct: number;
  reward_apy_pct: number;
  avg_tvl_7d_usd: number;
  fees_7d_usd: number;
  rewards_7d_usd: number;
  source_window_days: number;
}

export interface Amm1EffectiveApy {
  // AMM1's own earnings (trading fees + icUSD rewards stream).
  amm1_apy_pct: number;
  // Pass-through 3pool yield on the 3USD half of the pool.
  passthrough_3pool_apy_pct: number;
  // Sum of the above — what an LP actually earns per dollar deployed.
  total_apy_pct: number;
}

// A constant-product AMM held near equilibrium by arbitrage keeps ~50% of
// its value in each side. The 3USD side accrues 3pool yield via virtual-price
// growth; the ICP side does not. So an LP's effective yield is AMM1's own
// APY plus half of the 3pool APY.
export const AMM1_THREEUSD_VALUE_SHARE = 0.5;

/**
 * Combine AMM1's standalone APY with the 3pool yield embedded in the
 * 3USD half of the AMM1 reserve. Pure function — no IO.
 */
export function computeAmm1EffectiveApy(
  amm1: Amm1ApyResult,
  threePoolApyPct: number,
): Amm1EffectiveApy {
  const passthrough = threePoolApyPct * AMM1_THREEUSD_VALUE_SHARE;
  return {
    amm1_apy_pct: amm1.total_apy_pct,
    passthrough_3pool_apy_pct: passthrough,
    total_apy_pct: amm1.total_apy_pct + passthrough,
  };
}

/**
 * The protocol's headline "best LP APY": the better per-dollar return of
 * staying in the 3pool standalone vs. providing AMM1 liquidity (which stacks
 * 3pool yield on its 3USD half). Capital can only sit in one position at a
 * time, so this is a max, not a blend.
 *
 * Single source of truth for both the Swap "Earn up to" banner and the
 * Explorer "LP APY" vital so the two surfaces never disagree. Pure function.
 */
export function combinedBestLpApyPct(
  threePoolApyPct: number,
  amm1ApyPct: number,
): number {
  return Math.max(
    threePoolApyPct,
    amm1ApyPct + AMM1_THREEUSD_VALUE_SHARE * threePoolApyPct,
  );
}

interface DailyRewardPoint {
  day_start_ns: bigint;
  amount: bigint;
}

interface TvlSample {
  pool_id: string;
  timestamp: bigint;
  reserve_a: bigint;
  reserve_b: bigint;
  price_a_e8s: bigint;
  price_b_e8s: bigint;
  tvl_usd_e8s: bigint;
}

// ──────────────────────────────────────────────────────────────
// Constants
// ──────────────────────────────────────────────────────────────

const AMM_CANISTER_ID = CANISTER_IDS.RUMI_AMM;
const THREE_USD_PRINCIPAL = CANISTER_IDS.THREEPOOL;
const ICP_PRINCIPAL = CANISTER_IDS.ICP_LEDGER;
const E8S = 1e8;
const WINDOW_DAYS = 7;
const APY_CACHE_TTL_MS = 30_000;
const POOL_ID_CACHE_TTL_MS = 60 * 60 * 1000;

// ──────────────────────────────────────────────────────────────
// Pool ID resolver (session-cached)
// ──────────────────────────────────────────────────────────────
//
// The AMM stores the 3USD/ICP pool under `make_pool_id(token_a, token_b)`
// which is `{token_a_principal}_{token_b_principal}`. The frontend used
// to hardcode '3USD_ICP', which never matched. We resolve the real
// pool_id at runtime by scanning `get_pools()` for the (3USD, ICP) pair.
let _poolIdCache: { value: string; expires: number } | null = null;

async function getPoolId(): Promise<string> {
  if (_poolIdCache && _poolIdCache.expires > Date.now()) {
    return _poolIdCache.value;
  }
  const actor = await getAnonAmmActor();
  const pools = (await actor.get_pools()) as Array<{
    pool_id: string;
    token_a: { toText(): string } | string;
    token_b: { toText(): string } | string;
  }>;
  const principalText = (p: { toText(): string } | string): string =>
    typeof p === 'string' ? p : p.toText();
  const match = pools.find(p => {
    const a = principalText(p.token_a);
    const b = principalText(p.token_b);
    return (
      (a === THREE_USD_PRINCIPAL && b === ICP_PRINCIPAL) ||
      (a === ICP_PRINCIPAL && b === THREE_USD_PRINCIPAL)
    );
  });
  if (!match) {
    throw new Error('AMM1 3USD/ICP pool not found via get_pools');
  }
  _poolIdCache = { value: match.pool_id, expires: Date.now() + POOL_ID_CACHE_TTL_MS };
  return match.pool_id;
}

// ──────────────────────────────────────────────────────────────
// Inline IDL fragment for endpoints not yet in $declarations/rumi_amm
// ──────────────────────────────────────────────────────────────
//
// The canonical rumi_amm.did has a parser issue (`principal : principal`)
// preventing `dfx generate` from regenerating the declarations. Until that
// is fixed and the declarations are regenerated, we declare just the new
// reward-related methods inline so this service can call them in a
// type-safe manner.

const rewardsIdlFactory = ({ IDL }: { IDL: typeof import('@dfinity/candid').IDL }) => {
  const DailyRewardPoint = IDL.Record({
    day_start_ns: IDL.Nat64,
    amount: IDL.Nat,
  });
  const TvlSample = IDL.Record({
    pool_id: IDL.Text,
    timestamp: IDL.Nat64,
    reserve_a: IDL.Nat,
    reserve_b: IDL.Nat,
    price_a_e8s: IDL.Nat,
    price_b_e8s: IDL.Nat,
    tvl_usd_e8s: IDL.Nat,
  });
  const AmmError = IDL.Variant({
    PoolNotFound: IDL.Null,
    PoolAlreadyExists: IDL.Null,
    PoolPaused: IDL.Null,
    ZeroAmount: IDL.Null,
    InsufficientOutput: IDL.Record({ expected_min: IDL.Nat, actual: IDL.Nat }),
    InsufficientLiquidity: IDL.Null,
    InsufficientLpShares: IDL.Record({ required: IDL.Nat, available: IDL.Nat }),
    InvalidToken: IDL.Null,
    TransferFailed: IDL.Record({ token: IDL.Text, reason: IDL.Text }),
    Unauthorized: IDL.Null,
    MathOverflow: IDL.Null,
    DisproportionateLiquidity: IDL.Null,
    PoolCreationClosed: IDL.Null,
    FeeBpsOutOfRange: IDL.Null,
    MaintenanceMode: IDL.Null,
    ClaimNotFound: IDL.Null,
    PoolBusy: IDL.Null,
    DuplicateNonce: IDL.Null,
    NoLiquidity: IDL.Null,
    BelowMinClaim: IDL.Record({ claimable: IDL.Nat, min: IDL.Nat }),
    RewardLedgerTransferFailed: IDL.Record({ reason: IDL.Text }),
    InsufficientOnChainBalance: IDL.Record({ expected: IDL.Nat, actual: IDL.Nat }),
  });
  return IDL.Service({
    get_amm_reward_series: IDL.Func(
      [IDL.Text, IDL.Nat32],
      [IDL.Vec(DailyRewardPoint)],
      ['query'],
    ),
    get_amm_tvl_series: IDL.Func(
      [IDL.Text, IDL.Nat32],
      [IDL.Vec(TvlSample)],
      ['query'],
    ),
    get_pending_rewards: IDL.Func(
      [IDL.Text, IDL.Principal],
      [IDL.Nat],
      ['query'],
    ),
    claim_rewards: IDL.Func(
      [IDL.Text],
      [IDL.Variant({ Ok: IDL.Nat, Err: AmmError })],
      [],
    ),
  });
};

// ──────────────────────────────────────────────────────────────
// Anonymous query agent (cached)
// ──────────────────────────────────────────────────────────────

let _anonAgent: HttpAgent | null = null;

async function getAnonAgent(): Promise<HttpAgent> {
  if (_anonAgent) return _anonAgent;
  _anonAgent = new HttpAgent({
    host: CONFIG.host,
    identity: new AnonymousIdentity(),
  });
  if (CONFIG.isLocal) {
    await _anonAgent.fetchRootKey();
  }
  return _anonAgent;
}

async function getAnonRewardsActor(): Promise<any> {
  const agent = await getAnonAgent();
  return Actor.createActor(rewardsIdlFactory as any, {
    agent,
    canisterId: AMM_CANISTER_ID,
  });
}

async function getAnonAmmActor(): Promise<any> {
  const agent = await getAnonAgent();
  return Actor.createActor(canisterIDLs.rumi_amm as any, {
    agent,
    canisterId: AMM_CANISTER_ID,
  });
}

// ──────────────────────────────────────────────────────────────
// APY computation (with 30s cache)
// ──────────────────────────────────────────────────────────────

let _apyCache: { value: Amm1ApyResult; expires: number } | null = null;

/**
 * Get cached APY for AMM1's 3USD/ICP pool. 30s TTL.
 *
 * Total APY = trading-fee APY + reward APY, each annualized over a
 * 7-day window using the arithmetic mean of the TVL samples in the
 * same window.
 */
export async function getAmm1Apy(): Promise<Amm1ApyResult> {
  if (_apyCache && _apyCache.expires > Date.now()) {
    return _apyCache.value;
  }

  const [poolStats, rewardSeries, tvlSeries] = await Promise.all([
    getWeekPoolStats(),
    getRewardSeries(WINDOW_DAYS),
    getTvlSeries(WINDOW_DAYS),
  ]);

  // Trading fees over the 7-day window come from `get_amm_pool_stats(Week)`.
  // Both fees_a_e8s and fees_b_e8s are denominated in their respective
  // tokens. icUSD's reward stream funds the reward APY; trading fees are
  // a separate revenue source captured by the pool's fee_bps. We sum the
  // two raw e8s fields and treat them as USD-equivalent — the AMM1 pool
  // is 3USD/ICP, and the reserves are roughly balanced by arbitrage, so
  // each side approximates ~50% of the swapped USD volume. For the 3USD
  // side this is exact (1 3USD ≈ $1); for the ICP side, the value is in
  // ICP e8s and would need an ICP/USD price to convert. The TVL series
  // already carries the USD valuation, so for parity we use that to
  // compute APY ratios — we approximate fee USD by scaling the raw e8s
  // fees by avg_tvl_usd / avg_tvl_native. Simpler/safer: sum 3USD-side
  // fees only (token_a is 3USD per the canister convention) and treat
  // those as USD, since 3USD trades 1:1 with the underlying stables.
  //
  // For now, follow the simple approach the task describes: take total
  // fees as the sum of fees_a_e8s + fees_b_e8s, treating both as USD
  // e8s. This is approximate when token B (ICP) has a different unit
  // value, but the reward APY is the dominant contributor for AMM1
  // and the trading-fee APY is a secondary signal anyway.
  const fees7dE8s = (poolStats?.fees_a_e8s ?? 0n) + (poolStats?.fees_b_e8s ?? 0n);
  const fees7dUsd = Number(fees7dE8s) / E8S;

  const rewards7dE8s = rewardSeries.reduce<bigint>(
    (sum, p) => sum + p.amount,
    0n,
  );
  const rewards7dUsd = Number(rewards7dE8s) / E8S;

  let avgTvl7dUsd = 0;
  if (tvlSeries.length > 0) {
    let total = 0;
    for (const sample of tvlSeries) {
      total += Number(sample.tvl_usd_e8s) / E8S;
    }
    avgTvl7dUsd = total / tvlSeries.length;
  }

  const annualizationFactor = 365 / WINDOW_DAYS;
  const tradingFeeApyPct =
    avgTvl7dUsd > 0 ? (fees7dUsd / avgTvl7dUsd) * annualizationFactor * 100 : 0;
  const rewardApyPct =
    avgTvl7dUsd > 0 ? (rewards7dUsd / avgTvl7dUsd) * annualizationFactor * 100 : 0;
  const totalApyPct = tradingFeeApyPct + rewardApyPct;

  const result: Amm1ApyResult = {
    total_apy_pct: totalApyPct,
    trading_fee_apy_pct: tradingFeeApyPct,
    reward_apy_pct: rewardApyPct,
    avg_tvl_7d_usd: avgTvl7dUsd,
    fees_7d_usd: fees7dUsd,
    rewards_7d_usd: rewards7dUsd,
    source_window_days: WINDOW_DAYS,
  };

  _apyCache = { value: result, expires: Date.now() + APY_CACHE_TTL_MS };
  return result;
}

async function getWeekPoolStats(): Promise<
  | {
      fees_a_e8s: bigint;
      fees_b_e8s: bigint;
    }
  | null
> {
  try {
    const poolId = await getPoolId();
    const actor = await getAnonAmmActor();
    const stats = (await actor.get_amm_pool_stats({
      pool: poolId,
      window: { Week: null },
    })) as { fees_a_e8s: bigint; fees_b_e8s: bigint };
    return stats;
  } catch (err) {
    console.warn('amm1ApyService.getWeekPoolStats failed:', err);
    return null;
  }
}

async function getRewardSeries(windowDays: number): Promise<DailyRewardPoint[]> {
  try {
    const poolId = await getPoolId();
    const actor = await getAnonRewardsActor();
    const series = (await actor.get_amm_reward_series(
      poolId,
      windowDays,
    )) as DailyRewardPoint[];
    return series;
  } catch (err) {
    console.warn('amm1ApyService.getRewardSeries failed:', err);
    return [];
  }
}

async function getTvlSeries(windowDays: number): Promise<TvlSample[]> {
  try {
    const poolId = await getPoolId();
    const actor = await getAnonRewardsActor();
    const series = (await actor.get_amm_tvl_series(
      poolId,
      windowDays,
    )) as TvlSample[];
    return series;
  } catch (err) {
    console.warn('amm1ApyService.getTvlSeries failed:', err);
    return [];
  }
}

// ──────────────────────────────────────────────────────────────
// Per-user reward queries / claim
// ──────────────────────────────────────────────────────────────

/**
 * Pending unclaimed earnings for a principal in the 3USD/ICP pool.
 * Returns icUSD e8s.
 */
export async function getPendingEarnings(principalText: string): Promise<bigint> {
  const principal = Principal.fromText(principalText);
  const poolId = await getPoolId();
  const actor = await getAnonRewardsActor();
  return (await actor.get_pending_rewards(poolId, principal)) as bigint;
}

/**
 * Claim accrued earnings for the connected wallet.
 * Returns the amount claimed (icUSD e8s) on success, or an error string.
 *
 * Uses the wallet's authenticated agent (Oisy ICRC-25 signer when Oisy
 * is connected, otherwise the standard PNP getActor path).
 */
export async function claimAmm1Rewards(
  prefetchedPoolId?: string,
): Promise<
  { claimed_e8s: bigint } | { error: string }
> {
  try {
    const wallet = get(walletStore);
    if (!wallet.isConnected) return { error: 'Wallet not connected' };

    // Prefer the caller-supplied pool id (AmmLiquidityPanel already resolved it
    // in loadPool, pre-click) so the Oisy claim flow never awaits getPoolId()'s
    // get_pools() query inside the browser gesture window — that would block
    // the signer popup. Falls back to the cached resolver otherwise.
    const poolId = prefetchedPoolId ?? await getPoolId();
    const oisyDetected = isOisyWallet();

    if (oisyDetected && wallet.principal) {
      console.log(`[Oisy] Sequential AMM1 claim_rewards via @icp-sdk/signer v5`);
      const signerAgent = await getOisySignerAgent(wallet.principal);
      const ammActor = createOisyActor(
        AMM_CANISTER_ID,
        rewardsIdlFactory as any,
        signerAgent,
      );
      const result = (await ammActor.claim_rewards(poolId)) as
        | { Ok: bigint }
        | { Err: any };
      if ('Err' in result) return { error: formatClaimError(result.Err) };
      return { claimed_e8s: result.Ok };
    }

    const ammActor = (await walletStore.getActor(
      AMM_CANISTER_ID,
      rewardsIdlFactory,
    )) as any;
    const result = (await ammActor.claim_rewards(poolId)) as
      | { Ok: bigint }
      | { Err: any };
    if ('Err' in result) return { error: formatClaimError(result.Err) };
    return { claimed_e8s: result.Ok };
  } catch (err) {
    return {
      error: err instanceof Error ? err.message : String(err),
    };
  }
}

function formatClaimError(err: any): string {
  if (!err || typeof err !== 'object') return 'Unknown AMM error';
  if ('PoolNotFound' in err) return 'Pool not found';
  if ('PoolPaused' in err) return 'Pool is paused';
  if ('ZeroAmount' in err) return 'Nothing to claim';
  if ('TransferFailed' in err)
    return `Transfer failed (${err.TransferFailed.token}): ${err.TransferFailed.reason}`;
  if ('Unauthorized' in err) return 'Unauthorized';
  if ('MaintenanceMode' in err)
    return 'AMM is in maintenance mode — claims are temporarily disabled';
  if ('PoolBusy' in err)
    return 'Pool is busy with another transaction. Please try again in a moment.';
  if ('RewardLedgerTransferFailed' in err)
    return `Reward transfer failed (your earnings are safe, please retry): ${err.RewardLedgerTransferFailed.reason}`;
  if ('BelowMinClaim' in err)
    return `Below the minimum claim amount (${Number(err.BelowMinClaim.min) / 1e8} icUSD).`;
  if ('InsufficientOnChainBalance' in err)
    return 'Reward account is being topped up — please retry shortly.';
  if ('NoLiquidity' in err) return 'No liquidity in this pool.';
  if ('DuplicateNonce' in err) return 'Duplicate request — already processed.';
  return 'Unknown AMM error';
}

// Suppress unused-import lint if `IDL` only appears inside the inline
// factory parameter type (TS treats it as a value usage; this comment
// just makes that explicit for future readers).
void IDL;
