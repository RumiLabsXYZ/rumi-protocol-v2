import { Principal } from '@dfinity/principal';
import { Actor, HttpAgent, AnonymousIdentity } from '@dfinity/agent';
import { canisterIDLs } from './pnp';
import { walletStore } from '../stores/wallet';
import { get } from 'svelte/store';
import { CANISTER_IDS, CONFIG } from '../config';
import { isOisyWallet } from './protocol/walletOperations';
import { getOisySignerAgent, createOisyActor } from './oisySigner';
import { fetchLedgerFee, getCachedLedgerFee } from './ledgerFeeService';

// ──────────────────────────────────────────────────────────────
// Types — mirrors the AMM Candid interface
// ──────────────────────────────────────────────────────────────

export interface PoolInfo {
  pool_id: string;
  token_a: Principal;
  token_b: Principal;
  reserve_a: bigint;
  reserve_b: bigint;
  fee_bps: number;
  protocol_fee_bps: number;
  curve: { ConstantProduct: null };
  total_lp_shares: bigint;
  paused: boolean;
}

export interface SwapResult {
  amount_out: bigint;
  fee: bigint;
}

export interface SwapPayoutResult extends SwapResult {
  /** Exact output credited after the AMM's pinned ICRC-1 payout fee. */
  amount_out_net: bigint;
}

// ── Analytics window variants (mirrors Candid AmmStatsWindow) ──

export type AmmStatsWindow = 'Hour' | 'Day' | 'Week' | 'Month' | 'All';

type AmmIntent = { id: Uint8Array; fingerprint: string };

function ammIntentFingerprint(kind: string, args: unknown[]): string {
  return JSON.stringify([kind, ...args.map(value => typeof value === 'bigint' ? value.toString() : value)]);
}

function bytesToHex(bytes: Uint8Array): string {
  return Array.from(bytes, byte => byte.toString(16).padStart(2, '0')).join('');
}

function hexToBytes(hex: string): Uint8Array | null {
  if (!/^[0-9a-f]{64}$/i.test(hex)) return null;
  return Uint8Array.from(hex.match(/.{2}/g)!, byte => parseInt(byte, 16));
}

function onChainFingerprint(operation: any): string | null {
  if (!operation?.kind || !operation.pool_id) return null;
  if ('Swap' in operation.kind) {
    const x = operation.kind.Swap;
    return ammIntentFingerprint('swap', [operation.pool_id, x.token_in.toText(), BigInt(x.amount_in), BigInt(x.min_amount_out)]);
  }
  if ('AddLiquidity' in operation.kind) {
    const x = operation.kind.AddLiquidity;
    return ammIntentFingerprint('add', [operation.pool_id, BigInt(x.amount_a), BigInt(x.amount_b), BigInt(x.min_lp_shares)]);
  }
  if ('RemoveLiquidity' in operation.kind) {
    const x = operation.kind.RemoveLiquidity;
    return ammIntentFingerprint('remove', [operation.pool_id, BigInt(x.lp_shares), BigInt(x.min_amount_a), BigInt(x.min_amount_b)]);
  }
  return null;
}

function unwrapCandidOpt<T>(value: any): T | null {
  return Array.isArray(value) ? ((value[0] as T | undefined) ?? null) : ((value as T | undefined) ?? null);
}

function windowToVariant(window: AmmStatsWindow): Record<string, null> {
  return { [window]: null };
}

// ──────────────────────────────────────────────────────────────
// Token metadata for AMM-tradeable tokens
// ──────────────────────────────────────────────────────────────

export interface AmmToken {
  symbol: string;
  ledgerId: string;
  decimals: number;
  color: string;
  /** Wallet store key for balance lookup */
  balanceKey: string;
  /** Whether this is the 3pool LP token (3USD) */
  is3USD: boolean;
  /** 3pool index if this is a stablecoin in the 3pool (-1 if not) */
  threePoolIndex: number;
}

