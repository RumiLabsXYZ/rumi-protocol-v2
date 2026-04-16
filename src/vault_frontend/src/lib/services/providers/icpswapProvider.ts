import { Actor, HttpAgent } from '@dfinity/agent';
import type { _SERVICE as IcpswapPool } from '$declarations/icpswap_pool/icpswap_pool.did';
import { idlFactory as icpswapPoolIDL } from '$declarations/icpswap_pool/icpswap_pool.did.js';
import { CONFIG, CANISTER_IDS } from '../../config';
import { walletStore } from '../../stores/wallet';
import { canisterIDLs } from '../pnp';
import type { AmmToken } from '../ammService';
import type { SwapProvider, ProviderQuote, ProviderSwapResult, ProviderId } from './types';

/**
 * Returns the exact ICRC-1 transfer fee for a known ledger.
 * ICPswap pool's depositFrom/withdraw expect this value in the `fee` field
 * to match the pool's internal fee cache.
 */
export function icrc1Fee(ledgerId: string): bigint {
  switch (ledgerId) {
    case CANISTER_IDS.ICP_LEDGER:   return 10_000n;     // 0.0001 ICP
    case CANISTER_IDS.ICUSD_LEDGER: return 100_000n;    // 0.001 icUSD
    case CANISTER_IDS.THREEPOOL:    return 0n;           // 3USD has zero fee
    case CANISTER_IDS.CKUSDT_LEDGER: return 10_000n;    // ckUSDT
    case CANISTER_IDS.CKUSDC_LEDGER: return 10_000n;    // ckUSDC
    default:
      throw new Error(`Unknown ledger fee for ${ledgerId}; add it to icrc1Fee()`);
  }
}

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
    const pool = await this.getAnonActor();
    const zeroForOne = tokenIn.ledgerId === this.config.token0LedgerId;
    const result = await pool.quote({
      amountIn: amountIn.toString(),
      zeroForOne,
      amountOutMinimum: '0',
    });
    const amountOut = this.unwrapResult(result, 'quote');
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
    // depositFrom pulls from msg.caller's account, so the actor MUST be built
    // with the user's identity. The anonymous agent used by quote() would
    // resolve to the anonymous principal (2vxsx-fae) and fail with no balance.
    const pool = await this.getAuthActor();

    // Validate the provider-specific hint rather than silently coercing. A
    // missing or non-boolean zeroForOne would route the swap the wrong way.
    const zeroForOne = quote.meta.zeroForOne;
    if (typeof zeroForOne !== 'boolean') {
      throw new Error('ICPswap swap: quote.meta.zeroForOne must be a boolean');
    }

    // Step 1: depositFrom pulls tokens via ICRC-2 (caller must have pre-approved
    // the pool canister). Approval is the caller's responsibility, wired in by
    // the swapRouter in Task 9.
    const depositResult = await pool.depositFrom({
      token: tokenIn.ledgerId,
      amount: amountIn,
      fee: icrc1Fee(tokenIn.ledgerId),
    });
    this.unwrapResult(depositResult, 'depositFrom');

    // After a successful deposit the input token sits on the pool's internal
    // subaccount. If swap or withdraw fails the balance stays there, so we
    // enrich those errors with recovery instructions. Only wrap steps that
    // could leave funds stranded -- the post-withdraw slippage check below
    // runs on funds already back in the user's wallet.
    let withdrawn: bigint;
    try {
      // Step 2: swap within the pool (amountOutMinimum is the primary slippage
      // guard, enforced by the pool itself).
      const swapResult = await pool.swap({
        amountIn: amountIn.toString(),
        zeroForOne,
        amountOutMinimum: minOut.toString(),
      });
      const swapOut = this.unwrapResult(swapResult, 'swap');

      // Step 3: withdraw output back to caller's ledger account.
      const withdrawResult = await pool.withdraw({
        token: tokenOut.ledgerId,
        amount: swapOut,
        fee: icrc1Fee(tokenOut.ledgerId),
      });
      withdrawn = this.unwrapResult(withdrawResult, 'withdraw');
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      throw new Error(
        `${msg} (your ${tokenIn.symbol} deposit is stranded on ICPswap pool ${this.config.poolCanisterId}; recover it by calling withdraw on that canister)`,
        { cause: err },
      );
    }

    // Defense-in-depth slippage check. The pool's amountOutMinimum above is
    // the authoritative guard; this catches the edge case where the swap
    // succeeds but the withdraw returns less than expected (e.g., ledger fee
    // larger than anticipated). At this point the funds are in the user's
    // wallet, so no recovery hint is needed.
    if (withdrawn < minOut) {
      throw new Error(`ICPswap swap failed slippage check: got ${withdrawn}, minimum ${minOut}`);
    }

    return { amountOut: withdrawn };
  }

  /**
   * Anonymous actor for query calls (quote). No identity needed; safe to cache.
   */
  private async getAnonActor(): Promise<IcpswapPool> {
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

  /**
   * Authenticated actor for update calls (depositFrom/swap/withdraw). Built
   * per-call via walletStore so the current wallet's identity is used;
   * not cached because the active wallet can change between swaps.
   */
  private async getAuthActor(): Promise<IcpswapPool> {
    const actor = await walletStore.getActor(
      this.config.poolCanisterId, canisterIDLs.icpswap_pool,
    );
    return actor as unknown as IcpswapPool;
  }

  private unwrapResult(result: { ok: bigint } | { err: unknown }, operation: string): bigint {
    if ('ok' in result) return result.ok;
    throw new Error(`ICPswap ${operation} failed: ${JSON.stringify(result.err)}`);
  }
}
