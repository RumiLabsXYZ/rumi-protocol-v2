import { beforeEach, describe, expect, it, vi } from 'vitest';

// `loadDestinations()` runs the REAL `loadBridgeConfig` (so the real `get_bridge_info` -> `resolveBridgeConfig`
// wiring is what these specs exercise). Only the world around it is replaced: the agent and the wallet stores.
const mocks = vi.hoisted(() => ({ getBridgeInfo: vi.fn() }));

vi.mock('$lib/config', () => ({
  CANISTER_IDS: { ICUSD_BRIDGE: 'tfesu-vyaaa-aaaap-qrd7a-cai' },
  CONFIG: { isLocal: false, host: 'https://icp0.io', currentIcusdLedgerId: 't6bor-paaaa-aaaap-qrd5q-cai' },
}));
vi.mock('@dfinity/agent', async (importOriginal) => {
  const original = await importOriginal<typeof import('@dfinity/agent')>();
  return {
    ...original,
    Actor: { createActor: () => ({ get_bridge_info: mocks.getBridgeInfo }) },
    HttpAgent: class {
      async fetchRootKey() {}
    },
    AnonymousIdentity: class {},
  };
});
vi.mock('$lib/services/auth', () => ({ walletSessionGeneration: { subscribe: () => () => {} } }));
vi.mock('$lib/stores/wallet', () => ({ principal: { subscribe: () => () => {} }, walletStore: { getActor: vi.fn() } }));
vi.mock('$lib/services/protocol/walletOperations', () => ({ isOisyWallet: () => false }));
vi.mock('$lib/services/ledgerFeeService', () => ({ fetchLedgerFeeStrict: vi.fn() }));

import type { BridgeInfo } from '$declarations/rumi_icusd_bridge/rumi_icusd_bridge.did';
import { resolveBridgeConfig } from './chains';
import {
  BASE_DESTINATION_ID,
  baseDestination,
  buildDestinations,
  findDestination,
  nextEnabledDestination,
  pickSelected,
  unavailableEvmDestinations,
  unreachableDestinations,
  type DestinationView,
} from './destinations';
import { loadDestinations } from './destinationLoader';
import { makeBridgeInfo } from './testSupport';

/** En and em dashes are not allowed in UI copy. Built from code points so this file holds none itself. */
const DASHES = new RegExp(`[${String.fromCharCode(0x2013, 0x2014)}]`);

const BRIDGE_ID = 'tfesu-vyaaa-aaaap-qrd7a-cai';
const OTHER_IDS = ['ethereum', 'arbitrum', 'robinhood', 'hyperevm', 'monad'];

function byId(list: DestinationView[], id: string): DestinationView {
  const found = list.find((destination) => destination.id === id);
  if (!found) throw new Error(`no destination ${id}`);
  return found;
}

beforeEach(() => {
  mocks.getBridgeInfo.mockReset().mockResolvedValue(makeBridgeInfo());
});

