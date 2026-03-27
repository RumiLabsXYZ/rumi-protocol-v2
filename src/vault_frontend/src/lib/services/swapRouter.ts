import { Principal } from '@dfinity/principal';
import { threePoolService } from './threePoolService';
import { ammService, AMM_TOKENS, type AmmToken } from './ammService';
import { CANISTER_IDS } from '../config';

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

  // Case 1: Stablecoin <-> Stablecoin (3pool swap)
  if (isStablecoin(from) && isStablecoin(to)) {
    const output = await threePoolService.calcSwap(from.threePoolIndex, to.threePoolIndex, amountIn);
    return {
      type: 'three_pool_swap',
      pathDisplay: `${from.symbol} → ${to.symbol}`,
      hops: 1,
      estimatedOutput: output,
      feeDisplay: '0.20%',
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

  // Case 4: 3USD <-> ICP (direct AMM swap)
  if ((is3USD(from) && isICP(to)) || (isICP(from) && is3USD(to))) {
    const poolId = await getAmmPoolId();
    const tokenIn = Principal.fromText(from.ledgerId);
    const output = await ammService.getQuote(poolId, tokenIn, amountIn);
    return {
      type: 'amm_swap',
      pathDisplay: `${from.symbol} → ${to.symbol}`,
      hops: 1,
      estimatedOutput: output,
      feeDisplay: '0.30%',
    };
  }

  // Case 5: Stablecoin -> ICP (two-hop: deposit + AMM swap)
  if (isStablecoin(from) && isICP(to)) {
    const amounts = [0n, 0n, 0n];
    amounts[from.threePoolIndex] = amountIn;
    const threeUsdOut = await threePoolService.calcAddLiquidity(amounts);

    const poolId = await getAmmPoolId();
    const threeUsdPrincipal = Principal.fromText(CANISTER_IDS.THREEPOOL);
    const icpOut = await ammService.getQuote(poolId, threeUsdPrincipal, threeUsdOut);

    return {
      type: 'stable_to_icp',
      pathDisplay: `${from.symbol} → 3USD → ICP`,
      hops: 2,
      estimatedOutput: icpOut,
      feeDisplay: '~0.30%',
    };
  }

  // Case 6: ICP -> Stablecoin (two-hop: AMM swap + redeem)
  if (isICP(from) && isStablecoin(to)) {
    const poolId = await getAmmPoolId();
    const icpPrincipal = Principal.fromText(CANISTER_IDS.ICP_LEDGER);
    const threeUsdOut = await ammService.getQuote(poolId, icpPrincipal, amountIn);

    const stableOut = await threePoolService.calcRemoveOneCoin(threeUsdOut, to.threePoolIndex);

    return {
      type: 'icp_to_stable',
      pathDisplay: `ICP → 3USD → ${to.symbol}`,
      hops: 2,
      estimatedOutput: stableOut,
      feeDisplay: '~0.30%',
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
      const poolId = await getAmmPoolId();
      const tokenIn = Principal.fromText(from.ledgerId);
      const result = await ammService.swap(poolId, tokenIn, amountIn, minOutput, from);
      return result.amount_out;
    }

    case 'stable_to_icp': {
      // Hop 1: Deposit stablecoin -> 3USD
      const amounts = [0n, 0n, 0n];
      amounts[from.threePoolIndex] = amountIn;
      // Split slippage budget across two hops
      const threeUsdEstimate = await threePoolService.calcAddLiquidity(amounts);
      const threeUsdMinOutput = threeUsdEstimate * BigInt(10000 - Math.ceil(slippageBps / 2)) / 10000n;
      const threeUsdReceived = await threePoolService.addLiquidity(amounts, threeUsdMinOutput);

      // Hop 2: Swap 3USD -> ICP
      const poolId = await getAmmPoolId();
      const threeUsdPrincipal = Principal.fromText(CANISTER_IDS.THREEPOOL);
      const icpEstimate = await ammService.getQuote(poolId, threeUsdPrincipal, threeUsdReceived);
      const icpMinOutput = icpEstimate * BigInt(10000 - Math.ceil(slippageBps / 2)) / 10000n;
      const threeUsdToken = AMM_TOKENS.find(t => t.is3USD)!;
      const result = await ammService.swap(poolId, threeUsdPrincipal, threeUsdReceived, icpMinOutput, threeUsdToken);
      return result.amount_out;
    }

    case 'icp_to_stable': {
      // Hop 1: Swap ICP -> 3USD
      const poolId = await getAmmPoolId();
      const icpPrincipal = Principal.fromText(CANISTER_IDS.ICP_LEDGER);
      const threeUsdEstimate = await ammService.getQuote(poolId, icpPrincipal, amountIn);
      const threeUsdMinOutput = threeUsdEstimate * BigInt(10000 - Math.ceil(slippageBps / 2)) / 10000n;
      const icpToken = AMM_TOKENS.find(t => t.symbol === 'ICP')!;
      const hop1 = await ammService.swap(poolId, icpPrincipal, amountIn, threeUsdMinOutput, icpToken);

      // Hop 2: Redeem 3USD -> Stablecoin
      const stableEstimate = await threePoolService.calcRemoveOneCoin(hop1.amount_out, to.threePoolIndex);
      const stableMinOutput = stableEstimate * BigInt(10000 - Math.ceil(slippageBps / 2)) / 10000n;
      return await threePoolService.removeOneCoin(hop1.amount_out, to.threePoolIndex, stableMinOutput);
    }

    default:
      throw new Error(`Unknown route type: ${route.type}`);
  }
}
