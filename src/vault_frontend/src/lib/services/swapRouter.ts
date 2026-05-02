import { Principal } from '@dfinity/principal';
import { threePoolService, POOL_TOKENS } from './threePoolService';
import { ammService, AMM_TOKENS, approvalAmount, tokenFee, type AmmToken } from './ammService';
import { CANISTER_IDS, CONFIG } from '../config';
import { isOisyWallet } from './protocol/walletOperations';
import { getOisySignerAgent, createOisyActor } from './oisySigner';
import { canisterIDLs } from './pnp';
import { walletStore } from '../stores/wallet';
import { get } from 'svelte/store';
import { RumiAmmProvider } from './providers/rumiAmmProvider';
import { IcpswapProvider } from './providers/icpswapProvider';
import { ProviderRegistry } from './providers/providerRegistry';
import { fetchLedgerFee } from './ledgerFeeService';
import type { ProviderQuote } from './providers/types';

// ──────────────────────────────────────────────────────────────
// Provider registry for the 3USD <-> ICP hop.
// Quotes Rumi AMM and ICPswap 3USD/ICP in parallel and picks the winner.
// ──────────────────────────────────────────────────────────────

const _threeUsdIcpRegistryFull = new ProviderRegistry([
  new RumiAmmProvider(),
  new IcpswapProvider({
    id: 'icpswap_3usd_icp',
    poolCanisterId: CANISTER_IDS.ICPSWAP_3USD_ICP_POOL,
    token0LedgerId: CANISTER_IDS.THREEPOOL,
    token1LedgerId: CANISTER_IDS.ICP_LEDGER,
    feeBps: 30,
  }),
]);

// Rumi-only fallback used when the ICPswap kill switch is off.
const _threeUsdIcpRegistryRumiOnly = new ProviderRegistry([new RumiAmmProvider()]);

// Dedicated registry for the direct icUSD/ICP pool on ICPswap (Task 11).
// Only ICPswap currently hosts this pair; if a Rumi AMM icUSD/ICP pool is
// added later, wire in RumiAmmProvider here too.
const _icUsdIcpRegistry = new ProviderRegistry([
  new IcpswapProvider({
    id: 'icpswap_icusd_icp',
    poolCanisterId: CANISTER_IDS.ICPSWAP_ICUSD_ICP_POOL,
    token0LedgerId: CANISTER_IDS.ICP_LEDGER,
    token1LedgerId: CANISTER_IDS.ICUSD_LEDGER,
    feeBps: 30,
  }),
]);

// ──────────────────────────────────────────────────────────────
// ICPswap routing kill switch
//
// Mirrors the backend `icpswap_routing_enabled` admin flag. When false (the
// default), the router skips every ICPswap provider and behaves as if only
// Rumi AMM + the 3pool existed. Fetched on app boot via
// `initIcpswapRoutingFlag()`; a page reload picks up admin flips. The default
// is intentionally off so a stale frontend never routes through ICPswap
// unless the backend has explicitly opted in.
// ──────────────────────────────────────────────────────────────

let _icpswapEnabled = false;

export function setIcpswapRoutingEnabled(enabled: boolean): void {
  _icpswapEnabled = enabled;
}

export function isIcpswapRoutingEnabled(): boolean {
  return _icpswapEnabled;
}

/**
 * Fetch the ICPswap routing kill switch from the backend and apply it.
 * Called once on app boot from the root layout. Errors are swallowed — a
 * failed fetch leaves the flag at its default (off), which is the safe
 * fallback.
 */
export async function initIcpswapRoutingFlag(): Promise<void> {
  try {
    const { publicActor } = await import('./protocol/apiClient');
    const enabled = await (publicActor as any).get_icpswap_routing_enabled();
    _icpswapEnabled = Boolean(enabled);
    console.log(`[swapRouter] initIcpswapRoutingFlag: ${_icpswapEnabled}`);
  } catch (err) {
    console.warn('[swapRouter] Failed to fetch icpswap_routing_enabled, defaulting to off:', err);
    _icpswapEnabled = false;
  }
}

function threeUsdIcpRegistry(): ProviderRegistry {
  return _icpswapEnabled ? _threeUsdIcpRegistryFull : _threeUsdIcpRegistryRumiOnly;
}

// ──────────────────────────────────────────────────────────────
// Route types
// ──────────────────────────────────────────────────────────────

export type RouteType =
  | 'three_pool_swap'       // Stablecoin <-> Stablecoin (direct 3pool)
  | 'three_pool_deposit'    // Stablecoin -> 3USD (mint via 3pool)
  | 'three_pool_redeem'     // 3USD -> Stablecoin (redeem via 3pool)
  | 'amm_swap'              // 3USD <-> ICP (direct AMM)
  | 'stable_to_icp'         // Stablecoin -> ICP (3pool deposit + AMM swap)
  | 'icp_to_stable'         // ICP -> Stablecoin (AMM swap + 3pool redeem)
  | 'icusd_icp_direct';     // icUSD <-> ICP (direct ICPswap icUSD/ICP pool)

export interface SwapRoute {
  type: RouteType;
  /** Human-readable path, e.g. "ckUSDC -> 3USD -> ICP" */
  pathDisplay: string;
  /** Number of on-chain hops */
  hops: number;
  /** Estimated output in raw units of the output token */
  estimatedOutput: bigint;
  /** Combined fee display (percentage) */
  feeDisplay: string;
  /** For multi-hop routes: intermediate output (e.g. 3USD amount between hops) */
  intermediateOutput?: bigint;
  /**
   * Cached Rumi AMM pool ID. Populated when the 3USD/ICP hop resolves to
   * Rumi AMM so the Oisy batched executor can reuse it without an extra
   * canister query.
   */
  poolId?: string;
  /**
   * Winning provider quote for single-hop `amm_swap` routes. Populated for
   * Case 4 (3USD <-> ICP). Passed back to the provider during execution.
   */
  providerQuote?: ProviderQuote;
  /**
   * Winning provider quote for the 3USD/ICP leg of a two-hop route
   * (Cases 5/6: stable <-> ICP).
   */
  hopProviderQuote?: ProviderQuote;
}

