import { describe, it, expect, vi, beforeEach } from 'vitest';
import type { AmmToken } from './ammService';
import type { ProviderQuote } from './providers/types';

// ──────────────────────────────────────────────────────────────
// Module-level mocks. swapRouter.ts constructs a ProviderRegistry at
// import time with RumiAmmProvider + IcpswapProvider instances, so we
// intercept those classes and hand back controllable mocks. Tests
// drive behaviour by reassigning the mock's `quote` / `swap` per case.
//
// `vi.hoisted` lets us create shared mocks before vi.mock factories run
// (vi.mock is hoisted above all imports / consts).
// ──────────────────────────────────────────────────────────────

const mocks = vi.hoisted(() => ({
  rumiAmmMock: {
    id: 'rumi_amm' as const,
    supports: vi.fn(() => true),
    quote: vi.fn(),
    swap: vi.fn(),
  },
  icpswapMock: {
    id: 'icpswap_3usd_icp' as const,
    supports: vi.fn(() => true),
    quote: vi.fn(),
    swap: vi.fn(),
  },
  threePoolMock: {
    quoteSwap: vi.fn(),
    calcAddLiquidity: vi.fn(),
    calcRemoveOneCoin: vi.fn(),
    addLiquidity: vi.fn(),
    removeOneCoin: vi.fn(),
    swap: vi.fn(),
  },
  isOisyWalletMock: vi.fn(() => false),
}));

const { rumiAmmMock, icpswapMock, threePoolMock, isOisyWalletMock } = mocks;

vi.mock('./providers/rumiAmmProvider', () => ({
  RumiAmmProvider: vi.fn(() => mocks.rumiAmmMock),
}));

vi.mock('./providers/icpswapProvider', () => ({
  IcpswapProvider: vi.fn(() => mocks.icpswapMock),
}));

// Neutralise ammService — the provider mocks bypass it, but getAmmPoolId
// and other helpers still import the module.
vi.mock('./ammService', async () => {
  const actual = await vi.importActual<typeof import('./ammService')>('./ammService');
  return {
    ...actual,
    ammService: {
      getPools: vi.fn(),
      getQuote: vi.fn(),
      swap: vi.fn(),
    },
  };
});

// 3pool service — used by Cases 5 / 6 and execution of two-hop routes.
vi.mock('./threePoolService', async () => {
  const actual = await vi.importActual<typeof import('./threePoolService')>('./threePoolService');
  return {
    ...actual,
    threePoolService: mocks.threePoolMock,
  };
});

// isOisyWallet gets called in executeRoute; default to false for these tests.
vi.mock('./protocol/walletOperations', () => ({
  isOisyWallet: mocks.isOisyWalletMock,
}));

// Keep oisySigner and pnp importable without side effects.
vi.mock('./oisySigner', () => ({
  getOisySignerAgent: vi.fn(),
  createOisyActor: vi.fn(),
}));
vi.mock('./pnp', () => ({ canisterIDLs: {} }));
vi.mock('../stores/wallet', () => ({
  walletStore: { subscribe: () => () => {} },
}));

import { resolveRoute, executeRoute, type SwapRoute } from './swapRouter';

// ──────────────────────────────────────────────────────────────
// Test fixtures
// ──────────────────────────────────────────────────────────────

const icp: AmmToken = {
  symbol: 'ICP',
  ledgerId: 'ryjl3-tyaaa-aaaaa-aaaba-cai',
  decimals: 8,
  color: '#29abe2',
  balanceKey: 'ICP',
  is3USD: false,
  threePoolIndex: -1,
};

const threeUsd: AmmToken = {
  symbol: '3USD',
  ledgerId: 'fohh4-yyaaa-aaaap-qtkpa-cai',
  decimals: 8,
  color: '#34d399',
  balanceKey: 'THREEUSD',
  is3USD: true,
  threePoolIndex: -1,
};

const ckUsdc: AmmToken = {
  symbol: 'ckUSDC',
  ledgerId: 'xevnm-gaaaa-aaaar-qafnq-cai',
  decimals: 6,
  color: '#2775CA',
  balanceKey: 'CKUSDC',
  is3USD: false,
  threePoolIndex: 2,
};

function rumiQuote(amountOut: bigint, overrides: Partial<ProviderQuote> = {}): ProviderQuote {
  return {
    provider: 'rumi_amm',
    label: 'rumi label',
    amountOut,
    feeDisplay: '0.30%',
    priceImpactBps: 0,
    meta: { poolId: 'rumi-pool-1', feeBps: 30 },
    ...overrides,
  };
}

function icpswapQuote(amountOut: bigint, overrides: Partial<ProviderQuote> = {}): ProviderQuote {
  return {
    provider: 'icpswap_3usd_icp',
    label: 'icpswap label',
    amountOut,
    feeDisplay: '0.30%',
    priceImpactBps: 0,
    meta: { poolCanisterId: 'icpswap-pool', zeroForOne: true },
    ...overrides,
  };
}

