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
 * Transitional guard: direct Case 4 (3USD <-> ICP) Oisy executions only know
 * how to route through Rumi AMM. Two-hop cases (stable_to_icp / icp_to_stable)
 * now dispatch ICPswap and Rumi AMM natively via the Oisy helpers below.
 * Rejects Oisy + ICPswap combos for `amm_swap` until that path is added.
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
    throw new Error('ICPswap hopQuote missing meta.zeroForOne');
  }

  const threeUsdEstimate = route.intermediateOutput!;
  const threeUsdMinOutput = threeUsdEstimate * BigInt(10000 - Math.ceil(slippageBps / 2)) / 10000n;
  const icpMinOutput = route.estimatedOutput * BigInt(10000 - slippageBps) / 10000n;
  const fromPoolToken = POOL_TOKENS[from.threePoolIndex];

  const amounts: [bigint, bigint, bigint] = [0n, 0n, 0n];
  amounts[from.threePoolIndex] = amountIn;

  const signerAgent = await getOisySignerAgent(wallet.principal);

  const stableLedger = createOisyActor(fromPoolToken.ledgerId, CONFIG.icusd_ledgerIDL, signerAgent);
  const threeUsdLedger = createOisyActor(THREEPOOL_ID, canisterIDLs.three_pool, signerAgent);
  const icpswapPool = createOisyActor(icpswapPoolId, canisterIDLs.icpswap_pool, signerAgent);

  // Step 1: approve stablecoin → 3pool
  signerAgent.batch();
  const p1 = stableLedger.icrc2_approve({
    amount: amountIn + getLedgerFee(from) * 2n,
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
  // NOTE: fee: 0n matches IcpswapProvider's current TODO; wire icrc1_fee() later.
  signerAgent.batch();
  const p4 = icpswapPool.depositFrom({
    token: CANISTER_IDS.THREEPOOL,
    amount: threeUsdEstimate,
    fee: 0n,
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
    fee: 0n,
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
 * 5. Approve 3USD → 3pool (for remove_one_coin)
 * 6. 3pool.remove_one_coin 3USD → stablecoin
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
    throw new Error('ICPswap hopQuote missing meta.zeroForOne');
  }

  const icpToken = AMM_TOKENS.find(t => t.symbol === 'ICP')!;
  const threeUsdEstimate = route.intermediateOutput!;
  const threeUsdMinFromSwap = threeUsdEstimate * BigInt(10000 - Math.ceil(slippageBps / 2)) / 10000n;
  const stableMinOutput = route.estimatedOutput * BigInt(10000 - slippageBps) / 10000n;

  const signerAgent = await getOisySignerAgent(wallet.principal);

  const icpLedger = createOisyActor(ICP_LEDGER_ID, CONFIG.icusd_ledgerIDL, signerAgent);
  const icpswapPool = createOisyActor(icpswapPoolId, canisterIDLs.icpswap_pool, signerAgent);
  const threeUsdLedger = createOisyActor(THREEPOOL_ID, canisterIDLs.three_pool, signerAgent);
  const poolActor = createOisyActor(THREEPOOL_ID, canisterIDLs.three_pool, signerAgent);

  // Step 1: approve ICP → ICPswap pool
  signerAgent.batch();
  const p1 = icpLedger.icrc2_approve({
    amount: amountIn + getLedgerFee(icpToken) * 2n,
    spender: { owner: Principal.fromText(icpswapPoolId), subaccount: [] },
    expires_at: [], expected_allowance: [], memo: [], fee: [],
    from_subaccount: [], created_at_time: [],
  });

  // Step 2: ICPswap depositFrom (pulls ICP)
  signerAgent.batch();
  const p2 = icpswapPool.depositFrom({
    token: ICP_LEDGER_ID,
    amount: amountIn,
    fee: 0n,
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
    fee: 0n,
  });

  // Step 5: approve 3USD → 3pool for remove_one_coin
  signerAgent.batch();
  const p5 = threeUsdLedger.icrc2_approve({
    amount: threeUsdMinFromSwap * 101n / 100n,
    spender: { owner: Principal.fromText(THREEPOOL_ID), subaccount: [] },
    expires_at: [], expected_allowance: [], memo: [], fee: [],
    from_subaccount: [], created_at_time: [],
  });

  // Step 6: 3pool remove_one_coin (3USD → target stablecoin)
  // We pass threeUsdMinFromSwap (conservative) to match the withdraw amount.
  signerAgent.batch();
  const p6 = poolActor.remove_one_coin(threeUsdMinFromSwap, to.threePoolIndex, stableMinOutput);

  await signerAgent.execute();
  const [r1, r2, r3, r4, r5, r6] = await Promise.all([p1, p2, p3, p4, p5, p6]);

  if (r1 && 'Err' in r1) throw new Error(`ICP approval failed: ${JSON.stringify(r1.Err)}`);
  if ('err' in r2) throw new Error(`ICPswap depositFrom failed: ${JSON.stringify(r2.err)}`);
  if ('err' in r3) throw new Error(`ICPswap swap failed: ${JSON.stringify(r3.err)}`);
  if ('err' in r4) throw new Error(`ICPswap withdraw failed: ${JSON.stringify(r4.err)}`);
  if (r5 && 'Err' in r5) throw new Error(`3USD approval failed: ${JSON.stringify(r5.Err)}`);
  if ('Err' in r6) throw new Error(`3pool redeem failed: ${JSON.stringify(r6.Err)}`);
  return r6.Ok;
}
