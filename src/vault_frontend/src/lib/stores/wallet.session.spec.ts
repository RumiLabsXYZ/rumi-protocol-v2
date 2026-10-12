import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { writable, get } from 'svelte/store';
import { Principal } from '@dfinity/principal';

// ──────────────────────────────────────────────────────────────
// Regression coverage for the *user-facing* walletStore — the store
// WalletConnector.svelte actually renders from. A valid persisted auth
// session must become visible here without waiting on (or being reverted
// by) balance/price/protocol-status network calls.
// ──────────────────────────────────────────────────────────────

// Deterministic non-anonymous synthetic principal (the anonymous principal
// would let auth-not-actually-invoked bugs pass these assertions silently).
const TEST_PRINCIPAL = Principal.fromUint8Array(new Uint8Array([1, 2, 3, 4, 5]));

// vi.mock factories are hoisted above all top-level imports, so the fake
// `auth` store must be built from primitives inside vi.hoisted rather than
// via the real `writable()` from 'svelte/store'.
const mocks = vi.hoisted(() => {
  type AuthState = {
    isConnected: boolean;
    account: { owner: unknown; balance: bigint } | null;
    isInitialized: boolean;
    walletType: string | null;
  };
  let authState: AuthState = {
    isConnected: false,
    account: null,
    isInitialized: false,
    walletType: null,
  };
  const subscribers = new Set<(v: AuthState) => void>();
  const authStore = {
    subscribe(fn: (v: AuthState) => void) {
      fn(authState);
      subscribers.add(fn);
      return () => subscribers.delete(fn);
    },
    set(v: AuthState) {
      authState = v;
      subscribers.forEach((fn) => fn(authState));
    },
  };

  return {
    authStore,
    authInitialize: vi.fn(),
    authConnect: vi.fn(),
    authDisconnect: vi.fn(),
    beginWalletSessionTransition: vi.fn(),
    pnpConnect: vi.fn(),
    pnpDisconnect: vi.fn(),
    setWalletState: vi.fn(),
    fetchBalances: vi.fn(),
    fetchProtocolStatus: vi.fn(),
    getTokenBalance: vi.fn(),
    getThreeUsdPrice: vi.fn(),
    fetchSupportedCollateral: vi.fn(),
    clearVaultCache: vi.fn(),
  };
});

vi.mock('../services/auth', () => ({
  auth: {
    ...mocks.authStore,
    initialize: mocks.authInitialize,
    connect: mocks.authConnect,
    disconnect: mocks.authDisconnect,
  },
  WALLET_TYPES: { PLUG: 'plug', INTERNET_IDENTITY: 'internet-identity', OISY: 'oisy' },
  currentWalletType: writable(null),
  beginWalletSessionTransition: mocks.beginWalletSessionTransition,
  selectedWalletId: writable(null),
  connectionError: writable(null),
}));

vi.mock('../services/pnp', () => ({
  pnp: { connect: mocks.pnpConnect, disconnect: mocks.pnpDisconnect },
  canisterIDLs: {},
}));

vi.mock('../services/tokenService', () => ({
  TokenService: {
    getTokenBalance: mocks.getTokenBalance,
    formatBalance: (b: bigint | null) => (b ?? 0n).toString(),
  },
}));

vi.mock('../services/RequestDeduplicator', () => ({
  RequestDeduplicator: { deduplicate: vi.fn() },
}));

vi.mock('../services/threeUsdPrice', () => ({
  getThreeUsdPrice: mocks.getThreeUsdPrice,
}));

vi.mock('./appDataStore', () => ({
  appDataStore: {
    setWalletState: mocks.setWalletState,
    fetchBalances: mocks.fetchBalances,
    fetchProtocolStatus: mocks.fetchProtocolStatus,
  },
}));

vi.mock('./collateralStore', () => ({
  collateralStore: { fetchSupportedCollateral: mocks.fetchSupportedCollateral },
}));

vi.mock('../services/protocol/apiClient', () => ({
  ApiClient: { clearVaultCache: mocks.clearVaultCache },
}));

import { walletStore } from './wallet';

