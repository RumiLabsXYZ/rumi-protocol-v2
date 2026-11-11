import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { get } from 'svelte/store';
import { Principal } from '@dfinity/principal';
import { createPNP, BaseSignerAdapter } from '@windoge98/plug-n-play';

// ──────────────────────────────────────────────────────────────
// Regression coverage for Internet Identity session persistence
// across ordinary page refreshes. The bug class this guards against:
// gating the visible connected state on a fallible balance/data call,
// and clearing LAST_WALLET/WAS_CONNECTED when that call fails.
// ──────────────────────────────────────────────────────────────

const mocks = vi.hoisted(() => ({
  isAuthenticated: vi.fn(),
  getIdentity: vi.fn(),
  login: vi.fn(),
  logout: vi.fn().mockResolvedValue(undefined),
  getTokenBalance: vi.fn(),
  fetchRootKey: vi.fn().mockResolvedValue(undefined),
  pnpConnect: vi.fn(),
  pnpInstance: vi.fn(),
}));

vi.mock('@dfinity/auth-client', () => ({
  AuthClient: {
    create: vi.fn().mockResolvedValue({
      isAuthenticated: mocks.isAuthenticated,
      getIdentity: mocks.getIdentity,
      login: mocks.login,
      logout: mocks.logout,
    }),
  },
}));

vi.mock('@dfinity/agent', async () => {
  const actual = await vi.importActual<typeof import('@dfinity/agent')>('@dfinity/agent');
  return {
    ...actual,
    HttpAgent: vi.fn(() => ({ fetchRootKey: mocks.fetchRootKey })),
  };
});

vi.mock('./tokenService', () => ({
  TokenService: {
    getTokenBalance: mocks.getTokenBalance,
    formatBalance: (b: bigint | null) => (b ?? 0n).toString(),
  },
}));

vi.mock('./pnp', () => ({
  pnp: {},
  canisterIDLs: {},
  connectWithComprehensivePermissions: mocks.pnpConnect,
  getPnpInstance: mocks.pnpInstance,
  silentPlugReconnect: vi.fn(),
}));

vi.mock('./PermissionManager', () => ({
  permissionManager: {
    clearCache: vi.fn(),
    hasPermissions: vi.fn().mockReturnValue(true),
    ensurePermissions: vi.fn().mockResolvedValue(true),
  },
}));

vi.mock('./oisySigner', () => ({
  clearOisySigner: vi.fn(),
}));

import { auth, WALLET_TYPES } from './auth';

// Deterministic, distinct, non-anonymous principals. The anonymous principal
// (2vxsx-fae) must never stand in for an authenticated identity in these
// tests — backfillBalance's owner check would be trivially satisfied by two
// "different" anonymous references, masking cross-account leakage bugs.
const TEST_PRINCIPAL = Principal.fromUint8Array(new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8, 9, 10]));
const OTHER_PRINCIPAL = Principal.fromUint8Array(new Uint8Array([9, 8, 7, 6, 5, 4, 3, 2, 1, 0]));

function seedRestorableSession() {
  localStorage.setItem('rumi_last_wallet', WALLET_TYPES.INTERNET_IDENTITY);
  localStorage.setItem('rumi_was_connected', 'true');
}