// The 3USD/ICP pool ID — cached after first lookup
let _cachedPoolId: string | null = null;

async function getAmmPoolId(): Promise<string> {
  if (_cachedPoolId) return _cachedPoolId;
  const pools = await ammService.getPools();
  const threeUsdPrincipal = CANISTER_IDS.THREEPOOL;
  const icpPrincipal = CANISTER_IDS.ICP_LEDGER;
  const pool = pools.find(p => {
    const a = p.token_a.toText();
    const b = p.token_b.toText();
    return (a === threeUsdPrincipal && b === icpPrincipal) ||
           (a === icpPrincipal && b === threeUsdPrincipal);
  });
  if (!pool) throw new Error('3USD/ICP AMM pool not found');
  _cachedPoolId = pool.pool_id;
  return _cachedPoolId;
}

/** Reset cached pool ID (e.g. when pools change) */
export function clearPoolIdCache() {
  _cachedPoolId = null;
}

// ──────────────────────────────────────────────────────────────
// Route resolver
// ──────────────────────────────────────────────────────────────

function isStablecoin(token: AmmToken): boolean {
  return token.threePoolIndex >= 0;
}

function is3USD(token: AmmToken): boolean {
  return token.is3USD;
}

function isICP(token: AmmToken): boolean {
  return token.symbol === 'ICP';
}

/**
 * Determine the swap route and fetch a combined quote.
 */
export async function resolveRoute(
  from: AmmToken,
  to: AmmToken,
  amountIn: bigint,
): Promise<SwapRoute> {

  // Case 1: Stablecoin <-> Stablecoin (3pool swap, dynamic fee)
  if (isStablecoin(from) && isStablecoin(to)) {
    const quote = await threePoolService.quoteSwap(from.threePoolIndex, to.threePoolIndex, amountIn);
    const feePct = (quote.fee_bps / 100).toFixed(2);
    return {
      type: 'three_pool_swap',
      pathDisplay: `${from.symbol} → ${to.symbol}`,
      hops: 1,
      estimatedOutput: quote.amount_out,
      feeDisplay: `${feePct}%${quote.is_rebalancing ? ' (rebalancing)' : ''}`,
    };
  }

  // Case 2: Stablecoin -> 3USD (3pool deposit)
  if (isStablecoin(from) && is3USD(to)) {
    const amounts = [0n, 0n, 0n];
    amounts[from.threePoolIndex] = amountIn;
    const output = await threePoolService.calcAddLiquidity(amounts);
    return {
      type: 'three_pool_deposit',
      pathDisplay: `${from.symbol} → 3USD`,
      hops: 1,
      estimatedOutput: output,
      feeDisplay: '~0%',
    };
  }

  // Case 3: 3USD -> Stablecoin (3pool redeem)
  if (is3USD(from) && isStablecoin(to)) {
    const output = await threePoolService.calcRemoveOneCoin(amountIn, to.threePoolIndex);
    return {
      type: 'three_pool_redeem',
      pathDisplay: `3USD → ${to.symbol}`,
      hops: 1,
      estimatedOutput: output,
      feeDisplay: '~0%',
    };
  }

  // Case 4: 3USD <-> ICP (best of Rumi AMM and ICPswap 3USD/ICP)
  if ((is3USD(from) && isICP(to)) || (isICP(from) && is3USD(to))) {
    const quote = await threeUsdIcpRegistry().bestQuote(from, to, amountIn);
    return {
      type: 'amm_swap',
      pathDisplay: quote.label,
      hops: 1,
      estimatedOutput: quote.amountOut,
      feeDisplay: quote.feeDisplay,
      providerQuote: quote,
      // Keep poolId populated when Rumi AMM wins so the Oisy helper can
      // reuse it without an extra canister query.
      poolId: quote.provider === 'rumi_amm' ? (quote.meta.poolId as string) : undefined,
    };
  }

  // Case 5a: icUSD <-> ICP (direct ICPswap icUSD/ICP pool vs 2-hop via 3pool).
  // icUSD is a stablecoin, so this MUST sit before Case 5 to take precedence.
  // Direct wins on ties (one fewer fee, simpler execution).
  const isIcUsd = (t: AmmToken) => t.symbol === 'icUSD';
  if ((isIcUsd(from) && isICP(to)) || (isICP(from) && isIcUsd(to))) {
    console.log(`[swapRouter] Case 5a: icUSD<->ICP, _icpswapEnabled=${_icpswapEnabled}`);
    // Option A: direct via ICPswap icUSD/ICP. Skipped entirely when the
    // kill switch is off — there is no Rumi-hosted icUSD/ICP pool, so with
    // ICPswap disabled the router always falls through to the 2-hop path.
    let directQuote: ProviderQuote | null = null;
    if (_icpswapEnabled) {
      try {
        directQuote = await _icUsdIcpRegistry.bestQuote(from, to, amountIn);
      } catch {
        // Pool too thin / offline; continue with 2-hop only
      }
    }

    // Option B: 2-hop via 3pool + best-of(3USD/ICP)
    const threeUsdToken = AMM_TOKENS.find(t => t.is3USD)!;
    let twoHopOutput: bigint;
    let twoHopIntermediate: bigint;
    let twoHopQuote: ProviderQuote;

    if (isIcUsd(from)) {
      const amounts = [0n, 0n, 0n];
      amounts[from.threePoolIndex] = amountIn;
      twoHopIntermediate = await threePoolService.calcAddLiquidity(amounts);
      twoHopQuote = await threeUsdIcpRegistry().bestQuote(threeUsdToken, to, twoHopIntermediate);
      twoHopOutput = twoHopQuote.amountOut;
    } else {
      twoHopQuote = await threeUsdIcpRegistry().bestQuote(from, threeUsdToken, amountIn);
      twoHopIntermediate = twoHopQuote.amountOut;
      twoHopOutput = await threePoolService.calcRemoveOneCoin(twoHopIntermediate, to.threePoolIndex);
    }

    // Direct wins on ties (one fewer fee)
    if (directQuote && directQuote.amountOut >= twoHopOutput) {
      return {
        type: 'icusd_icp_direct',
        pathDisplay: directQuote.label,
        hops: 1,
        estimatedOutput: directQuote.amountOut,
        feeDisplay: directQuote.feeDisplay,
        providerQuote: directQuote,
      };
    }

    // Fall back to 2-hop (reuses existing stable_to_icp / icp_to_stable execution)
    return {
      type: isIcUsd(from) ? 'stable_to_icp' : 'icp_to_stable',
      pathDisplay: isIcUsd(from)
        ? `icUSD → 3USD → ICP`
        : `ICP → 3USD → icUSD`,
      hops: 2,
      estimatedOutput: twoHopOutput,
      feeDisplay: twoHopQuote.feeDisplay,
      intermediateOutput: twoHopIntermediate,
      hopProviderQuote: twoHopQuote,
      poolId: twoHopQuote.provider === 'rumi_amm' ? (twoHopQuote.meta.poolId as string) : undefined,
    };
  }

  // Case 5: Stablecoin -> ICP (two-hop: 3pool deposit + best 3USD->ICP swap)
  if (isStablecoin(from) && isICP(to)) {
    const amounts = [0n, 0n, 0n];
    amounts[from.threePoolIndex] = amountIn;
    const threeUsdOut = await threePoolService.calcAddLiquidity(amounts);

    const threeUsdToken = AMM_TOKENS.find(t => t.is3USD)!;
    const hopQuote = await threeUsdIcpRegistry().bestQuote(threeUsdToken, to, threeUsdOut);

    return {
      type: 'stable_to_icp',
      pathDisplay: `${from.symbol} → 3USD → ICP`,
      hops: 2,
      estimatedOutput: hopQuote.amountOut,
      feeDisplay: hopQuote.feeDisplay,
      intermediateOutput: threeUsdOut,
      hopProviderQuote: hopQuote,
      poolId: hopQuote.provider === 'rumi_amm' ? (hopQuote.meta.poolId as string) : undefined,
    };
  }

  // Case 6: ICP -> Stablecoin (two-hop: best ICP->3USD swap + 3pool redeem)
  if (isICP(from) && isStablecoin(to)) {
    const threeUsdToken = AMM_TOKENS.find(t => t.is3USD)!;
    const hopQuote = await threeUsdIcpRegistry().bestQuote(from, threeUsdToken, amountIn);

    const stableOut = await threePoolService.calcRemoveOneCoin(hopQuote.amountOut, to.threePoolIndex);

    return {
      type: 'icp_to_stable',
      pathDisplay: `ICP → 3USD → ${to.symbol}`,
      hops: 2,
      estimatedOutput: stableOut,
      feeDisplay: hopQuote.feeDisplay,
      intermediateOutput: hopQuote.amountOut,
      hopProviderQuote: hopQuote,
      poolId: hopQuote.provider === 'rumi_amm' ? (hopQuote.meta.poolId as string) : undefined,
    };
  }

  throw new Error(`No route found for ${from.symbol} → ${to.symbol}`);
}

