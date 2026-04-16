import type { AmmToken } from '../ammService';

export interface ProviderQuote {
  /** Provider identifier, e.g. "rumi_amm" or "icpswap_3usd_icp". */
  provider: ProviderId;
  /** Token pair summary for display, e.g. "3USD/ICP via Rumi AMM". */
  label: string;
  /** Estimated output in raw units of the output token. */
  amountOut: bigint;
  /** Fee percentage (for display only), e.g. "0.30%". */
  feeDisplay: string;
  /** Estimated price impact in basis points (0 to 10000). */
  priceImpactBps: number;
  /** Provider-specific hints passed back to `swap()` (e.g. pool ID for Rumi AMM). */
  meta: Record<string, unknown>;
}

export interface ProviderSwapResult {
  /** Actual output amount received. */
  amountOut: bigint;
}

export type ProviderId =
  | 'rumi_amm'
  | 'icpswap_3usd_icp'
  | 'icpswap_icusd_icp';

export interface SwapProvider {
  readonly id: ProviderId;

  /** Whether this provider can quote and swap this pair. */
  supports(tokenIn: AmmToken, tokenOut: AmmToken): boolean;

  /** Query-only quote. Must not mutate state. */
  quote(tokenIn: AmmToken, tokenOut: AmmToken, amountIn: bigint): Promise<ProviderQuote>;

  /**
   * Execute the swap. Assumes the caller already has a quote from `quote()`
   * and passes it back so the provider can reuse cached lookups.
   */
  swap(
    tokenIn: AmmToken,
    tokenOut: AmmToken,
    amountIn: bigint,
    minOut: bigint,
    quote: ProviderQuote,
  ): Promise<ProviderSwapResult>;
}