export const AMM_TOKENS: AmmToken[] = [
  {
    symbol: 'ICP',
    ledgerId: CANISTER_IDS.ICP_LEDGER,
    decimals: 8,
    color: '#29abe2',
    balanceKey: 'ICP',
    is3USD: false,
    threePoolIndex: -1,
  },
  {
    symbol: '3USD',
    ledgerId: CANISTER_IDS.THREEPOOL,
    decimals: 8,
    color: '#34d399',
    balanceKey: 'THREEUSD',
    is3USD: true,
    threePoolIndex: -1,
  },
  {
    symbol: 'icUSD',
    ledgerId: CANISTER_IDS.ICUSD_LEDGER,
    decimals: 8,
    color: '#818cf8',
    balanceKey: 'ICUSD',
    is3USD: false,
    threePoolIndex: 0,
  },
  {
    symbol: 'ckUSDT',
    ledgerId: CANISTER_IDS.CKUSDT_LEDGER,
    decimals: 6,
    color: '#26A17B',
    balanceKey: 'CKUSDT',
    is3USD: false,
    threePoolIndex: 1,
  },
  {
    symbol: 'ckUSDC',
    ledgerId: CANISTER_IDS.CKUSDC_LEDGER,
    decimals: 6,
    color: '#2775CA',
    balanceKey: 'CKUSDC',
    is3USD: false,
    threePoolIndex: 2,
  },
];

/** Live ledger transfer fee for an AMM token (audit ICRC-005). */
export function tokenFee(token: AmmToken): Promise<bigint> {
  return fetchLedgerFee({
    ledgerId: token.ledgerId,
    decimals: token.decimals,
    symbol: token.symbol,
  });
}

/**
 * Synchronous version of `tokenFee`. Returns the cached fee or the hardcoded
 * fallback. Use inside Oisy signer code paths where a click-handler gesture
 * window must be preserved (no `await` between click and the first Oisy
 * popup). Callers MUST ensure the cache is warmed first via
 * `preWarmOisyFees()` during the quote phase.
 */
export function tokenFeeCached(token: AmmToken): bigint {
  return getCachedLedgerFee({
    ledgerId: token.ledgerId,
    decimals: token.decimals,
    symbol: token.symbol,
  });
}

/** Compute approval amount: transfer amount + the live ledger fee. */
export async function approvalAmount(amount: bigint, token: AmmToken): Promise<bigint> {
  const fee = await tokenFee(token);
  return amount + fee;
}

export function parseTokenAmount(amount: string, decimals: number): bigint {
  const trimmed = amount.trim();
  if (trimmed === '' || trimmed === '.') throw new Error('Invalid amount');

  const parts = trimmed.split('.');
  if (parts.length > 2) throw new Error('Invalid amount');

  const whole = parts[0] || '0';
  let frac = parts.length === 2 ? parts[1] : '';

  // Pad or truncate fractional part to exact `decimals` digits
  if (frac.length > decimals) {
    frac = frac.slice(0, decimals);
  } else {
    frac = frac.padEnd(decimals, '0');
  }

  const raw = BigInt(whole) * BigInt(10 ** decimals) + BigInt(frac);
  if (raw < 0n) throw new Error('Invalid amount');
  return raw;
}

export function formatTokenAmount(amount: bigint, decimals: number): string {
  const divisor = 10n ** BigInt(decimals);
  const whole = amount / divisor;
  const frac = amount % divisor;

  // Pad fractional part to full decimals width
  const fracStr = frac.toString().padStart(decimals, '0');

  // Show up to 4 decimal places for normal values, more for tiny values
  const threshold = divisor / 100n; // 0.01 in token units

  if (amount > 0n && amount < threshold) {
    // Tiny value — show up to 6 decimals
    const places = Math.min(decimals, 6);
    const trimmedFrac = fracStr.slice(0, places).replace(/0+$/, '') || '0';
    return `${whole}.${trimmedFrac}`;
  }

  // Normal: 4 decimal places, trim trailing zeros but keep at least 2
  let display = fracStr.slice(0, 4);
  display = display.replace(/0+$/, '');
  if (display.length === 0) display = '00';
  else if (display.length === 1) display += '0';

  return `${whole}.${display}`;
}

