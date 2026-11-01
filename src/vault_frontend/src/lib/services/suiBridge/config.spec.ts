import { readFileSync } from 'node:fs';
import path from 'node:path';
import { describe, expect, it, vi } from 'vitest';
import { loadDestinations } from '../baseBridge/destinationLoader';
import {
  buildDestinations,
  findDestination,
  isEnabled,
  nextEnabledDestination,
  pickSelected,
  suiDestination,
  unavailableNativeDestinations,
  unreachableDestinations,
  unreachableSuiDestination,
  withSuiDestination,
} from '../baseBridge/destinations';
import { resolveBridgeConfig } from '../baseBridge/chains';
import { makeBridgeInfo } from '../baseBridge/testSupport';
import { parseProfile } from '../baseBridge/profiles';
import { resolveSuiConfig } from './config';
import { SUI_MAINNET, SUI_NETWORKS, SUI_TESTNET, suiNetworkByChainIdentifier } from './networks';
import { KEYS, SUI_BRIDGE_ID, makeLane, makeObserved, makeRoles, makeSuiConfig, makeSuiConfigView } from './testSupport';
import { getSuiBridgeCanisterId, loadSuiDestination } from './suiActor';

vi.mock('$lib/config', () => ({
  CANISTER_IDS: { ICUSD_BRIDGE: 'tfesu-vyaaa-aaaap-qrd7a-cai' },
  CONFIG: { isLocal: false, host: 'https://icp0.io', currentIcusdLedgerId: 't6bor-paaaa-aaaap-qrd5q-cai' },
}));

describe('the network table follows the destination profile', () => {
  it('has the same networks and chain identifiers as src/rumi_bridge_destinations/profiles/sui.json', () => {
    const file = path.resolve(__dirname, '../../../../../rumi_bridge_destinations/profiles/sui.json');
    const profile = parseProfile(JSON.parse(readFileSync(file, 'utf8')));
    expect(profile.family).toBe('sui');
    expect(SUI_NETWORKS.map((n) => [n.networkId, n.chainIdentifierBase58])).toEqual(profile.networks.map((n) => [n.networkId, n.identity.value]));
    // Every native profile network stays disabled: nothing here enables a destination.
    expect(profile.networks.every((n) => n.status === 'disabled' && n.instanceRef === null)).toBe(true);
  });

  it('knows only full digests: a four byte prefix or an unknown digest is not a network', () => {
    expect(suiNetworkByChainIdentifier(SUI_MAINNET.chainIdentifier)).toBe(SUI_MAINNET);
    expect(suiNetworkByChainIdentifier(SUI_TESTNET.chainIdentifier)).toBe(SUI_TESTNET);
    expect(suiNetworkByChainIdentifier('35834a8a')).toBeNull();
    expect(suiNetworkByChainIdentifier('00'.repeat(32))).toBeNull();
  });
});

