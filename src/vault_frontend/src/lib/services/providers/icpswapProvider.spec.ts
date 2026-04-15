import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@dfinity/agent', () => ({
  Actor: { createActor: vi.fn() },
  HttpAgent: {
    create: vi.fn().mockResolvedValue({
      fetchRootKey: vi.fn().mockResolvedValue(undefined),
    }),
  },
}));

// IcpswapProvider now pulls in walletStore + canisterIDLs so .swap() can
// build an authenticated actor (the anonymous agent was caller=anonymous,
// which made depositFrom fail for every II/Plug swap). Mock both so the
// spec doesn't drag in the real wallet/pnp stack.
vi.mock('../pnp', () => ({
  canisterIDLs: { icpswap_pool: {} },
}));
vi.mock('../../stores/wallet', () => ({
  walletStore: {
    subscribe: () => () => {},
    getActor: vi.fn(),
  },
}));

import { Actor } from '@dfinity/agent';
import { IcpswapProvider } from './icpswapProvider';
import type { AmmToken } from '../ammService';
import { walletStore } from '../../stores/wallet';

/** Route both the anonymous quote actor (Actor.createActor) and the
 *  authenticated swap actor (walletStore.getActor) to the same pool
 *  mock, so a test can drive the full quote->swap flow. */
function routeMockPoolToBothActors(mockPool: unknown) {
  vi.mocked(Actor.createActor).mockReturnValue(mockPool as any);
  vi.mocked(walletStore.getActor).mockResolvedValue(mockPool as any);
}

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
    routeMockPoolToBothActors(mockPool);

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

describe('IcpswapProvider.swap', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('executes depositFrom -> swap -> withdraw and returns the withdrawn amount', async () => {
    const mockPool = {
      quote: vi.fn().mockResolvedValue({ ok: 500_000_000n }),
      depositFrom: vi.fn().mockResolvedValue({ ok: 500_000_000n }),
      swap: vi.fn().mockResolvedValue({ ok: 495_000_000n }),
      withdraw: vi.fn().mockResolvedValue({ ok: 495_000_000n }),
    };
    routeMockPoolToBothActors(mockPool);

    const provider = new IcpswapProvider({
      id: 'icpswap_icusd_icp',
      poolCanisterId: 'abc-icusd',
      token0LedgerId: 't6bor-paaaa-aaaap-qrd5q-cai',
      token1LedgerId: 'ryjl3-tyaaa-aaaaa-aaaba-cai',
      feeBps: 30,
    });

    const quote = await provider.quote(icUsd, icp, 500_000_000n);
    const result = await provider.swap(icUsd, icp, 500_000_000n, 490_000_000n, quote);

    expect(result.amountOut).toBe(495_000_000n);
    // Lock in the bigint contract on deposit/withdraw args (different from swap,
    // which takes strings).
    expect(mockPool.depositFrom).toHaveBeenCalledWith({
      token: 't6bor-paaaa-aaaap-qrd5q-cai',
      amount: 500_000_000n,
      fee: 0n,
    });
    expect(mockPool.swap).toHaveBeenCalledWith({
      amountIn: '500000000',
      zeroForOne: true,
      amountOutMinimum: '490000000',
    });
    expect(mockPool.withdraw).toHaveBeenCalledWith({
      token: 'ryjl3-tyaaa-aaaaa-aaaba-cai',
      amount: 495_000_000n,
      fee: 0n,
    });
  });

  it('throws if the withdrawn amount is below minOut', async () => {
    const mockPool = {
      quote: vi.fn().mockResolvedValue({ ok: 500_000_000n }),
      depositFrom: vi.fn().mockResolvedValue({ ok: 500_000_000n }),
      swap: vi.fn().mockResolvedValue({ ok: 100_000_000n }),
      withdraw: vi.fn().mockResolvedValue({ ok: 100_000_000n }),
    };
    routeMockPoolToBothActors(mockPool);

    const provider = new IcpswapProvider({
      id: 'icpswap_icusd_icp',
      poolCanisterId: 'abc-icusd',
      token0LedgerId: 't6bor-paaaa-aaaap-qrd5q-cai',
      token1LedgerId: 'ryjl3-tyaaa-aaaaa-aaaba-cai',
      feeBps: 30,
    });

    const quote = await provider.quote(icUsd, icp, 500_000_000n);
    await expect(
      provider.swap(icUsd, icp, 500_000_000n, 490_000_000n, quote)
    ).rejects.toThrow(/slippage/i);
  });

  it('throws with swap context and recovery hint when pool.swap returns err', async () => {
    // The pool's amountOutMinimum is the primary slippage guard. When it
    // fires, pool.swap returns err (not a small ok). Verify that path raises
    // a clear error and skips withdraw (there's nothing to withdraw).
    const mockPool = {
      quote: vi.fn().mockResolvedValue({ ok: 500_000_000n }),
      depositFrom: vi.fn().mockResolvedValue({ ok: 500_000_000n }),
      swap: vi.fn().mockResolvedValue({ err: { InternalError: 'pool frozen' } }),
      withdraw: vi.fn(),
    };
    routeMockPoolToBothActors(mockPool);

    const provider = new IcpswapProvider({
      id: 'icpswap_icusd_icp',
      poolCanisterId: 'abc-icusd',
      token0LedgerId: 't6bor-paaaa-aaaap-qrd5q-cai',
      token1LedgerId: 'ryjl3-tyaaa-aaaaa-aaaba-cai',
      feeBps: 30,
    });

    const quote = await provider.quote(icUsd, icp, 500_000_000n);
    await expect(
      provider.swap(icUsd, icp, 500_000_000n, 490_000_000n, quote),
    ).rejects.toThrow(/swap failed.*recover/is);
    expect(mockPool.withdraw).not.toHaveBeenCalled();
  });

  it('rejects a quote whose meta.zeroForOne is not a boolean', async () => {
    const mockPool = {
      depositFrom: vi.fn(),
      swap: vi.fn(),
      withdraw: vi.fn(),
    };
    routeMockPoolToBothActors(mockPool);

    const provider = new IcpswapProvider({
      id: 'icpswap_icusd_icp',
      poolCanisterId: 'abc-icusd',
      token0LedgerId: 't6bor-paaaa-aaaap-qrd5q-cai',
      token1LedgerId: 'ryjl3-tyaaa-aaaaa-aaaba-cai',
      feeBps: 30,
    });

    const badQuote = {
      provider: 'icpswap_icusd_icp',
      label: 'icUSD -> ICP via ICPswap',
      amountOut: 100n,
      feeDisplay: '0.30%',
      priceImpactBps: 0,
      meta: { poolCanisterId: 'abc-icusd' }, // zeroForOne missing
    } as any;

    await expect(
      provider.swap(icUsd, icp, 500_000_000n, 0n, badQuote),
    ).rejects.toThrow(/zeroForOne/i);
    expect(mockPool.depositFrom).not.toHaveBeenCalled();
  });
});
