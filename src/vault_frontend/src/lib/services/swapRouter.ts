import { Principal } from '@dfinity/principal';
import { threePoolService, POOL_TOKENS } from './threePoolService';
import { ammService, AMM_TOKENS, approvalAmount, getLedgerFee, type AmmToken } from './ammService';
import { CANISTER_IDS, CONFIG } from '../config';
import { isOisyWallet } from './protocol/walletOperations';
import { getOisySignerAgent, createOisyActor } from './oisySigner';
import { canisterIDLs } from './pnp';
import { walletStore } from '../stores/wallet';
import { get } from 'svelte/store';
import { RumiAmmProvider } from './providers/rumiAmmProvider';
import { IcpswapProvider } from './providers/icpswapProvider';
import { ProviderRegistry } from './providers/providerRegistry';
import type { ProviderQuote } from './providers/types';

// ──────────────────────────────────────────────────────────────
// Provider registry for the 3USD <-> ICP hop.
// Quotes Rumi AMM and ICPswap 3USD/ICP in parallel and picks the winner.
// ──────────────────────────────────────────────────────────────

const _threeUsdIcpRegistry = new ProviderRegistry([
  new RumiAmmProvider(),
  new IcpswapProvider({
    id: 'icpswap_3usd_icp',
    poolCanisterId: CANISTER_IDS.ICPSWAP_3USD_ICP_POOL,
    token0LedgerId: CANISTER_IDS.THREEPOOL,
    token1LedgerId: CANISTER_IDS.ICP_LEDGER,
    feeBps: 30,
  }),
]);

// ──────────────────────────────────────────────────────────────
// Route types
// ──────────────────────────────────────────────────────────────

export type RouteType =
  | 'three_pool_swap'       // Stablecoin <-> Stablecoin (direct 3pool)
  | 'three_pool_deposit'    // Stablecoin -> 3USD (mint via 3pool)
  | 'three_pool_redeem'     // 3USD -> Stablecoin (redeem via 3pool)
  | 'amm_swap'              // 3USD <-> ICP (direct AMM)
  | 'stable_to_icp'         // Stablecoin -> ICP (3pool deposit + AMM swap)
  | 'icp_to_stable';        // ICP -> Stablecoin (AMM swap + 3pool redeem)

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
   * canister query. Removed in Task 10 once Oisy dispatches through the
   * provider registry.
   */
  poolId?: string;
  /**
   * Winning provider quote for single-hop `amm_swap` routes. Populated for
   * Case 4 (3USD <-> ICP). Passed back to the provider during execution.
   */
  providerQuote?: ProviderQuote;
  /**
   * Winning provider quote for the 3USD/ICP leg of a two-hop route
   * (Cases 5/6: stable <-> ICP). Task 10 uses this to dispatch via the
   * provider registry; Task 9 only populates it.
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
    const quote = await _threeUsdIcpRegistry.bestQuote(from, to, amountIn);
    return {
      type: 'amm_swap',
      pathDisplay: quote.label,
      hops: 1,
      estimatedOutput: quote.amountOut,
      feeDisplay: quote.feeDisplay,
      providerQuote: quote,
      // Keep poolId populated when Rumi AMM wins so the Oisy helper can
      // reuse it without an extra canister query. Task 10 removes this.
      poolId: quote.provider === 'rumi_amm' ? (quote.meta.poolId as string) : undefined,
    };
  }

  // Case 5: Stablecoin -> ICP (two-hop: 3pool deposit + best 3USD->ICP swap)
  if (isStablecoin(from) && isICP(to)) {
    const amounts = [0n, 0n, 0n];
    amounts[from.threePoolIndex] = amountIn;
    const threeUsdOut = await threePoolService.calcAddLiquidity(amounts);

    const threeUsdToken = AMM_TOKENS.find(t => t.is3USD)!;
    const hopQuote = await _threeUsdIcpRegistry.bestQuote(threeUsdToken, to, threeUsdOut);

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
    const hopQuote = await _threeUsdIcpRegistry.bestQuote(from, threeUsdToken, amountIn);

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
      assertOisyProviderSupported(route);
      const quote = route.providerQuote;
      if (!quote) throw new Error('amm_swap route missing providerQuote');
      const provider = _threeUsdIcpRegistry.get(quote.provider);
      const result = await provider.swap(from, to, amountIn, minOutput, quote);
      return result.amountOut;
    }

    case 'stable_to_icp': {
      assertOisyProviderSupported(route);
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
      const provider = _threeUsdIcpRegistry.get(hopQuote.provider);
      const freshQuote = await provider.quote(threeUsdToken, to, threeUsdReceived);
      const icpMinOutput = freshQuote.amountOut * BigInt(10000 - Math.ceil(slippageBps / 2)) / 10000n;
      const result = await provider.swap(threeUsdToken, to, threeUsdReceived, icpMinOutput, freshQuote);
      return result.amountOut;
    }

    case 'icp_to_stable': {
      assertOisyProviderSupported(route);
      if (isOisyWallet()) {
        return await executeIcpToStableOisy(route, to, amountIn, slippageBps);
      }
      const hopQuote = route.hopProviderQuote;
      if (!hopQuote) throw new Error('icp_to_stable route missing hopProviderQuote');

      // Hop 1: ICP -> 3USD via winning provider
      const threeUsdToken = AMM_TOKENS.find(t => t.is3USD)!;
      const provider = _threeUsdIcpRegistry.get(hopQuote.provider);
      const threeUsdMinOutput = hopQuote.amountOut * BigInt(10000 - Math.ceil(slippageBps / 2)) / 10000n;
      const hop1 = await provider.swap(from, threeUsdToken, amountIn, threeUsdMinOutput, hopQuote);

      // Hop 2: 3USD -> stablecoin (3pool redeem)
      const stableEstimate = await threePoolService.calcRemoveOneCoin(hop1.amountOut, to.threePoolIndex);
      const stableMinOutput = stableEstimate * BigInt(10000 - Math.ceil(slippageBps / 2)) / 10000n;
      return await threePoolService.removeOneCoin(hop1.amountOut, to.threePoolIndex, stableMinOutput);
    }

    default:
      throw new Error(`Unknown route type: ${route.type}`);
  }
}

/**
 * Transitional guard for Task 9: the Oisy batched helpers only know how to
 * execute Rumi AMM swaps. Until Task 10 teaches them to dispatch via the
 * provider registry, reject Oisy executions where ICPswap won the quote.
 */