describe('walletStore.initialize() — visible connection state (wallet.ts)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.getTokenBalance.mockResolvedValue(0n);
    mocks.getThreeUsdPrice.mockResolvedValue(1);
    mocks.fetchSupportedCollateral.mockResolvedValue([]);
    mocks.authInitialize.mockResolvedValue(undefined);
    mocks.authDisconnect.mockResolvedValue(undefined);
    mocks.clearVaultCache.mockResolvedValue(undefined);
    mocks.authStore.set({
      isConnected: true,
      account: { owner: TEST_PRINCIPAL, balance: 0n },
      isInitialized: true,
      walletType: 'internet-identity',
    });
  });

  // The real refreshInterval set by startBalanceRefresh() is module-level
  // singleton state shared across tests in this file. Leaving it running
  // after a test lets startBalanceRefresh() no-op on the next test (it only
  // starts if refreshInterval is null), silently skipping the very refresh
  // behavior the next test means to exercise.
  afterEach(async () => {
    await walletStore.disconnect();
  });

  it('publishes isConnected=true for a valid persisted II delegation even when balance/protocol-status fetches reject', async () => {
    mocks.fetchBalances.mockRejectedValue(new Error('balances endpoint down'));
    mocks.fetchProtocolStatus.mockRejectedValue(new Error('status endpoint down'));

    const result = await walletStore.initialize();

    expect(result).toBe(true);
    const state = get(walletStore);
    expect(state.isConnected).toBe(true);
    expect(state.principal?.toString()).toBe(TEST_PRINCIPAL.toString());
  });

  it('publishes the connected state before a delayed balance fetch resolves', async () => {
    mocks.fetchBalances.mockReturnValue(new Promise(() => {})); // never resolves
    mocks.fetchProtocolStatus.mockReturnValue(new Promise(() => {}));

    const result = await walletStore.initialize();

    // initialize() must resolve (not hang) even though startBalanceRefresh's
    // data fetch is still pending, and that fetch must actually have been
    // invoked — proving startBalanceRefresh ran rather than silently no-op'ing
    // because a previous test's interval was left set.
    expect(result).toBe(true);
    expect(mocks.fetchBalances).toHaveBeenCalled();

    const state = get(walletStore);
    expect(state.isConnected).toBe(true);
    expect(state.principal?.toString()).toBe(TEST_PRINCIPAL.toString());
  });
});

describe('walletStore.connect() — fresh login visible connection state (wallet.ts)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocks.getTokenBalance.mockResolvedValue(0n);
    mocks.getThreeUsdPrice.mockResolvedValue(1);
    mocks.fetchSupportedCollateral.mockResolvedValue([]);
    mocks.authConnect.mockResolvedValue({ owner: TEST_PRINCIPAL, balance: 0n });
    mocks.authDisconnect.mockResolvedValue(undefined);
    mocks.clearVaultCache.mockResolvedValue(undefined);
  });

  // See note above: leaving the module-level refreshInterval running after a
  // test makes startBalanceRefresh() no-op on the next test.
  afterEach(async () => {
    await walletStore.disconnect();
  });

  it('resolves true and publishes the connected state immediately, even though the background balance fetch rejects', async () => {
    mocks.fetchBalances.mockRejectedValue(new Error('balances endpoint down'));
    mocks.fetchProtocolStatus.mockResolvedValue({ lastIcpRate: 5 });

    const result = await walletStore.connect('internet-identity');

    // auth.connect must genuinely be invoked and return a non-anonymous account.
    expect(mocks.authConnect).toHaveBeenCalledWith('internet-identity');
    expect(TEST_PRINCIPAL.isAnonymous()).toBe(false);

    expect(result).toBe(true);

    const state = get(walletStore);
    expect(state.isConnected).toBe(true);
    expect(state.loading).toBe(false);
    expect(state.principal?.toString()).toBe(TEST_PRINCIPAL.toString());
    expect(state.icon).toBe('/wallets/01InfinityMarkHEX.svg');

    expect(mocks.setWalletState).toHaveBeenCalledWith(true, expect.anything());
    expect(mocks.setWalletState).not.toHaveBeenCalledWith(false, null);

    // Background refresh must actually have been kicked off (proving the
    // rejection above was exercised, not skipped).
    expect(mocks.fetchBalances).toHaveBeenCalled();
  });

  it('resolves true and publishes the connected state immediately, without waiting on a never-resolving background fetch', async () => {
    mocks.fetchBalances.mockReturnValue(new Promise(() => {})); // never resolves
    mocks.fetchProtocolStatus.mockReturnValue(new Promise(() => {})); // never resolves

    const result = await walletStore.connect('internet-identity');

    expect(mocks.authConnect).toHaveBeenCalledWith('internet-identity');

    // connect() must resolve (not hang) even though the background fetch is
    // still pending, and that fetch must actually have been invoked — proving
    // startBalanceRefresh ran rather than silently no-op'ing.
    expect(result).toBe(true);
    expect(mocks.fetchBalances).toHaveBeenCalled();

    const state = get(walletStore);
    expect(state.isConnected).toBe(true);
    expect(state.loading).toBe(false);
    expect(state.principal?.toString()).toBe(TEST_PRINCIPAL.toString());

    expect(mocks.setWalletState).toHaveBeenCalledWith(true, expect.anything());
    expect(mocks.setWalletState).not.toHaveBeenCalledWith(false, null);
  });
});