// ──────────────────────────────────────────────────────────────
// Route execution
// ──────────────────────────────────────────────────────────────

/**
 * Execute a resolved route.
 * For two-hop routes, splits slippage budget across both hops.
 * @returns The final output amount received
 */
export async function executeRoute(
  route: SwapRoute,
  from: AmmToken,
  to: AmmToken,
  amountIn: bigint,
  slippageBps: number,
): Promise<bigint> {
  const minOutput = route.estimatedOutput * BigInt(10000 - slippageBps) / 10000n;

  // Kill-switch guard: a route quoted while ICPswap routing was enabled must
  // not execute if the admin flag has since been flipped off. Re-quote instead
  // of silently sending the user down a disabled path.
  if (!_icpswapEnabled) {
    const winner = route.providerQuote?.provider ?? route.hopProviderQuote?.provider;
    if ((winner && isIcpswapProvider(winner)) || route.type === 'icusd_icp_direct') {
      throw new Error(
        'ICPswap routing is currently disabled. Please refresh the quote and try again.',
      );
    }
  }

  switch (route.type) {
    case 'three_pool_swap': {
      return await threePoolService.swap(
        from.threePoolIndex, to.threePoolIndex, amountIn, minOutput
      );
    }

    case 'three_pool_deposit': {
      const amounts = [0n, 0n, 0n];
      amounts[from.threePoolIndex] = amountIn;
      return await threePoolService.addLiquidity(amounts, minOutput);
    }

    case 'three_pool_redeem': {
      return await threePoolService.removeOneCoin(amountIn, to.threePoolIndex, minOutput);
    }

    case 'amm_swap': {
      const quote = route.providerQuote;
      if (!quote) throw new Error('amm_swap route missing providerQuote');

      // Oisy + ICPswap needs the batched executor: ICPswap's deposit -> swap
      // -> withdraw flow has to land in one signer popup. Rumi AMM works with
      // Oisy through the standard provider.swap path because ammService.swap
      // already routes through the signer agent. Plain II also uses
      // provider.swap for both providers.
      if (isOisyWallet() && isIcpswapProvider(quote.provider)) {
        return await executeIcpswapDirectOisy(route, from, to, amountIn, slippageBps);
      }

      const provider = threeUsdIcpRegistry().get(quote.provider);

      // RumiAmmProvider handles approval internally via ammService.swap.
      // ICPswap needs an explicit ICRC-2 approval to the pool canister before
      // the provider's depositFrom can pull funds.
      if (isIcpswapProvider(quote.provider)) {
        const poolCanisterId = quote.meta.poolCanisterId as string | undefined;
        if (typeof poolCanisterId !== 'string') {
          throw new Error('amm_swap: ICPswap quote missing meta.poolCanisterId');
        }
        await approveIcpswapPool(from, amountIn, poolCanisterId);
      }

      const result = await provider.swap(from, to, amountIn, minOutput, quote);
      return result.amountOut;
    }

    case 'stable_to_icp': {
      if (isOisyWallet()) {
        return await executeStableToIcpOisy(route, from, amountIn, slippageBps);
      }
      const hopQuote = route.hopProviderQuote;
      if (!hopQuote) throw new Error('stable_to_icp route missing hopProviderQuote');

      // Hop 1: stablecoin -> 3USD (3pool deposit)
      const amounts = [0n, 0n, 0n];
      amounts[from.threePoolIndex] = amountIn;
      const threeUsdEstimate = await threePoolService.calcAddLiquidity(amounts);
      const threeUsdMinOutput = threeUsdEstimate * BigInt(10000 - Math.ceil(slippageBps / 2)) / 10000n;
      const threeUsdReceived = await threePoolService.addLiquidity(amounts, threeUsdMinOutput);

      // Hop 2: 3USD -> ICP via winning provider. Re-quote with actual received
      // amount so the slippage budget reflects what we really have.
      const threeUsdToken = AMM_TOKENS.find(t => t.is3USD)!;
      const provider = threeUsdIcpRegistry().get(hopQuote.provider);
      const freshQuote = await provider.quote(threeUsdToken, to, threeUsdReceived);

      // ICPswap needs an explicit ICRC-2 approval to the pool canister; Rumi
      // AMM handles its own approval via ammService.swap.
      if (isIcpswapProvider(freshQuote.provider)) {
        const poolCanisterId = freshQuote.meta.poolCanisterId as string | undefined;
        if (typeof poolCanisterId !== 'string') {
          throw new Error('stable_to_icp: ICPswap quote missing meta.poolCanisterId');
        }
        await approveIcpswapPool(threeUsdToken, threeUsdReceived, poolCanisterId);
      }

      const icpMinOutput = freshQuote.amountOut * BigInt(10000 - Math.ceil(slippageBps / 2)) / 10000n;
      const result = await provider.swap(threeUsdToken, to, threeUsdReceived, icpMinOutput, freshQuote);
      return result.amountOut;
    }

    case 'icp_to_stable': {
      if (isOisyWallet()) {
        return await executeIcpToStableOisy(route, to, amountIn, slippageBps);
      }
      const hopQuote = route.hopProviderQuote;
      if (!hopQuote) throw new Error('icp_to_stable route missing hopProviderQuote');

      // Hop 1: ICP -> 3USD via winning provider
      const threeUsdToken = AMM_TOKENS.find(t => t.is3USD)!;
      const provider = threeUsdIcpRegistry().get(hopQuote.provider);

      // ICPswap needs an explicit ICRC-2 approval to the pool canister; Rumi
      // AMM handles its own approval via ammService.swap.
      if (isIcpswapProvider(hopQuote.provider)) {
        const poolCanisterId = hopQuote.meta.poolCanisterId as string | undefined;
        if (typeof poolCanisterId !== 'string') {
          throw new Error('icp_to_stable: ICPswap quote missing meta.poolCanisterId');
        }
        await approveIcpswapPool(from, amountIn, poolCanisterId);
      }

      const threeUsdMinOutput = hopQuote.amountOut * BigInt(10000 - Math.ceil(slippageBps / 2)) / 10000n;
      const hop1 = await provider.swap(from, threeUsdToken, amountIn, threeUsdMinOutput, hopQuote);

      // Hop 2: 3USD -> stablecoin (3pool redeem)
      const stableEstimate = await threePoolService.calcRemoveOneCoin(hop1.amountOut, to.threePoolIndex);
      const stableMinOutput = stableEstimate * BigInt(10000 - Math.ceil(slippageBps / 2)) / 10000n;
      return await threePoolService.removeOneCoin(hop1.amountOut, to.threePoolIndex, stableMinOutput);
    }

    case 'icusd_icp_direct': {
      const q = route.providerQuote;
      if (!q) throw new Error('icusd_icp_direct route missing providerQuote');

      if (isOisyWallet()) {
        return await executeIcpswapDirectOisy(route, from, to, amountIn, slippageBps);
      }

      // Non-Oisy: pre-approve input token to the ICPswap pool, then call provider.swap
      // (the provider does depositFrom -> swap -> withdraw internally).
      const poolCanisterId = q.meta.poolCanisterId as string | undefined;
      if (typeof poolCanisterId !== 'string') {
        throw new Error('icusd_icp_direct route has invalid meta.poolCanisterId');
      }

      await approveIcpswapPool(from, amountIn, poolCanisterId);

      const provider = _icUsdIcpRegistry.get(q.provider);
      const result = await provider.swap(from, to, amountIn, minOutput, q);
      return result.amountOut;
    }

    default:
      throw new Error(`Unknown route type: ${route.type}`);
  }
}