describe('swapRouter — provider registry integration', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    // restore the default supports() after clearAllMocks
    rumiAmmMock.supports.mockReturnValue(true);
    icpswapMock.supports.mockReturnValue(true);
  });

  // ──────────────────────────────────────────────────────────────
  // Case 4: direct 3USD <-> ICP
  // ──────────────────────────────────────────────────────────────

  describe('Case 4 (3USD <-> ICP)', () => {
    it('populates providerQuote with ICPswap when it returns a higher amountOut', async () => {
      rumiAmmMock.quote.mockResolvedValue(rumiQuote(1_000n));
      icpswapMock.quote.mockResolvedValue(icpswapQuote(1_500n));

      const route = await resolveRoute(threeUsd, icp, 100n);

      expect(route.type).toBe('amm_swap');
      expect(route.providerQuote?.provider).toBe('icpswap_3usd_icp');
      expect(route.estimatedOutput).toBe(1_500n);
      // ICPswap winner => poolId not populated (Rumi-only optimisation)
      expect(route.poolId).toBeUndefined();
      expect(route.pathDisplay).toBe('icpswap label');
    });

    it('populates providerQuote with Rumi AMM when it wins and caches poolId', async () => {
      rumiAmmMock.quote.mockResolvedValue(rumiQuote(2_000n));
      icpswapMock.quote.mockResolvedValue(icpswapQuote(1_500n));

      const route = await resolveRoute(icp, threeUsd, 100n);

      expect(route.providerQuote?.provider).toBe('rumi_amm');
      expect(route.estimatedOutput).toBe(2_000n);
      expect(route.poolId).toBe('rumi-pool-1');
    });
  });

  // ──────────────────────────────────────────────────────────────
  // Case 5: stable -> ICP (two-hop)
  // ──────────────────────────────────────────────────────────────

  describe('Case 5 (stable -> ICP)', () => {
    it('populates hopProviderQuote with the winning 3USD/ICP provider and threads intermediateOutput', async () => {
      threePoolMock.calcAddLiquidity.mockResolvedValue(900n);
      rumiAmmMock.quote.mockResolvedValue(rumiQuote(2_000n));
      icpswapMock.quote.mockResolvedValue(icpswapQuote(3_000n));

      const route = await resolveRoute(ckUsdc, icp, 1_000n);

      expect(route.type).toBe('stable_to_icp');
      expect(route.intermediateOutput).toBe(900n);
      expect(route.hopProviderQuote?.provider).toBe('icpswap_3usd_icp');
      expect(route.estimatedOutput).toBe(3_000n);
      // ICPswap winner => poolId undefined
      expect(route.poolId).toBeUndefined();
      // bestQuote was called with (3USD, icp, threeUsdOut=900n)
      expect(rumiAmmMock.quote).toHaveBeenCalledWith(
        expect.objectContaining({ is3USD: true }),
        expect.objectContaining({ symbol: 'ICP' }),
        900n,
      );
    });
  });

  // ──────────────────────────────────────────────────────────────
  // Case 6: ICP -> stable (two-hop)
  // ──────────────────────────────────────────────────────────────

  describe('Case 6 (ICP -> stable)', () => {
    it('populates hopProviderQuote with the winning ICP/3USD provider and derives final stable estimate', async () => {
      rumiAmmMock.quote.mockResolvedValue(rumiQuote(1_800n));
      icpswapMock.quote.mockResolvedValue(icpswapQuote(2_500n));
      threePoolMock.calcRemoveOneCoin.mockResolvedValue(2_400n);

      const route = await resolveRoute(icp, ckUsdc, 10_000n);

      expect(route.type).toBe('icp_to_stable');
      expect(route.hopProviderQuote?.provider).toBe('icpswap_3usd_icp');
      expect(route.intermediateOutput).toBe(2_500n);
      expect(route.estimatedOutput).toBe(2_400n);
      // calcRemoveOneCoin was called with the winning hop's amountOut
      expect(threePoolMock.calcRemoveOneCoin).toHaveBeenCalledWith(2_500n, ckUsdc.threePoolIndex);
    });

    it('uses Rumi AMM when it wins and caches poolId', async () => {
      rumiAmmMock.quote.mockResolvedValue(rumiQuote(3_000n));
      icpswapMock.quote.mockResolvedValue(icpswapQuote(2_500n));
      threePoolMock.calcRemoveOneCoin.mockResolvedValue(2_900n);

      const route = await resolveRoute(icp, ckUsdc, 10_000n);

      expect(route.hopProviderQuote?.provider).toBe('rumi_amm');
      expect(route.poolId).toBe('rumi-pool-1');
    });
  });

  // ──────────────────────────────────────────────────────────────
  // executeRoute dispatches via provider.swap
  // ──────────────────────────────────────────────────────────────

  describe('executeRoute (amm_swap)', () => {
    it('calls the winning provider.swap with the cached quote', async () => {
      const winningQuote = icpswapQuote(1_500n);
      const route: SwapRoute = {
        type: 'amm_swap',
        pathDisplay: 'x',
        hops: 1,
        estimatedOutput: 1_500n,
        feeDisplay: '0.30%',
        providerQuote: winningQuote,
      };
      icpswapMock.swap.mockResolvedValue({ amountOut: 1_499n });

      const out = await executeRoute(route, threeUsd, icp, 100n, 50);

      expect(icpswapMock.swap).toHaveBeenCalledTimes(1);
      expect(icpswapMock.swap).toHaveBeenCalledWith(
        threeUsd, icp, 100n, expect.any(BigInt), winningQuote,
      );
      expect(rumiAmmMock.swap).not.toHaveBeenCalled();
      expect(out).toBe(1_499n);
    });
  });

  describe('Oisy transitional guard', () => {
    it('rejects Oisy executions when ICPswap wins', async () => {
      isOisyWalletMock.mockReturnValue(true);
      const route: SwapRoute = {
        type: 'amm_swap',
        pathDisplay: 'x',
        hops: 1,
        estimatedOutput: 1_500n,
        feeDisplay: '0.30%',
        providerQuote: icpswapQuote(1_500n),
      };
      await expect(executeRoute(route, threeUsd, icp, 100n, 50))
        .rejects.toThrow(/not yet supported \(Task 10\)/);
    });
  });
});