describe('walletStore.refreshWallet() — session identity transition', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.spyOn(globalThis, 'setTimeout').mockImplementation(((callback: () => void) => {
      queueMicrotask(callback);
      return 0;
    }) as typeof setTimeout);
    mocks.getTokenBalance.mockResolvedValue(0n);
    mocks.getThreeUsdPrice.mockResolvedValue(1);
    mocks.fetchSupportedCollateral.mockResolvedValue([]);
    mocks.fetchBalances.mockResolvedValue({ icpBalance: 0n, icusdBalance: 0n });
    mocks.fetchProtocolStatus.mockResolvedValue({ lastIcpRate: 1 });
    mocks.authConnect.mockResolvedValue({ owner: TEST_PRINCIPAL, balance: 0n });
    mocks.authDisconnect.mockResolvedValue(undefined);
    mocks.clearVaultCache.mockResolvedValue(undefined);
    mocks.pnpDisconnect.mockResolvedValue(undefined);
  });

  afterEach(async () => {
    await walletStore.disconnect();
    vi.restoreAllMocks();
  });

  it('invalidates in-flight action contexts before reconnect and publishes a changed provider owner', async () => {
    const ownerB = Principal.fromUint8Array(new Uint8Array([9, 8, 7, 6, 5]));
    await walletStore.connect('plug');
    localStorage.setItem('rumi_last_wallet', 'plug');
    mocks.beginWalletSessionTransition.mockClear();
    mocks.pnpConnect.mockResolvedValue({ owner: ownerB, balance: 0n });

    await walletStore.refreshWallet();

    expect(mocks.beginWalletSessionTransition).toHaveBeenCalledOnce();
    expect(mocks.beginWalletSessionTransition.mock.invocationCallOrder[0])
      .toBeLessThan(mocks.pnpDisconnect.mock.invocationCallOrder[0]);
    expect(get(walletStore).isConnected).toBe(true);
    expect(get(walletStore).principal?.toText()).toBe(ownerB.toText());
    expect(mocks.setWalletState).toHaveBeenLastCalledWith(true, ownerB);
  });

  it('fails closed and clears the published identity if reconnect fails', async () => {
    await walletStore.connect('plug');
    localStorage.setItem('rumi_last_wallet', 'plug');
    mocks.beginWalletSessionTransition.mockClear();
    mocks.pnpConnect.mockRejectedValue(new Error('provider reconnect failed'));

    await expect(walletStore.refreshWallet()).rejects.toThrow(/provider reconnect failed/i);

    expect(mocks.beginWalletSessionTransition).toHaveBeenCalledOnce();
    expect(get(walletStore)).toMatchObject({ isConnected: false, principal: null, loading: false });
    expect(mocks.setWalletState).toHaveBeenLastCalledWith(false, null);
  });
});