describe('resolving the instance configuration (identity availability)', () => {
  const resolve = (config = makeSuiConfigView(), observed: ReturnType<typeof makeObserved> | null = makeObserved(), roles = makeRoles(), lane = makeLane()) => resolveSuiConfig(config, roles, lane, observed, SUI_BRIDGE_ID);

  it('resolves an initialized, bound, observed instance into an enabled native config', () => {
    const result = resolve();
    if (!result.ok) throw new Error(result.message);
    expect(result.config).toMatchObject({
      family: 'sui',
      instanceCanisterId: SUI_BRIDGE_ID,
      network: SUI_MAINNET,
      packageId: KEYS.packageId,
      gateId: KEYS.gateId,
      gateInitialSharedVersion: '42',
      coinType: `${KEYS.packageId}::rumi_icusd_gate::RUMI_ICUSD_GATE`,
      mintCapId: KEYS.mintCap,
      adminCapId: KEYS.adminCap,
      roleAddresses: [KEYS.treasury, KEYS.gas, KEYS.custody],
      icusdLedger: 't6bor-paaaa-aaaap-qrd5q-cai',
      minDepositE8s: 100_000_000n,
      maxDepositE8s: 1_000_000_000n,
      minBridgeOutE8s: 1n,
      depositFeeE8s: 10_000_000n,
      releaseFeeE8s: 20_000n,
      halted: null,
      mintLaneReady: true,
      mintLaneBlockedBy: [],
    });
    expect(makeSuiConfig({ network: SUI_TESTNET }).network).toBe(SUI_TESTNET);
  });

  it('enables from the REAL accepted canister response, where module_digest_observed is false (MC-DEF-13), and discloses it', () => {
    // The accepted backend (bind.rs at 29295fb4) ALWAYS stores false: the published module bytes are not compared with a build.
    // The fixtures use that real value; a page that needed `true` could never be enabled by the real canister.
    expect(makeObserved().module_digest_observed).toBe(false);
    const real = resolve();
    if (!real.ok) throw new Error(real.message);
    expect(real.config.moduleBytesMatched).toBe(false);
    expect(real.config).toMatchObject({ packageId: KEYS.packageId, gateId: KEYS.gateId, mintLaneReady: true });
    // A canister that one day does match the module bytes is reported as such; it is disclosure, not a gate.
    const matched = resolve(makeSuiConfigView(), makeObserved({ module_digest_observed: true }));
    if (!matched.ok) throw new Error(matched.message);
    expect(matched.config.moduleBytesMatched).toBe(true);
  });

  it('still refuses every real identity gap with the real (false) module flag', () => {
    const real = (observed: Parameters<typeof makeObserved>[0]) => resolve(makeSuiConfigView(), makeObserved({ module_digest_observed: false, ...observed }));
    expect(real({ decimals: 9 })).toMatchObject({ ok: false });
    expect(real({ mint_cap_id: KEYS.adminCap })).toMatchObject({ ok: false });
    expect(real({ admin_cap_id: KEYS.mintCap })).toMatchObject({ ok: false });
    expect(real({ initial_shared_version: 0n })).toMatchObject({ ok: false });
    expect(resolve(makeSuiConfigView(), null)).toMatchObject({ ok: false });
    expect(resolve(makeSuiConfigView({ chain_identifier: '00'.repeat(32) }), makeObserved())).toMatchObject({ ok: false });
    expect(resolve(makeSuiConfigView(), makeObserved(), makeRoles({ Treasury: null }))).toMatchObject({ ok: false });
  });

  it('never reads the legacy signing_enabled flag: it is not a readiness signal either way', () => {
    for (const flag of [true, false]) {
      const result = resolve(makeSuiConfigView({ signing_enabled: flag }));
      expect(result.ok, String(flag)).toBe(true);
      expect(JSON.stringify(result, (_k, v) => (typeof v === 'bigint' ? v.toString() : v))).not.toContain('signing');
    }
  });

  it('keeps a direction pause or halt apart from identity: the destination stays available', () => {
    const paused = resolve(makeSuiConfigView({ pauses: { mints: true, observer: true, releases: true, deposits: true }, halted: ['stop everything'] }));
    if (!paused.ok) throw new Error(paused.message);
    expect(paused.config).toMatchObject({ depositsPaused: true, mintsPaused: true, releasesPaused: true, halted: 'stop everything' });
  });

  it('carries the mint lane\'s own readiness and reasons, without disabling the destination', () => {
    const blocked = resolve(makeSuiConfigView(), makeObserved(), makeRoles(), makeLane({ ready: false, blocked_by: ['mints are paused', 'no gas coin is configured'] }));
    if (!blocked.ok) throw new Error(blocked.message);
    expect(blocked.config).toMatchObject({ mintLaneReady: false, mintLaneBlockedBy: ['mints are paused', 'no gas coin is configured'] });
  });

  it('refuses, with a reason, every identity gap: not set up, unknown network, mismatched network, no binding', () => {
    expect(resolve(makeSuiConfigView({ initialized: false }))).toMatchObject({ ok: false, message: 'The Sui bridge is not set up yet.' });
    expect(resolve(makeSuiConfigView({ chain_identifier: '00'.repeat(32) }))).toMatchObject({ ok: false, network: null, message: expect.stringContaining('network this app does not know') });
    expect(resolve(makeSuiConfigView({ chain_identifier: '35834a8a' }))).toMatchObject({ ok: false, network: null });
    expect(resolve(makeSuiConfigView({ network: { Testnet: null } }))).toMatchObject({ ok: false, message: expect.stringContaining('does not match its chain identifier') });
    expect(resolve(makeSuiConfigView({ binding: [] }))).toMatchObject({ ok: false, message: expect.stringContaining('no icUSD Gate bound') });
  });

  it('refuses a binding that is not the exact package, Gate, 8-decimal coin and caps', () => {
    const binding = makeSuiConfigView().binding[0]!;
    const withBinding = (patch: Partial<typeof binding>) => resolve(makeSuiConfigView({ binding: [{ ...binding, ...patch }] }));
    expect(withBinding({ package_id: '0x2' })).toMatchObject({ ok: false, message: expect.stringContaining('package id is not a Sui address in strict form') });
    expect(withBinding({ gate_id: `0x${'A'.repeat(64)}` })).toMatchObject({ ok: false });
    expect(withBinding({ decimals: 9 })).toMatchObject({ ok: false, message: expect.stringContaining('8 decimals') });
    expect(withBinding({ coin_type: '0x2::sui::SUI' })).toMatchObject({ ok: false, message: expect.stringContaining('do not belong to its package') });
    expect(withBinding({ gate_type: `${KEYS.gateId}::rumi_icusd_gate::Gate` })).toMatchObject({ ok: false });
  });

  it('refuses when the independent observation is missing or disagrees with the binding', () => {
    expect(resolve(makeSuiConfigView(), null)).toMatchObject({ ok: false, message: expect.stringContaining('not yet observed') });
    expect(resolve(makeSuiConfigView(), makeObserved({ decimals: 9 }))).toMatchObject({ ok: false });
    expect(resolve(makeSuiConfigView(), makeObserved({ mint_cap_id: KEYS.adminCap }))).toMatchObject({ ok: false, message: expect.stringContaining('differs from the one it is bound to') });
    expect(resolve(makeSuiConfigView(), makeObserved({ initial_shared_version: 0n }))).toMatchObject({ ok: false });
  });

  it('refuses when a signing role has no address yet', () => {
    expect(resolve(makeSuiConfigView(), makeObserved(), makeRoles({ Gas: null }))).toMatchObject({ ok: false, message: expect.stringContaining('signing role') });
    expect(resolve(makeSuiConfigView(), makeObserved(), makeRoles().slice(0, 2))).toMatchObject({ ok: false });
  });
});