describe('loadDestinations: the Base entry', () => {
  it('is synthesized from get_bridge_info exactly as resolveBridgeConfig reads it', async () => {
    const info = makeBridgeInfo();
    mocks.getBridgeInfo.mockResolvedValue(info);
    const list = await loadDestinations();
    const base = byId(list, BASE_DESTINATION_ID);
    const resolved = resolveBridgeConfig(info);
    if (!resolved.ok) throw new Error('unreachable');

    expect(mocks.getBridgeInfo).toHaveBeenCalledTimes(1);
    expect(base).toMatchObject({
      id: 'base',
      family: 'evm',
      name: 'Base',
      chainId: '8453',
      network: 'mainnet',
      status: 'enabled',
      reason: null,
      actorCanisterId: BRIDGE_ID,
    });
    // The live configuration the forms use is the very object resolveBridgeConfig produces.
    expect(base.config).toEqual(resolved.config);
    expect(base.limits).toEqual({
      minDepositE8s: resolved.config.minDepositE8s,
      maxDepositE8s: resolved.config.maxDepositE8s,
      minBridgeOutE8s: resolved.config.minBridgeOutE8s,
    });
    expect(base.fees).toEqual({ depositFeeE8s: 10_000n, releaseFeeE8s: 0n, ledgerFeeE8s: 100_000n });
  });

  it('labels a Base Sepolia canister as a testnet', async () => {
    mocks.getBridgeInfo.mockResolvedValue(makeBridgeInfo({ chain_id: 84532n }));
    const base = byId(await loadDestinations(), 'base');
    expect(base).toMatchObject({ name: 'Base Sepolia', chainId: '84532', network: 'testnet', status: 'enabled' });
  });

  it('is disabled with the same message when the contract is not bound, and still names the chain', async () => {
    mocks.getBridgeInfo.mockResolvedValue(makeBridgeInfo({ contract: [] }));
    const base = byId(await loadDestinations(), 'base');
    expect(base).toMatchObject({
      status: 'disabled',
      reason: 'The Base contract is not bound yet.',
      name: 'Base',
      chainId: '8453',
      config: null,
      limits: null,
      fees: null,
      actorCanisterId: BRIDGE_ID,
    });
  });

  it('is disabled with the same message for an unsupported chain or an invalid contract', async () => {
    mocks.getBridgeInfo.mockResolvedValue(makeBridgeInfo({ chain_id: 1n }));
    const unsupported = byId(await loadDestinations(), 'base');
    expect(unsupported.status).toBe('disabled');
    expect(unsupported.reason).toBe('The bridge is configured for chain 1, which this app does not support.');
    expect(unsupported.chainId).toBe('');

    const bad = makeBridgeInfo();
    bad.contract = [{ ...bad.contract[0]!, address: '0x1234' }];
    mocks.getBridgeInfo.mockResolvedValue(bad);
    const invalid = byId(await loadDestinations(), 'base');
    expect(invalid).toMatchObject({ status: 'disabled', reason: 'The bridge reported an invalid Base contract address.' });
  });

  it('stays ENABLED while paused or halted: paused is not disabled', async () => {
    mocks.getBridgeInfo.mockResolvedValue(
      makeBridgeInfo({ pauses: { mints: true, observer: false, releases: true, deposits: true }, halted: ['coverage mismatch'] }),
    );
    const base = byId(await loadDestinations(), 'base');
    expect(base.status).toBe('enabled');
    expect(base.reason).toBeNull();
    expect(base.config).toMatchObject({ depositsPaused: true, releasesPaused: true, mintsPaused: true, halted: 'coverage mismatch' });
  });

  it('rejects when the canister cannot be read, so a caller can keep its last good list', async () => {
    mocks.getBridgeInfo.mockRejectedValue(new Error('replica offline'));
    await expect(loadDestinations()).rejects.toThrow('replica offline');
  });

  it('takes the Base read from a seam when one is given', async () => {
    const info = makeBridgeInfo({ contract: [] });
    const list = await loadDestinations(async () => resolveBridgeConfig(info));
    expect(byId(list, 'base').status).toBe('disabled');
    expect(mocks.getBridgeInfo).not.toHaveBeenCalled();
  });
});

describe('the other EVM destinations', () => {
  it('lists Ethereum, Arbitrum, Robinhood Chain, HyperEVM and Monad, all DISABLED, after Base', async () => {
    const list = await loadDestinations();
    expect(list.map((destination) => destination.id)).toEqual(['base', ...OTHER_IDS]);
    for (const id of OTHER_IDS) {
      const entry = byId(list, id);
      expect(entry).toMatchObject({
        family: 'evm',
        network: 'mainnet',
        status: 'disabled',
        actorCanisterId: null,
        config: null,
        limits: null,
        fees: null,
      });
    }
  });

  it('gives every chain identifier as a STRING', async () => {
    const list = await loadDestinations();
    expect(list.map((destination) => destination.chainId)).toEqual(['8453', '1', '42161', '4663', '999', '143']);
    for (const destination of list) expect(typeof destination.chainId).toBe('string');
  });

  it('explains each one in plain words, with no jargon and no em dash', () => {
    for (const entry of unavailableEvmDestinations()) {
      expect(entry.reason).toBe(`Not available yet: there is no icUSD bridge to ${entry.name} at the moment.`);
      expect(entry.reason).not.toMatch(DASHES);
    }
  });

  it('never enables an entry other than Base', () => {
    const enabled = (list: DestinationView[]) => list.filter((destination) => destination.status === 'enabled').map((destination) => destination.id);
    expect(enabled(buildDestinations(resolveBridgeConfig(makeBridgeInfo()), BRIDGE_ID))).toEqual(['base']);
    expect(enabled(unreachableDestinations('x'))).toEqual([]);
    // A canister that reports another chain id to the Base page does not enable that chain's entry either.
    expect(enabled(buildDestinations(resolveBridgeConfig(makeBridgeInfo({ chain_id: 143n })), BRIDGE_ID))).toEqual([]);
  });
});