describe('II session persistence (auth.ts)', () => {
  beforeEach(() => {
    localStorage.clear();
    sessionStorage.clear();
    vi.clearAllMocks();
    mocks.logout.mockResolvedValue(undefined);
    mocks.getIdentity.mockReturnValue({ getPrincipal: () => TEST_PRINCIPAL });
  });

  afterEach(async () => {
    await auth.disconnect();
  });

  it('restores a visible connected state even when the restore-time balance fetch is rejected', async () => {
    seedRestorableSession();
    mocks.isAuthenticated.mockResolvedValue(true);
    mocks.getTokenBalance.mockRejectedValue(new Error('balance query failed'));

    await auth.initialize();

    const state = get(auth);
    expect(state.isConnected).toBe(true);
    expect(state.account?.owner.toString()).toBe(TEST_PRINCIPAL.toString());
    expect(state.walletType).toBe(WALLET_TYPES.INTERNET_IDENTITY);

    // A failed data call must never clear the persisted session markers —
    // otherwise the next refresh would skip restoration entirely.
    expect(localStorage.getItem('rumi_last_wallet')).toBe(WALLET_TYPES.INTERNET_IDENTITY);
    expect(localStorage.getItem('rumi_was_connected')).toBe('true');
  });

  it('publishes the connected state before a delayed balance fetch resolves', async () => {
    seedRestorableSession();
    mocks.isAuthenticated.mockResolvedValue(true);

    let releaseBalance: (v: bigint) => void = () => {};
    mocks.getTokenBalance.mockReturnValue(
      new Promise<bigint>((resolve) => { releaseBalance = resolve; })
    );

    await auth.initialize();

    // initialize() must resolve without waiting on the still-pending balance call.
    expect(get(auth).isConnected).toBe(true);
    expect(mocks.getTokenBalance).toHaveBeenCalled();

    // Resolving afterward should top up the balance but never toggle isConnected.
    releaseBalance(42n);
    await Promise.resolve();
    await Promise.resolve();

    const state = get(auth);
    expect(state.isConnected).toBe(true);
    expect(state.account?.balance).toBe(42n);
  });

  it('records the session on successful II authorization and survives a rejected post-auth balance fetch', async () => {
    mocks.login.mockImplementation((opts: { onSuccess: () => void }) => {
      opts.onSuccess();
    });
    mocks.getTokenBalance.mockRejectedValue(new Error('post-auth balance failure'));

    const result = await auth.connect(WALLET_TYPES.INTERNET_IDENTITY);

    expect(result?.owner.toString()).toBe(TEST_PRINCIPAL.toString());
    expect(get(auth).isConnected).toBe(true);
    expect(localStorage.getItem('rumi_last_wallet')).toBe(WALLET_TYPES.INTERNET_IDENTITY);
    expect(localStorage.getItem('rumi_was_connected')).toBe('true');

    // Simulate the next page refresh: restoration must not be skipped just
    // because the previous session's balance call failed.
    mocks.isAuthenticated.mockResolvedValue(true);
    mocks.getTokenBalance.mockResolvedValue(7n);

    await auth.initialize();

    const restored = get(auth);
    expect(restored.isConnected).toBe(true);
    expect(restored.account?.owner.toString()).toBe(TEST_PRINCIPAL.toString());
  });

  it('does not let a stale backfill for a previous principal overwrite the current account balance', async () => {
    let releaseFirstBalance: (v: bigint) => void = () => {};
    const firstBalancePromise = new Promise<bigint>((resolve) => {
      releaseFirstBalance = resolve;
    });

    mocks.getTokenBalance.mockImplementation((_ledgerId: string, principal: Principal) =>
      principal.toString() === TEST_PRINCIPAL.toString()
        ? firstBalancePromise
        : Promise.resolve(99n)
    );

    // Log in as the first principal. Its backfill kicks off but stays pending.
    mocks.login.mockImplementationOnce((opts: { onSuccess: () => void }) => opts.onSuccess());
    mocks.getIdentity.mockReturnValueOnce({ getPrincipal: () => TEST_PRINCIPAL });

    await auth.connect(WALLET_TYPES.INTERNET_IDENTITY);
    expect(get(auth).account?.owner.toString()).toBe(TEST_PRINCIPAL.toString());

    // Before the first backfill resolves, the user disconnects and switches
    // the active II session to a different, non-anonymous principal.
    mocks.login.mockImplementationOnce((opts: { onSuccess: () => void }) => opts.onSuccess());
    mocks.getIdentity.mockReturnValueOnce({ getPrincipal: () => OTHER_PRINCIPAL });

    await auth.connect(WALLET_TYPES.INTERNET_IDENTITY);
    await Promise.resolve();
    await Promise.resolve();

    const switchedState = get(auth);
    expect(switchedState.account?.owner.toString()).toBe(OTHER_PRINCIPAL.toString());
    expect(switchedState.account?.balance).toBe(99n);

    // Now the stale backfill for the first principal resolves. It must never
    // land on the account the user has since switched to.
    releaseFirstBalance(1234n);
    await Promise.resolve();
    await Promise.resolve();

    const finalState = get(auth);
    expect(finalState.account?.owner.toString()).toBe(OTHER_PRINCIPAL.toString());
    expect(finalState.account?.balance).toBe(99n);
  });
});

// Use the installed OISY adapter class, including its real connect/cache/disconnect.
// Only the popup transport/account reply is synthetic; no live wallet is contacted.
const InstalledOisyAdapter = createPNP().config.adapters.oisy.adapter as typeof BaseSignerAdapter;
class OisyLifecycleFixture extends InstalledOisyAdapter {
  readonly accountsRpc = vi.fn();
  constructor(readonly liveOwner: Principal) {
    super({ adapter: { id: 'oisy', walletName: 'Oisy', enabled: true, chain: 'ICP', config: { fetchRootKey: false } },
      config: { hostUrl: 'https://icp-api.io', disableAccountSelection: true } } as any);
    this.accountsRpc.mockResolvedValue([{ owner: liveOwner }]);
  }
  protected async ensureTransportInitialized() {
    this.signer = { closeChannel: vi.fn() } as any;
    this.signerAgent = { signer: { accounts: this.accountsRpc }, replaceAccount: vi.fn() } as any;
  }
}