describe('the destination list', () => {
  const base = () => resolveBridgeConfig(makeBridgeInfo());

  it('shows Sui as a static, DISABLED row with no actor, and never selects it', () => {
    const list = buildDestinations(base(), 'tfesu-vyaaa-aaaap-qrd7a-cai');
    const sui = findDestination(list, 'sui-mainnet')!;
    expect(sui).toMatchObject({ family: 'sui', profileId: 'sui', status: 'disabled', actorCanisterId: null, config: null, native: null, limits: null, fees: null, chainId: SUI_MAINNET.chainIdentifier });
    expect(sui.reason).toBe('Not available yet: there is no icUSD bridge to Sui at the moment.');
    expect(pickSelected(list, 'sui-mainnet')?.id).toBe('sui-mainnet');
    expect(nextEnabledDestination(list, 'base-mainnet', 'End')?.id).toBe('base-mainnet');
    expect(unavailableNativeDestinations().map((d) => d.id)).toEqual(['solana-mainnet', 'sui-mainnet']);
  });

  it('builds an enabled entry only from the canister\'s own passing answer, and a refusal is disabled with its reason', () => {
    const ok = suiDestination({ ok: true, config: makeSuiConfig() }, SUI_BRIDGE_ID);
    expect(ok).toMatchObject({ id: 'sui-mainnet', family: 'sui', status: 'enabled', reason: null, actorCanisterId: SUI_BRIDGE_ID, config: null });
    expect(ok.native?.family === 'sui' ? ok.native.packageId : null).toBe(KEYS.packageId);
    expect(ok.limits).toEqual({ minDepositE8s: 100_000_000n, maxDepositE8s: 1_000_000_000n, minBridgeOutE8s: 1n });
    const refused = suiDestination({ ok: false, message: 'The Sui bridge is not set up yet.', network: SUI_MAINNET }, SUI_BRIDGE_ID);
    expect(refused).toMatchObject({ status: 'disabled', native: null, reason: 'The Sui bridge is not set up yet.' });
    expect(unreachableSuiDestination('replica offline', SUI_BRIDGE_ID).reason).toBe('The Sui bridge could not be reached: replica offline');
  });

  it('replaces the static row of the same network, and a paused direction does not make the row unavailable', () => {
    const list = buildDestinations(base(), 'tfesu-vyaaa-aaaap-qrd7a-cai');
    const paused = suiDestination({ ok: true, config: makeSuiConfig({ config: { pauses: { mints: true, observer: false, releases: true, deposits: true } } }) }, SUI_BRIDGE_ID);
    const replaced = withSuiDestination(list, paused);
    expect(replaced.map((d) => d.id)).toEqual(list.map((d) => d.id));
    expect(isEnabled(findDestination(replaced, 'sui-mainnet')!)).toBe(true);
    const testnet = suiDestination({ ok: false, message: 'x', network: SUI_TESTNET }, SUI_BRIDGE_ID);
    expect(withSuiDestination(list, testnet).map((d) => d.id)).toEqual([...list.map((d) => d.id), 'sui-testnet']);
    expect(unreachableDestinations('offline').map((d) => d.id)).toContain('sui-mainnet');
  });
});

