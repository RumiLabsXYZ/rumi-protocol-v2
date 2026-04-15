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
    tokenIn: AmmToken, tokenOut: AmmToken, amountIn: bigint, minOut: bigint, quote: ProviderQuote,
  ): Promise<ProviderSwapResult> {
    const pool = await this.getActor();
    const zeroForOne = quote.meta.zeroForOne as boolean;

    // Step 1: depositFrom pulls tokens via ICRC-2 (caller must have pre-approved
    // the pool canister). Approval is the caller's responsibility -- wired in
    // by the swapRouter in Task 9.
    // TODO: query icrc1_fee() from the input ledger; 0n is a placeholder.
    const depositResult = await pool.depositFrom({
      token: tokenIn.ledgerId,
      amount: amountIn,
      fee: 0n,
    });
    this.unwrapResult(depositResult);

    // Step 2: swap within the pool
    const swapResult = await pool.swap({
      amountIn: amountIn.toString(),
      zeroForOne,
      amountOutMinimum: minOut.toString(),
    });
    const swapOut = this.unwrapResult(swapResult);

    // Step 3: withdraw output back to caller's ledger account
    // TODO: query icrc1_fee() from the output ledger; 0n is a placeholder.
    const withdrawResult = await pool.withdraw({
      token: tokenOut.ledgerId,
      amount: swapOut,
      fee: 0n,
    });
    const withdrawn = this.unwrapResult(withdrawResult);

    if (withdrawn < minOut) {
      throw new Error(`ICPswap swap failed slippage check: got ${withdrawn}, minimum ${minOut}`);
    }

    return { amountOut: withdrawn };
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