// ──────────────────────────────────────────────────────────────
// Service
// ──────────────────────────────────────────────────────────────

const AMM_CANISTER_ID = CANISTER_IDS.RUMI_AMM;

class AmmService {
  async swapWithPreapprovedActor(
    actor: any,
    principal: Principal,
    poolId: string,
    tokenIn: Principal,
    amountIn: bigint,
    minAmountOut: bigint,
  ): Promise<SwapPayoutResult> {
    const fingerprint = ammIntentFingerprint('swap', [poolId, tokenIn.toText(), amountIn, minAmountOut]);
    const requestId = await this.resolveIntentId(actor, principal, fingerprint);
    const result = await actor.swap_v2(requestId, poolId, tokenIn, amountIn, minAmountOut);
    if ('Err' in result) {
      await this.clearTerminalIntent(actor, principal, fingerprint);
      throw new Error(this.formatError(result.Err));
    }
    const operation = unwrapCandidOpt<any>(await actor.get_my_amm_operation());
    const fee = operation?.computed_values?.[3] !== undefined ? BigInt(operation.computed_values[3]) : 0n;
    const payoutResult = { ...result.Ok, amount_out_net: result.Ok.amount_out > fee ? result.Ok.amount_out - fee : 0n };
    this.clearIntentId(principal, fingerprint);
    return payoutResult;
  }

  private async resolveIntentId(actor: any, principal: Principal, fingerprint: string): Promise<Uint8Array> {
    const storageKey = `rumi-amm-v2:${principal.toText()}`;
    let cached: { fingerprint: string; id: string } | null = null;
    try {
      const raw = localStorage.getItem(storageKey);
      if (raw) cached = JSON.parse(raw);
    } catch {
      throw new Error('AMM retry identity storage is unavailable; refusing a request that could not be safely resumed');
    }
    const pending = unwrapCandidOpt<any>(await actor.get_my_amm_operation());
    const pendingId = pending ? Uint8Array.from(pending.request_id) : null;
    const complete = pending?.phase && 'Complete' in pending.phase;

    if (pending && !complete) {
      const exact = onChainFingerprint(pending);
      if (exact !== fingerprint) {
        throw new Error('A prior AMM operation is unresolved. Resume it with its original arguments before starting another operation.');
      }
      const id = pendingId!;
      try { localStorage.setItem(storageKey, JSON.stringify({ fingerprint, id: bytesToHex(id) })); }
      catch { throw new Error('Could not persist the AMM retry identity; refusing to proceed'); }
      return id;
    }

    if (cached?.fingerprint === fingerprint) {
      const id = hexToBytes(cached.id);
      if (id && pendingId && bytesToHex(id) === bytesToHex(pendingId) && onChainFingerprint(pending) === fingerprint) return id;
    }

    let previous = 0n;
    if (pendingId?.length === 32) {
      for (const byte of pendingId.slice(0, 8)) previous = (previous << 8n) | BigInt(byte);
    }
    if (previous >= 0xffffffffffffffffn) throw new Error('AMM request identity space is exhausted for this wallet');
    const id = new Uint8Array(32);
    let sequence = previous + 1n;
    for (let i = 7; i >= 0; i--) { id[i] = Number(sequence & 0xffn); sequence >>= 8n; }
    crypto.getRandomValues(id.subarray(8));
    try { localStorage.setItem(storageKey, JSON.stringify({ fingerprint, id: bytesToHex(id) })); }
    catch { throw new Error('Could not persist the AMM retry identity; refusing to proceed'); }
    return id;
  }

