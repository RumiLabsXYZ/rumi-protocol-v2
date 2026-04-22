import { Principal } from '@dfinity/principal';
import { Actor, HttpAgent, AnonymousIdentity } from '@dfinity/agent';
import { canisterIDLs } from './pnp';
import { walletStore } from '../stores/wallet';
import { get } from 'svelte/store';
import { CANISTER_IDS, CONFIG } from '../config';
import { isOisyWallet } from './protocol/walletOperations';
import { getOisySignerAgent, createOisyActor } from './oisySigner';

// ──────────────────────────────────────────────────────────────
// Types — mirrors the Candid interface
// ──────────────────────────────────────────────────────────────

export interface TokenConfig {
  ledger_id: Principal;
  symbol: string;
  decimals: number;
  precision_mul: bigint;
}

export interface PoolStatus {
  balances: bigint[];
  lp_total_supply: bigint;
  current_a: bigint;
  virtual_price: bigint;
  swap_fee_bps: bigint;
  admin_fee_bps: bigint;
  tokens: TokenConfig[];
}

export interface VirtualPriceSnapshot {
  timestamp_secs: bigint;
  virtual_price: bigint;
  lp_total_supply: bigint;
}

export interface QuoteSwapResult {
  token_in: number;
  token_out: number;
  amount_in: bigint;
  amount_out: bigint;
  fee_native: bigint;
  fee_bps: number;
  imbalance_before: bigint;
  imbalance_after: bigint;
  is_rebalancing: boolean;
  virtual_price_before: bigint;
  virtual_price_after: bigint;
}

// ──────────────────────────────────────────────────────────────
// Token metadata for the 3pool (matches canister init config)
// ──────────────────────────────────────────────────────────────

export interface SwapToken {
  index: number;
  symbol: string;
  ledgerId: string;
  decimals: number;
  color: string;
}

export const POOL_TOKENS: SwapToken[] = [
  { index: 0, symbol: 'icUSD',  ledgerId: CANISTER_IDS.ICUSD_LEDGER,  decimals: 8, color: '#818cf8' },
  { index: 1, symbol: 'ckUSDT', ledgerId: CANISTER_IDS.CKUSDT_LEDGER, decimals: 6, color: '#26A17B' },
  { index: 2, symbol: 'ckUSDC', ledgerId: CANISTER_IDS.CKUSDC_LEDGER, decimals: 6, color: '#2775CA' },
];

// ──────────────────────────────────────────────────────────────
// Helpers
// ──────────────────────────────────────────────────────────────

export function parseTokenAmount(amount: string, decimals: number): bigint {
  const value = parseFloat(amount);
  if (isNaN(value) || value < 0) throw new Error('Invalid amount');
  return BigInt(Math.floor(value * Math.pow(10, decimals)));
}

export function formatTokenAmount(amount: bigint, decimals: number): string {
  const divisor = Math.pow(10, decimals);
  const value = Number(amount) / divisor;
  if (value > 0 && value < 0.01) {
    return value.toFixed(Math.min(decimals, 6));
  }
  const fixed = value.toFixed(4);
  // Trim trailing zeros but keep at least 2 decimal places
  let trimmed = fixed.replace(/0+$/, '');
  if (trimmed.endsWith('.')) trimmed += '00';
  else if (trimmed.indexOf('.') !== -1 && trimmed.split('.')[1].length < 2) {
    trimmed += '0';
  }
  return trimmed;
}

/** Ledger transfer fee per token (approve + transfer_from both charge a fee). */
export function getLedgerFee(decimals: number): bigint {
  // icUSD (8 decimals) = 0.001 = 100_000 e8s
  // ckUSDC / ckUSDT (6 decimals) = 0.01 = 10_000
  return decimals === 8 ? 100_000n : 10_000n;
}

/** Compute approval amount: transfer amount + ledger fee (for transfer_from). */
function approvalAmount(amount: bigint, decimals: number): bigint {
  return amount + getLedgerFee(decimals);
}

// ──────────────────────────────────────────────────────────────
// APY Calculation
// ──────────────────────────────────────────────────────────────

/**
 * Calculate APY from virtual price snapshots over a given window.
 * @param currentVp Current virtual price (scaled by 1e18)
 * @param snapshots Historical VP snapshots sorted oldest-first
 * @param days Window in days (e.g. 1, 7, 30)
 * @returns APY as a decimal (e.g. 0.05 = 5%), or null if insufficient data
 */
