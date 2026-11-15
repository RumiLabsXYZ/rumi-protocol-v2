import { afterEach, describe, expect, it, vi } from 'vitest';
import { get } from 'svelte/store';
import { Principal } from '@dfinity/principal';
import { appDataStore } from './appDataStore';

const OWNER_A = Principal.fromUint8Array(new Uint8Array([1, 2, 3, 4, 5]));
const OWNER_B = Principal.fromUint8Array(new Uint8Array([6, 7, 8, 9, 10]));

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

describe('appDataStore balance session ownership', () => {
  afterEach(() => {
    appDataStore.clearAll();
    vi.restoreAllMocks();
  });

  it('ignores an old wallet network failure without clearing the new wallet loading state', async () => {
    const oldResponse = deferred<{ icpBalance: bigint; icusdBalance: bigint }>();
    const newResponse = deferred<{ icpBalance: bigint; icusdBalance: bigint }>();
    const fetch = vi.spyOn(appDataStore, '_fetchBalancesFromAPI')
      .mockImplementationOnce(() => oldResponse.promise)
      .mockImplementationOnce(() => newResponse.promise);

    appDataStore.setWalletState(true, OWNER_A);
    let ownerAIsCurrent = true;
    const ownerARequest = appDataStore.fetchBalances(
      OWNER_A, true, undefined, () => ownerAIsCurrent,
    );
    const ignoreOldFailure = ownerARequest.catch(() => undefined);

    ownerAIsCurrent = false;
    appDataStore.setWalletState(true, OWNER_B);
    let ownerBIsCurrent = true;
    const ownerBRequest = appDataStore.fetchBalances(
      OWNER_B, true, undefined, () => ownerBIsCurrent,
    );

    expect(get(appDataStore).balancesLoading).toBe(true);
    expect(get(appDataStore).walletPrincipal?.toText()).toBe(OWNER_B.toText());
    expect(fetch).toHaveBeenCalledTimes(2);

    oldResponse.reject(new Error('old wallet endpoint failed'));
    await ignoreOldFailure;

    expect(get(appDataStore).balancesLoading).toBe(true);
    expect(get(appDataStore).balancesError).toBeNull();
    expect(get(appDataStore).icpBalance).toBeNull();
    expect(get(appDataStore).walletPrincipal?.toText()).toBe(OWNER_B.toText());

    newResponse.resolve({ icpBalance: 22n, icusdBalance: 33n });
    await expect(ownerBRequest).resolves.toEqual({ icpBalance: 22n, icusdBalance: 33n });
    expect(get(appDataStore).balancesLoading).toBe(false);
    expect(get(appDataStore).balancesError).toBeNull();
    expect(get(appDataStore).icpBalance).toBe(22n);
    expect(get(appDataStore).icusdBalance).toBe(33n);
    ownerBIsCurrent = false;
  });

  it('does not cache a late successful balance response after the owner changes', async () => {
    const oldResponse = deferred<{ icpBalance: bigint; icusdBalance: bigint }>();
    const fetch = vi.spyOn(appDataStore, '_fetchBalancesFromAPI').mockReturnValueOnce(oldResponse.promise);

    appDataStore.setWalletState(true, OWNER_A);
    let ownerAIsCurrent = true;
    const ownerARequest = appDataStore.fetchBalances(
      OWNER_A, true, undefined, () => ownerAIsCurrent,
    );
    ownerAIsCurrent = false;
    appDataStore.setWalletState(true, OWNER_B);

    oldResponse.resolve({ icpBalance: 11n, icusdBalance: 12n });
    await expect(ownerARequest).resolves.toEqual({ icpBalance: 11n, icusdBalance: 12n });

    expect(fetch).toHaveBeenCalledTimes(1);
    expect(get(appDataStore).walletPrincipal?.toText()).toBe(OWNER_B.toText());
    expect(get(appDataStore).icpBalance).toBeNull();
    expect(get(appDataStore).icusdBalance).toBeNull();
    expect(get(appDataStore).balancesLastFetch).toBe(0);
    expect(get(appDataStore).balancesLoading).toBe(false);
  });

  it('does not join a new same-principal session to the retired request or lose its loading marker', async () => {
    const oldResponse = deferred<{ icpBalance: bigint; icusdBalance: bigint }>();
    const newResponse = deferred<{ icpBalance: bigint; icusdBalance: bigint }>();
    const fetch = vi.spyOn(appDataStore, '_fetchBalancesFromAPI')
      .mockImplementationOnce(() => oldResponse.promise)
      .mockImplementationOnce(() => newResponse.promise);

    appDataStore.setWalletState(true, OWNER_A, 101);
    let oldSessionCurrent = true;
    const oldRequest = appDataStore.fetchBalances(OWNER_A, true, undefined, () => oldSessionCurrent);
    const consumeOldResult = oldRequest.catch(() => undefined);

    // Disconnect/reconnect to the identical principal creates a new session scope.
    oldSessionCurrent = false;
    appDataStore.setWalletState(false, null, 102);
    appDataStore.setWalletState(true, OWNER_A, 103);
    const newRequest = appDataStore.fetchBalances(OWNER_A, true, undefined, () => true);

    expect(fetch).toHaveBeenCalledTimes(2);
    expect(get(appDataStore).balancesLoading).toBe(true);

    oldResponse.resolve({ icpBalance: 7n, icusdBalance: 8n });
    await consumeOldResult;
    expect(get(appDataStore).balancesLoading).toBe(true);
    expect(get(appDataStore).icpBalance).toBeNull();
    expect(get(appDataStore).icusdBalance).toBeNull();

    newResponse.resolve({ icpBalance: 70n, icusdBalance: 80n });
    await expect(newRequest).resolves.toEqual({ icpBalance: 70n, icusdBalance: 80n });
    expect(get(appDataStore).balancesLoading).toBe(false);
    expect(get(appDataStore).icpBalance).toBe(70n);
    expect(get(appDataStore).icusdBalance).toBe(80n);
  });

  it('ignores an old same-principal session rejection without erasing the new request marker', async () => {
    const oldResponse = deferred<{ icpBalance: bigint; icusdBalance: bigint }>();
    const newResponse = deferred<{ icpBalance: bigint; icusdBalance: bigint }>();
    const fetch = vi.spyOn(appDataStore, '_fetchBalancesFromAPI')
      .mockImplementationOnce(() => oldResponse.promise)
      .mockImplementationOnce(() => newResponse.promise);

    appDataStore.setWalletState(true, OWNER_A, 201);
    let oldSessionCurrent = true;
    const oldRequest = appDataStore.fetchBalances(OWNER_A, true, undefined, () => oldSessionCurrent);
    const consumeOldFailure = oldRequest.catch(() => undefined);
    oldSessionCurrent = false;
    appDataStore.setWalletState(false, null, 202);
    appDataStore.setWalletState(true, OWNER_A, 203);
    const newRequest = appDataStore.fetchBalances(OWNER_A, true, undefined, () => true);

    expect(fetch).toHaveBeenCalledTimes(2);
    oldResponse.reject(new Error('retired session network failure'));
    await consumeOldFailure;
    expect(get(appDataStore).balancesLoading).toBe(true);
    expect(get(appDataStore).balancesError).toBeNull();
    expect(get(appDataStore).icpBalance).toBeNull();

    newResponse.resolve({ icpBalance: 90n, icusdBalance: 100n });
    await expect(newRequest).resolves.toEqual({ icpBalance: 90n, icusdBalance: 100n });
    expect(get(appDataStore).balancesLoading).toBe(false);
    expect(get(appDataStore).icpBalance).toBe(90n);
    expect(get(appDataStore).icusdBalance).toBe(100n);
  });

  it('keeps same-session unbound balance requests deduplicated', async () => {
    const response = deferred<{ icpBalance: bigint; icusdBalance: bigint }>();
    const fetch = vi.spyOn(appDataStore, '_fetchBalancesFromAPI').mockReturnValueOnce(response.promise);
    appDataStore.setWalletState(true, OWNER_A, 301);

    const first = appDataStore.fetchBalances(OWNER_A, true);
    const second = appDataStore.fetchBalances(OWNER_A, true);
    expect(fetch).toHaveBeenCalledTimes(1);

    response.resolve({ icpBalance: 3n, icusdBalance: 4n });
    await expect(first).resolves.toEqual({ icpBalance: 3n, icusdBalance: 4n });
    await expect(second).resolves.toEqual({ icpBalance: 3n, icusdBalance: 4n });
  });
});