  private clearIntentId(principal: Principal, fingerprint: string): void {
    const key = `rumi-amm-v2:${principal.toText()}`;
    try {
      const raw = localStorage.getItem(key);
      if (raw && JSON.parse(raw).fingerprint === fingerprint) localStorage.removeItem(key);
    } catch { /* Keep the durable request ID if browser storage is unreadable. */ }
  }

  private async clearTerminalIntent(actor: any, principal: Principal, fingerprint: string): Promise<void> {
    try {
      const op = unwrapCandidOpt<any>(await actor.get_my_amm_operation());
      if (op?.phase && 'Complete' in op.phase) {
        const error = op.last_error;
        if ((Array.isArray(error) && error.length > 0) || (typeof error === 'string' && error.length > 0)) {
          this.clearIntentId(principal, fingerprint);
        }
      }
    } catch { /* retain the identity if terminal state cannot be confirmed */ }
  }

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
    return Actor.createActor(canisterIDLs.rumi_amm as any, {
      agent: this._anonAgent,
      canisterId: AMM_CANISTER_ID,
    });
  }

  async getMyAmmOperation(): Promise<any | null> {
    const wallet = get(walletStore);
    if (!wallet.isConnected || !wallet.principal) throw new Error('Wallet not connected');
    const actor = await walletStore.getActor(AMM_CANISTER_ID, canisterIDLs.rumi_amm) as any;
    const operation = await actor.get_my_amm_operation();
    return unwrapCandidOpt<any>(operation);
  }

  async reconcileMyAmmIngress(requestId: Uint8Array): Promise<boolean> {
    const wallet = get(walletStore);
    if (!wallet.isConnected || !wallet.principal) throw new Error('Wallet not connected');
    const actor = await walletStore.getActor(AMM_CANISTER_ID, canisterIDLs.rumi_amm) as any;
    const result = await actor.reconcile_amm_ingress(requestId);
    if ('Err' in result) throw new Error(this.formatError(result.Err));
    return result.Ok;
  }

  async getMyPendingAmmPayouts(): Promise<any[]> {
    const wallet = get(walletStore);
    if (!wallet.isConnected || !wallet.principal) throw new Error('Wallet not connected');
    const actor = await walletStore.getActor(AMM_CANISTER_ID, canisterIDLs.rumi_amm) as any;
    return await actor.get_pending_amm_payouts();
  }

  async retryAmmPayout(payoutId: bigint): Promise<bigint> {
    const wallet = get(walletStore);
    if (!wallet.isConnected || !wallet.principal) throw new Error('Wallet not connected');
    const actor = await walletStore.getActor(AMM_CANISTER_ID, canisterIDLs.rumi_amm) as any;
    const result = await actor.retry_amm_payout(payoutId);
    if ('Err' in result) throw new Error(this.formatError(result.Err));
    return result.Ok;
  }

  async reconcileAmmPayout(payoutId: bigint): Promise<boolean> {
    const wallet = get(walletStore);
    if (!wallet.isConnected || !wallet.principal) throw new Error('Wallet not connected');
    const actor = await walletStore.getActor(AMM_CANISTER_ID, canisterIDLs.rumi_amm) as any;
    const result = await actor.reconcile_amm_payout(payoutId);
    if ('Err' in result) throw new Error(this.formatError(result.Err));
    return result.Ok;
  }

  // ── Queries (anonymous) ──

  async getPool(poolId: string): Promise<PoolInfo | null> {
    const actor = await this.getQueryActor();
    const result = await actor.get_pool(poolId);
    return result.length > 0 ? result[0] : null;
  }

  async getPools(): Promise<PoolInfo[]> {
    const actor = await this.getQueryActor();
    return await actor.get_pools();
  }

  async getQuote(poolId: string, tokenIn: Principal, amountIn: bigint): Promise<bigint> {
    const actor = await this.getQueryActor();
    const result = await actor.get_quote(poolId, tokenIn, amountIn) as { Ok: bigint } | { Err: any };
    if ('Err' in result) throw new Error(this.formatError(result.Err));
    return result.Ok;
  }

  async getLpBalance(poolId: string, owner: Principal): Promise<bigint> {
    const actor = await this.getQueryActor();
    return await actor.get_lp_balance(poolId, owner);
  }

  async getSwapEvents(start: bigint, length: bigint): Promise<any[]> {
    const actor = await this.getQueryActor();
    return await actor.get_amm_swap_events(start, length);
  }

  async getSwapEventCount(): Promise<bigint> {
    const actor = await this.getQueryActor();
    return await actor.get_amm_swap_event_count();
  }

  async getLiquidityEvents(start: bigint, length: bigint): Promise<any[]> {
    const actor = await this.getQueryActor();
    return await actor.get_amm_liquidity_events(start, length);
  }

  async getLiquidityEventCount(): Promise<bigint> {
    const actor = await this.getQueryActor();
    return await actor.get_amm_liquidity_event_count();
  }

  async getAdminEvents(start: bigint, length: bigint): Promise<any[]> {
    const actor = await this.getQueryActor();
    return await actor.get_amm_admin_events(start, length);
  }

  async getAdminEventCount(): Promise<bigint> {
    const actor = await this.getQueryActor();
    return await actor.get_amm_admin_event_count();
  }

  // ── Analytics (per-pool) ──
  //
  // Mirrors rumi_3pool's analytics surface so /e/pool/{id} can render
  // AMM pools at parity. Queries key by the AMM pool_id (e.g.,
  // "fohh4-…_ryjl3-…") and are served from a 60s TTL cache canister-side.

  async getVolumeSeries(poolId: string, window: AmmStatsWindow, points: number): Promise<any[]> {
    const actor = await this.getQueryActor();
    return await actor.get_amm_volume_series({ pool: poolId, window: windowToVariant(window), points });
  }

  async getBalanceSeries(poolId: string, window: AmmStatsWindow, points: number): Promise<any[]> {
    const actor = await this.getQueryActor();
    return await actor.get_amm_balance_series({ pool: poolId, window: windowToVariant(window), points });
  }

  async getFeeSeries(poolId: string, window: AmmStatsWindow, points: number): Promise<any[]> {
    const actor = await this.getQueryActor();
    return await actor.get_amm_fee_series({ pool: poolId, window: windowToVariant(window), points });
  }

  async getPoolStats(poolId: string, window: AmmStatsWindow): Promise<any> {
    const actor = await this.getQueryActor();
    return await actor.get_amm_pool_stats({ pool: poolId, window: windowToVariant(window) });
  }

  async getTopSwappers(poolId: string, window: AmmStatsWindow, limit: number): Promise<Array<[Principal, bigint, bigint]>> {
    const actor = await this.getQueryActor();
    return await actor.get_amm_top_swappers({ pool: poolId, window: windowToVariant(window), limit });
  }

  async getTopLps(poolId: string, limit: number): Promise<Array<[Principal, bigint, number]>> {
    const actor = await this.getQueryActor();
    return await actor.get_amm_top_lps({ pool: poolId, limit });
  }

  async getSwapEventsByPrincipal(poolId: string, who: Principal, start: bigint, length: bigint): Promise<any[]> {
    const actor = await this.getQueryActor();
    return await actor.get_amm_swap_events_by_principal({ pool: poolId, who, start, length });
  }

  async getLiquidityEventsByPrincipal(poolId: string, who: Principal, start: bigint, length: bigint): Promise<any[]> {
    const actor = await this.getQueryActor();
    return await actor.get_amm_liquidity_events_by_principal({ pool: poolId, who, start, length });
  }

  async getSwapEventsByTimeRange(poolId: string, startNs: bigint, endNs: bigint, limit: bigint): Promise<any[]> {
    const actor = await this.getQueryActor();
    return await actor.get_amm_swap_events_by_time_range({
      pool: poolId,
      start_ns: startNs,
      end_ns: endNs,
      limit,
    });
  }

  // ── Mutations ──

  async swap(
    poolId: string,
    tokenIn: Principal,
    amountIn: bigint,
    minAmountOut: bigint,
    inputToken: AmmToken
  ): Promise<SwapResult> {
    const wallet = get(walletStore);
    if (!wallet.isConnected) throw new Error('Wallet not connected');
    if (!wallet.principal) throw new Error('Connected wallet principal is unavailable');

    const oisyDetected = isOisyWallet();
    const approveAmt = await approvalAmount(amountIn, inputToken);

    if (oisyDetected && wallet.principal) {
      console.log(`[Oisy] Sequential approve + AMM swap via @icp-sdk/signer v5`);
      const signerAgent = await getOisySignerAgent(wallet.principal);
      const ledgerActor = createOisyActor(inputToken.ledgerId, CONFIG.icusd_ledgerIDL, signerAgent);
      const ammActor = createOisyActor(AMM_CANISTER_ID, canisterIDLs.rumi_amm, signerAgent);

      // 1) Approve (first Oisy consent screen, Tier 1 native).
      const approveResult = await ledgerActor.icrc2_approve({
        amount: approveAmt,
        spender: { owner: Principal.fromText(AMM_CANISTER_ID), subaccount: [] },
        expires_at: [], expected_allowance: [], memo: [], fee: [],
        from_subaccount: [], created_at_time: [],
      });
      if (approveResult && 'Err' in approveResult) {
        throw new Error(`Approval failed: ${JSON.stringify(approveResult.Err)}`);
      }

      // 2) AMM swap (second Oisy consent screen).
      const fingerprint = ammIntentFingerprint('swap', [poolId, tokenIn.toText(), amountIn, minAmountOut]);
      const requestId = await this.resolveIntentId(ammActor, wallet.principal, fingerprint);
      const swapResult = await ammActor.swap_v2(requestId, poolId, tokenIn, amountIn, minAmountOut);
      if ('Err' in swapResult) {
        await this.clearTerminalIntent(ammActor, wallet.principal, fingerprint);
        throw new Error(this.formatError(swapResult.Err));
      }
      this.clearIntentId(wallet.principal, fingerprint);
      return swapResult.Ok;
    } else {
      const ledgerActor = await walletStore.getActor(inputToken.ledgerId, CONFIG.icusd_ledgerIDL) as any;
      const approveResult = await ledgerActor.icrc2_approve({
        amount: approveAmt,
        spender: { owner: Principal.fromText(AMM_CANISTER_ID), subaccount: [] },
        expires_at: [], expected_allowance: [], memo: [], fee: [],
        from_subaccount: [], created_at_time: [],
      });

      if (approveResult && 'Err' in approveResult) {
        throw new Error(`Approval failed: ${JSON.stringify(approveResult.Err)}`);
      }

      await new Promise(r => setTimeout(r, 2000));

      const ammActor = await walletStore.getActor(AMM_CANISTER_ID, canisterIDLs.rumi_amm) as any;
      const fingerprint = ammIntentFingerprint('swap', [poolId, tokenIn.toText(), amountIn, minAmountOut]);
      const requestId = await this.resolveIntentId(ammActor, wallet.principal, fingerprint);
      const result = await ammActor.swap_v2(requestId, poolId, tokenIn, amountIn, minAmountOut);
      if ('Err' in result) {
        await this.clearTerminalIntent(ammActor, wallet.principal, fingerprint);
        throw new Error(this.formatError(result.Err));
      }
      this.clearIntentId(wallet.principal, fingerprint);
      return result.Ok;
    }
  }

  async addLiquidity(
    poolId: string,
    amountA: bigint,
    amountB: bigint,
    minLpShares: bigint,
    tokenA: AmmToken,
    tokenB: AmmToken
  ): Promise<bigint> {
    const wallet = get(walletStore);
    if (!wallet.isConnected) throw new Error('Wallet not connected');
    if (!wallet.principal) throw new Error('Connected wallet principal is unavailable');

    const oisyDetected = isOisyWallet();

    // Pre-compute approval amounts from the WARM fee cache (synchronous). For
    // Oisy, awaiting a live icrc1_fee() here would burn the browser
    // user-gesture window before the first consent screen and trip the
    // "Signer window should not be opened outside of click handler" guard.
    // AmmLiquidityPanel warms the fee cache on mount.
    const approveA = amountA > 0n ? amountA + tokenFeeCached(tokenA) : 0n;
    const approveB = amountB > 0n ? amountB + tokenFeeCached(tokenB) : 0n;

    if (oisyDetected && wallet.principal) {
      console.log(`[Oisy] Sequential approve(s) + AMM add_liquidity via @icp-sdk/signer v5`);
      const signerAgent = await getOisySignerAgent(wallet.principal);

      // 1) Approve token A (first Oisy consent screen, if needed).
      if (amountA > 0n) {
        const ledgerA = createOisyActor(tokenA.ledgerId, CONFIG.icusd_ledgerIDL, signerAgent);
        const approveResultA = await ledgerA.icrc2_approve({
          amount: approveA,
          spender: { owner: Principal.fromText(AMM_CANISTER_ID), subaccount: [] },
          expires_at: [], expected_allowance: [], memo: [], fee: [],
          from_subaccount: [], created_at_time: [],
        });
        if (approveResultA && 'Err' in approveResultA) {
          throw new Error(`Approval failed for ${tokenA.symbol}: ${JSON.stringify(approveResultA.Err)}`);
        }
      }

      // 2) Approve token B (second consent screen, if needed).
      if (amountB > 0n) {
        const ledgerB = createOisyActor(tokenB.ledgerId, CONFIG.icusd_ledgerIDL, signerAgent);
        const approveResultB = await ledgerB.icrc2_approve({
          amount: approveB,
          spender: { owner: Principal.fromText(AMM_CANISTER_ID), subaccount: [] },
          expires_at: [], expected_allowance: [], memo: [], fee: [],
          from_subaccount: [], created_at_time: [],
        });
        if (approveResultB && 'Err' in approveResultB) {
          throw new Error(`Approval failed for ${tokenB.symbol}: ${JSON.stringify(approveResultB.Err)}`);
        }
      }

      // 3) add_liquidity (final consent screen).
      const ammActor = createOisyActor(AMM_CANISTER_ID, canisterIDLs.rumi_amm, signerAgent);
      const fingerprint = ammIntentFingerprint('add', [poolId, amountA, amountB, minLpShares]);
      const requestId = await this.resolveIntentId(ammActor, wallet.principal, fingerprint);
      const addResult = await ammActor.add_liquidity_v2(requestId, poolId, amountA, amountB, minLpShares);
      if ('Err' in addResult) {
        await this.clearTerminalIntent(ammActor, wallet.principal, fingerprint);
        throw new Error(this.formatError(addResult.Err));
      }
      this.clearIntentId(wallet.principal, fingerprint);
      return addResult.Ok;
    } else {
      const spender = { owner: Principal.fromText(AMM_CANISTER_ID), subaccount: [] };

      if (amountA > 0n) {
        const ledgerA = await walletStore.getActor(tokenA.ledgerId, CONFIG.icusd_ledgerIDL) as any;
        const r = await ledgerA.icrc2_approve({
          amount: approveA, spender,
          expires_at: [], expected_allowance: [], memo: [], fee: [],
          from_subaccount: [], created_at_time: [],
        });
        if (r && 'Err' in r) throw new Error(`Approval failed for ${tokenA.symbol}: ${JSON.stringify(r.Err)}`);
        await new Promise(r => setTimeout(r, 2000));
      }

      if (amountB > 0n) {
        const ledgerB = await walletStore.getActor(tokenB.ledgerId, CONFIG.icusd_ledgerIDL) as any;
        const r = await ledgerB.icrc2_approve({
          amount: approveB, spender,
          expires_at: [], expected_allowance: [], memo: [], fee: [],
          from_subaccount: [], created_at_time: [],
        });
        if (r && 'Err' in r) throw new Error(`Approval failed for ${tokenB.symbol}: ${JSON.stringify(r.Err)}`);
        await new Promise(r => setTimeout(r, 2000));
      }

      const ammActor = await walletStore.getActor(AMM_CANISTER_ID, canisterIDLs.rumi_amm) as any;
      const fingerprint = ammIntentFingerprint('add', [poolId, amountA, amountB, minLpShares]);
      const requestId = await this.resolveIntentId(ammActor, wallet.principal, fingerprint);
      const result = await ammActor.add_liquidity_v2(requestId, poolId, amountA, amountB, minLpShares);
      if ('Err' in result) {
        await this.clearTerminalIntent(ammActor, wallet.principal, fingerprint);
        throw new Error(this.formatError(result.Err));
      }
      this.clearIntentId(wallet.principal, fingerprint);
      return result.Ok;
    }
  }

  async removeLiquidity(
    poolId: string,
    lpShares: bigint,
    minAmountA: bigint,
    minAmountB: bigint
  ): Promise<{ amountA: bigint; amountB: bigint }> {
    const wallet = get(walletStore);
    if (!wallet.isConnected) throw new Error('Wallet not connected');
    if (!wallet.principal) throw new Error('Connected wallet principal is unavailable');

    const ammActor = await walletStore.getActor(AMM_CANISTER_ID, canisterIDLs.rumi_amm) as any;
    const fingerprint = ammIntentFingerprint('remove', [poolId, lpShares, minAmountA, minAmountB]);
    const requestId = await this.resolveIntentId(ammActor, wallet.principal, fingerprint);
    const result = await ammActor.remove_liquidity_v2(requestId, poolId, lpShares, minAmountA, minAmountB);
    if ('Err' in result) {
      await this.clearTerminalIntent(ammActor, wallet.principal, fingerprint);
      throw new Error(this.formatError(result.Err));
    }
    this.clearIntentId(wallet.principal, fingerprint);
    const [amountA, amountB] = result.Ok;
    return { amountA, amountB };
  }

  // ── Error formatting ──

  private formatError(err: any): string {
    if ('InsufficientOutput' in err) {
      return `Insufficient output: expected at least ${err.InsufficientOutput.expected_min}, got ${err.InsufficientOutput.actual}`;
    }
    if ('InsufficientLiquidity' in err) return 'Insufficient liquidity in the pool';
    if ('InsufficientLpShares' in err) return `Insufficient LP shares: need ${err.InsufficientLpShares.required}, have ${err.InsufficientLpShares.available}`;
    if ('PoolNotFound' in err) return 'Pool not found';
    if ('PoolAlreadyExists' in err) return 'Pool already exists';
    if ('PoolPaused' in err) return 'Pool is paused';
    if ('ZeroAmount' in err) return 'Amount must be greater than zero';
    if ('InvalidToken' in err) return 'Invalid token';
    if ('TransferFailed' in err) return `Transfer failed (${err.TransferFailed.token}): ${err.TransferFailed.reason}`;
    if ('Unauthorized' in err) return 'Unauthorized';
    if ('MathOverflow' in err) return 'Math overflow';
    if ('DisproportionateLiquidity' in err) return 'Amounts must be proportional to pool reserves';
    if ('PoolCreationClosed' in err) return 'Pool creation is currently closed';
    if ('FeeBpsOutOfRange' in err) return 'Fee must be between 0.01% and 10%';
    if ('MaintenanceMode' in err) return 'AMM is in maintenance mode — swaps and deposits are temporarily disabled';
    if ('ClaimNotFound' in err) return 'Claim not found (it may have already been resolved)';
    if ('PoolBusy' in err) return 'Pool is busy with another transaction. Please try again in a moment.';
    return 'Unknown AMM error';
  }
}

export const ammService = new AmmService();