export function calculateApy(
  currentVp: bigint,
  snapshots: VirtualPriceSnapshot[],
  days: number
): number | null {
  if (snapshots.length === 0 || currentVp === 0n) return null;

  const nowSecs = Math.floor(Date.now() / 1000);
  const targetSecs = nowSecs - days * 86400;

  // Find the snapshot closest to `days` ago
  let closest: VirtualPriceSnapshot | null = null;
  let closestDist = Infinity;

  for (const snap of snapshots) {
    const ts = Number(snap.timestamp_secs);
    const dist = Math.abs(ts - targetSecs);
    if (dist < closestDist) {
      closestDist = dist;
      closest = snap;
    }
  }

  if (!closest || closest.virtual_price === 0n) return null;

  // Need at least 1 hour of elapsed time for meaningful APY
  const elapsedSecs = nowSecs - Number(closest.timestamp_secs);
  if (elapsedSecs < 3600) return null;

  const vpNow = Number(currentVp);
  const vpThen = Number(closest.virtual_price);
  const periodReturn = vpNow / vpThen - 1;

  if (periodReturn < 0) return 0; // VP shouldn't decrease, but clamp to 0

  const daysElapsed = elapsedSecs / 86400;
  const apy = Math.pow(1 + periodReturn, 365 / daysElapsed) - 1;

  return apy;
}

/**
 * Calculate theoretical APY for 3pool LPs based on protocol borrowing interest.
 *
 * Formula:
 *   totalApr = sum over collaterals of (weightedRate × threePoolShare × totalDebt / poolTvl)
 *   apy = (1 + totalApr / 365)^365 - 1
 *
 * @param threePoolShareBps  3pool's share of interest in basis points (e.g. 5000 = 50%)
 * @param perCollateralInterest  Array of { totalDebtE8s, weightedInterestRate } per collateral
 * @param poolTvlE8s  Total stablecoin balance in the 3pool (in e8s, normalized)
 * @returns APY as a decimal (e.g. 0.047 = 4.7%), or null if insufficient data
 */
export function calculateTheoreticalApy(
  threePoolShareBps: number,
  perCollateralInterest: { totalDebtE8s: number; weightedInterestRate: number }[],
  poolTvlE8s: number,
): number | null {
  if (poolTvlE8s <= 0 || perCollateralInterest.length === 0) return null;

  const threePoolShare = threePoolShareBps / 10_000;
  let totalApr = 0;

  for (const info of perCollateralInterest) {
    if (info.totalDebtE8s === 0 || info.weightedInterestRate === 0) continue;
    totalApr += (info.weightedInterestRate * threePoolShare * info.totalDebtE8s) / poolTvlE8s;
  }

  if (totalApr === 0) return null;

  // Daily compounding: APY = (1 + APR/365)^365 - 1
  const apy = Math.pow(1 + totalApr / 365, 365) - 1;
  return apy;
}

// ──────────────────────────────────────────────────────────────
// Service
// ──────────────────────────────────────────────────────────────

const THREEPOOL_CANISTER_ID = CANISTER_IDS.THREEPOOL;

class ThreePoolService {
  private _anonAgent: HttpAgent | null = null;

  private async getQueryActor(): Promise<any> {
    if (!this._anonAgent) {
      this._anonAgent = new HttpAgent({
        host: CONFIG.host,
        identity: new AnonymousIdentity(),
      });
      if (CONFIG.isLocal) {
        await this._anonAgent.fetchRootKey();
      }
    }
    return Actor.createActor(canisterIDLs.three_pool as any, {
      agent: this._anonAgent,
      canisterId: THREEPOOL_CANISTER_ID,
    });
  }

  // ── Queries (anonymous, no wallet needed) ──

  async getPoolStatus(): Promise<PoolStatus> {
    const actor = await this.getQueryActor();
    return await actor.get_pool_status() as PoolStatus;
  }

  async calcSwap(fromIndex: number, toIndex: number, dxRaw: bigint): Promise<bigint> {
    const actor = await this.getQueryActor();
    const result = await actor.calc_swap(fromIndex, toIndex, dxRaw) as { Ok: bigint } | { Err: any };
    if ('Err' in result) {
      throw new Error(this.formatError(result.Err));
    }
    return result.Ok;
  }