describe('reading the destination list', () => {
  const base = () => resolveBridgeConfig(makeBridgeInfo());
  const enabled = () => suiDestination({ ok: true, config: makeSuiConfig() }, SUI_BRIDGE_ID);

  it('makes no Sui request at all while no instance is configured', async () => {
    const fetchSpy = vi.spyOn(globalThis, 'fetch');
    expect(getSuiBridgeCanisterId()).toBeNull();
    expect(await loadSuiDestination()).toBeNull();
    const list = await loadDestinations(async () => base());
    expect(findDestination(list, 'sui-mainnet')).toMatchObject({ status: 'disabled', native: null, actorCanisterId: null });
    expect(fetchSpy).not.toHaveBeenCalled();
    fetchSpy.mockRestore();
  });

  it('loads an injected enabled instance, keeps Base untouched, and survives a failing Sui read', async () => {
    const list = await loadDestinations(async () => base(), async () => null, async () => enabled());
    expect(findDestination(list, 'sui-mainnet')?.status).toBe('enabled');
    expect(findDestination(list, 'base-mainnet')?.status).toBe('enabled');
    const failing = await loadDestinations(async () => base(), async () => null, async () => {
      throw new Error('replica offline');
    });
    expect(findDestination(failing, 'sui-mainnet')).toMatchObject({ status: 'disabled' });
    expect(findDestination(failing, 'base-mainnet')?.status).toBe('enabled');
  });

  it('keeps a failing Solana read from hiding Sui, and a failing Sui read from hiding Solana', async () => {
    const list = await loadDestinations(
      async () => base(),
      async () => {
        throw new Error('sol down');
      },
      async () => enabled(),
    );
    expect(findDestination(list, 'sui-mainnet')?.status).toBe('enabled');
    expect(findDestination(list, 'solana-mainnet')?.status).toBe('disabled');
  });

  it('keeps a usable Sui entry when Base is UNREACHABLE, with the Base row unavailable and the reason shown', async () => {
    const list = await loadDestinations(
      async () => {
        throw new Error('replica offline');
      },
      async () => null,
      async () => enabled(),
    );
    expect(findDestination(list, 'sui-mainnet')?.status).toBe('enabled');
    const unreachable = list[0];
    expect(unreachable).toMatchObject({ status: 'disabled', reason: 'The bridge could not be reached: replica offline' });
    expect(pickSelected(list, null)?.id).toBe('sui-mainnet');
  });

  it('still rejects, exactly as before, when Base is unreachable and no native instance answers', async () => {
    await expect(
      loadDestinations(async () => {
        throw new Error('replica offline');
      }),
    ).rejects.toThrow('replica offline');
  });

  it('does not wait for Base before reading the native instances', async () => {
    const order: string[] = [];
    let releaseBase!: () => void;
    const slowBase = new Promise<ReturnType<typeof base>>((resolve) => {
      releaseBase = () => resolve(base());
    });
    const pending = loadDestinations(
      () => {
        order.push('base started');
        return slowBase;
      },
      async () => null,
      async () => {
        order.push('sui read');
        return enabled();
      },
    );
    await Promise.resolve();
    await Promise.resolve();
    expect(order).toEqual(['base started', 'sui read']);
    releaseBase();
    expect(findDestination(await pending, 'sui-mainnet')?.status).toBe('enabled');
  });
});

