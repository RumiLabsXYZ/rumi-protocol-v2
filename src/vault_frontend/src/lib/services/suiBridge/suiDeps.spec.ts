import { Principal } from '@dfinity/principal';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const mocks = vi.hoisted(() => ({ lane: vi.fn(), config: vi.fn(), roles: vi.fn(), observed: vi.fn() }));

vi.mock('$lib/config', () => ({
  CANISTER_IDS: { ICUSD_SUI_BRIDGE: 'rrkah-fqaaa-aaaaa-aaaaq-cai' },
  CONFIG: { isLocal: false, host: 'https://icp0.io', currentIcusdLedgerId: 't6bor-paaaa-aaaap-qrd5q-cai' },
}));
vi.mock('$lib/stores/wallet', () => ({ walletStore: { getActor: vi.fn() } }));
vi.mock('$lib/services/ledgerFeeService', () => ({ fetchLedgerFeeStrict: vi.fn() }));
vi.mock('./suiActor', () => ({
  getSuiBridgeCanisterId: () => 'rrkah-fqaaa-aaaaa-aaaaq-cai',
  getAnonymousSuiBridge: async () => ({ get_config: mocks.config, get_roles: mocks.roles, get_mint_lane: mocks.lane, get_observed_deployment: mocks.observed }),
}));
vi.mock('../solanaBridge/solanaActor', () => ({ getAnonymousAgent: vi.fn() }));

import { buildSuiPrepareDeps } from './suiDeps';
import { TEST_OWNER_TEXT, makeLane, makeObserved, makeRoles, makeSuiConfig, makeSuiConfigView } from './testSupport';

beforeEach(() => {
  mocks.config.mockReset().mockResolvedValue(makeSuiConfigView());
  mocks.roles.mockReset().mockResolvedValue(makeRoles());
  mocks.lane.mockReset().mockResolvedValue(makeLane());
  mocks.observed.mockReset().mockResolvedValue([makeObserved()]);
});

describe('the quote reads of the ICP to Sui direction', () => {
  const deps = () => buildSuiPrepareDeps(Principal.fromText(TEST_OWNER_TEXT), makeSuiConfig());

  it('take the live fees and limits from the canister\'s own answers when they pass every identity check and the ledger is the configured one', async () => {
    const config = await deps().fetchConfig();
    expect(config).toEqual({
      depositFeeE8s: 10_000_000n,
      bridgeLedgerFeeE8s: null,
      limits: { minDepositE8s: 100_000_000n, maxDepositE8s: 1_000_000_000n, depositsPaused: false, halted: null },
    });
  });

  it('refuse a bridge that escrows a DIFFERENT icUSD ledger than this app uses, so an approval could never apply to it', async () => {
    mocks.config.mockResolvedValue(makeSuiConfigView({ icusd_ledger: Principal.fromText('rrkah-fqaaa-aaaaa-aaaaq-cai') }));
    await expect(deps().fetchConfig()).rejects.toThrow('escrows a different icUSD ledger');
  });

  it('refuse an instance whose answers no longer pass the identity checks (nothing is quoted)', async () => {
    mocks.observed.mockResolvedValue([]);
    await expect(deps().fetchConfig()).rejects.toThrow('not yet observed');
    mocks.observed.mockResolvedValue([makeObserved()]);
    mocks.config.mockResolvedValue(makeSuiConfigView({ initialized: false }));
    await expect(deps().fetchConfig()).rejects.toThrow('not set up');
  });
});