/** True when the provider id points at any ICPswap pool. Used by non-Oisy
 *  branches to decide whether an explicit ICRC-2 approval is required before
 *  dispatching to provider.swap(). */
function isIcpswapProvider(providerId: string): boolean {
  return providerId === 'icpswap_3usd_icp' || providerId === 'icpswap_icusd_icp';
}

/**
 * Approve an arbitrary ICPswap pool canister to pull `amountIn` of `from`
 * via ICRC-2 depositFrom. Mirrors the non-Oisy approval pattern in
 * threePoolService.swap (walletStore.getActor + icrc2_approve + ledger sync
 * delay) and reuses the `approvalAmount(amountIn, fromToken)` helper so the
 * fee buffer stays consistent with the rest of the codebase.
 */
async function approveIcpswapPool(
  fromToken: AmmToken,
  amountIn: bigint,
  poolCanisterId: string,
): Promise<void> {
  const approveAmt = await approvalAmount(amountIn, fromToken);
  const ledgerActor = await walletStore.getActor(
    fromToken.ledgerId, CONFIG.icusd_ledgerIDL
  ) as any;

  const approveResult = await ledgerActor.icrc2_approve({
    amount: approveAmt,
    spender: { owner: Principal.fromText(poolCanisterId), subaccount: [] },
    expires_at: [], expected_allowance: [], memo: [], fee: [],
    from_subaccount: [], created_at_time: [],
  });

  if (approveResult && 'Err' in approveResult) {
    throw new Error(`${fromToken.symbol} approval failed: ${JSON.stringify(approveResult.Err)}`);
  }

  // Small delay for ledger sync (matches threePoolService.swap non-Oisy path).
  await new Promise(r => setTimeout(r, 2000));
}