  async quoteSwap(fromIndex: number, toIndex: number, dxRaw: bigint): Promise<QuoteSwapResult> {
    const actor = await this.getQueryActor();
    const result = await actor.quote_swap(fromIndex, toIndex, dxRaw) as { Ok: QuoteSwapResult } | { Err: any };
    if ('Err' in result) {
      throw new Error(this.formatError(result.Err));
    }
    return result.Ok;
  }

  async getLpBalance(principal: Principal): Promise<bigint> {
    const actor = await this.getQueryActor();
    return await actor.get_lp_balance(principal) as bigint;
  }

  async calcAddLiquidity(amounts: bigint[]): Promise<bigint> {
    const actor = await this.getQueryActor();
    const result = await actor.calc_add_liquidity_query(amounts, BigInt(0)) as { Ok: bigint } | { Err: any };
    if ('Err' in result) throw new Error(this.formatError(result.Err));
    return result.Ok;
  }

  async calcRemoveLiquidity(lpBurn: bigint): Promise<bigint[]> {
    const actor = await this.getQueryActor();
    const result = await actor.calc_remove_liquidity_query(lpBurn) as { Ok: bigint[] } | { Err: any };
    if ('Err' in result) throw new Error(this.formatError(result.Err));
    return result.Ok;
  }

  async calcRemoveOneCoin(lpBurn: bigint, coinIndex: number): Promise<bigint> {
    const actor = await this.getQueryActor();
    const result = await actor.calc_remove_one_coin_query(lpBurn, coinIndex) as { Ok: bigint } | { Err: any };
    if ('Err' in result) throw new Error(this.formatError(result.Err));
    return result.Ok;
  }

  async getVpSnapshots(): Promise<VirtualPriceSnapshot[]> {
    const actor = await this.getQueryActor();
    return await actor.get_vp_snapshots() as VirtualPriceSnapshot[];
  }

  async getSwapEvents(start: bigint, length: bigint): Promise<any[]> {
    const actor = await this.getQueryActor();
    return await actor.get_swap_events(start, length) as any[];
  }

  async getSwapEventCount(): Promise<bigint> {
    const actor = await this.getQueryActor();
    return await actor.get_swap_event_count() as bigint;
  }

  // v2 liquidity endpoint is newest-first: offset skips the N most-recent events, limit takes the next batch.
  // The returned array is reversed by the canister so events[0] is the newest event in the returned window.
  async getLiquidityEvents(limit: bigint, offset: bigint): Promise<any[]> {
    const actor = await this.getQueryActor();
    return await actor.get_liquidity_events_v2(limit, offset) as any[];
  }

  async getLiquidityEventCount(): Promise<bigint> {
    const actor = await this.getQueryActor();
    return await actor.get_liquidity_event_count_v2() as bigint;
  }

  async getAdminEvents(start: bigint, length: bigint): Promise<any[]> {
    const actor = await this.getQueryActor();
    return await actor.get_admin_events(start, length) as any[];
  }

  async getAdminEventCount(): Promise<bigint> {
    const actor = await this.getQueryActor();
    return await actor.get_admin_event_count() as bigint;
  }

  // ── Rich analytics queries (PoolStateView / PoolStats / PoolHealth + series) ──

  async getPoolState(): Promise<any> {
    const actor = await this.getQueryActor();
    return await actor.get_pool_state();
  }

  async getPoolStats(window: 'Last24h' | 'Last7d' | 'Last30d' | 'AllTime' = 'Last24h'): Promise<any> {
    const actor = await this.getQueryActor();
    return await actor.get_pool_stats({ [window]: null } as any);
  }

  async getPoolHealth(): Promise<any> {
    const actor = await this.getQueryActor();
    return await actor.get_pool_health();
  }

  async getVolumeSeries(
    window: 'Last24h' | 'Last7d' | 'Last30d' | 'AllTime' = 'Last7d',
    bucketSecs: bigint = 3600n,
  ): Promise<any[]> {
    const actor = await this.getQueryActor();
    return await actor.get_volume_series({ [window]: null } as any, bucketSecs) as any[];
  }

  async getBalanceSeries(
    window: 'Last24h' | 'Last7d' | 'Last30d' | 'AllTime' = 'Last7d',
    bucketSecs: bigint = 3600n,
  ): Promise<any[]> {
    const actor = await this.getQueryActor();
    return await actor.get_balance_series({ [window]: null } as any, bucketSecs) as any[];
  }

