import { afterEach, describe, expect, it, vi } from 'vitest';
import { get } from 'svelte/store';
import { Principal } from '@dfinity/principal';
import { appDataStore } from './appDataStore';
import { beginWalletSessionTransition } from '../services/auth';

const OWNER_A = Principal.fromUint8Array(new Uint8Array([11, 12, 13]));
const OWNER_B = Principal.fromUint8Array(new Uint8Array([21, 22, 23]));
const vault = (vaultId: number) => ({ vaultId }) as any;

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

describe('appDataStore user vault session ownership', () => {
  afterEach(() => {
    appDataStore.clearAll();
    vi.restoreAllMocks();
  });

  it('does not let an old owner success overwrite a newer owner vault request', async () => {
    const oldResponse = deferred<any[]>();
    const newResponse = deferred<any[]>();
    const fetch = vi.spyOn(appDataStore, '_fetchUserVaultsFromAPI')
      .mockImplementationOnce(() => oldResponse.promise)
      .mockImplementationOnce(() => newResponse.promise);

    appDataStore.setWalletState(true, OWNER_A, 101);
    const oldRequest = appDataStore.fetchUserVaults(OWNER_A, true);
    appDataStore.setWalletState(true, OWNER_B, 102);
    const newRequest = appDataStore.fetchUserVaults(OWNER_B, true);

    expect(fetch).toHaveBeenCalledTimes(2);
    expect(get(appDataStore).walletPrincipal?.toText()).toBe(OWNER_B.toText());
    expect(get(appDataStore).userVaultsLoading).toBe(true);

    oldResponse.resolve([vault(1)]);
    await expect(oldRequest).resolves.toEqual([vault(1)]);
    expect(get(appDataStore).userVaults).toEqual([]);
    expect(get(appDataStore).userVaultsLoading).toBe(true);
    expect(get(appDataStore).userVaultsError).toBeNull();
    expect(get(appDataStore).userVaultsLastFetch).toBe(0);

    newResponse.resolve([vault(2)]);
    await expect(newRequest).resolves.toEqual([vault(2)]);
    expect(get(appDataStore).userVaults).toEqual([vault(2)]);
    expect(get(appDataStore).userVaultsLoading).toBe(false);
    expect(get(appDataStore).userVaultsError).toBeNull();
  });

  it('does not let an old same-principal session failure overwrite the reconnected session', async () => {
    const oldResponse = deferred<any[]>();
    const newResponse = deferred<any[]>();
    const fetch = vi.spyOn(appDataStore, '_fetchUserVaultsFromAPI')
      .mockImplementationOnce(() => oldResponse.promise)
      .mockImplementationOnce(() => newResponse.promise);

    appDataStore.setWalletState(true, OWNER_A, 201);
    let oldSessionCurrent = true;
    const oldRequest = appDataStore.fetchUserVaults(OWNER_A, true, {
      assertCurrent: () => oldSessionCurrent,
    });
    const consumeOldFailure = oldRequest.catch(() => undefined);

    oldSessionCurrent = false;
    appDataStore.setWalletState(false, null, 202);
    appDataStore.setWalletState(true, OWNER_A, 203);
    const newRequest = appDataStore.fetchUserVaults(OWNER_A, true, { assertCurrent: () => true });

    expect(fetch).toHaveBeenCalledTimes(2);
    expect(get(appDataStore).userVaultsLoading).toBe(true);
    oldResponse.reject(new Error('retired wallet session failed'));
    await consumeOldFailure;

    expect(get(appDataStore).walletConnected).toBe(true);
    expect(get(appDataStore).walletPrincipal?.toText()).toBe(OWNER_A.toText());
    expect(get(appDataStore).userVaults).toEqual([]);
    expect(get(appDataStore).userVaultsLoading).toBe(true);
    expect(get(appDataStore).userVaultsError).toBeNull();

    newResponse.resolve([vault(3)]);
    await expect(newRequest).resolves.toEqual([vault(3)]);
    expect(get(appDataStore).userVaults).toEqual([vault(3)]);
    expect(get(appDataStore).userVaultsLoading).toBe(false);
    expect(get(appDataStore).userVaultsError).toBeNull();
  });

  it('binds refreshAll user data to its captured session while preserving global refreshes', async () => {
    const oldResponse = deferred<any[]>();
    const fetchVaults = vi.spyOn(appDataStore, '_fetchUserVaultsFromAPI').mockReturnValueOnce(oldResponse.promise);
    const protocol = vi.spyOn(appDataStore, 'fetchProtocolStatus').mockResolvedValue({} as any);
    const totals = vi.spyOn(appDataStore, 'fetchCollateralTotals').mockResolvedValue({});
    const balances = vi.spyOn(appDataStore, 'fetchBalances').mockResolvedValue({ icpBalance: 1n, icusdBalance: 2n });

    appDataStore.setWalletState(true, OWNER_A, 301);
    let capturedSessionCurrent = true;
    const oldRefresh = appDataStore.refreshAll(OWNER_A, { assertCurrent: () => capturedSessionCurrent });
    expect(fetchVaults).toHaveBeenCalledOnce();
    appDataStore.setWalletState(true, OWNER_B, 302);
    capturedSessionCurrent = false;
    oldResponse.resolve([vault(4)]);
    await oldRefresh;

    expect(protocol).toHaveBeenCalledOnce();
    expect(totals).toHaveBeenCalledOnce();
    expect(balances).toHaveBeenCalledOnce();
    expect(get(appDataStore).walletPrincipal?.toText()).toBe(OWNER_B.toText());
    expect(get(appDataStore).userVaults).toEqual([]);
    expect(get(appDataStore).userVaultsLastFetch).toBe(0);
  });

  it('retires unbound user reads synchronously when refreshWallet begins before the old principal is unpublished', async () => {
    const oldVaults = deferred<any[]>();
    const oldBalances = deferred<{ icpBalance: bigint; icusdBalance: bigint }>();
    const fetchVaults = vi.spyOn(appDataStore, '_fetchUserVaultsFromAPI').mockReturnValueOnce(oldVaults.promise);
    const fetchBalances = vi.spyOn(appDataStore, '_fetchBalancesFromAPI').mockReturnValueOnce(oldBalances.promise);

    appDataStore.setWalletState(true, OWNER_A, 401);
    const vaultRequest = appDataStore.fetchUserVaults(OWNER_A, true);
    const balanceRequest = appDataStore.fetchBalances(OWNER_A, true);
    const consumeBalance = balanceRequest.catch(() => undefined);
    expect(fetchVaults).toHaveBeenCalledOnce();
    expect(fetchBalances).toHaveBeenCalledOnce();
    expect(get(appDataStore).userVaultsLoading).toBe(true);
    expect(get(appDataStore).balancesLoading).toBe(true);

    // This is the synchronous start of refreshWallet's disconnect/reconnect saga.
    beginWalletSessionTransition();
    expect(get(appDataStore).walletConnected).toBe(true);
    expect(get(appDataStore).walletPrincipal?.toText()).toBe(OWNER_A.toText());
    expect(get(appDataStore).userVaultsLoading).toBe(false);
    expect(get(appDataStore).balancesLoading).toBe(false);

    oldVaults.resolve([vault(99)]);
    oldBalances.resolve({ icpBalance: 99n, icusdBalance: 100n });
    await expect(vaultRequest).resolves.toEqual([vault(99)]);
    await consumeBalance;

    const state = get(appDataStore);
    expect(state.walletConnected).toBe(true);
    expect(state.walletPrincipal?.toText()).toBe(OWNER_A.toText());
    expect(state.userVaults).toEqual([]);
    expect(state.userVaultsLoading).toBe(false);
    expect(state.userVaultsError).toBeNull();
    expect(state.userVaultsLastFetch).toBe(0);
    expect(state.icpBalance).toBeNull();
    expect(state.icusdBalance).toBeNull();
    expect(state.balancesLoading).toBe(false);
    expect(state.balancesError).toBeNull();
    expect(state.balancesLastFetch).toBe(0);
  });
});
