import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// `getThreePoolApy` caches its result for 30s in a module-level variable, so
// each test gets a fresh module instance (and thus a fresh cache and fresh
// mocks, since vi.mock factories re-run after resetModules).
beforeEach(() => {
  vi.resetModules();
});

afterEach(() => {
  vi.restoreAllMocks();
});

const POOL_STATUS = {
  balances: [500_000_000_000n, 300_000_000n, 200_000_000n], // icUSD(8d) + ckUSDT/ckUSDC(6d) -> 5000 + 300 + 200 icUSD
  lp_total_supply: 0n,
  current_a: 0n,
  virtual_price: 0n,
  swap_fee_bps: 0n,
  admin_fee_bps: 0n,
  tokens: [],
};

// TVL from POOL_STATUS normalizes to 5500 icUSD; these values are chosen so
// the resulting APR (and its daily-compounded APY) stays a small, realistic,
// finite number rather than overflowing to Infinity.
const PROTOCOL_STATUS = {
  perCollateralInterest: [
    { collateralType: 'icp', totalDebtE8s: 1000, weightedInterestRate: 0.05 },
  ],
} as any;

function mockThreePoolService(overrides: {
  getPoolStatus?: () => Promise<any>;
  getSwapFeesOverWindow?: () => Promise<bigint>;
} = {}) {
  vi.doMock('./threePoolService', async (importOriginal) => {
    const actual = await importOriginal<typeof import('./threePoolService')>();
    return {
      ...actual,
      threePoolService: {
        ...actual.threePoolService,
        getPoolStatus: overrides.getPoolStatus ?? (async () => POOL_STATUS),
        getSwapFeesOverWindow: overrides.getSwapFeesOverWindow ?? (async () => 700_000_000n),
      },
    };
  });
}

function mockProtocol(getProtocolStatus?: () => Promise<any>) {
  vi.doMock('./protocol', () => ({
    ProtocolService: {
      getProtocolStatus: getProtocolStatus ?? (async () => PROTOCOL_STATUS),
    },
  }));
}

function mockPublicActor(get_interest_split?: () => Promise<any>) {
  vi.doMock('./protocol/apiClient', () => ({
    publicActor: {
      get_interest_split:
        get_interest_split ?? (async () => [{ destination: 'three_pool', bps: 5000n }]),
    },
  }));
}

async function loadService() {
  return import('./threePoolApyService');
}

describe('getThreePoolApy completeness', () => {
  it('marks a result complete when every input is fetched live and yields a finite APY', async () => {
    mockThreePoolService();
    mockProtocol();
    mockPublicActor();
    const { getThreePoolApy } = await loadService();

    const result = await getThreePoolApy();

    expect(result.complete).toBe(true);
    expect(Number.isFinite(result.total_apy_pct)).toBe(true);
    expect(result.total_apy_pct).toBeGreaterThan(0);
  });

  it('marks a true zero APY as complete (no debt, no fees) rather than unavailable', async () => {
    mockThreePoolService({ getSwapFeesOverWindow: async () => 0n });
    mockProtocol(async () => ({ perCollateralInterest: [] }));
    mockPublicActor();
    const { getThreePoolApy } = await loadService();

    const result = await getThreePoolApy();

    expect(result.complete).toBe(true);
    expect(result.total_apy_pct).toBe(0);
  });

  it('marks the result incomplete when the protocol status fetch fails', async () => {
    mockThreePoolService();
    mockProtocol(async () => {
      throw new Error('protocol status unavailable');
    });
    mockPublicActor();
    const { getThreePoolApy } = await loadService();

    const result = await getThreePoolApy();

    expect(result.complete).toBe(false);
  });

  it('marks the result incomplete when the interest split fetch fails (falls back to the legacy default share)', async () => {
    mockThreePoolService();
    mockProtocol();
    mockPublicActor(async () => {
      throw new Error('interest split unavailable');
    });
    const { getThreePoolApy } = await loadService();

    const result = await getThreePoolApy();

    expect(result.complete).toBe(false);
    // The existing math still runs with its legacy 50% default so old
    // consumers keep behaving the same way.
    expect(result.three_pool_share_bps).toBe(5000);
  });

  it('marks the result incomplete when the swap-fee window fetch fails (falls back to zero fees)', async () => {
    mockThreePoolService({
      getSwapFeesOverWindow: async () => {
        throw new Error('fee window unavailable');
      },
    });
    mockProtocol();
    mockPublicActor();
    const { getThreePoolApy } = await loadService();

    const result = await getThreePoolApy();

    expect(result.complete).toBe(false);
  });

  it('marks the result incomplete when pool TVL is zero (nothing to compute a rate over)', async () => {
    mockThreePoolService({ getPoolStatus: async () => ({ ...POOL_STATUS, balances: [0n, 0n, 0n] }) });
    mockProtocol();
    mockPublicActor();
    const { getThreePoolApy } = await loadService();

    const result = await getThreePoolApy();

    expect(result.complete).toBe(false);
    expect(result.total_apy_pct).toBe(0);
  });

  it('propagates a pool-status fetch failure as a rejected promise (not a fabricated result)', async () => {
    mockThreePoolService({
      getPoolStatus: async () => {
        throw new Error('pool status unavailable');
      },
    });
    mockProtocol();
    mockPublicActor();
    const { getThreePoolApy } = await loadService();

    await expect(getThreePoolApy()).rejects.toThrow('pool status unavailable');
  });
});
