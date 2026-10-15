import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { mount, tick, unmount } from 'svelte';
import type { Writable } from 'svelte/store';
import { Principal } from '@dfinity/principal';

const calls = vi.hoisted(() => ({ createActor: vi.fn(), load: vi.fn(), goto: vi.fn() }));
vi.mock('$app/navigation', () => ({ goto: calls.goto }));
vi.mock('$lib/stores/wallet', async () => {
  const { writable } = await import('svelte/store');
  return { walletStore: writable({ isConnected: false, principal: null }) };
});
vi.mock('$lib/services/auth', async () => {
  const { writable } = await import('svelte/store');
  return {
    currentWalletType: writable('oisy'), walletSessionGeneration: writable(0),
    auth: {}, WALLET_TYPES: { OISY: 'oisy' },
  };
});
vi.mock('$lib/services/pnp', () => ({ pnp: { openChannel: vi.fn() } }));
vi.mock('$lib/config', () => ({
  CONFIG: { isLocal: false },
  CANISTER_IDS: { CYCLE_SENTINEL: 'joh3a-5aaaa-aaaap-quy6a-cai' },
  isCycleSentinelConfigured: true,
}));
vi.mock('$lib/services/cycleSentinelService', async (importOriginal) => ({
  ...await importOriginal<typeof import('$lib/services/cycleSentinelService')>(),
  createAuthenticatedSentinelActor: calls.createActor,
  loadOperatorDashboard: calls.load,
}));

import { walletStore } from '$lib/stores/wallet';
import TelemetryPage from './+page.svelte';

const wallet = walletStore as unknown as Writable<{ isConnected: boolean; principal: Principal | null }>;
const approved = Principal.fromText('zegjz-jpi6k-qkand-c2bgf-qw6za-xk4si-nz3gx-qzzia-fk6fg-snepb-tae');
let page: ReturnType<typeof mount>;
let target: HTMLDivElement;

beforeEach(() => {
  vi.clearAllMocks();
  localStorage.clear();
  sessionStorage.clear();
  wallet.set({ isConnected: false, principal: null });
  calls.createActor.mockResolvedValue({});
  calls.load.mockRejectedValue(new Error('Signer window could not be opened'));
  target = document.createElement('div');
  document.body.append(target);
});
afterEach(async () => { if (page) await unmount(page); target.remove(); });

describe('Sentinel telemetry wallet restoration', () => {
  it('does not issue a stale private read when the wallet disconnects while opening', async () => {
    let finishOpening!: (actor: object) => void;
    calls.createActor.mockImplementation(() => new Promise(resolve => { finishOpening = resolve; }));
    wallet.set({ isConnected: true, principal: approved });
    page = mount(TelemetryPage, { target });
    await tick();
    [...target.querySelectorAll('button')].find(button => button.textContent?.includes('Load telemetry'))!.click();
    wallet.set({ isConnected: false, principal: null });
    await tick();
    finishOpening({});
    await tick();
    expect(calls.load).not.toHaveBeenCalled();
    expect(target.textContent).toContain('Connect an approved wallet');
  });

  it('stays on the route during restoration and waits for a deliberate read click', async () => {
    page = mount(TelemetryPage, { target });
    await tick();
    expect(calls.goto).not.toHaveBeenCalled();
    wallet.set({ isConnected: true, principal: approved });
    await tick();
    expect(calls.createActor).not.toHaveBeenCalled();
    expect(calls.load).not.toHaveBeenCalled();
    const load = [...target.querySelectorAll('button')].find(button => button.textContent?.includes('Load telemetry'));
    expect(load).toBeDefined();
    load!.click();
    await vi.waitFor(() => expect(calls.load).toHaveBeenCalledTimes(1));
    await tick();
    expect(target.textContent).toContain('Signer window could not be opened');
    expect(target.textContent).not.toContain('unavailable to this identity');
    expect([...target.querySelectorAll('button')].some(button => button.textContent?.includes('Load telemetry'))).toBe(true);
  });
});