describe('when the bridge cannot be reached at all', () => {
  it('shows every destination unavailable, with the reason on Base', () => {
    const list = unreachableDestinations('replica offline');
    expect(list.map((destination) => destination.status)).toEqual(Array(6).fill('disabled'));
    expect(byId(list, 'base').reason).toBe('The bridge could not be reached: replica offline');
  });
});

describe('selection', () => {
  const list = (): DestinationView[] => buildDestinations(resolveBridgeConfig(makeBridgeInfo()), BRIDGE_ID);

  function twoEnabled(): DestinationView[] {
    // A future state: a second instance exists. The helpers must already skip the disabled entries between.
    const all = list();
    const second: DestinationView = { ...byId(all, 'arbitrum'), status: 'enabled', reason: null };
    return [all[0], all[1], second, ...all.slice(3).filter((destination) => destination.id !== 'arbitrum')];
  }

  it('picks the chosen destination, even once it is disabled, else the first enabled one, else the first', () => {
    const all = list();
    expect(pickSelected(all, null)?.id).toBe('base');
    expect(pickSelected(all, 'monad')?.id).toBe('monad'); // chosen earlier, now disabled: the page says so
    expect(pickSelected(all, 'nope')?.id).toBe('base');
    const none = unreachableDestinations('x');
    expect(pickSelected(none, null)?.id).toBe('base'); // nothing enabled: the first entry explains why
    expect(pickSelected([], null)).toBeNull();
    expect(findDestination(all, 'monad')?.name).toBe('Monad');
    expect(findDestination(all, null)).toBeNull();
  });

  it('moves between ENABLED entries only, so a disabled one is never reached by a key', () => {
    const entries = twoEnabled();
    expect(entries.map((destination) => destination.id)).toEqual(['base', 'ethereum', 'arbitrum', 'robinhood', 'hyperevm', 'monad']);
    expect(nextEnabledDestination(entries, 'base', 'ArrowRight')?.id).toBe('arbitrum');
    expect(nextEnabledDestination(entries, 'base', 'ArrowDown')?.id).toBe('arbitrum');
    expect(nextEnabledDestination(entries, 'arbitrum', 'ArrowRight')?.id).toBe('base');
    expect(nextEnabledDestination(entries, 'base', 'ArrowLeft')?.id).toBe('arbitrum');
    expect(nextEnabledDestination(entries, 'arbitrum', 'ArrowUp')?.id).toBe('base');
    expect(nextEnabledDestination(entries, 'arbitrum', 'Home')?.id).toBe('base');
    expect(nextEnabledDestination(entries, 'base', 'End')?.id).toBe('arbitrum');
  });

  it('stays put with one enabled entry, and returns nothing when none is enabled', () => {
    expect(nextEnabledDestination(list(), 'base', 'ArrowRight')?.id).toBe('base');
    expect(nextEnabledDestination(list(), 'base', 'End')?.id).toBe('base');
    expect(nextEnabledDestination(unreachableDestinations('x'), null, 'ArrowRight')).toBeNull();
    // From a disabled current entry the keys land on the first or last enabled one.
    expect(nextEnabledDestination(list(), 'monad', 'ArrowRight')?.id).toBe('base');
    expect(nextEnabledDestination(list(), 'monad', 'ArrowLeft')?.id).toBe('base');
  });
});

describe('baseDestination', () => {
  it('carries the bridge canister id even when the configuration is unusable', () => {
    const failed = resolveBridgeConfig({ ...makeBridgeInfo(), contract: [] } as unknown as BridgeInfo);
    expect(baseDestination(failed, BRIDGE_ID).actorCanisterId).toBe(BRIDGE_ID);
  });
});
