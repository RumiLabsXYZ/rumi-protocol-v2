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

const { rumiAmmMock, icpswapMock, icpswapIcUsdMock, threePoolMock, isOisyWalletMock } = mocks;

vi.mock('./providers/rumiAmmProvider', () => ({
  RumiAmmProvider: vi.fn(() => mocks.rumiAmmMock),
}));

vi.mock('./providers/icpswapProvider', () => ({
  IcpswapProvider: vi.fn((config: { id: string }) => (
    config.id === 'icpswap_icusd_icp' ? mocks.icpswapIcUsdMock : mocks.icpswapMock
  )),
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

import { resolveRoute, executeRoute, setIcpswapRoutingEnabled, type SwapRoute } from './swapRouter';

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

describe('swapRouter — provider registry integration', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    // restore the default supports() after clearAllMocks
    rumiAmmMock.supports.mockReturnValue(true);
    icpswapMock.supports.mockReturnValue(true);
    icpswapIcUsdMock.supports.mockReturnValue(true);
    // Most tests exercise routes where ICPswap is a valid option; the
    // kill-switch-off behaviour is covered in its own describe block.
    setIcpswapRoutingEnabled(true);
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
      const fakePool = {
        depositFrom: vi.fn().mockResolvedValue({ ok: 0n }),
        swap: vi.fn().mockResolvedValue({ ok: 0n }),
        withdraw: vi.fn().mockResolvedValue({ ok: 1_499n }),
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
      expect(fakePool.withdraw).toHaveBeenCalledTimes(1);
      // Returns the `ok` value from the final withdraw call
      expect(out).toBe(1_499n);
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
