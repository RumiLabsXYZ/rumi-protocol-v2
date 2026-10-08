import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const { getEpochStatus, getPointsConfig, getFiatStablePointsPolicy } = vi.hoisted(() => ({
  getEpochStatus: vi.fn(),
  getPointsConfig: vi.fn(),
  getFiatStablePointsPolicy: vi.fn(),
}));

vi.mock('$lib/config', () => ({ POINTS_ENABLED: true }));
vi.mock('$lib/services/pointsService', () => ({
  getEpochStatus,
  getPointsConfig,
  getFiatStablePointsPolicy,
}));
vi.mock('$lib/utils/points', () => ({ seasonState: vi.fn(() => 'live') }));

const flatPolicy = {
  mode: 'flat4x',
  source: 'runtime',
  effectiveEpoch: 19n,
  legacyEpoch: null,
  historicalLedgerCutoff: 401n,
  inlineTopupRows: 0n,
  historicalComplete: false,
  inlineTopupsComplete: true,
  pendingCutover: false,
  migration: { status: 'in_progress', processed: 200n, total: null },
};

const legacyPolicy = {
  mode: 'legacy',
  source: 'legacy-fallback',
  effectiveEpoch: null,
  legacyEpoch: null,
  historicalLedgerCutoff: null,
  inlineTopupRows: null,
  historicalComplete: null,
  inlineTopupsComplete: null,
  pendingCutover: false,
  migration: { status: 'unknown', processed: null, total: null },
};

async function loadStore() {
  vi.resetModules();
  return import('./seasonStore');
}

async function readLoadedState(seasonStore: { subscribe: (fn: (value: any) => void) => () => void; ensureLoaded: () => Promise<void> }) {
  let current: any;
  const unsubscribe = seasonStore.subscribe((value) => {
    current = value;
  });
  await seasonStore.ensureLoaded();
  unsubscribe();
  return current;
}

describe('seasonStore policy isolation', () => {
  beforeEach(() => {
    getEpochStatus.mockReset();
    getPointsConfig.mockReset();
    getFiatStablePointsPolicy.mockReset();
    vi.spyOn(console, 'error').mockImplementation(() => {});
  });

  afterEach(() => vi.restoreAllMocks());

  it('preserves a successful flat policy when status and config fail', async () => {
    getEpochStatus.mockRejectedValue(new Error('status unavailable'));
    getPointsConfig.mockRejectedValue(new Error('config unavailable'));
    getFiatStablePointsPolicy.mockResolvedValue(flatPolicy);

    const { seasonStore } = await loadStore();
    const state = await readLoadedState(seasonStore);

    expect(state.status).toBeNull();
    expect(state.config).toBeNull();
    expect(state.policy.mode).toBe('flat4x');
    expect(state.loaded).toBe(true);
  });

  it('uses unknown when the policy query fails instead of legacy', async () => {
    getEpochStatus.mockResolvedValue({});
    getPointsConfig.mockResolvedValue({});
    getFiatStablePointsPolicy.mockRejectedValue(new Error('policy unavailable'));

    const { seasonStore } = await loadStore();
    const state = await readLoadedState(seasonStore);

    expect(state.policy.mode).toBe('unknown');
    expect(state.policy.source).toBe('runtime');
  });

  it('preserves legacy only when the service explicitly confirms method absence', async () => {
    getEpochStatus.mockResolvedValue({});
    getPointsConfig.mockResolvedValue({});
    getFiatStablePointsPolicy.mockResolvedValue(legacyPolicy);

    const { seasonStore } = await loadStore();
    const state = await readLoadedState(seasonStore);

    expect(state.policy.mode).toBe('legacy');
    expect(state.policy.source).toBe('legacy-fallback');
  });
});