// ──────────────────────────────────────────────────────────────
// Oisy signer agent pre-warming
// ──────────────────────────────────────────────────────────────

/**
 * Pre-warm the Oisy signer agent so it's cached and ready when the
 * user clicks "Swap". Call this during the quote phase for Oisy wallets.
 * This does NOT open a popup — only execute() does.
 */
export async function preWarmOisySigner(): Promise<void> {
  const wallet = get(walletStore);
  if (!wallet.principal) return;
  if (!isOisyWallet()) return;
  await getOisySignerAgent(wallet.principal);
}

// ──────────────────────────────────────────────────────────────
// Oisy-batched multi-hop execution
//
// CRITICAL: These functions must NOT make any async canister calls.
// All estimates and pool IDs come pre-computed from resolveRoute()
// (stored on the SwapRoute object). The signer agent is pre-warmed
// during the quote phase. This ensures signerAgent.execute() opens
// its popup synchronously within the browser's click handler context.
// ──────────────────────────────────────────────────────────────

const THREEPOOL_ID = CANISTER_IDS.THREEPOOL;
const AMM_ID = CANISTER_IDS.RUMI_AMM;
const ICP_LEDGER_ID = CANISTER_IDS.ICP_LEDGER;

/**
 * Stablecoin → ICP (Oisy batched):
 * 1. Approve stablecoin → 3pool
 * 2. Approve 3USD → AMM (pre-approve estimated amount)
 * 3. 3pool.add_liquidity
 * 4. AMM.swap (using estimated 3USD amount)
 *
 * All estimates come from route.intermediateOutput / route.estimatedOutput
 * which were computed during resolveRoute(). No canister queries here.
 */
async function executeStableToIcpOisy(
  route: SwapRoute,
  from: AmmToken,
  amountIn: bigint,
  slippageBps: number,
): Promise<bigint> {
  const wallet = get(walletStore);
  if (!wallet.principal) throw new Error('Wallet not connected');

  const hopQuote = route.hopProviderQuote;
  if (!hopQuote) throw new Error('stable_to_icp Oisy route missing hopProviderQuote');

  if (hopQuote.provider === 'icpswap_3usd_icp') {
    return await executeStableToIcpOisyIcpswap(route, from, amountIn, slippageBps, hopQuote);
  }

  // All values pre-computed during resolveRoute — no canister calls
  const poolId = route.poolId!;
  const threeUsdEstimate = route.intermediateOutput!;
  const threeUsdMinOutput = threeUsdEstimate * BigInt(10000 - Math.ceil(slippageBps / 2)) / 10000n;
  const icpMinOutput = route.estimatedOutput * BigInt(10000 - slippageBps) / 10000n;
  const fromPoolToken = POOL_TOKENS[from.threePoolIndex];

  const amounts: [bigint, bigint, bigint] = [0n, 0n, 0n];
  amounts[from.threePoolIndex] = amountIn;

  // Pre-fetch live ICRC-1 fee before entering the signer batch (no awaits allowed mid-batch).
  const fromFee = await tokenFee(from);

  // Signer agent was pre-warmed during quote phase — cached, no popup
  const signerAgent = await getOisySignerAgent(wallet.principal);

  const stableLedger = createOisyActor(fromPoolToken.ledgerId, CONFIG.icusd_ledgerIDL, signerAgent);
  const threeUsdLedger = createOisyActor(THREEPOOL_ID, canisterIDLs.three_pool, signerAgent);
  const ammActor = createOisyActor(AMM_ID, canisterIDLs.rumi_amm, signerAgent);

  // Step 1: Approve stablecoin → 3pool
  signerAgent.batch();
  const p1 = stableLedger.icrc2_approve({
    amount: amountIn + fromFee * 2n,
    spender: { owner: Principal.fromText(THREEPOOL_ID), subaccount: [] },
    expires_at: [], expected_allowance: [], memo: [], fee: [],
    from_subaccount: [], created_at_time: [],
  });

  // Step 2: Approve 3USD → AMM (generous: estimate + 1% buffer)
  const threeUsdApprovalAmt = threeUsdEstimate * 101n / 100n;
  signerAgent.batch();
  const p2 = threeUsdLedger.icrc2_approve({
    amount: threeUsdApprovalAmt,
    spender: { owner: Principal.fromText(AMM_ID), subaccount: [] },
    expires_at: [], expected_allowance: [], memo: [], fee: [],
    from_subaccount: [], created_at_time: [],
  });

  // Step 3: 3pool deposit
  signerAgent.batch();
  const p3 = threeUsdLedger.add_liquidity(amounts, threeUsdMinOutput);

  // Step 4: AMM swap (use estimated 3USD amount — slippage protection via minOutput)
  signerAgent.batch();
  const p4 = ammActor.swap(poolId, Principal.fromText(THREEPOOL_ID), threeUsdEstimate, icpMinOutput);

  // This opens the signer popup — must happen close to click handler
  await signerAgent.execute();
  const [r1, r2, r3, r4] = await Promise.all([p1, p2, p3, p4]);

  if (r1 && 'Err' in r1) throw new Error(`Stablecoin approval failed: ${JSON.stringify(r1.Err)}`);
  if (r2 && 'Err' in r2) throw new Error(`3USD approval failed: ${JSON.stringify(r2.Err)}`);
  if ('Err' in r3) throw new Error(`3pool deposit failed: ${JSON.stringify(r3.Err)}`);
  if ('Err' in r4) throw new Error(`AMM swap failed: ${JSON.stringify(r4.Err)}`);
  return r4.Ok.amount_out;
}

