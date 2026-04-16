import { describe, it, expect, vi } from 'vitest';
import type { SwapProvider, ProviderQuote } from './types';
import type { AmmToken } from '../ammService';
import { ProviderRegistry } from './providerRegistry';

const tokenA: AmmToken = { symbol: 'A', ledgerId: 'aaa', decimals: 8, threePoolIndex: -1, is3USD: false } as AmmToken;
const tokenB: AmmToken = { symbol: 'B', ledgerId: 'bbb', decimals: 8, threePoolIndex: -1, is3USD: false } as AmmToken;

function makeProvider(id: string, amountOut: bigint): SwapProvider {
  return {
    id: id as any,
    supports: () => true,
    quote: vi.fn().mockResolvedValue({
      provider: id, label: `${id} label`, amountOut, feeDisplay: '0.30%', priceImpactBps: 0, meta: {},
    } as ProviderQuote),
    swap: vi.fn(),
  };
}

describe('ProviderRegistry', () => {
  it('quoteAll returns quotes from every supporting provider', async () => {
    const reg = new ProviderRegistry([
      makeProvider('rumi_amm', 100n),
      makeProvider('icpswap_3usd_icp', 110n),
    ]);
    const quotes = await reg.quoteAll(tokenA, tokenB, 500n);
    expect(quotes).toHaveLength(2);
    expect(quotes.map(q => q.amountOut).sort()).toEqual([100n, 110n]);
  });

  it('bestQuote picks the provider with the highest amountOut', async () => {
    const reg = new ProviderRegistry([
      makeProvider('rumi_amm', 100n),
      makeProvider('icpswap_3usd_icp', 110n),
    ]);
    const best = await reg.bestQuote(tokenA, tokenB, 500n);
    expect(best.provider).toBe('icpswap_3usd_icp');
    expect(best.amountOut).toBe(110n);
  });

  it('bestQuote skips providers that throw during quote', async () => {
    const working = makeProvider('rumi_amm', 100n);
    const broken = makeProvider('icpswap_3usd_icp', 0n);
    (broken.quote as any).mockRejectedValue(new Error('pool paused'));
    const reg = new ProviderRegistry([working, broken]);
    const best = await reg.bestQuote(tokenA, tokenB, 500n);
    expect(best.provider).toBe('rumi_amm');
  });

  it('throws when no provider supports the pair', async () => {
    const reg = new ProviderRegistry([
      { ...makeProvider('rumi_amm', 100n), supports: () => false } as SwapProvider,
    ]);
    await expect(reg.bestQuote(tokenA, tokenB, 500n)).rejects.toThrow(/no provider/i);
  });

  it('throws when every supporting provider errors', async () => {
    const a = makeProvider('rumi_amm', 0n);
    const b = makeProvider('icpswap_3usd_icp', 0n);
    (a.quote as any).mockRejectedValue(new Error('a down'));
    (b.quote as any).mockRejectedValue(new Error('b down'));
    const reg = new ProviderRegistry([a, b]);
    await expect(reg.bestQuote(tokenA, tokenB, 500n)).rejects.toThrow(/no provider/i);
  });

  it('keeps the earlier provider on a tied amountOut', async () => {
    // Lock in first-wins tie semantics so a future refactor to `>=` cannot
    // silently flip winners.
    const reg = new ProviderRegistry([
      makeProvider('rumi_amm', 100n),
      makeProvider('icpswap_3usd_icp', 100n),
    ]);
    const best = await reg.bestQuote(tokenA, tokenB, 500n);
    expect(best.provider).toBe('rumi_amm');
  });

  it('returns the only quote when a single provider supports the pair', async () => {
    const reg = new ProviderRegistry([makeProvider('rumi_amm', 100n)]);
    const best = await reg.bestQuote(tokenA, tokenB, 500n);
    expect(best.provider).toBe('rumi_amm');
    expect(best.amountOut).toBe(100n);
  });

  it('get returns the provider by id and throws on unknown', () => {
    const p = makeProvider('rumi_amm', 100n);
    const reg = new ProviderRegistry([p]);
    expect(reg.get('rumi_amm')).toBe(p);
    expect(() => reg.get('icpswap_3usd_icp')).toThrow(/unknown provider/i);
  });
});