  async getVirtualPriceSeries(
    window: 'Last24h' | 'Last7d' | 'Last30d' | 'AllTime' = 'Last7d',
    bucketSecs: bigint = 3600n,
  ): Promise<any[]> {
    const actor = await this.getQueryActor();
    return await actor.get_virtual_price_series({ [window]: null } as any, bucketSecs) as any[];
  }

  // ── Mutations ──

  async swap(fromIndex: number, toIndex: number, dxRaw: bigint, minDyRaw: bigint): Promise<bigint> {
    const wallet = get(walletStore);
    if (!wallet.isConnected) throw new Error('Wallet not connected');

    const fromToken = POOL_TOKENS[fromIndex];
    const oisyDetected = isOisyWallet();

    if (oisyDetected && wallet.principal) {
      // ─── Oisy ICRC-112 batched path ───
      const signerAgent = await getOisySignerAgent(wallet.principal);

      const ledgerActor = createOisyActor(
        fromToken.ledgerId, CONFIG.icusd_ledgerIDL, signerAgent
      );
      const poolActor = createOisyActor(
        THREEPOOL_CANISTER_ID, canisterIDLs.three_pool, signerAgent
      );

      // Sequence 0: approve
      signerAgent.batch();
      const approvePromise = ledgerActor.icrc2_approve({
        amount: approvalAmount(dxRaw, fromToken.decimals),
        spender: { owner: Principal.fromText(THREEPOOL_CANISTER_ID), subaccount: [] },
        expires_at: [], expected_allowance: [], memo: [], fee: [],
        from_subaccount: [], created_at_time: []
      });

      // Sequence 1: swap
      signerAgent.batch();
      const swapPromise = poolActor.swap(fromIndex, toIndex, dxRaw, minDyRaw);

      await signerAgent.execute();
      const [approveResult, swapResult] = await Promise.all([approvePromise, swapPromise]);

      if (approveResult && 'Err' in approveResult) {
        throw new Error(`Approval failed: ${JSON.stringify(approveResult.Err)}`);
      }
      if ('Err' in swapResult) {
        throw new Error(this.formatError(swapResult.Err));
      }
      return swapResult.Ok;
    } else {
      // ─── Non-Oisy path (Plug, II, etc.) ───
      const ledgerActor = await walletStore.getActor(
        fromToken.ledgerId, CONFIG.icusd_ledgerIDL
      ) as any;

      const approveResult = await ledgerActor.icrc2_approve({
        amount: approvalAmount(dxRaw, fromToken.decimals),
        spender: { owner: Principal.fromText(THREEPOOL_CANISTER_ID), subaccount: [] },
        expires_at: [], expected_allowance: [], memo: [], fee: [],
        from_subaccount: [], created_at_time: []
      });

      if (approveResult && 'Err' in approveResult) {
        throw new Error(`Approval failed: ${JSON.stringify(approveResult.Err)}`);
      }

      // Small delay for ledger sync
      await new Promise(r => setTimeout(r, 2000));

      const poolActor = await walletStore.getActor(
        THREEPOOL_CANISTER_ID, canisterIDLs.three_pool
      ) as any;
      const result = await poolActor.swap(fromIndex, toIndex, dxRaw, minDyRaw) as { Ok: bigint } | { Err: any };
      if ('Err' in result) {
        throw new Error(this.formatError(result.Err));
      }
      return result.Ok;
    }
  }