describe('one unfinished read never gates another family (fake clock, no network)', () => {
  const base = () => resolveBridgeConfig(makeBridgeInfo());
  const enabled = () => suiDestination({ ok: true, config: makeSuiConfig() }, SUI_BRIDGE_ID);
  const never = <T>() => new Promise<T>(() => {});
  /** Far beyond any sensible bound; the loader must answer well inside it. */
  const BOUND_MS = 10_000;

  async function race<T>(promise: Promise<T>) {
    let settled: { value: T } | null = null;
    void promise.then((value) => (settled = { value }));
    await vi.advanceTimersByTimeAsync(BOUND_MS);
    return settled as { value: T } | null;
  }

  it('shows a healthy Sui entry within a bounded time when a configured Base read NEVER settles, and the Base row says why', async () => {
    vi.useFakeTimers();
    try {
      const done = await race(loadDestinations(() => never(), async () => null, async () => enabled()));
      expect(done, 'loadDestinations must not wait for a read that never settles').not.toBeNull();
      const list = done!.value;
      expect(findDestination(list, 'sui-mainnet')?.status).toBe('enabled');
      expect(list[0]).toMatchObject({ status: 'disabled' });
      expect(list[0].reason).toContain('did not answer');
      expect(pickSelected(list, null)?.id).toBe('sui-mainnet');
    } finally {
      vi.useRealTimers();
    }
  });

  it('shows a healthy Sui entry when a configured Solana read NEVER settles, and the Solana row says why', async () => {
    vi.useFakeTimers();
    try {
      const done = await race(loadDestinations(async () => base(), () => never(), async () => enabled()));
      expect(done, 'loadDestinations must not wait for a read that never settles').not.toBeNull();
      const list = done!.value;
      expect(findDestination(list, 'sui-mainnet')?.status).toBe('enabled');
      expect(findDestination(list, 'base-mainnet')?.status).toBe('enabled');
      expect(findDestination(list, 'solana-mainnet')).toMatchObject({ status: 'disabled' });
      expect(findDestination(list, 'solana-mainnet')!.reason).toContain('did not answer');
    } finally {
      vi.useRealTimers();
    }
  });

  it('keeps a stalled Sui read local too: Base stays usable and the Sui row says why', async () => {
    vi.useFakeTimers();
    try {
      const done = await race(loadDestinations(async () => base(), async () => null, () => never()));
      const list = done!.value;
      expect(findDestination(list, 'base-mainnet')?.status).toBe('enabled');
      expect(findDestination(list, 'sui-mainnet')).toMatchObject({ status: 'disabled', native: null });
      expect(findDestination(list, 'sui-mainnet')!.reason).toContain('did not answer');
    } finally {
      vi.useRealTimers();
    }
  });

  it('does not start a timer or wait when every read answers, and Base-only behavior is unchanged (a slow Base is awaited, not timed out)', async () => {
    vi.useFakeTimers();
    try {
      const quick = await loadDestinations(async () => base(), async () => null, async () => enabled());
      expect(findDestination(quick, 'sui-mainnet')?.status).toBe('enabled');
      expect(vi.getTimerCount()).toBe(0);
      // Only Base is configured (both natives answer null): no deadline applies, however slow Base is.
      let release!: () => void;
      const slow = new Promise<ReturnType<typeof base>>((resolve) => (release = () => resolve(base())));
      let settled = false;
      const pending = loadDestinations(() => slow, async () => null, async () => null).then((list) => ((settled = true), list));
      await vi.advanceTimersByTimeAsync(BOUND_MS * 6);
      expect(settled).toBe(false);
      release();
      await vi.advanceTimersByTimeAsync(1);
      expect(findDestination(await pending, 'base-mainnet')?.status).toBe('enabled');
      expect(vi.getTimerCount()).toBe(0);
    } finally {
      vi.useRealTimers();
    }
  });
});