/**
 * ICP → Stablecoin (Oisy batched):
 * 1. Approve ICP → AMM
 * 2. AMM.swap ICP → 3USD
 * 3. 3pool.remove_one_coin (burns 3USD LP, no approval needed)
 *
 * All estimates come from route.intermediateOutput / route.estimatedOutput.
 * No canister queries here.
 */
async function executeIcpToStableOisy(
  route: SwapRoute,
  to: AmmToken,
  amountIn: bigint,
  slippageBps: number,
): Promise<bigint> {
  const wallet = get(walletStore);
  if (!wallet.principal) throw new Error('Wallet not connected');

  const hopQuote = route.hopProviderQuote;
  if (!hopQuote) throw new Error('icp_to_stable Oisy route missing hopProviderQuote');

  if (hopQuote.provider === 'icpswap_3usd_icp') {
    return await executeIcpToStableOisyIcpswap(route, to, amountIn, slippageBps, hopQuote);
  }

  // All values pre-computed during resolveRoute — no canister calls
  const poolId = route.poolId!;
  const threeUsdEstimate = route.intermediateOutput!;
  const icpToken = AMM_TOKENS.find(t => t.symbol === 'ICP')!;
  const threeUsdMinOutput = threeUsdEstimate * BigInt(10000 - Math.ceil(slippageBps / 2)) / 10000n;
  const stableMinOutput = route.estimatedOutput * BigInt(10000 - slippageBps) / 10000n;

  // Pre-fetch live ICRC-1 fee before entering the signer batch.
  const icpFee = await tokenFee(icpToken);

  // Signer agent was pre-warmed during quote phase — cached, no popup
  const signerAgent = await getOisySignerAgent(wallet.principal);

  const icpLedger = createOisyActor(ICP_LEDGER_ID, CONFIG.icusd_ledgerIDL, signerAgent);
  const ammActor = createOisyActor(AMM_ID, canisterIDLs.rumi_amm, signerAgent);
  const poolActor = createOisyActor(THREEPOOL_ID, canisterIDLs.three_pool, signerAgent);

  // Step 1: Approve ICP → AMM
  signerAgent.batch();
  const p1 = icpLedger.icrc2_approve({
    amount: amountIn + icpFee * 2n,
    spender: { owner: Principal.fromText(AMM_ID), subaccount: [] },
    expires_at: [], expected_allowance: [], memo: [], fee: [],
    from_subaccount: [], created_at_time: [],
  });

  // Step 2: AMM swap ICP → 3USD
  signerAgent.batch();
  const p2 = ammActor.swap(poolId, Principal.fromText(ICP_LEDGER_ID), amountIn, threeUsdMinOutput);

  // Step 3: 3pool redeem 3USD → stablecoin (no approval: burns caller's LP tokens)
  signerAgent.batch();
  const p3 = poolActor.remove_one_coin(threeUsdEstimate, to.threePoolIndex, stableMinOutput);

  // This opens the signer popup — must happen close to click handler
  await signerAgent.execute();
  const [r1, r2, r3] = await Promise.all([p1, p2, p3]);

  if (r1 && 'Err' in r1) throw new Error(`ICP approval failed: ${JSON.stringify(r1.Err)}`);
  if ('Err' in r2) throw new Error(`AMM swap failed: ${JSON.stringify(r2.Err)}`);
  if ('Err' in r3) throw new Error(`3pool redeem failed: ${JSON.stringify(r3.Err)}`);
  return r3.Ok;
}

/**
 * Stablecoin → ICP via ICPswap (Oisy batched):
 * 1. Approve stablecoin → 3pool (for add_liquidity)
 * 2. 3pool.add_liquidity (burns stablecoin, mints 3USD)
 * 3. Approve 3USD → ICPswap pool (for depositFrom)
 * 4. ICPswap.depositFrom 3USD
 * 5. ICPswap.swap 3USD → ICP
 * 6. ICPswap.withdraw ICP to caller
 *
 * Known limitation: ICPswap withdraw amount must be pre-committed (no async
 * query between swap and withdraw in a batch). We use icpMinOutput as the
 * withdraw amount. Any positive slippage (pool pays more than minimum) stays
 * on the pool's internal subaccount and can be recovered manually later.
 */
