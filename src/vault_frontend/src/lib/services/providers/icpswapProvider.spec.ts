import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@dfinity/agent', () => ({
  Actor: { createActor: vi.fn() },
  HttpAgent: {
    create: vi.fn().mockResolvedValue({
      fetchRootKey: vi.fn().mockResolvedValue(undefined),
    }),
  },
}));

import { Actor } from '@dfinity/agent';
import { IcpswapProvider } from './icpswapProvider';
import type { AmmToken } from '../ammService';

const icUsd: AmmToken = {
  symbol: 'icUSD', ledgerId: 't6bor-paaaa-aaaap-qrd5q-cai',
  decimals: 8, threePoolIndex: 0, is3USD: false,
} as AmmToken;
const icp: AmmToken = {
  symbol: 'ICP', ledgerId: 'ryjl3-tyaaa-aaaaa-aaaba-cai',
  decimals: 8, threePoolIndex: -1, is3USD: false,
} as AmmToken;

describe('IcpswapProvider (quote)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('id is icpswap_icusd_icp when constructed for icUSD/ICP pool', () => {
    const provider = new IcpswapProvider({
      id: 'icpswap_icusd_icp',
      poolCanisterId: 'abc-icusd',
      token0LedgerId: 't6bor-paaaa-aaaap-qrd5q-cai',
      token1LedgerId: 'ryjl3-tyaaa-aaaaa-aaaba-cai',
      feeBps: 30,
    });
    expect(provider.id).toBe('icpswap_icusd_icp');
  });

  it('supports the configured token pair in both directions', () => {
    const provider = new IcpswapProvider({
      id: 'icpswap_icusd_icp',
      poolCanisterId: 'abc-icusd',
      token0LedgerId: 't6bor-paaaa-aaaap-qrd5q-cai',
      token1LedgerId: 'ryjl3-tyaaa-aaaaa-aaaba-cai',
      feeBps: 30,
    });
    expect(provider.supports(icUsd, icp)).toBe(true);
    expect(provider.supports(icp, icUsd)).toBe(true);
  });

  it('calls pool.quote with zeroForOne=true when tokenIn is token0', async () => {
    const mockPool = {
      quote: vi.fn().mockResolvedValue({ ok: 1_000_000_000n }),
    };
    vi.mocked(Actor.createActor).mockReturnValue(mockPool as any);

    const provider = new IcpswapProvider({
      id: 'icpswap_icusd_icp',
      poolCanisterId: 'abc-icusd',
      token0LedgerId: 't6bor-paaaa-aaaap-qrd5q-cai',
      token1LedgerId: 'ryjl3-tyaaa-aaaaa-aaaba-cai',
      feeBps: 30,
    });

    const q = await provider.quote(icUsd, icp, 500_000_000n);

    expect(q.provider).toBe('icpswap_icusd_icp');
    expect(q.amountOut).toBe(1_000_000_000n);
    expect(mockPool.quote).toHaveBeenCalledWith({
      amountIn: '500000000',
      zeroForOne: true,
      amountOutMinimum: '0',
    });
  });
});