  async addLiquidity(amounts: bigint[], minLp: bigint): Promise<bigint> {
    const wallet = get(walletStore);
    if (!wallet.isConnected) throw new Error('Wallet not connected');

    const oisyDetected = isOisyWallet();
    const spender = { owner: Principal.fromText(THREEPOOL_CANISTER_ID), subaccount: [] };

    if (oisyDetected && wallet.principal) {
      // ─── Oisy ICRC-112 batched path ───
      const signerAgent = await getOisySignerAgent(wallet.principal);

      // Queue approvals for each non-zero token
      const approvePromises: Promise<any>[] = [];
      for (let k = 0; k < 3; k++) {
        if (amounts[k] > 0n) {
          const token = POOL_TOKENS[k];
          const ledgerActor = createOisyActor(token.ledgerId, CONFIG.icusd_ledgerIDL, signerAgent);
          signerAgent.batch();
          approvePromises.push(ledgerActor.icrc2_approve({
            amount: approvalAmount(amounts[k], POOL_TOKENS[k].decimals),
            spender, expires_at: [], expected_allowance: [], memo: [], fee: [],
            from_subaccount: [], created_at_time: []
          }));
        }
      }

      // Queue add_liquidity call
      const poolActor = createOisyActor(THREEPOOL_CANISTER_ID, canisterIDLs.three_pool, signerAgent);
      signerAgent.batch();
      const addPromise = poolActor.add_liquidity(amounts, minLp);

      await signerAgent.execute();
      const results = await Promise.all([...approvePromises, addPromise]);

      // Check approve results
      for (let i = 0; i < approvePromises.length; i++) {
        const r = results[i];
        if (r && 'Err' in r) throw new Error(`Approval failed: ${JSON.stringify(r.Err)}`);
      }

      const addResult = results[results.length - 1] as { Ok: bigint } | { Err: any };
      if ('Err' in addResult) throw new Error(this.formatError(addResult.Err));
      return addResult.Ok;
    } else {
      // ─── Non-Oisy path: sequential approvals ───
      for (let k = 0; k < 3; k++) {
        if (amounts[k] > 0n) {
          const token = POOL_TOKENS[k];
          const ledgerActor = await walletStore.getActor(token.ledgerId, CONFIG.icusd_ledgerIDL) as any;
          const approveResult = await ledgerActor.icrc2_approve({
            amount: approvalAmount(amounts[k], POOL_TOKENS[k].decimals),
            spender, expires_at: [], expected_allowance: [], memo: [], fee: [],
            from_subaccount: [], created_at_time: []
          });
          if (approveResult && 'Err' in approveResult) {
            throw new Error(`Approval failed for ${token.symbol}: ${JSON.stringify(approveResult.Err)}`);
          }
          await new Promise(r => setTimeout(r, 2000));
        }
      }

      const poolActor = await walletStore.getActor(THREEPOOL_CANISTER_ID, canisterIDLs.three_pool) as any;
      const result = await poolActor.add_liquidity(amounts, minLp) as { Ok: bigint } | { Err: any };
      if ('Err' in result) throw new Error(this.formatError(result.Err));
      return result.Ok;
    }
  }

  async removeLiquidity(lpBurn: bigint, minAmounts: bigint[]): Promise<bigint[]> {
    const wallet = get(walletStore);
    if (!wallet.isConnected) throw new Error('Wallet not connected');

    const poolActor = await walletStore.getActor(THREEPOOL_CANISTER_ID, canisterIDLs.three_pool) as any;
    const result = await poolActor.remove_liquidity(lpBurn, minAmounts) as { Ok: bigint[] } | { Err: any };
    if ('Err' in result) throw new Error(this.formatError(result.Err));
    return result.Ok;
  }

  async removeOneCoin(lpBurn: bigint, coinIndex: number, minAmount: bigint): Promise<bigint> {
    const wallet = get(walletStore);
    if (!wallet.isConnected) throw new Error('Wallet not connected');

    const poolActor = await walletStore.getActor(THREEPOOL_CANISTER_ID, canisterIDLs.three_pool) as any;
    const result = await poolActor.remove_one_coin(lpBurn, coinIndex, minAmount) as { Ok: bigint } | { Err: any };
    if ('Err' in result) throw new Error(this.formatError(result.Err));
    return result.Ok;
  }

  // ── Error formatting ──

  private formatError(err: any): string {
    if ('InsufficientOutput' in err) {
      return `Insufficient output: expected at least ${err.InsufficientOutput.expected_min}, got ${err.InsufficientOutput.actual}`;
    }
    if ('InsufficientLiquidity' in err) return 'Insufficient liquidity in the pool';
    if ('InvalidCoinIndex' in err) return 'Invalid token index';
    if ('ZeroAmount' in err) return 'Amount must be greater than zero';
    if ('PoolEmpty' in err) return 'Pool has no liquidity';
    if ('SlippageExceeded' in err) return 'Slippage tolerance exceeded';
    if ('TransferFailed' in err) return `Transfer failed (${err.TransferFailed.token}): ${err.TransferFailed.reason}`;
    if ('Unauthorized' in err) return 'Unauthorized';
    if ('MathOverflow' in err) return 'Math overflow';
    if ('InvariantNotConverged' in err) return 'Invariant calculation failed';
    if ('PoolPaused' in err) return 'Pool is currently paused';
    return 'Unknown error';
  }
}

export const threePoolService = new ThreePoolService();
