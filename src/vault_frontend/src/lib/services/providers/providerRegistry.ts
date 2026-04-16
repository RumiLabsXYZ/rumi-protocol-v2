import type { AmmToken } from '../ammService';
import type { SwapProvider, ProviderQuote, ProviderId } from './types';

export class ProviderRegistry {
  constructor(private readonly providers: SwapProvider[]) {}

  /**
   * Quote every provider that supports the pair, in parallel. Providers that
   * throw are silently skipped -- their failures show up as missing entries
   * in the returned array. Callers that need to surface individual failures
   * should use the providers directly.
   */
  async quoteAll(tokenIn: AmmToken, tokenOut: AmmToken, amountIn: bigint): Promise<ProviderQuote[]> {
    const supporting = this.providers.filter(p => p.supports(tokenIn, tokenOut));
    console.log(`[ProviderRegistry] quoteAll ${tokenIn.symbol}->${tokenOut.symbol} amt=${amountIn}, ${supporting.length} providers: [${supporting.map(p => p.id).join(', ')}]`);
    const results = await Promise.allSettled(
      supporting.map(p => p.quote(tokenIn, tokenOut, amountIn)),
    );
    // Log rejected quotes so silent failures become visible in the console.
    results.forEach((r, i) => {
      if (r.status === 'rejected') {
        console.warn(`[ProviderRegistry] ${supporting[i].id} quote FAILED:`, r.reason);
      } else {
        console.log(`[ProviderRegistry] ${supporting[i].id} quote OK: amountOut=${r.value.amountOut}`);
      }
    });
    return results
      .filter((r): r is PromiseFulfilledResult<ProviderQuote> => r.status === 'fulfilled')
      .map(r => r.value);
  }

  /**
   * Returns the quote with the highest amountOut. Throws if no provider
   * supports the pair or every supporting provider errored.
   */
  async bestQuote(tokenIn: AmmToken, tokenOut: AmmToken, amountIn: bigint): Promise<ProviderQuote> {
    const quotes = await this.quoteAll(tokenIn, tokenOut, amountIn);
    if (quotes.length === 0) {
      throw new Error(`No provider supports ${tokenIn.symbol} -> ${tokenOut.symbol}`);
    }
    return quotes.reduce((best, q) => (q.amountOut > best.amountOut ? q : best));
  }

  /** Returns the provider instance by ID. Throws on unknown. */
  get(id: ProviderId): SwapProvider {
    const p = this.providers.find(x => x.id === id);
    if (!p) throw new Error(`Unknown provider: ${id}`);
    return p;
  }
}
