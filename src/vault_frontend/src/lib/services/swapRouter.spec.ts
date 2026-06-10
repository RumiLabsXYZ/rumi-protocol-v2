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

describe('swapRouter — provider registry integration', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    // restore the default supports() after clearAllMocks
    rumiAmmMock.supports.mockReturnValue(true);
    icpswapMock.supports.mockReturnValue(true);
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

    it('populates providerQuote with Rumi AMM when it wins and caches poolId', async () => {
      rumiAmmMock.quote.mockResolvedValue(rumiQuote(2_000n));
      icpswapMock.quote.mockResolvedValue(icpswapQuote(1_500n));

      const route = await resolveRoute(icp, threeUsd, 100n);

      expect(route.providerQuote?.provider).toBe('rumi_amm');
      expect(route.estimatedOutput).toBe(1_990n);
      expect(route.grossOutput).toBe(2_000n);
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
      // FE-003: NET of the 10n output ledger fee
      expect(route.estimatedOutput).toBe(2_990n);
      expect(route.grossOutput).toBe(3_000n);
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
      // FE-003: NET of the 10n output ledger fee
      expect(route.estimatedOutput).toBe(2_390n);
      expect(route.grossOutput).toBe(2_400n);
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

    it('FE-003: passes a NET min_amount_out to Rumi AMM (backend enforces net, PR #230)', async () => {
      const winningQuote = rumiQuote(1_500n);
      const route: SwapRoute = {
        type: 'amm_swap',
        pathDisplay: 'x',
        hops: 1,
        estimatedOutput: 1_490n, // 1_500n gross - 10n mocked ledger fee
        grossOutput: 1_500n,
        feeDisplay: '0.30%',
        providerQuote: winningQuote,
      };
      rumiAmmMock.swap.mockResolvedValue({ amountOut: 1_489n });

      const out = await executeRoute(route, threeUsd, icp, 100n, 50);

      // min = net estimate * (1 - 0.5%) = 1_490 * 9_950 / 10_000 = 1_482n
      expect(rumiAmmMock.swap).toHaveBeenCalledWith(
        threeUsd, icp, 100n, 1_490n * 9_950n / 10_000n, winningQuote,
      );
      expect(out).toBe(1_489n);
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

  // ──────────────────────────────────────────────────────────────
  // icp_to_stable hop-2: burn the NET 3USD received from hop 1.
  //
  // rumi_amm's swap pays out `amount_out - ledger_fee` (transfer_to_user), so
  // after the ICP->3USD hop the caller's 3USD balance grows by one ledger fee
  // LESS than the AMM's returned amount_out. The 3pool's remove_one_coin burns
  // the caller's LP directly and rejects with InsufficientLiquidity when
  // lp_burn exceeds the held balance, so hop 2 must burn the NET amount
  // (gross - 3USD ledger fee, mocked to 10n), not the gross. ICPswap's withdraw
  // already nets the fee, so its returned amountOut is the held amount as-is.
  // ──────────────────────────────────────────────────────────────

  describe('icp_to_stable hop-2 burns NET 3USD (gross - ledger fee)', () => {
    it('non-Oisy: removeOneCoin burns gross minus the 3USD ledger fee (Rumi AMM hop)', async () => {
      // Hop 1 (ICP -> 3USD) returns a GROSS amount_out of 2_000n; the user's
      // 3USD balance only grows by 2_000n - 10n (one ledger fee).
      rumiAmmMock.swap.mockResolvedValue({ amountOut: 2_000n });
      threePoolMock.calcRemoveOneCoin.mockResolvedValue(1_000n);
      threePoolMock.removeOneCoin.mockResolvedValue(989n);

      const route: SwapRoute = {
        type: 'icp_to_stable',
        pathDisplay: 'x',
        hops: 2,
        estimatedOutput: 990n,
        grossOutput: 1_000n,
        feeDisplay: '0.30%',
        intermediateOutput: 2_000n,
        hopProviderQuote: rumiQuote(2_000n),
        poolId: 'rumi-pool-1',
      };

      const out = await executeRoute(route, icp, ckUsdc, 10_000n, 50);

      // Both the estimate and the burn use the NET 3USD (2_000n - 10n), never
      // the gross 2_000n that would over-burn the caller's LP balance.
      expect(threePoolMock.calcRemoveOneCoin).toHaveBeenCalledWith(1_990n, ckUsdc.threePoolIndex);
      // stableMinOutput = net(1_000n)=990n * (10000-25)/10000 = 987n
      expect(threePoolMock.removeOneCoin).toHaveBeenCalledWith(1_990n, ckUsdc.threePoolIndex, 987n);
      expect(out).toBe(989n);
    });

    it('Oisy: remove_one_coin burns gross minus the 3USD ledger fee (Rumi AMM hop)', async () => {
      isOisyWalletMock.mockReturnValue(true);

      const fakeSignerAgent = {};
      const fakeIcpLedger = { icrc2_approve: vi.fn().mockResolvedValue({ Ok: 1n }) };
      const fakeAmm = { swap: vi.fn().mockResolvedValue({ Ok: { amount_out: 2_000n } }) };
      const fakePool = { remove_one_coin: vi.fn().mockResolvedValue({ Ok: 988n }) };

      const oisySigner = await import('./oisySigner');
      vi.mocked(oisySigner.getOisySignerAgent).mockResolvedValue(fakeSignerAgent as any);
      // swapRouter's module constants use the mainnet IDs regardless of network.
      vi.mocked(oisySigner.createOisyActor).mockImplementation(((canisterId: string) => {
        if (canisterId === 'ijlzs-2yaaa-aaaap-quaaq-cai') return fakeAmm;   // RUMI_AMM
        if (canisterId === 'fohh4-yyaaa-aaaap-qtkpa-cai') return fakePool;  // THREEPOOL
        return fakeIcpLedger;                                               // ICP ledger
      }) as any);

      const route: SwapRoute = {
        type: 'icp_to_stable',
        pathDisplay: 'x',
        hops: 2,
        estimatedOutput: 990n,
        grossOutput: 1_000n,
        feeDisplay: '0.30%',
        intermediateOutput: 2_000n, // gross 3USD estimate from the AMM hop
        hopProviderQuote: rumiQuote(2_000n),
        poolId: 'rumi-pool-1',
      };

      const out = await executeRoute(route, icp, ckUsdc, 10_000n, 50);

      // remove_one_coin burns 2_000n - 10n, NOT the gross 2_000n. The caller only
      // holds the net amount, so burning the gross would trip InsufficientLiquidity.
      // stableMinOutput = estimatedOutput(990n) * (10000-50)/10000 = 985n.
      expect(fakePool.remove_one_coin).toHaveBeenCalledWith(1_990n, ckUsdc.threePoolIndex, 985n);
      expect(out).toBe(988n);
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

    it('still uses provider.swap when Oisy wins with Rumi AMM (Rumi handles signer internally)', async () => {
      isOisyWalletMock.mockReturnValue(true);
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

      const out = await executeRoute(route, threeUsd, icp, 100n, 50);
      expect(rumiAmmMock.swap).toHaveBeenCalledTimes(1);
      expect(out).toBe(990n);
    });
  });

  // ──────────────────────────────────────────────────────────────
  // Kill switch: when `icpswap_routing_enabled` is off, the router must
  // behave as if only Rumi AMM + the 3pool existed.
  // ──────────────────────────────────────────────────────────────

  describe('ICPswap kill switch', () => {
    it('ignores ICPswap quotes on 3USD->ICP and picks Rumi AMM even when ICPswap quotes higher', async () => {
      setIcpswapRoutingEnabled(false);
      rumiAmmMock.quote.mockResolvedValue(rumiQuote(1_000n));
      // ICPswap would win if it were queried — but the router must not call it.
      icpswapMock.quote.mockResolvedValue(icpswapQuote(9_999n));

      const route = await resolveRoute(threeUsd, icp, 100n);

      expect(route.providerQuote?.provider).toBe('rumi_amm');
      // FE-003: NET of the 10n output ledger fee
      expect(route.estimatedOutput).toBe(990n);
      expect(route.grossOutput).toBe(1_000n);
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

    it('still allows pure-Rumi routes to execute when ICPswap is disabled', async () => {
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

      const out = await executeRoute(route, threeUsd, icp, 100n, 50);
      expect(out).toBe(990n);
      expect(rumiAmmMock.swap).toHaveBeenCalled();
    });

    it('falls back to 2-hop for icUSD<->ICP when ICPswap is disabled (no direct attempt)', async () => {
      setIcpswapRoutingEnabled(false);
      const icUsd: AmmToken = {
        symbol: 'icUSD',
        ledgerId: 't6bor-paaaa-aaaap-qrd5q-cai',
        decimals: 8,
        color: '#000',
        balanceKey: 'ICUSD',
        is3USD: false,
        threePoolIndex: 0,
      };

      // Two-hop: 3pool add_liquidity then Rumi AMM on 3USD->ICP
      threePoolMock.calcAddLiquidity.mockResolvedValue(950n);
      rumiAmmMock.quote.mockResolvedValue(rumiQuote(500n));
      // If the direct icUSD/ICP ICPswap path were attempted the mock would
      // need to be called — assert it wasn't.
      icpswapMock.quote.mockResolvedValue(icpswapQuote(9_999n));

      const route = await resolveRoute(icUsd, icp, 100n);
      expect(route.type).toBe('stable_to_icp');
      // icpswapMock.quote should only have been consulted for the 3USD/ICP
      // hop — which it shouldn't have been either, since the flag is off.
      expect(icpswapMock.quote).not.toHaveBeenCalled();
    });
  });
});
