import { Principal } from '@dfinity/principal';
import { ammService, type AmmToken } from '../ammService';
import { CANISTER_IDS } from '../../config';
import type { SwapProvider, ProviderQuote, ProviderSwapResult } from './types';

const FEE_DISPLAY = '0.30%';
const FEE_BPS = 30;

export class RumiAmmProvider implements SwapProvider {
  readonly id = 'rumi_amm' as const;
  private _cachedPoolId: string | null = null;

  supports(tokenIn: AmmToken, tokenOut: AmmToken): boolean {
    const isPair = (a: AmmToken, b: AmmToken) => a.is3USD && b.symbol === 'ICP';
    return isPair(tokenIn, tokenOut) || isPair(tokenOut, tokenIn);
  }

  async quote(tokenIn: AmmToken, tokenOut: AmmToken, amountIn: bigint): Promise<ProviderQuote> {
    const poolId = await this.getPoolId();
    const tokenInPrincipal = Principal.fromText(tokenIn.ledgerId);
    const amountOut = await ammService.getQuote(poolId, tokenInPrincipal, amountIn);
    return {
      provider: this.id,
      label: `${tokenIn.symbol} -> ${tokenOut.symbol} via Rumi AMM`,
      amountOut,
      feeDisplay: FEE_DISPLAY,
      priceImpactBps: this.estimatePriceImpactBps(amountIn, amountOut, tokenIn, tokenOut),
      meta: { poolId, feeBps: FEE_BPS },
    };
  }

  async swap(
    tokenIn: AmmToken, tokenOut: AmmToken, amountIn: bigint, minOut: bigint, quote: ProviderQuote,
  ): Promise<ProviderSwapResult> {
    const poolId = quote.meta.poolId as string;
    const tokenInPrincipal = Principal.fromText(tokenIn.ledgerId);
    const result = await ammService.swap(poolId, tokenInPrincipal, amountIn, minOut, tokenIn);
    return { amountOut: result.amount_out };
  }

  private async getPoolId(): Promise<string> {
    if (this._cachedPoolId) return this._cachedPoolId;
    const pools = await ammService.getPools();
    const threeUsd = CANISTER_IDS.THREEPOOL;
    const icp = CANISTER_IDS.ICP_LEDGER;
    const pool = pools.find((p: { pool_id: string; token_a: Principal; token_b: Principal }) => {
      const a = p.token_a.toText();
      const b = p.token_b.toText();
      return (a === threeUsd && b === icp) || (a === icp && b === threeUsd);
    });
    if (!pool) throw new Error('3USD/ICP Rumi AMM pool not found');
    this._cachedPoolId = pool.pool_id;
    return pool.pool_id;
  }

  private estimatePriceImpactBps(
    amountIn: bigint, amountOut: bigint, tokenIn: AmmToken, tokenOut: AmmToken,
  ): number {
    // Rough estimate; a precise value would require reserves. Leave at 0 for MVP
    // and improve later if the UI needs finer slippage guidance.
    return 0;
  }
}
