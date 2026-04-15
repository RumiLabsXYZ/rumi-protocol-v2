import { Actor, HttpAgent } from '@dfinity/agent';
import type { _SERVICE as IcpswapPool } from '$declarations/icpswap_pool/icpswap_pool.did';
import { idlFactory as icpswapPoolIDL } from '$declarations/icpswap_pool/icpswap_pool.did.js';
import { CONFIG } from '../../config';
import type { AmmToken } from '../ammService';
import type { SwapProvider, ProviderQuote, ProviderSwapResult, ProviderId } from './types';

export interface IcpswapProviderConfig {
  id: Extract<ProviderId, 'icpswap_3usd_icp' | 'icpswap_icusd_icp'>;
  /** ICPswap pool canister ID. */
  poolCanisterId: string;
  /** Ledger IDs as declared in the pool metadata (token0, token1). */
  token0LedgerId: string;
  token1LedgerId: string;
  /** Fee tier in basis points (3, 30, or 100). */
  feeBps: number;
}

export class IcpswapProvider implements SwapProvider {
  readonly id: ProviderId;
  private readonly config: IcpswapProviderConfig;
  private _actor: IcpswapPool | null = null;

  constructor(config: IcpswapProviderConfig) {
    this.id = config.id;
    this.config = config;
  }

  supports(tokenIn: AmmToken, tokenOut: AmmToken): boolean {
    const { token0LedgerId, token1LedgerId } = this.config;
    const isPair = (a: string, b: string) =>
      (a === token0LedgerId && b === token1LedgerId) ||
      (a === token1LedgerId && b === token0LedgerId);
    return isPair(tokenIn.ledgerId, tokenOut.ledgerId);
  }

  async quote(tokenIn: AmmToken, tokenOut: AmmToken, amountIn: bigint): Promise<ProviderQuote> {
    const pool = await this.getActor();
    const zeroForOne = tokenIn.ledgerId === this.config.token0LedgerId;
    const result = await pool.quote({
      amountIn: amountIn.toString(),
      zeroForOne,
      amountOutMinimum: '0',
    });
    const amountOut = this.unwrapResult(result);
    const feePct = (this.config.feeBps / 100).toFixed(2);
    return {
      provider: this.id,
      label: `${tokenIn.symbol} -> ${tokenOut.symbol} via ICPswap`,
      amountOut,
      feeDisplay: `${feePct}%`,
      priceImpactBps: 0, // refined in future
      meta: {
        poolCanisterId: this.config.poolCanisterId,
        zeroForOne,
      },
    };
  }

  async swap(
    _tokenIn: AmmToken, _tokenOut: AmmToken, _amountIn: bigint, _minOut: bigint, _quote: ProviderQuote,
  ): Promise<ProviderSwapResult> {
    throw new Error('IcpswapProvider.swap not implemented (see Task 7)');
  }

  private async getActor(): Promise<IcpswapPool> {
    if (this._actor) return this._actor;
    const agent = await HttpAgent.create({ host: CONFIG.host });
    if (CONFIG.isLocal) {
      await agent.fetchRootKey();
    }
    this._actor = Actor.createActor<IcpswapPool>(icpswapPoolIDL, {
      agent,
      canisterId: this.config.poolCanisterId,
    });
    return this._actor;
  }

  private unwrapResult(result: { ok: bigint } | { err: unknown }): bigint {
    if ('ok' in result) return result.ok;
    throw new Error(`ICPswap quote failed: ${JSON.stringify(result.err)}`);
  }
}