function assertOisyProviderSupported(route: SwapRoute): void {
  if (!isOisyWallet()) return;
  const winner = route.providerQuote?.provider ?? route.hopProviderQuote?.provider;
  if (winner && winner !== 'rumi_amm') {
    throw new Error(
      `Oisy batched execution for ${winner} is not yet supported (Task 10). ` +
      `Please use Internet Identity for this swap, or switch to a smaller amount that routes through Rumi AMM.`,
    );
  }
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

  // All values pre-computed during resolveRoute — no canister calls
  const poolId = route.poolId!;
  const threeUsdEstimate = route.intermediateOutput!;
  const threeUsdMinOutput = threeUsdEstimate * BigInt(10000 - Math.ceil(slippageBps / 2)) / 10000n;
  const icpMinOutput = route.estimatedOutput * BigInt(10000 - slippageBps) / 10000n;
  const fromPoolToken = POOL_TOKENS[from.threePoolIndex];

  const amounts: [bigint, bigint, bigint] = [0n, 0n, 0n];
  amounts[from.threePoolIndex] = amountIn;

  // Signer agent was pre-warmed during quote phase — cached, no popup
  const signerAgent = await getOisySignerAgent(wallet.principal);

  const stableLedger = createOisyActor(fromPoolToken.ledgerId, CONFIG.icusd_ledgerIDL, signerAgent);
  const threeUsdLedger = createOisyActor(THREEPOOL_ID, canisterIDLs.three_pool, signerAgent);
  const ammActor = createOisyActor(AMM_ID, canisterIDLs.rumi_amm, signerAgent);

  // Step 1: Approve stablecoin → 3pool
  signerAgent.batch();
  const p1 = stableLedger.icrc2_approve({
    amount: amountIn + getLedgerFee(from) * 2n,
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

  // All values pre-computed during resolveRoute — no canister calls
  const poolId = route.poolId!;
  const threeUsdEstimate = route.intermediateOutput!;
  const icpToken = AMM_TOKENS.find(t => t.symbol === 'ICP')!;
  const threeUsdMinOutput = threeUsdEstimate * BigInt(10000 - Math.ceil(slippageBps / 2)) / 10000n;
  const stableMinOutput = route.estimatedOutput * BigInt(10000 - slippageBps) / 10000n;

  // Signer agent was pre-warmed during quote phase — cached, no popup
  const signerAgent = await getOisySignerAgent(wallet.principal);

  const icpLedger = createOisyActor(ICP_LEDGER_ID, CONFIG.icusd_ledgerIDL, signerAgent);
  const ammActor = createOisyActor(AMM_ID, canisterIDLs.rumi_amm, signerAgent);
  const poolActor = createOisyActor(THREEPOOL_ID, canisterIDLs.three_pool, signerAgent);

  // Step 1: Approve ICP → AMM
  signerAgent.batch();
  const p1 = icpLedger.icrc2_approve({
    amount: amountIn + getLedgerFee(icpToken) * 2n,
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
