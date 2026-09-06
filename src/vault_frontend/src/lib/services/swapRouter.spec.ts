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
  icpswapIcUsdMock: {
    id: 'icpswap_icusd_icp' as const,
    supports: vi.fn(() => true),
    quote: vi.fn(),
    swap: vi.fn(),
  },
  // Case 1 stablecoin <-> stablecoin ICPswap direct pools. Default supports()
  // to false so ProviderRegistry.bestQuote doesn't call an un-configured
  // mock's quote() (which would return undefined, not a Promise, and blow up
  // Promise.allSettled). Tests flip supports() to true for the pool under
  // test and set its quote() return value explicitly.
  stableCkusdtIcusdMock: {
    id: 'icpswap_ckusdt_icusd' as const,
    supports: vi.fn(() => false),
    quote: vi.fn(),
    swap: vi.fn(),
  },
  stableIcusdCkusdcMock: {
    id: 'icpswap_icusd_ckusdc' as const,
    supports: vi.fn(() => false),
    quote: vi.fn(),
    swap: vi.fn(),
  },
  stableCkusdtCkusdcMock: {
    id: 'icpswap_ckusdt_ckusdc' as const,
    supports: vi.fn(() => false),
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

const {
  rumiAmmMock, icpswapMock, icpswapIcUsdMock,
  stableCkusdtIcusdMock, stableIcusdCkusdcMock, stableCkusdtCkusdcMock,
  threePoolMock, isOisyWalletMock,
} = mocks;

vi.mock('./providers/rumiAmmProvider', () => ({
  RumiAmmProvider: vi.fn(() => mocks.rumiAmmMock),
}));

vi.mock('./providers/icpswapProvider', () => ({
  IcpswapProvider: vi.fn((config: { id: string }) => {
    switch (config.id) {
      case 'icpswap_icusd_icp': return mocks.icpswapIcUsdMock;
      case 'icpswap_ckusdt_icusd': return mocks.stableCkusdtIcusdMock;
      case 'icpswap_icusd_ckusdc': return mocks.stableIcusdCkusdcMock;
      case 'icpswap_ckusdt_ckusdc': return mocks.stableCkusdtCkusdcMock;
      default: return mocks.icpswapMock;
    }
  }),
}));

// Audit ICRC-005 (frontend half): the Oisy batched executor now pulls fees
// from the live ledger via ledgerFeeService instead of hardcoded constants.
// Mock it so tests don't hit a real agent.
vi.mock('./ledgerFeeService', () => ({
  fetchLedgerFee: vi.fn().mockResolvedValue(10n),
  getCachedLedgerFee: vi.fn(() => 10n),
  _clearLedgerFeeCache: vi.fn(),
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
    // approvalAmount and tokenFee are async since the live-fee migration —
    // override with deterministic stubs so swapRouter's pre-batch awaits
    // don't reach the network.
    tokenFee: vi.fn().mockResolvedValue(10n),
    approvalAmount: vi.fn(async (amount: bigint) => amount + 10n),
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

// Keep oisySigner and pnp importable without side effects. The Oisy ICPswap
// direct dispatch test below overrides these with concrete fakes via
// vi.mocked(...).mockResolvedValueOnce(...).
vi.mock('./oisySigner', () => ({
  getOisySignerAgent: vi.fn(),
  createOisyActor: vi.fn(),
}));
vi.mock('./pnp', () => ({ canisterIDLs: {} }));
vi.mock('../stores/wallet', () => ({
  walletStore: {
    // svelte/store's `get()` calls subscribe(set) and reads back synchronously,
    // so we must invoke `set` with a value. Oisy branches need a truthy
    // principal to clear the "Wallet not connected" guard.
    subscribe: (set: (v: any) => void) => {
      set({ principal: { toText: () => 'aaaaa-aa' } });
      return () => {};
    },
    // Non-Oisy ICPswap branches call walletStore.getActor to build an
    // authenticated actor and (separately) to run icrc2_approve. Return
    // a stub that satisfies both call sites; tests that care about the
    // approval payload should override this per-case.
    getActor: vi.fn().mockResolvedValue({
      icrc2_approve: vi.fn().mockResolvedValue({ Ok: 1n }),
      // Authenticated pool actor stub; tests drive provider.swap via the
      // ICPswap mock in `rumiAmmProvider`/`icpswapProvider` module mocks
      // above, so this only needs to be non-throwing.
      depositFrom: vi.fn().mockResolvedValue({ ok: 0n }),
      swap: vi.fn().mockResolvedValue({ ok: 0n }),
      withdraw: vi.fn().mockResolvedValue({ ok: 0n }),
    }),
  },
}));

import { resolveRoute, executeRoute, setIcpswapRoutingEnabled, dustThreshold, type SwapRoute } from './swapRouter';

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

const ckUsdt: AmmToken = {
  symbol: 'ckUSDT',
  ledgerId: 'cngnf-vqaaa-aaaar-qag4q-cai',
  decimals: 6,
  color: '#26A17B',
  balanceKey: 'CKUSDT',
  is3USD: false,
  threePoolIndex: 1,
};

const icUsd: AmmToken = {
  symbol: 'icUSD',
  ledgerId: 't6bor-paaaa-aaaap-qrd5q-cai',
  decimals: 8,
  color: '#818cf8',
  balanceKey: 'ICUSD',
  is3USD: false,
  threePoolIndex: 0,
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
    // Real ICPswap 3USD/ICP pool canister ID — needs to be a valid Principal
    // string because executeRoute now calls Principal.fromText on it for the
    // pre-swap ICRC-2 approval (added in the B2 blocker fix).
    meta: { poolCanisterId: 'mu2zw-6iaaa-aaaar-qb56q-cai', zeroForOne: true },
    ...overrides,
  };
}

function icpswapIcUsdQuote(amountOut: bigint, overrides: Partial<ProviderQuote> = {}): ProviderQuote {
  return {
    provider: 'icpswap_icusd_icp',
    label: 'icUSD/ICP via ICPswap',
    amountOut,
    feeDisplay: '0.30%',
    priceImpactBps: 0,
    meta: { poolCanisterId: 'nqxwe-hiaaa-aaaar-qb5yq-cai', zeroForOne: true },
    ...overrides,
  };
}

const STABLE_ICPSWAP_POOL_IDS = {
  icpswap_ckusdt_icusd: 'jogrm-gqaaa-aaaar-qcg2a-cai',
  icpswap_icusd_ckusdc: 'eb25l-dyaaa-aaaar-qb4lq-cai',
  icpswap_ckusdt_ckusdc: 'heq6n-fyaaa-aaaag-qkcpq-cai',
} as const;

/** Quote fixture for one of the three Case-1 stablecoin <-> stablecoin
 *  ICPswap direct pools. Real pool canister IDs so executeRoute's
 *  Principal.fromText(meta.poolCanisterId) doesn't choke. */
function stableIcpswapQuote(
  provider: keyof typeof STABLE_ICPSWAP_POOL_IDS,
  amountOut: bigint,
  overrides: Partial<ProviderQuote> = {},
): ProviderQuote {
  return {
    provider,
    label: `${provider} label`,
    amountOut,
    feeDisplay: '0.30%',
    priceImpactBps: 0,
    meta: { poolCanisterId: STABLE_ICPSWAP_POOL_IDS[provider], zeroForOne: true },
    ...overrides,
  };
}

describe('swapRouter — provider registry integration', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    // restore the default supports() after clearAllMocks
    rumiAmmMock.supports.mockReturnValue(true);
    icpswapMock.supports.mockReturnValue(true);
    icpswapIcUsdMock.supports.mockReturnValue(true);
    // Case 1 stable<->stable pools default to unsupported; each test opts
    // the specific pool it's exercising back in (see the mock declaration
    // above for why).
    stableCkusdtIcusdMock.supports.mockReturnValue(false);
    stableIcusdCkusdcMock.supports.mockReturnValue(false);
    stableCkusdtCkusdcMock.supports.mockReturnValue(false);
    // Most tests exercise routes where ICPswap is a valid option; the
    // kill-switch-off behaviour is covered in its own describe block.
    setIcpswapRoutingEnabled(true);
    // clearAllMocks() clears call history but not a mock's implementation,
    // so a test that flips this to true would otherwise leak it into every
    // test that runs afterward. Re-assert the non-Oisy default explicitly,
    // same as the supports() resets above.
    isOisyWalletMock.mockReturnValue(false);
  });

  // ──────────────────────────────────────────────────────────────
  // Case 1: stablecoin <-> stablecoin. The 3pool quote is always fetched;
  // ICPswap's direct stable pools (ckUSDT/icUSD, icUSD/ckUSDC, ckUSDT/ckUSDC)
  // compete for the same pair and must strictly beat the 3pool to win.
  // Exercised on the ckUSDT <-> ckUSDC pair (icpswap_ckusdt_ckusdc pool).
  // ──────────────────────────────────────────────────────────────

  describe('Case 1 (stablecoin <-> stablecoin, 3pool vs ICPswap)', () => {
    it('routes to icpswap_stable_direct when ICPswap strictly beats the 3pool', async () => {
      threePoolMock.quoteSwap.mockResolvedValue({ amount_out: 900n, fee_bps: 30, is_rebalancing: false });
      stableCkusdtCkusdcMock.supports.mockReturnValue(true);
      stableCkusdtCkusdcMock.quote.mockResolvedValue(stableIcpswapQuote('icpswap_ckusdt_ckusdc', 950n));

      const route = await resolveRoute(ckUsdt, ckUsdc, 1_000n);

      expect(route.type).toBe('icpswap_stable_direct');
      expect(route.providerQuote?.provider).toBe('icpswap_ckusdt_ckusdc');
      expect(route.grossOutput).toBe(950n);
      // FE-003: NET of the 10n mocked output ledger fee
      expect(route.estimatedOutput).toBe(940n);
    });

    it('routes to three_pool_swap when the 3pool quote beats ICPswap', async () => {
      threePoolMock.quoteSwap.mockResolvedValue({ amount_out: 950n, fee_bps: 30, is_rebalancing: false });
      stableCkusdtCkusdcMock.supports.mockReturnValue(true);
      stableCkusdtCkusdcMock.quote.mockResolvedValue(stableIcpswapQuote('icpswap_ckusdt_ckusdc', 900n));

      const route = await resolveRoute(ckUsdt, ckUsdc, 1_000n);

      expect(route.type).toBe('three_pool_swap');
      expect(route.grossOutput).toBe(950n);
      expect(route.providerQuote).toBeUndefined();
    });

    it('routes to three_pool_swap on an exact tie (3pool wins ties)', async () => {
      threePoolMock.quoteSwap.mockResolvedValue({ amount_out: 900n, fee_bps: 30, is_rebalancing: false });
      stableCkusdtCkusdcMock.supports.mockReturnValue(true);
      stableCkusdtCkusdcMock.quote.mockResolvedValue(stableIcpswapQuote('icpswap_ckusdt_ckusdc', 900n));

      const route = await resolveRoute(ckUsdt, ckUsdc, 1_000n);

      expect(route.type).toBe('three_pool_swap');
      expect(route.grossOutput).toBe(900n);
    });

    it('never quotes ICPswap and always returns three_pool_swap when the kill switch is off', async () => {
      setIcpswapRoutingEnabled(false);
      threePoolMock.quoteSwap.mockResolvedValue({ amount_out: 900n, fee_bps: 30, is_rebalancing: false });
      stableCkusdtCkusdcMock.supports.mockReturnValue(true);
      // Even though this would beat the 3pool, it must never be consulted.
      stableCkusdtCkusdcMock.quote.mockResolvedValue(stableIcpswapQuote('icpswap_ckusdt_ckusdc', 999_999n));

      const route = await resolveRoute(ckUsdt, ckUsdc, 1_000n);

      expect(route.type).toBe('three_pool_swap');
      expect(route.grossOutput).toBe(900n);
      expect(stableCkusdtCkusdcMock.quote).not.toHaveBeenCalled();
    });

    it('falls back to three_pool_swap when the ICPswap registry throws', async () => {
      threePoolMock.quoteSwap.mockResolvedValue({ amount_out: 900n, fee_bps: 30, is_rebalancing: false });
      stableCkusdtCkusdcMock.supports.mockReturnValue(true);
      stableCkusdtCkusdcMock.quote.mockRejectedValue(new Error('pool offline'));

      const route = await resolveRoute(ckUsdt, ckUsdc, 1_000n);

      expect(route.type).toBe('three_pool_swap');
      expect(route.grossOutput).toBe(900n);
    });

    // The 3pool genuinely errors here (calc_swap_output can throw
    // InsufficientLiquidity, and the pool can be paused). The 3pool quote
    // and the ICPswap quote must be independent: a 3pool failure must not
    // take the ICPswap fallback down with it.
    it('falls back to icpswap_stable_direct when the 3pool quote rejects but ICPswap succeeds', async () => {
      threePoolMock.quoteSwap.mockRejectedValue(new Error('InsufficientLiquidity'));
      stableCkusdtCkusdcMock.supports.mockReturnValue(true);
      stableCkusdtCkusdcMock.quote.mockResolvedValue(stableIcpswapQuote('icpswap_ckusdt_ckusdc', 950n));

      const route = await resolveRoute(ckUsdt, ckUsdc, 1_000n);

      expect(route.type).toBe('icpswap_stable_direct');
      expect(route.providerQuote?.provider).toBe('icpswap_ckusdt_ckusdc');
      expect(route.grossOutput).toBe(950n);
    });

    it('throws when both the 3pool and ICPswap quotes fail', async () => {
      threePoolMock.quoteSwap.mockRejectedValue(new Error('InsufficientLiquidity'));
      stableCkusdtCkusdcMock.supports.mockReturnValue(true);
      stableCkusdtCkusdcMock.quote.mockRejectedValue(new Error('pool offline'));

      // The thrown error is the 3pool's (the primary, first-party venue),
      // not the ICPswap one.
      await expect(resolveRoute(ckUsdt, ckUsdc, 1_000n)).rejects.toThrow(/InsufficientLiquidity/);
    });
  });

  describe('executeRoute (icpswap_stable_direct)', () => {
    it('approves the pool then calls the winning provider.swap with a GROSS min-out bound', async () => {
      const winningQuote = stableIcpswapQuote('icpswap_ckusdt_ckusdc', 950n);
      const route: SwapRoute = {
        type: 'icpswap_stable_direct',
        pathDisplay: 'x',
        hops: 1,
        estimatedOutput: 940n,
        grossOutput: 950n,
        feeDisplay: '0.30%',
        providerQuote: winningQuote,
      };
      stableCkusdtCkusdcMock.swap.mockResolvedValue({ amountOut: 948n });

      const out = await executeRoute(route, ckUsdt, ckUsdc, 1_000n, 50);

      expect(stableCkusdtCkusdcMock.swap).toHaveBeenCalledWith(
        ckUsdt, ckUsdc, 1_000n, 950n * 9_950n / 10_000n, winningQuote,
      );
      expect(out).toBe(948n);
    });

    it('throws the refresh-the-quote error when the kill switch flips off between quote and execute', async () => {
      const route: SwapRoute = {
        type: 'icpswap_stable_direct',
        pathDisplay: 'x',
        hops: 1,
        estimatedOutput: 940n,
        grossOutput: 950n,
        feeDisplay: '0.30%',
        providerQuote: stableIcpswapQuote('icpswap_ckusdt_ckusdc', 950n),
      };
      setIcpswapRoutingEnabled(false);

      await expect(executeRoute(route, ckUsdt, ckUsdc, 1_000n, 50))
        .rejects.toThrow(/ICPswap routing is currently disabled/i);
      expect(stableCkusdtCkusdcMock.swap).not.toHaveBeenCalled();
    });

    it('dispatches through the sequential Oisy executor instead of provider.swap when Oisy is the wallet', async () => {
      isOisyWalletMock.mockReturnValue(true);

      // Same v5 sequential-Oisy pattern as the amm_swap dispatch test below:
      // approve the from-ledger, then depositFrom -> swap -> withdraw on the
      // pool actor directly (no batch/execute concept).
      const fakeSignerAgent = {};
      const fakeFromLedger = {
        icrc2_approve: vi.fn().mockResolvedValue({ Ok: 1n }),
      };
      const fakePool = {
        depositFrom: vi.fn().mockResolvedValue({ ok: 0n }),
        swap: vi.fn().mockResolvedValue({ ok: 0n }),
        withdraw: vi.fn().mockResolvedValue({ ok: 948n }),
      };
      const oisySigner = await import('./oisySigner');
      vi.mocked(oisySigner.getOisySignerAgent).mockResolvedValue(fakeSignerAgent as any);
      vi.mocked(oisySigner.createOisyActor).mockImplementation(((canisterId: string) => {
        // Pool ID from stableIpcswapQuote('icpswap_ckusdt_ckusdc', ...) fixture
        if (canisterId === STABLE_ICPSWAP_POOL_IDS.icpswap_ckusdt_ckusdc) return fakePool;
        return fakeFromLedger;
      }) as any);

      const route: SwapRoute = {
        type: 'icpswap_stable_direct',
        pathDisplay: 'x',
        hops: 1,
        estimatedOutput: 940n,
        grossOutput: 950n,
        feeDisplay: '0.30%',
        providerQuote: stableIcpswapQuote('icpswap_ckusdt_ckusdc', 950n),
      };

      const out = await executeRoute(route, ckUsdt, ckUsdc, 1_000n, 50);

      // Provider.swap path was bypassed
      expect(stableCkusdtCkusdcMock.swap).not.toHaveBeenCalled();
      expect(fakeFromLedger.icrc2_approve).toHaveBeenCalledTimes(1);
      expect(fakePool.depositFrom).toHaveBeenCalledTimes(1);
      expect(fakePool.swap).toHaveBeenCalledTimes(1);
      expect(fakePool.withdraw).toHaveBeenCalledTimes(1);
      expect(out).toBe(948n);
    });
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
      // FE-003: estimatedOutput is NET of the 10n output ledger fee (mocked above)
      expect(route.estimatedOutput).toBe(1_490n);
      expect(route.grossOutput).toBe(1_500n);
      // ICPswap winner => poolId not populated (Rumi-only optimisation)
      expect(route.poolId).toBeUndefined();
      expect(route.pathDisplay).toBe('icpswap label');
    });

    it('does not quote Rumi AMM while AMM1 routing is paused', async () => {
      rumiAmmMock.quote.mockResolvedValue(rumiQuote(2_000n));
      icpswapMock.quote.mockResolvedValue(icpswapQuote(1_500n));

      const route = await resolveRoute(icp, threeUsd, 100n);

      expect(route.providerQuote?.provider).toBe('icpswap_3usd_icp');
      expect(route.estimatedOutput).toBe(1_490n);
      expect(route.grossOutput).toBe(1_500n);
      expect(rumiAmmMock.quote).not.toHaveBeenCalled();
    });
  });

  // ──────────────────────────────────────────────────────────────
  // Case 5: stable -> ICP through icUSD (3pool + ICPswap, AMM1 paused)
  // ──────────────────────────────────────────────────────────────

  describe('Case 5 (stable -> ICP via icUSD)', () => {
    it('routes stablecoin -> icUSD in 3pool, then icUSD -> ICP on ICPswap', async () => {
      threePoolMock.quoteSwap.mockResolvedValue({ amount_out: 900n, fee_bps: 4, is_rebalancing: false });
      icpswapIcUsdMock.quote.mockResolvedValue(icpswapIcUsdQuote(3_000n));

      const route = await resolveRoute(ckUsdc, icp, 1_000n);

      expect(route.type).toBe('stable_to_icp_via_icusd');
      // 3pool's 900n gross output arrives as 890n icUSD after ledger fee.
      expect(route.intermediateOutput).toBe(890n);
      expect(route.hopProviderQuote?.provider).toBe('icpswap_icusd_icp');
      // FE-003: NET of the 10n output ledger fee
      expect(route.estimatedOutput).toBe(2_990n);
      expect(route.grossOutput).toBe(3_000n);
      expect(route.pathDisplay).toBe('ckUSDC → icUSD → ICP');
      expect(icpswapIcUsdMock.quote).toHaveBeenCalledWith(
        expect.objectContaining({ symbol: 'icUSD' }),
        expect.objectContaining({ symbol: 'ICP' }),
        890n,
      );
      expect(rumiAmmMock.quote).not.toHaveBeenCalled();
    });
  });

  // ──────────────────────────────────────────────────────────────
  // Case 6: ICP -> stable through icUSD (ICPswap + 3pool, AMM1 paused)
  // ──────────────────────────────────────────────────────────────

  describe('Case 6 (ICP -> stable via icUSD)', () => {
    it('routes ICP -> icUSD on ICPswap, then icUSD -> ckUSDT in the 3pool', async () => {
      icpswapIcUsdMock.quote.mockResolvedValue(icpswapIcUsdQuote(2_500n));
      threePoolMock.quoteSwap.mockResolvedValue({ amount_out: 2_400n, fee_bps: 4, is_rebalancing: false });

      const route = await resolveRoute(icp, ckUsdt, 10_000n);

      expect(route.type).toBe('icp_to_stable_via_icusd');
      expect(route.hopProviderQuote?.provider).toBe('icpswap_icusd_icp');
      // The 2,500n ICPswap gross output becomes 2,490n usable icUSD.
      expect(route.intermediateOutput).toBe(2_490n);
      // FE-003: NET of the 10n output ledger fee
      expect(route.estimatedOutput).toBe(2_390n);
      expect(route.grossOutput).toBe(2_400n);
      expect(route.pathDisplay).toBe('ICP → icUSD → ckUSDT');
      expect(threePoolMock.quoteSwap).toHaveBeenCalledWith(0, ckUsdt.threePoolIndex, 2_490n);
      expect(rumiAmmMock.quote).not.toHaveBeenCalled();
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
        estimatedOutput: 1_490n,
        grossOutput: 1_500n,
        feeDisplay: '0.30%',
        providerQuote: winningQuote,
      };
      icpswapMock.swap.mockResolvedValue({ amountOut: 1_499n });

      const out = await executeRoute(route, threeUsd, icp, 100n, 50);

      expect(icpswapMock.swap).toHaveBeenCalledTimes(1);
      // FE-003: ICPswap's amountOutMinimum is a GROSS in-pool bound,
      // derived from the gross quote (1_500n), not the net estimate.
      expect(icpswapMock.swap).toHaveBeenCalledWith(
        threeUsd, icp, 100n, 1_500n * 9_950n / 10_000n, winningQuote,
      );
      expect(rumiAmmMock.swap).not.toHaveBeenCalled();
      expect(out).toBe(1_499n);
    });

  });

  // ──────────────────────────────────────────────────────────────
  // FE-003: net-of-ledger-fee estimates and min bounds.
  // The 3pool / Rumi AMM pay `amount - ledger_fee` and enforce min_dy /
  // min_amount_out against that NET amount (PR #230). The mocked ledger
  // fee is 10n (see the ledgerFeeService mock above).
  // ──────────────────────────────────────────────────────────────

  describe('FE-003: net output semantics', () => {
    it('FE-003: three_pool_swap estimate is net and min_dy derives from it', async () => {
      threePoolMock.quoteSwap.mockResolvedValue({
        amount_out: 1_000n, fee_bps: 30, is_rebalancing: false,
      });
      const icUsd: AmmToken = { ...ckUsdc, symbol: 'icUSD', threePoolIndex: 0 };

      const route = await resolveRoute(icUsd, ckUsdc, 2_000n);
      expect(route.type).toBe('three_pool_swap');
      expect(route.grossOutput).toBe(1_000n);
      expect(route.estimatedOutput).toBe(990n);

      threePoolMock.swap.mockResolvedValue(989n);
      await executeRoute(route, icUsd, ckUsdc, 2_000n, 50);
      // min_dy = 990 * 9_950 / 10_000 = 985n (net bound)
      expect(threePoolMock.swap).toHaveBeenCalledWith(0, 2, 2_000n, 985n);
    });

    it('FE-003: three_pool_redeem estimate is net and min_amount derives from it', async () => {
      threePoolMock.calcRemoveOneCoin.mockResolvedValue(1_000n);

      const route = await resolveRoute(threeUsd, ckUsdc, 2_000n);
      expect(route.type).toBe('three_pool_redeem');
      expect(route.grossOutput).toBe(1_000n);
      expect(route.estimatedOutput).toBe(990n);

      threePoolMock.removeOneCoin.mockResolvedValue(989n);
      await executeRoute(route, threeUsd, ckUsdc, 2_000n, 50);
      expect(threePoolMock.removeOneCoin).toHaveBeenCalledWith(2_000n, 2, 985n);
    });

    it('FE-003: three_pool_deposit stays gross (LP mint pays no ledger fee)', async () => {
      threePoolMock.calcAddLiquidity.mockResolvedValue(1_000n);

      const route = await resolveRoute(ckUsdc, threeUsd, 2_000n);
      expect(route.type).toBe('three_pool_deposit');
      expect(route.grossOutput).toBe(1_000n);
      expect(route.estimatedOutput).toBe(1_000n);

      threePoolMock.addLiquidity.mockResolvedValue(1_000n);
      await executeRoute(route, ckUsdc, threeUsd, 2_000n, 50);
      // min_lp = 1_000 * 9_950 / 10_000 = 995n (gross bound, no fee deduction)
      expect(threePoolMock.addLiquidity).toHaveBeenCalledWith([0n, 0n, 2_000n], 995n);
    });

    it('FE-003: clamps the net estimate at zero when the fee exceeds a dust quote', async () => {
      threePoolMock.calcRemoveOneCoin.mockResolvedValue(7n);

      const route = await resolveRoute(threeUsd, ckUsdc, 10n);
      expect(route.estimatedOutput).toBe(0n);
      expect(route.grossOutput).toBe(7n);
    });
  });

  describe('paused AMM1 bridge execution', () => {
    it('executes ICP -> icUSD on ICPswap, then icUSD -> ckUSDT in the 3pool', async () => {
      const hopQuote = icpswapIcUsdQuote(2_500n);
      const route: SwapRoute = {
        type: 'icp_to_stable_via_icusd',
        pathDisplay: 'ICP → icUSD → ckUSDT',
        hops: 2,
        estimatedOutput: 2_390n,
        grossOutput: 2_400n,
        feeDisplay: 'ICPswap 0.30% + 3pool 0.04%',
        intermediateOutput: 2_490n,
        hopProviderQuote: hopQuote,
      };
      icpswapIcUsdMock.swap.mockResolvedValue({ amountOut: 2_485n });
      threePoolMock.quoteSwap.mockResolvedValue({ amount_out: 2_300n, fee_bps: 4, is_rebalancing: false });
      threePoolMock.swap.mockResolvedValue(2_280n);

      const out = await executeRoute(route, icp, ckUsdt, 10_000n, 50);

      // The ICPswap hop uses half of the 0.50% tolerance against its gross
      // output; the 3pool hop spends the remaining half against its NET output.
      expect(icpswapIcUsdMock.swap).toHaveBeenCalledWith(
        icp, icUsd, 10_000n, 2_500n * 9_975n / 10_000n, hopQuote,
      );
      expect(threePoolMock.swap).toHaveBeenCalledWith(
        0, ckUsdt.threePoolIndex, 2_485n, 2_290n * 9_975n / 10_000n,
      );
      expect(rumiAmmMock.swap).not.toHaveBeenCalled();
      expect(out).toBe(2_280n);
    });

    it('uses the same ICPswap -> 3pool sequence for an Oisy wallet', async () => {
      isOisyWalletMock.mockReturnValue(true);
      const hopQuote = icpswapIcUsdQuote(2_500n);
      const route: SwapRoute = {
        type: 'icp_to_stable_via_icusd',
        pathDisplay: 'ICP → icUSD → ckUSDT',
        hops: 2,
        estimatedOutput: 2_390n,
        grossOutput: 2_400n,
        feeDisplay: 'ICPswap 0.30% + 3pool 0.04%',
        intermediateOutput: 2_490n,
        hopProviderQuote: hopQuote,
      };
      const fakeSignerAgent = {};
      const fakeIcpLedger = { icrc2_approve: vi.fn().mockResolvedValue({ Ok: 1n }) };
      const fakeIcUsdLedger = { icrc2_approve: vi.fn().mockResolvedValue({ Ok: 1n }) };
      const fakeIcpswapPool = {
        depositFrom: vi.fn().mockResolvedValue({ ok: 0n }),
        swap: vi.fn().mockResolvedValue({ ok: 0n }),
        withdraw: vi.fn().mockResolvedValue({ ok: 2_485n }),
      };
      const fakeThreePool = { swap: vi.fn().mockResolvedValue({ Ok: 2_280n }) };
      const oisySigner = await import('./oisySigner');
      vi.mocked(oisySigner.getOisySignerAgent).mockResolvedValue(fakeSignerAgent as any);
      vi.mocked(oisySigner.createOisyActor).mockImplementation(((canisterId: string) => {
        if (canisterId === 'nqxwe-hiaaa-aaaar-qb5yq-cai') return fakeIcpswapPool;
        if (canisterId === 'fohh4-yyaaa-aaaap-qtkpa-cai') return fakeThreePool;
        if (canisterId === 't6bor-paaaa-aaaap-qrd5q-cai') return fakeIcUsdLedger;
        return fakeIcpLedger;
      }) as any);

      const out = await executeRoute(route, icp, ckUsdt, 10_000n, 50);

      expect(fakeIcpLedger.icrc2_approve).toHaveBeenCalledTimes(1);
      expect(fakeIcpswapPool.depositFrom).toHaveBeenCalledWith(expect.objectContaining({ token: icp.ledgerId }));
      expect(fakeIcpswapPool.swap).toHaveBeenCalledWith(expect.objectContaining({ amountIn: '10000' }));
      expect(fakeIcpswapPool.withdraw).toHaveBeenCalledWith(expect.objectContaining({ token: icUsd.ledgerId }));
      expect(fakeIcUsdLedger.icrc2_approve).toHaveBeenCalledTimes(1);
      expect(fakeThreePool.swap).toHaveBeenCalledWith(0, ckUsdt.threePoolIndex, 2_485n, 2_390n * 9_975n / 10_000n);
      expect(rumiAmmMock.swap).not.toHaveBeenCalled();
      expect(out).toBe(2_280n);
    });
  });

  // ──────────────────────────────────────────────────────────────
  // Oisy ICPswap direct dispatch (3USD <-> ICP, single-hop ICPswap winner).
  // Verifies the `amm_swap` case routes through the sequential Oisy executor
  // (executeIcpswapDirectOisy) when Oisy is the wallet, and through
  // provider.swap otherwise.
  //
  // @icp-sdk/signer v5 has no batch/commit/execute concept (see the
  // oisySigner.ts docstring): each canister call is an independent sequential
  // await. The executor approves the from-ledger, then calls
  // depositFrom -> swap -> withdraw on the pool actor directly — no execute().
  // ──────────────────────────────────────────────────────────────

  describe('Oisy ICPswap direct dispatch (amm_swap)', () => {
    it('dispatches through the sequential Oisy executor instead of provider.swap when ICPswap wins', async () => {
      isOisyWalletMock.mockReturnValue(true);

      // Wire up Oisy fakes. v5: getOisySignerAgent returns a SignerAgent that
      // is handed to createOisyActor; it has no batch/execute of its own, so a
      // plain placeholder stands in. The actors do the real work via sequential
      // awaits and must return the Ok/ok shapes the executor unpacks.
      const fakeSignerAgent = {};
      const fakeFromLedger = {
        icrc2_approve: vi.fn().mockResolvedValue({ Ok: 1n }),
      };
      // r3.ok (the ICPswap swap step's actual output) is deliberately NOT
      // equal to minOut, so this test can tell apart withdrawing the real
      // swap output (fixed, correct) from withdrawing the slippage floor
      // (the old fund-stranding bug). minOut here is
      // grossOutput(1_500n) * 9_950n / 10_000n = 1_492n (BigInt truncation);
      // 1_497n is comfortably different from that and still below
      // grossOutput, representing a swap that landed with some positive
      // slippage above the floor.
      const fakePool = {
        depositFrom: vi.fn().mockResolvedValue({ ok: 0n }),
        swap: vi.fn().mockResolvedValue({ ok: 1_497n }),
        withdraw: vi.fn().mockResolvedValue({ ok: 1_495n }),
      };
      const oisySigner = await import('./oisySigner');
      vi.mocked(oisySigner.getOisySignerAgent).mockResolvedValue(fakeSignerAgent as any);
      vi.mocked(oisySigner.createOisyActor).mockImplementation(((canisterId: string) => {
        // Pool ID from icpswapQuote() fixture above
        if (canisterId === 'mu2zw-6iaaa-aaaar-qb56q-cai') return fakePool;
        return fakeFromLedger;
      }) as any);

      const route: SwapRoute = {
        type: 'amm_swap',
        pathDisplay: 'x',
        hops: 1,
        estimatedOutput: 1_490n,
        grossOutput: 1_500n,
        feeDisplay: '0.30%',
        providerQuote: icpswapQuote(1_500n),
      };

      const out = await executeRoute(route, threeUsd, icp, 100n, 50);

      // Provider.swap path was bypassed
      expect(icpswapMock.swap).not.toHaveBeenCalled();
      // Oisy v5 sequential flow ran end to end: approve the from-ledger, then
      // depositFrom -> swap -> withdraw on the pool actor (no execute()).
      expect(fakeFromLedger.icrc2_approve).toHaveBeenCalledTimes(1);
      expect(fakePool.depositFrom).toHaveBeenCalledTimes(1);
      expect(fakePool.swap).toHaveBeenCalledTimes(1);
      // Withdraws the actual swap output (r3.ok = 1_497n), NOT the slippage
      // floor minOut (1_492n). If the withdraw amount were reverted back to
      // minOut, this would assert 1_497n but receive 1_492n and fail.
      expect(fakePool.withdraw).toHaveBeenCalledWith(
        expect.objectContaining({ amount: 1_497n }),
      );
      // Returns the `ok` value from the final withdraw call
      expect(out).toBe(1_495n);
    });

  });

  // ──────────────────────────────────────────────────────────────
  // Kill switch: while AMM1 is paused, disabling ICPswap means no bridge is
  // available. The router must fail closed instead of reviving AMM1.
  // ──────────────────────────────────────────────────────────────

  describe('ICPswap kill switch', () => {
    it('does not fall back to Rumi AMM when ICPswap is disabled', async () => {
      setIcpswapRoutingEnabled(false);

      await expect(resolveRoute(threeUsd, icp, 100n))
        .rejects.toThrow(/no route available while AMM1 routing is paused/i);
      expect(rumiAmmMock.quote).not.toHaveBeenCalled();
      expect(icpswapMock.quote).not.toHaveBeenCalled();
    });

    it('refuses to execute a stale ICPswap route if the flag flipped off after quoting', async () => {
      // Simulate the sequence: quote while enabled, then admin disables,
      // then user clicks execute.
      const route: SwapRoute = {
        type: 'amm_swap',
        pathDisplay: 'x',
        hops: 1,
        estimatedOutput: 1_490n,
        grossOutput: 1_500n,
        feeDisplay: '0.30%',
        providerQuote: icpswapQuote(1_500n),
      };
      setIcpswapRoutingEnabled(false);

      await expect(executeRoute(route, threeUsd, icp, 100n, 50))
        .rejects.toThrow(/ICPswap routing is currently disabled/i);
    });

    it('refuses to execute a stale Rumi AMM route while AMM1 is paused', async () => {
      setIcpswapRoutingEnabled(false);
      rumiAmmMock.swap.mockResolvedValue({ amountOut: 990n });

      const route: SwapRoute = {
        type: 'amm_swap',
        pathDisplay: 'x',
        hops: 1,
        estimatedOutput: 990n,
        grossOutput: 1_000n,
        feeDisplay: '0.30%',
        providerQuote: rumiQuote(1_000n),
      };

      await expect(executeRoute(route, threeUsd, icp, 100n, 50))
        .rejects.toThrow(/AMM1 routing is currently paused/i);
      expect(rumiAmmMock.swap).not.toHaveBeenCalled();
    });

    it('returns no route for icUSD<->ICP when ICPswap is disabled', async () => {
      setIcpswapRoutingEnabled(false);
      await expect(resolveRoute(icUsd, icp, 100n))
        .rejects.toThrow(/no route available while AMM1 routing is paused/i);
      expect(icpswapMock.quote).not.toHaveBeenCalled();
      expect(icpswapIcUsdMock.quote).not.toHaveBeenCalled();
    });

    it('returns no ICP -> stablecoin bridge when ICPswap is disabled', async () => {
      setIcpswapRoutingEnabled(false);
      await expect(resolveRoute(icp, ckUsdc, 100n))
        .rejects.toThrow(/no route available while AMM1 routing is paused/i);
      expect(rumiAmmMock.quote).not.toHaveBeenCalled();
    });
  });
});