async function executeStableToIcpOisyIcpswap(
  route: SwapRoute,
  from: AmmToken,
  amountIn: bigint,
  slippageBps: number,
  hopQuote: ProviderQuote,
): Promise<bigint> {
  const wallet = get(walletStore);
  if (!wallet.principal) throw new Error('Wallet not connected');

  const icpswapPoolId = hopQuote.meta.poolCanisterId as string | undefined;
  const zeroForOne = hopQuote.meta.zeroForOne;
  if (typeof icpswapPoolId !== 'string') {
    throw new Error('ICPswap hopQuote missing meta.poolCanisterId');
  }
  if (typeof zeroForOne !== 'boolean') {
    throw new Error('ICPswap hopQuote has invalid meta.zeroForOne (expected boolean)');
  }

  const threeUsdEstimate = route.intermediateOutput!;
  const threeUsdMinOutput = threeUsdEstimate * BigInt(10000 - Math.ceil(slippageBps / 2)) / 10000n;
  const icpMinOutput = route.estimatedOutput * BigInt(10000 - slippageBps) / 10000n;
  const fromPoolToken = POOL_TOKENS[from.threePoolIndex];

  const amounts: [bigint, bigint, bigint] = [0n, 0n, 0n];
  amounts[from.threePoolIndex] = amountIn;

  // Pre-fetch every live ICRC-1 fee the batched signer flow needs before
  // entering `signerAgent.batch()` (no awaits allowed mid-batch).
  const [fromFee, threeUsdFee, icpFee] = await Promise.all([
    tokenFee(from),
    fetchLedgerFee({ ledgerId: CANISTER_IDS.THREEPOOL, decimals: 8, symbol: '3USD' }),
    fetchLedgerFee({ ledgerId: CANISTER_IDS.ICP_LEDGER, decimals: 8, symbol: 'ICP' }),
  ]);

  const signerAgent = await getOisySignerAgent(wallet.principal);

  const stableLedger = createOisyActor(fromPoolToken.ledgerId, CONFIG.icusd_ledgerIDL, signerAgent);
  const threeUsdLedger = createOisyActor(THREEPOOL_ID, canisterIDLs.three_pool, signerAgent);
  const icpswapPool = createOisyActor(icpswapPoolId, canisterIDLs.icpswap_pool, signerAgent);

  // Step 1: approve stablecoin → 3pool
  signerAgent.batch();
  const p1 = stableLedger.icrc2_approve({
    amount: amountIn + fromFee * 2n,
    spender: { owner: Principal.fromText(THREEPOOL_ID), subaccount: [] },
    expires_at: [], expected_allowance: [], memo: [], fee: [],
    from_subaccount: [], created_at_time: [],
  });

  // Step 2: 3pool deposit (burns stablecoin, mints 3USD)
  signerAgent.batch();
  const p2 = threeUsdLedger.add_liquidity(amounts, threeUsdMinOutput);

  // Step 3: approve 3USD → ICPswap pool (depositFrom is ICRC-2 pull)
  const threeUsdApprovalAmt = threeUsdEstimate * 101n / 100n;
  signerAgent.batch();
  const p3 = threeUsdLedger.icrc2_approve({
    amount: threeUsdApprovalAmt,
    spender: { owner: Principal.fromText(icpswapPoolId), subaccount: [] },
    expires_at: [], expected_allowance: [], memo: [], fee: [],
    from_subaccount: [], created_at_time: [],
  });

  // Step 4: ICPswap depositFrom (pulls 3USD via ICRC-2)
  signerAgent.batch();
  const p4 = icpswapPool.depositFrom({
    token: CANISTER_IDS.THREEPOOL,
    amount: threeUsdEstimate,
    fee: threeUsdFee,
  });

  // Step 5: ICPswap swap (uses pre-committed 3USD amount; slippage via amountOutMinimum)
  signerAgent.batch();
  const p5 = icpswapPool.swap({
    amountIn: threeUsdEstimate.toString(),
    zeroForOne,
    amountOutMinimum: icpMinOutput.toString(),
  });

  // Step 6: ICPswap withdraw to caller's ICP ledger account.
  // Pre-committed amount; we can't await p5 before building p6 in a batch.
  signerAgent.batch();
  const p6 = icpswapPool.withdraw({
    token: CANISTER_IDS.ICP_LEDGER,
    amount: icpMinOutput,
    fee: icpFee,
  });

  await signerAgent.execute();
  const [r1, r2, r3, r4, r5, r6] = await Promise.all([p1, p2, p3, p4, p5, p6]);

  if (r1 && 'Err' in r1) throw new Error(`Stablecoin approval failed: ${JSON.stringify(r1.Err)}`);
  if ('Err' in r2) throw new Error(`3pool deposit failed: ${JSON.stringify(r2.Err)}`);
  if (r3 && 'Err' in r3) throw new Error(`3USD approval failed: ${JSON.stringify(r3.Err)}`);
  if ('err' in r4) throw new Error(`ICPswap depositFrom failed: ${JSON.stringify(r4.err)}`);
  if ('err' in r5) throw new Error(`ICPswap swap failed: ${JSON.stringify(r5.err)}`);
  if ('err' in r6) throw new Error(`ICPswap withdraw failed: ${JSON.stringify(r6.err)}`);
  return (r6 as { ok: bigint }).ok;
}

/**
 * ICP → Stablecoin via ICPswap (Oisy batched):
 * 1. Approve ICP → ICPswap pool (for depositFrom)
 * 2. ICPswap.depositFrom ICP
 * 3. ICPswap.swap ICP → 3USD
 * 4. ICPswap.withdraw 3USD to caller
 * 5. 3pool.remove_one_coin (burns caller's LP, no allowance needed)
 *
 * Same withdraw-amount limitation as the stable→ICP case: we pass
 * threeUsdMinFromSwap as the withdraw amount. Positive slippage stays on
 * the pool's internal subaccount for manual recovery.
 */
