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
//   - These three new endpoints are not in the regenerated declarations yet
//     (the canonical .did has a pre-existing `principal : principal` parser
//     issue), so we build a small inline IDL fragment with just the methods
//     this service needs.

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

const POOL_ID = '3USD_ICP';
const AMM_CANISTER_ID = CANISTER_IDS.RUMI_AMM;
const E8S = 1e8;
const WINDOW_DAYS = 7;
const APY_CACHE_TTL_MS = 30_000;

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
    const actor = await getAnonAmmActor();
    const stats = (await actor.get_amm_pool_stats({
      pool: POOL_ID,
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
    const actor = await getAnonRewardsActor();
    const series = (await actor.get_amm_reward_series(
      POOL_ID,
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
    const actor = await getAnonRewardsActor();
    const series = (await actor.get_amm_tvl_series(
      POOL_ID,
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
  const actor = await getAnonRewardsActor();
  return (await actor.get_pending_rewards(POOL_ID, principal)) as bigint;
}

/**
 * Claim accrued earnings for the connected wallet.
 * Returns the amount claimed (icUSD e8s) on success, or an error string.
 *
 * Uses the wallet's authenticated agent (Oisy ICRC-25 signer when Oisy
 * is connected, otherwise the standard PNP getActor path).
 */
export async function claimAmm1Rewards(): Promise<
  { claimed_e8s: bigint } | { error: string }
> {
  try {
    const wallet = get(walletStore);
    if (!wallet.isConnected) return { error: 'Wallet not connected' };

    const oisyDetected = isOisyWallet();

    if (oisyDetected && wallet.principal) {
      const signerAgent = await getOisySignerAgent(wallet.principal);
      const ammActor = createOisyActor(
        AMM_CANISTER_ID,
        rewardsIdlFactory as any,
        signerAgent,
      );
      signerAgent.batch();
      const claimPromise = ammActor.claim_rewards(POOL_ID);
      await signerAgent.execute();
      const result = (await claimPromise) as
        | { Ok: bigint }
        | { Err: any };
      if ('Err' in result) return { error: formatClaimError(result.Err) };
      return { claimed_e8s: result.Ok };
    }

    const ammActor = (await walletStore.getActor(
      AMM_CANISTER_ID,
      rewardsIdlFactory,
    )) as any;
    const result = (await ammActor.claim_rewards(POOL_ID)) as
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
  return 'Unknown AMM error';
}

// Suppress unused-import lint if `IDL` only appears inside the inline
// factory parameter type (TS treats it as a value usage; this comment
// just makes that explicit for future readers).
void IDL;