describe('OISY disconnect and installed PNP principal cache', () => {
  beforeEach(() => {
    localStorage.clear(); sessionStorage.clear(); vi.clearAllMocks();
    mocks.getTokenBalance.mockResolvedValue(0n);
  });
  it('retires the actual PNP cache so disconnect plus page reload cannot revive another signer owner', async () => {
    let provider = new OisyLifecycleFixture(TEST_PRINCIPAL);
    const disconnect = vi.fn(() => provider.disconnect());
    mocks.pnpInstance.mockReturnValue({ disconnect });
    mocks.pnpConnect.mockImplementation(async () => {
      const result = await provider.connect();
      return { ...result, owner: Principal.fromText(String(result.owner)) };
    });
    await auth.connect(WALLET_TYPES.OISY);
    expect(provider.accountsRpc).toHaveBeenCalledOnce();
    expect(localStorage.getItem('oisy_principal')).toBe(TEST_PRINCIPAL.toText());
    await auth.disconnect();
    const cacheAfterDisconnect = localStorage.getItem('oisy_principal');
    // A new provider simulates reload; the next explicit connection must read accounts.
    provider = new OisyLifecycleFixture(OTHER_PRINCIPAL);
    const next = await auth.connect(WALLET_TYPES.OISY);
    console.log('Installed OISY reconnect observation', { cachedPrincipalSurvived: cacheAfterDisconnect !== null, accountsRpcCount: provider.accountsRpc.mock.calls.length, returnedLiveOwner: next?.owner.toText() === OTHER_PRINCIPAL.toText() });
    expect(next?.owner.toText()).toBe(OTHER_PRINCIPAL.toText());
    expect(disconnect).toHaveBeenCalledOnce();
    expect(cacheAfterDisconnect).toBeNull();
    expect(provider.accountsRpc).toHaveBeenCalledOnce();
    expect(get(auth).account?.owner.toText()).toBe(OTHER_PRINCIPAL.toText());
    await auth.disconnect();
  });
  it('does not clear a replacement wallet session while OISY disconnect is pending', async () => {
    let owner = TEST_PRINCIPAL;
    mocks.pnpConnect.mockImplementation(async () => ({ owner }));
    let finish!: () => void;
    mocks.pnpInstance.mockReturnValue({ disconnect: vi.fn(() => new Promise<void>(r => { finish = r; })) });
    await auth.connect(WALLET_TYPES.OISY);
    const pending = auth.disconnect();
    owner = OTHER_PRINCIPAL;
    await auth.connect(WALLET_TYPES.OISY);
    finish(); await pending;
    expect(get(auth).isConnected).toBe(true);
    expect(get(auth).account?.owner.toText()).toBe(OTHER_PRINCIPAL.toText());
    expect(localStorage.getItem('rumi_last_wallet')).toBe(WALLET_TYPES.OISY);
  });
  it('keeps a cancelled fresh account request disconnected rather than reviving its retired cache', async () => {
    let provider = new OisyLifecycleFixture(TEST_PRINCIPAL);
    mocks.pnpInstance.mockImplementation(() => ({ disconnect: () => provider.disconnect() }));
    mocks.pnpConnect.mockImplementation(async () => { const r=await provider.connect(); return {...r,owner:Principal.fromText(String(r.owner))}; });
    await auth.connect(WALLET_TYPES.OISY); await auth.disconnect();
    provider = new OisyLifecycleFixture(OTHER_PRINCIPAL);
    provider.accountsRpc.mockRejectedValueOnce(new Error('Account permission cancelled'));
    await expect(auth.connect(WALLET_TYPES.OISY)).rejects.toThrow('Account permission cancelled');
    expect(provider.accountsRpc).toHaveBeenCalledOnce();
    expect(get(auth).isConnected).toBe(false);
    expect(localStorage.getItem('oisy_principal')).toBeNull();
  });
  it('does not report a clean disconnect if the PNP retirement fails', async () => {
    const provider = new OisyLifecycleFixture(TEST_PRINCIPAL);
    mocks.pnpConnect.mockImplementation(async () => {
      const result = await provider.connect();
      return { ...result, owner: Principal.fromText(String(result.owner)) };
    });
    mocks.pnpInstance.mockReturnValue({ disconnect: vi.fn().mockRejectedValue(new Error('PNP retirement failed')) });
    await auth.connect(WALLET_TYPES.OISY);
    await expect(auth.disconnect()).rejects.toThrow('PNP retirement failed');
    expect(get(auth).isConnected).toBe(true);
    expect(localStorage.getItem('oisy_principal')).toBe(TEST_PRINCIPAL.toText());
  });
});