// ──────────────────────────────────────────────────────────────
// dustThreshold: the decimals-aware raw-unit cutoff used throughout the
// ICPswap unused-balance recovery flow (checkIcpswapUnusedBalances,
// preWarmRecovery, recoverIcpswapBalance). checkIcpswapUnusedBalances itself
// dynamically imports '@dfinity/agent' and builds a live Actor, which none
// of the mocks above stand in for -- mocking that module just to exercise
// this arithmetic would be disproportionate to the bug being covered here.
// The bug is entirely in this helper's math (a flat e8s constant compared
// against 6-decimal balances), so it is tested directly and exported for
// that purpose.
// ──────────────────────────────────────────────────────────────

describe('dustThreshold', () => {
  it('returns the pre-existing flat 100_000n threshold at 8 decimals (icUSD/3USD/ICP)', () => {
    expect(dustThreshold(8)).toBe(100_000n);
  });

  it('returns 1_000n at 6 decimals (ckUSDT/ckUSDC), not the flat 100_000n constant', () => {
    // The bug this fixes: a stranded balance of, say, 50_000n raw units of a
    // 6-decimal token is 0.05 tokens, comfortably above the intended 0.001
    // token cutoff, but the old flat 100_000n constant would have hidden it.
    const threshold = dustThreshold(6);
    expect(threshold).toBe(1_000n);
    expect(threshold).not.toBe(100_000n);

    const strandedBalance = 50_000n; // 0.05 ckUSDT
    expect(strandedBalance > threshold).toBe(true); // now surfaced
    expect(strandedBalance > 100_000n).toBe(false); // old constant hid it
  });

  it('guards against nonsensical decimals instead of throwing on a negative exponent', () => {
    expect(() => dustThreshold(-1)).not.toThrow();
    expect(() => dustThreshold(0)).not.toThrow();
    expect(() => dustThreshold(NaN)).not.toThrow();
    expect(() => dustThreshold(1.5)).not.toThrow();
  });
});
