import { describe, expect, it, vi, beforeEach } from 'vitest';
import { get } from 'svelte/store';

const mocks = vi.hoisted(() => ({
  walletState: { isConnected: true, loading: false, principal: null as any },
  getProtocolStatus: vi.fn(),
  getUserVaults: vi.fn(),
  getVaultById: vi.fn(),
}));

vi.mock('./wallet', () => ({
  walletStore: {
    subscribe(run: (value: typeof mocks.walletState) => void) {
      run(mocks.walletState);
      return () => {};
    },
  },
}));

vi.mock('../services/protocol', () => ({
  protocolService: {
    getProtocolStatus: mocks.getProtocolStatus,
    getUserVaults: mocks.getUserVaults,
    getVaultById: mocks.getVaultById,
  },
  StaleActionSessionError: class StaleActionSessionError extends Error {},
}));

vi.mock('./collateralStore', () => ({ collateralStore: { getCollateralInfo: vi.fn(() => null) } }));

import { vaultStore } from './vaultStore';

describe('vault store action-bound refresh', () => {
  beforeEach(() => {
    vaultStore.reset();
    mocks.walletState = { isConnected: true, loading: false, principal: { toText: () => 'owner-a' } };
    mocks.getProtocolStatus.mockReset();
    mocks.getUserVaults.mockReset();
    mocks.getVaultById.mockReset();
  });

  it('discards a vault list response if the owner session changes during the refresh', async () => {
    let resolveStatus!: (status: { lastIcpRate: number }) => void;
    mocks.getProtocolStatus.mockImplementation(() => new Promise(resolve => { resolveStatus = resolve; }));
    const ctx = { expectedPrincipalText: 'owner-a', assertCurrent: () => mocks.walletState.principal?.toText() === 'owner-a' };
    const pending = vaultStore.loadVaults(true, ctx);

    mocks.walletState = { isConnected: true, loading: false, principal: { toText: () => 'owner-b' } };
    resolveStatus({ lastIcpRate: 1 });

    await expect(pending).rejects.toThrow('Wallet session changed');
    expect(get(vaultStore).vaults).toEqual([]);
    expect(get(vaultStore).isLoading).toBe(false);
    expect(mocks.getUserVaults).not.toHaveBeenCalled();
  });

  it('does not let a stale load clear a newer wallet load indicator', async () => {
    const resolvers: Array<(status: { lastIcpRate: number }) => void> = [];
    mocks.getProtocolStatus.mockImplementation(() => new Promise(resolve => {
      resolvers.push(resolve);
    }));
    const ctxA = { expectedPrincipalText: 'owner-a', assertCurrent: () => mocks.walletState.principal?.toText() === 'owner-a' };
    const pendingA = vaultStore.loadVaults(true, ctxA);

    mocks.walletState = { isConnected: true, loading: false, principal: { toText: () => 'owner-b' } };
    const ctxB = { expectedPrincipalText: 'owner-b', assertCurrent: () => mocks.walletState.principal?.toText() === 'owner-b' };
    const pendingB = vaultStore.loadVaults(true, ctxB);
    expect(get(vaultStore).isLoading).toBe(true);

    resolvers[0]({ lastIcpRate: 1 });
    await expect(pendingA).rejects.toThrow('Wallet session changed');
    expect(get(vaultStore).isLoading).toBe(true);

    mocks.getUserVaults.mockResolvedValue([]);
    resolvers[1]({ lastIcpRate: 1 });
    await expect(pendingB).resolves.toEqual([]);
    expect(get(vaultStore).isLoading).toBe(false);
  });

  it('ignores a stale normal network failure without changing the newer wallet state or cache', async () => {
    const requests: Array<{
      resolve: (status: { lastIcpRate: number }) => void;
      reject: (error: Error) => void;
    }> = [];
    mocks.getProtocolStatus.mockImplementation(() => new Promise((resolve, reject) => {
      requests.push({ resolve, reject });
    }));
    const ctxA = { expectedPrincipalText: 'owner-a', assertCurrent: () => mocks.walletState.principal?.toText() === 'owner-a' };
    const pendingA = vaultStore.loadVaults(true, ctxA);

    mocks.walletState = { isConnected: true, loading: false, principal: { toText: () => 'owner-b' } };
    const ctxB = { expectedPrincipalText: 'owner-b', assertCurrent: () => mocks.walletState.principal?.toText() === 'owner-b' };
    const pendingB = vaultStore.loadVaults(true, ctxB);
    const setItem = vi.spyOn(Storage.prototype, 'setItem');

    requests[0].reject(new Error('temporary network error for owner A'));
    await expect(pendingA).rejects.toThrow('vault refresh ended after its wallet session or request changed');
    expect(get(vaultStore)).toMatchObject({ vaults: [], error: null, isLoading: true });
    expect(setItem).not.toHaveBeenCalled();

    mocks.getUserVaults.mockResolvedValue([]);
    requests[1].resolve({ lastIcpRate: 1 });
    await expect(pendingB).resolves.toEqual([]);
    expect(get(vaultStore)).toMatchObject({ vaults: [], error: null, isLoading: false });
    setItem.mockRestore();
  });
});