async function executeIcpToStableOisyIcpswap(
  route: SwapRoute,
  to: AmmToken,
  amountIn: bigint,
  slippageBps: number,
  hopQuote: ProviderQuote,
): Promise<bigint> {
  const wallet = get(walletStore);
  if (!wallet.principal) throw new Error('Wallet not connected');

  const icpswapPoolId = hopQuote.meta.poolCanisterId as string | undefined;
  const zeroForOne = hopQuote.meta.zeroForOne;
  if (typeof icpswapPoolId !== 'string') {
    throw new Error('ICPswap hopQuote missing meta.poolCanisterId');
  }
  if (typeof zeroForOne !== 'boolean') {
    throw new Error('ICPswap hopQuote has invalid meta.zeroForOne (expected boolean)');
  }

  const icpToken = AMM_TOKENS.find(t => t.symbol === 'ICP')!;
  const threeUsdEstimate = route.intermediateOutput!;
  const threeUsdMinFromSwap = threeUsdEstimate * BigInt(10000 - Math.ceil(slippageBps / 2)) / 10000n;
  const stableMinOutput = route.estimatedOutput * BigInt(10000 - slippageBps) / 10000n;

  // Pre-fetch every live ICRC-1 fee the batched signer flow needs.
  const [icpFee, threeUsdFee] = await Promise.all([
    tokenFee(icpToken),
    fetchLedgerFee({ ledgerId: CANISTER_IDS.THREEPOOL, decimals: 8, symbol: '3USD' }),
  ]);

  const signerAgent = await getOisySignerAgent(wallet.principal);

  const icpLedger = createOisyActor(ICP_LEDGER_ID, CONFIG.icusd_ledgerIDL, signerAgent);
  const icpswapPool = createOisyActor(icpswapPoolId, canisterIDLs.icpswap_pool, signerAgent);
  const poolActor = createOisyActor(THREEPOOL_ID, canisterIDLs.three_pool, signerAgent);

  // Step 1: approve ICP → ICPswap pool
  signerAgent.batch();
  const p1 = icpLedger.icrc2_approve({
    amount: amountIn + icpFee * 2n,
    spender: { owner: Principal.fromText(icpswapPoolId), subaccount: [] },
    expires_at: [], expected_allowance: [], memo: [], fee: [],
    from_subaccount: [], created_at_time: [],
  });

  // Step 2: ICPswap depositFrom (pulls ICP)
  signerAgent.batch();
  const p2 = icpswapPool.depositFrom({
    token: ICP_LEDGER_ID,
    amount: amountIn,
    fee: icpFee,
  });

  // Step 3: ICPswap swap ICP → 3USD
  signerAgent.batch();
  const p3 = icpswapPool.swap({
    amountIn: amountIn.toString(),
    zeroForOne,
    amountOutMinimum: threeUsdMinFromSwap.toString(),
  });

  // Step 4: ICPswap withdraw 3USD to caller
  signerAgent.batch();
  const p4 = icpswapPool.withdraw({
    token: CANISTER_IDS.THREEPOOL,
    amount: threeUsdMinFromSwap,
    fee: threeUsdFee,
  });

  // Step 5: 3pool remove_one_coin (3USD → target stablecoin)
  // remove_one_coin burns the caller's LP directly (no ICRC-2 allowance required).
  // We pass threeUsdMinFromSwap (conservative) to match the withdraw amount.
  signerAgent.batch();
  const p5 = poolActor.remove_one_coin(threeUsdMinFromSwap, to.threePoolIndex, stableMinOutput);

  await signerAgent.execute();
  const [r1, r2, r3, r4, r5] = await Promise.all([p1, p2, p3, p4, p5]);

  if (r1 && 'Err' in r1) throw new Error(`ICP approval failed: ${JSON.stringify(r1.Err)}`);
  if ('err' in r2) throw new Error(`ICPswap depositFrom failed: ${JSON.stringify(r2.err)}`);
  if ('err' in r3) throw new Error(`ICPswap swap failed: ${JSON.stringify(r3.err)}`);
  if ('err' in r4) throw new Error(`ICPswap withdraw failed: ${JSON.stringify(r4.err)}`);
  if ('Err' in r5) throw new Error(`3pool redeem failed: ${JSON.stringify(r5.Err)}`);
  return r5.Ok;
}

/**
 * Direct ICPswap pool swap (Oisy batched). Used by both `icusd_icp_direct`
 * (icUSD <-> ICP) and `amm_swap` (3USD <-> ICP) when ICPswap wins the quote.
 *
 * 1. Approve input token -> ICPswap pool
 * 2. ICPswap.depositFrom (pulls input via ICRC-2)
 * 3. ICPswap.swap
 * 4. ICPswap.withdraw (output to caller)
 *
 * Same withdraw-amount limitation as the other ICPswap Oisy helpers:
 * withdraw amount is pre-committed to minOut; positive slippage stays on
 * the pool's internal subaccount until recovered.
 */
async function executeIcpswapDirectOisy(
  route: SwapRoute,
  from: AmmToken,
  to: AmmToken,
  amountIn: bigint,
  slippageBps: number,
): Promise<bigint> {
  const wallet = get(walletStore);
  if (!wallet.principal) throw new Error('Wallet not connected');

  const q = route.providerQuote;
  if (!q) throw new Error('ICPswap direct Oisy route missing providerQuote');

  const icpswapPoolId = q.meta.poolCanisterId as string | undefined;
  const zeroForOne = q.meta.zeroForOne;
  if (typeof icpswapPoolId !== 'string') {
    throw new Error('ICPswap direct route missing meta.poolCanisterId');
  }
  if (typeof zeroForOne !== 'boolean') {
    throw new Error('ICPswap direct route has invalid meta.zeroForOne (expected boolean)');
  }

  const minOut = route.estimatedOutput * BigInt(10000 - slippageBps) / 10000n;

  // Pre-fetch every live ICRC-1 fee the batched signer flow needs.
  const [fromFee, toFee] = await Promise.all([
    tokenFee(from),
    tokenFee(to),
  ]);

  const signerAgent = await getOisySignerAgent(wallet.principal);

  const fromLedger = createOisyActor(from.ledgerId, CONFIG.icusd_ledgerIDL, signerAgent);
  const icpswapPool = createOisyActor(icpswapPoolId, canisterIDLs.icpswap_pool, signerAgent);

  // Step 1: approve input -> ICPswap pool
  signerAgent.batch();
  const p1 = fromLedger.icrc2_approve({
    amount: amountIn + fromFee * 2n,
    spender: { owner: Principal.fromText(icpswapPoolId), subaccount: [] },
    expires_at: [], expected_allowance: [], memo: [], fee: [],
    from_subaccount: [], created_at_time: [],
  });

  // Step 2: ICPswap depositFrom
  signerAgent.batch();
  const p2 = icpswapPool.depositFrom({
    token: from.ledgerId,
    amount: amountIn,
    fee: fromFee,
  });

  // Step 3: ICPswap swap
  signerAgent.batch();
  const p3 = icpswapPool.swap({
    amountIn: amountIn.toString(),
    zeroForOne,
    amountOutMinimum: minOut.toString(),
  });

  // Step 4: ICPswap withdraw
  signerAgent.batch();
  const p4 = icpswapPool.withdraw({
    token: to.ledgerId,
    amount: minOut,
    fee: toFee,
  });

  await signerAgent.execute();
  const [r1, r2, r3, r4] = await Promise.all([p1, p2, p3, p4]);

  if (r1 && 'Err' in r1) throw new Error(`${from.symbol} approval failed: ${JSON.stringify(r1.Err)}`);
  if ('err' in r2) throw new Error(`ICPswap depositFrom failed: ${JSON.stringify(r2.err)}`);
  if ('err' in r3) throw new Error(`ICPswap swap failed: ${JSON.stringify(r3.err)}`);
  if ('err' in r4) throw new Error(`ICPswap withdraw failed: ${JSON.stringify(r4.err)}`);
  return (r4 as { ok: bigint }).ok;
}
