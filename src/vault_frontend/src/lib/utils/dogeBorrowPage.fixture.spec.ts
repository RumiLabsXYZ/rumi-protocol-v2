import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount, flushSync } from 'svelte';
import {
  PRINCIPAL_A,
  PRINCIPAL_B,
  PRINCIPAL_A_TEXT,
  PRINCIPAL_B_TEXT,
  CKDOGE_LEDGER_TEXT,
  fakeCollateralInfo,
  fakeRawVault,
  fakeVaultDto,
  fakeMinterInfo,
  fakeMintedUtxoResult,
  fakeIntentRecord,
  deferred,
  settle,
  setInputValue,
} from '../../../tests/doge-borrow-fixtures/testData';
import { saveIntent as saveIntentRaw, storageKeyForPrincipal, computeDogeBorrowRisk } from './dogeBorrowWizard';
import { koinuToDoge } from './dogeBorrowFlow';
import { formatNumber } from './format';

const TEST_NETWORK_SCOPE = 'local';
function saveIntent(storage: Storage, record: Parameters<typeof saveIntentRaw>[1]) {
  return saveIntentRaw(storage, record, TEST_NETWORK_SCOPE);
}

/**
 * Real-component fixture tests for /doge/borrow (src/routes/doge/borrow/+page.svelte).
 *
 * PINNED SNAPSHOT: this file was authored and last run GREEN against
 *   +page.svelte          sha256 c3b287780cfcb966f6eb8324f69a3f934c3ee829200be72df63492f3c5f9e918
 *   dogeBorrowWizard.ts   sha256 a99d5c27f65eb56fc6f7f78692e85ded121fbb0f98e52b06ca998ee8afdb5e84
 * The route was actively being rewritten by a concurrent repair worker while
 * this file was authored (schema version bumped 1->2, VaultCard/userVaults
 * wired in, unknown-vault-id recovery added, mid-flight). If this drifts,
 * see doge-borrow-fixture-report.md "compatibility notes" for what changed
 * last time and how to re-diagnose quickly.
 *
 * This mounts the ACTUAL route component with Svelte 5's native `mount()`/
 * `unmount()` (no @testing-library/svelte — not installed, and not needed:
 * svelte's own client runtime exports everything required). The only thing
 * standing between a plain `vitest.config.ts` run and this working is that
 * Vite's default "node" resolve condition pulls in svelte/internal/server,
 * where `mount()` throws `lifecycle_function_unavailable`. vitest.doge-borrow.config.ts
 * adds the `browser` resolve condition (scoped to this one spec file) so
 * Vite resolves svelte's client build under jsdom instead — see that file's
 * header comment for the full explanation.
 *
 * Only true external boundaries are mocked: the wallet store, collateral/appData
 * stores, protocolService, the public backend actor, the ckDOGE minter actors,
 * VaultCard.svelte (stubbed — see FakeVaultCard.svelte for why), and the
 * `qrcode` package. dogeBorrowWizard.ts and dogeBorrowFlow.ts — the actual new
 * orchestration/state-machine and math logic under test — are imported for
 * real, unmocked. No live canister calls, no real wallet/auth, no signatures.
 */

const fx = vi.hoisted(() => {
  function makeStore<T>(initial: T) {
    let value = initial;
    const subs = new Set<(v: T) => void>();
    return {
      subscribe(run: (v: T) => void) {
        subs.add(run);
        run(value);
        return () => subs.delete(run);
      },
      set(v: T) {
        value = v;
        subs.forEach((run) => run(value));
      },
      update(fn: (v: T) => T) {
        value = fn(value);
        subs.forEach((run) => run(value));
      },
      get: () => value,
    };
  }

  function makeDerived<T, U>(store: { subscribe: (run: (v: T) => void) => () => void }, project: (v: T) => U) {
    const subs = new Set<(v: U) => void>();
    let derivedValue: U;
    let hasValue = false;
    store.subscribe((v) => {
      derivedValue = project(v);
      hasValue = true;
      subs.forEach((run) => run(derivedValue));
    });
    return {
      subscribe(run: (v: U) => void) {
        subs.add(run);
        if (hasValue) run(derivedValue);
        return () => subs.delete(run);
      },
    };
  }

  const walletState = makeStore({
    isConnected: false,
    principal: null as unknown,
    balance: null,
    error: null,
    loading: false,
    icon: '',
    tokenBalances: {} as Record<string, { raw: bigint; formatted: string; usdValue: number | null }>,
  });
  const collateralState = makeStore({ collaterals: [] as unknown[], loading: false });
  const appDataState = makeStore({ protocolStatus: null as unknown, userVaults: [] as unknown[] });

  return {
    walletState,
    collateralState,
    appDataState,
    makeDerived,
    walletConnect: vi.fn(async (_walletId: string) => {}),
    walletDisconnect: vi.fn(async () => {}),
    walletRefreshBalance: vi.fn(async (_opts?: unknown) => {}),
    fetchSupportedCollateral: vi.fn(async (_force?: boolean) => {}),
    fetchProtocolStatus: vi.fn(async (_force?: boolean) => {}),
    refreshAll: vi.fn(async (_owner: unknown) => {}),
    fetchUserVaults: vi.fn(async (_owner: unknown, _force?: boolean) => [] as unknown[]),
    openVaultAndBorrowBound: vi.fn(async (_ctx: unknown, _collateralRaw: bigint, _icusdRaw: bigint, _p?: string) => ({
      kind: 'dispatched_err' as 'dispatched_err' | 'dispatched_ok' | 'predispatch_aborted' | 'ambiguous_transport',
      vaultId: null as number | null,
      blockIndex: null as number | null,
      partialZeroDebtVaultId: null as number | null,
      errorMessage: 'unset' as string | null,
      approvalMayHaveMutated: false,
      submittedCollateralRaw: 0n,
      submittedIcusdRaw: 0n,
    })),
    borrowFromVaultBound: vi.fn(async (_ctx: unknown, vaultId: number, _icusdRaw: bigint) => ({
      kind: 'dispatched_err' as const,
      vaultId,
      blockIndex: null as number | null,
      feePaidRaw: null as bigint | null,
      errorMessage: 'unset' as string | null,
      submittedIcusdRaw: 0n,
    })),
    getVaults: vi.fn(async (_owners: unknown[]) => [] as unknown[]),
    getPublicMinterActor: vi.fn(async () => ({
      get_doge_address: vi.fn(async () => 'DUNSET0000000000000000000000000'),
      get_minter_info: vi.fn(async () => fakeMinterInfo()),
    })),
    updateDogeBalanceForOwner: vi.fn(async (_owner: unknown, _isLive: () => boolean): Promise<{ Ok: unknown[] } | { Err: unknown }> => ({ Ok: [] })),
    qrToDataURL: vi.fn(async () => 'data:image/png;base64,fake'),
  };
});

vi.mock('$lib/stores/wallet', () => ({
  walletStore: {
    subscribe: fx.walletState.subscribe,
    connect: fx.walletConnect,
    disconnect: fx.walletDisconnect,
    refreshBalance: fx.walletRefreshBalance,
  },
  isConnected: fx.makeDerived(fx.walletState, (s: any) => s.isConnected),
  principal: fx.makeDerived(fx.walletState, (s: any) => s.principal),
}));

vi.mock('$lib/stores/collateralStore', () => ({
  collateralStore: {
    subscribe: fx.collateralState.subscribe,
    fetchSupportedCollateral: fx.fetchSupportedCollateral,
  },
}));

vi.mock('$lib/stores/appDataStore', () => ({
  appDataStore: {
    subscribe: fx.appDataState.subscribe,
    fetchProtocolStatus: fx.fetchProtocolStatus,
    refreshAll: fx.refreshAll,
    fetchUserVaults: fx.fetchUserVaults,
  },
  protocolStatus: fx.makeDerived(fx.appDataState, (s: any) => s.protocolStatus),
  userVaults: fx.makeDerived(fx.appDataState, (s: any) => s.userVaults),
}));

vi.mock('$lib/services/protocol', () => ({
  protocolService: {
    openVaultAndBorrowBound: fx.openVaultAndBorrowBound,
    borrowFromVaultBound: fx.borrowFromVaultBound,
  },
}));

vi.mock('$lib/services/protocol/apiClient', () => ({
  publicActor: { get_vaults: fx.getVaults },
}));

vi.mock('$lib/services/auth', () => ({
  WALLET_TYPES: { PLUG: 'plug', INTERNET_IDENTITY: 'internet-identity', OISY: 'oisy' },
}));

vi.mock('$lib/services/ckdogeMinterActors', () => ({
  getPublicMinterActor: fx.getPublicMinterActor,
  updateDogeBalanceForOwner: fx.updateDogeBalanceForOwner,
}));

vi.mock('qrcode', () => ({
  default: { toDataURL: fx.qrToDataURL },
}));

// The real VaultCard pulls in a large independent dependency graph (see
// FakeVaultCard.svelte's header comment) that is out of scope here — stubbed
// with a real (compiled) Svelte component exposing the same prop/event contract.
vi.mock('$lib/components/vault/VaultCard.svelte', async () => {
  const mod = await import('../../../tests/doge-borrow-fixtures/FakeVaultCard.svelte');
  return { default: mod.default };
});

// Imported AFTER the vi.mock registrations above (hoisted regardless of source
// order by Vitest, but written here for readability) — this is the real route.
import Page from '../../routes/doge/borrow/+page.svelte';

let host: HTMLDivElement;
let instance: unknown;

function resetWalletState() {
  fx.walletState.set({
    isConnected: false,
    principal: null,
    balance: null,
    error: null,
    loading: false,
    icon: '',
    tokenBalances: {},
  });
}

function connectAs(
  principal: typeof PRINCIPAL_A,
  tokenBalances: Record<string, { raw: bigint; formatted: string; usdValue: number | null }> = {}
) {
  fx.walletState.set({
    isConnected: true,
    principal,
    balance: null,
    error: null,
    loading: false,
    icon: '',
    tokenBalances,
  });
}

/**
 * jsdom does not implement the Web Locks API (navigator.locks), and the route's cross-tab
 * exclusive guard fails closed (refuses to dispatch) when it is unsupported — correct for a
 * real browser lacking it, but this fixture needs a real-enough Locks implementation so the
 * mutating-action tests can actually exercise dispatch. A fresh instance every test avoids
 * cross-test lock leakage.
 */
function installFakeLocksManager() {
  const held = new Set<string>();
  const manager = {
    async request(name: string, options: { ifAvailable: boolean }, callback: (lock: unknown) => Promise<unknown>) {
      if (held.has(name)) {
        return callback(null);
      }
      held.add(name);
      try {
        return await callback({ name });
      } finally {
        held.delete(name);
      }
    },
  };
  Object.defineProperty(navigator, 'locks', { value: manager, configurable: true });
}

function renderPage() {
  instance = mount(Page, { target: host });
  flushSync();
}

function q(sel: string): HTMLElement | null {
  return host.querySelector(sel);
}

function qAll(sel: string): HTMLElement[] {
  return Array.from(host.querySelectorAll(sel));
}

function findButtonByText(text: string): HTMLButtonElement | null {
  return (qAll('button').find((b) => b.textContent?.includes(text)) as HTMLButtonElement | undefined) ?? null;
}

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
  localStorage.clear();
  installFakeLocksManager();
  resetWalletState();
  fx.collateralState.set({ collaterals: [fakeCollateralInfo()], loading: false });
  fx.appDataState.set({
    protocolStatus: {
      globalIcusdMintCap: 1_000_000,
      borrowingFeeCurveResolved: [],
    },
    userVaults: [],
  });

  fx.walletConnect.mockReset().mockImplementation(async () => {});
  fx.walletDisconnect.mockReset().mockImplementation(async () => {});
  fx.walletRefreshBalance.mockReset().mockImplementation(async () => {});
  fx.fetchSupportedCollateral.mockReset().mockImplementation(async () => {});
  fx.fetchProtocolStatus.mockReset().mockImplementation(async () => {});
  fx.refreshAll.mockReset().mockImplementation(async () => {});
  fx.fetchUserVaults.mockReset().mockImplementation(async () => []);
  fx.openVaultAndBorrowBound.mockReset().mockResolvedValue({
    kind: 'dispatched_err',
    vaultId: null,
    blockIndex: null,
    partialZeroDebtVaultId: null,
    errorMessage: 'unset',
    approvalMayHaveMutated: false,
    submittedCollateralRaw: 0n,
    submittedIcusdRaw: 0n,
  });
  fx.borrowFromVaultBound.mockReset().mockImplementation(async (_ctx: unknown, vaultId: number) => ({
    kind: 'dispatched_err' as const,
    vaultId,
    blockIndex: null,
    feePaidRaw: null,
    errorMessage: 'unset',
    submittedIcusdRaw: 0n,
  }));
  fx.getVaults.mockReset().mockResolvedValue([]);
  fx.getPublicMinterActor.mockReset().mockResolvedValue({
    get_doge_address: vi.fn(async () => 'DUNSET0000000000000000000000000'),
    get_minter_info: vi.fn(async () => fakeMinterInfo()),
  });
  fx.updateDogeBalanceForOwner.mockReset().mockResolvedValue({ Ok: [] });
  fx.qrToDataURL.mockReset().mockResolvedValue('data:image/png;base64,fake');
});

afterEach(() => {
  if (instance) {
    unmount(instance as any);
    instance = undefined;
  }
  host.remove();
  localStorage.clear();
});

describe('/doge/borrow — disconnected calculator', () => {
  it('is reachable and adjustable before any authentication, and gates Continue behind sign-in', async () => {
    renderPage();

    // Calculator is on screen with no auth prompt yet.
    const collateralInput = q('#dbw-collateral') as HTMLInputElement;
    const icusdInput = q('#dbw-icusd') as HTMLInputElement;
    expect(collateralInput).toBeTruthy();
    expect(icusdInput).toBeTruthy();
    expect(collateralInput.value).toBe('1000');
    expect(findButtonByText('Continue with Internet Identity')).toBeNull();

    const before = q('.dbw-result-strip')?.textContent ?? '';

    setInputValue(collateralInput, '2000');
    await settle();

    const after = q('.dbw-result-strip')?.textContent ?? '';
    expect(after).not.toBe(before);

    // Cross-check the displayed collateral ratio against the real risk math
    // (same function the route imports), proving the DOM reflects the live
    // recompute, not a stale/hardcoded string.
    const expected = computeDogeBorrowRisk({
      collateralAmountDoge: 2000,
      icusdAmount: 50,
      collateralPriceUsd: 0.08,
      liquidationCr: 1.2,
      minimumCr: 1.35,
      borrowingFeeRate: 0.005,
      feeCurve: [],
    });
    expect(after).toContain(`${formatNumber(expected.collateralRatioPct)}%`);

    // Disconnected: clicking through goes to sign-in, not straight to send.
    findButtonByText('Continue with this loan')!.click();
    await settle();
    expect(findButtonByText('Continue with Internet Identity')).toBeTruthy();
    expect(fx.walletConnect).not.toHaveBeenCalled();
  });
});

describe('/doge/borrow — sign-in pipeline', () => {
  it('routes Internet Identity and Oisy buttons through the existing wallet store connect() with no auto-connect', async () => {
    renderPage();
    findButtonByText('Continue with this loan')!.click();
    await settle();

    expect(fx.walletConnect).not.toHaveBeenCalled();

    findButtonByText('Continue with Internet Identity')!.click();
    await settle();
    expect(fx.walletConnect).toHaveBeenCalledTimes(1);
    expect(fx.walletConnect).toHaveBeenCalledWith('internet-identity');
  });

  it('routes the Oisy button through connect("oisy")', async () => {
    renderPage();
    findButtonByText('Continue with this loan')!.click();
    await settle();

    findButtonByText('Prefer a crypto wallet? Use Oisy')!.click();
    await settle();
    expect(fx.walletConnect).toHaveBeenCalledTimes(1);
    expect(fx.walletConnect).toHaveBeenCalledWith('oisy');
  });
});

describe('/doge/borrow — already-connected account', () => {
  it('skips the sign-in step entirely for an existing connected account', async () => {
    connectAs(PRINCIPAL_A);
    fx.getPublicMinterActor.mockResolvedValue({
      get_doge_address: vi.fn(async () => 'DfakeAddressAlreadyConnected000001'),
      get_minter_info: vi.fn(async () => fakeMinterInfo()),
    });

    renderPage();
    await settle();

    findButtonByText('Continue with this loan')!.click();
    await settle();

    expect(findButtonByText('Continue with Internet Identity')).toBeNull();
    expect(fx.walletConnect).not.toHaveBeenCalled();
    expect(host.textContent).toContain('Send DOGE to your address');
    // Reaching "send" for a connected account should be the trigger that
    // fetches a real, owner-bound deposit address — not a wallet popup.
    expect(fx.getPublicMinterActor).toHaveBeenCalled();
  });
});

describe('/doge/borrow — account switching mid-flight', () => {
  it('does not let a stale in-flight address request for the old account write state after switching, and preserves the old account record', async () => {
    // Seed a pre-existing persisted record for A, as if returning to this page.
    saveIntent(
      localStorage,
      fakeIntentRecord({
        principal: PRINCIPAL_A_TEXT,
        step: 'send',
        collateralAmountDoge: 1000,
        icusdAmount: 50,
        vaultId: null,
        borrowConfirmed: false,
        now: Date.now(),
      })
    );
    const aRecordBefore = localStorage.getItem(storageKeyForPrincipal(PRINCIPAL_A_TEXT, TEST_NETWORK_SCOPE));
    expect(aRecordBefore).toBeTruthy();

    const addrForA = deferred<string>();
    const addrForB = deferred<string>();
    let callCount = 0;
    const getAddr = vi.fn(async () => {
      callCount += 1;
      return callCount === 1 ? addrForA.promise : addrForB.promise;
    });
    fx.getPublicMinterActor.mockResolvedValue({
      get_doge_address: getAddr,
      get_minter_info: vi.fn(async () => fakeMinterInfo()),
    });

    connectAs(PRINCIPAL_A);
    renderPage();
    await settle();
    expect(host.textContent).toContain('Send DOGE to your address');
    expect(getAddr).toHaveBeenCalledTimes(1);

    // Switch accounts while A's address request is still pending.
    connectAs(PRINCIPAL_B);
    await settle();

    // Resolve A's stale request — it must never land as the displayed address.
    addrForA.resolve('STALE_ADDRESS_FOR_A');
    await settle();
    expect(q('.dbw-address-btn span')?.textContent).not.toBe('STALE_ADDRESS_FOR_A');

    // A fresh, legitimate request for B should have been kicked off by the switch.
    expect(getAddr).toHaveBeenCalledTimes(2);
    addrForB.resolve('FRESH_ADDRESS_FOR_B');
    await settle();
    expect(q('.dbw-address-btn span')?.textContent).toBe('FRESH_ADDRESS_FOR_B');

    // A's own persisted record must be untouched by B's session.
    const aRecordAfter = localStorage.getItem(storageKeyForPrincipal(PRINCIPAL_A_TEXT, TEST_NETWORK_SCOPE));
    expect(aRecordAfter).toBe(aRecordBefore);
  });

  it('does not let a delayed QR result for the old session replace the new address QR', async () => {
    const qrForA = deferred<string>();
    const qrForB = deferred<string>();
    let qrCallCount = 0;
    fx.getPublicMinterActor.mockResolvedValue({
      get_doge_address: vi.fn(async () => {
        return qrCallCount === 0 ? 'DqrAddressForA00000000000000001' : 'DqrAddressForB00000000000000001';
      }),
      get_minter_info: vi.fn(async () => fakeMinterInfo()),
    });
    fx.qrToDataURL.mockImplementation(async () => {
      qrCallCount += 1;
      return qrCallCount === 1 ? qrForA.promise : qrForB.promise;
    });

    connectAs(PRINCIPAL_A);
    renderPage();
    await settle();
    findButtonByText('Continue with this loan')!.click();
    await settle();
    expect(q('.dbw-address-btn span')?.textContent).toBe('DqrAddressForA00000000000000001');

    connectAs(PRINCIPAL_B);
    await settle();
    expect(q('.dbw-address-btn span')?.textContent).toBe('DqrAddressForB00000000000000001');

    qrForA.resolve('data:image/png;base64,stale-a');
    await settle();
    expect(q('.dbw-qr')).toBeNull();

    qrForB.resolve('data:image/png;base64,fresh-b');
    await settle();
    expect((q('.dbw-qr') as HTMLImageElement)?.src).toContain('fresh-b');
  });
});

describe('/doge/borrow — deposit to confirm to explicit borrow', () => {
  it('only calls openVaultAndBorrowBound on the explicit Confirm and borrow click, with EXACT raw bigint amounts, after a real mocked minted receipt and refreshed final terms', async () => {
    // ledgerFee: 0 isolates this test's purpose (exact-bigint pass-through, no float rounding)
    // from the fee-reservation behavior, which has its own dedicated tests below.
    fx.collateralState.set({ collaterals: [fakeCollateralInfo({ ledgerFee: 0 })], loading: false });
    connectAs(PRINCIPAL_A);
    fx.getPublicMinterActor.mockResolvedValue({
      get_doge_address: vi.fn(async () => 'DconfirmFlowAddress00000000000001'),
      get_minter_info: vi.fn(async () => fakeMinterInfo()),
    });
    fx.updateDogeBalanceForOwner.mockResolvedValue(fakeMintedUtxoResult(7n, 100_000_000_000n)); // 1000 DOGE

    renderPage();
    await settle();
    findButtonByText('Continue with this loan')!.click();
    await settle();
    expect(host.textContent).toContain('Send DOGE to your address');

    findButtonByText('I sent the DOGE')!.click();
    await settle();

    expect(host.textContent).toContain('Minted 1000 DOGE worth of ckDOGE');
    expect(fx.openVaultAndBorrowBound).not.toHaveBeenCalled();

    findButtonByText('Continue to confirm borrow')!.click();
    await settle();

    // Reaching "confirm" refreshes live config/price before any submission.
    expect(fx.fetchSupportedCollateral).toHaveBeenCalledWith(true, { strict: true });
    expect(fx.fetchProtocolStatus).toHaveBeenCalledWith(true);
    expect(fx.openVaultAndBorrowBound).not.toHaveBeenCalled();

    // Fee-adjusted net output and liquidation price are disclosed using the
    // real risk math, not just the gross requested amount.
    const expected = computeDogeBorrowRisk({
      collateralAmountDoge: 1000,
      icusdAmount: 50,
      collateralPriceUsd: 0.08,
      liquidationCr: 1.2,
      minimumCr: 1.35,
      borrowingFeeRate: 0.005,
      feeCurve: [],
    });
    expect(host.textContent).toContain(formatNumber(expected.icusdReceived, 4));
    expect(host.textContent).toContain(`$${formatNumber(expected.liquidationPriceUsd, 4)}`);

    // Exactly 1000 DOGE minted (100_000_000_000 koinu) and 50 icUSD (5_000_000_000 e8s) —
    // the vault the mocked bound call reports back must match EXACTLY, no 95% tolerance.
    fx.getVaults.mockResolvedValueOnce([]).mockResolvedValueOnce([
      fakeRawVault({ vaultId: 42, collateralAmount: 100_000_000_000n, borrowedIcusd: 5_000_000_000n }),
    ]);
    fx.openVaultAndBorrowBound.mockResolvedValue({
      kind: 'dispatched_ok',
      vaultId: 42,
      blockIndex: 1,
      partialZeroDebtVaultId: null,
      errorMessage: null,
      approvalMayHaveMutated: true,
      submittedCollateralRaw: 100_000_000_000n,
      submittedIcusdRaw: 5_000_000_000n,
    });

    const confirmBtn = findButtonByText('Confirm and borrow')!;
    expect(confirmBtn.disabled).toBe(false);
    confirmBtn.click();
    await settle();

    expect(fx.openVaultAndBorrowBound).toHaveBeenCalledTimes(1);
    // The bound API takes RAW bigint wire amounts directly — the exact minted koinu and the
    // exact e8s for 50 icUSD, no float round-trip / +0.5 bias anywhere in this path.
    const [, submittedCollateralRaw, submittedIcusdRaw, submittedCollateralPrincipal] = fx.openVaultAndBorrowBound.mock.calls[0];
    expect(submittedCollateralRaw).toBe(100_000_000_000n);
    expect(submittedIcusdRaw).toBe(5_000_000_000n);
    expect(submittedCollateralPrincipal).toBe(CKDOGE_LEDGER_TEXT);
    expect(host.textContent).toContain('Vault #42');
  });

  it('reproduces the reported bug: a wallet holding exactly 52 ckDOGE submits the fee-adjusted amount instead of erroring InsufficientFunds', async () => {
    const LEDGER_FEE_KOINU = 10_000n;
    const FIFTY_TWO_DOGE_KOINU = 5_200_000_000n; // 52 DOGE at 8 decimals
    const SAFE_COLLATERAL_KOINU = FIFTY_TWO_DOGE_KOINU - LEDGER_FEE_KOINU * 2n; // 51.9998 DOGE

    fx.collateralState.set({
      collaterals: [fakeCollateralInfo({ ledgerFee: Number(LEDGER_FEE_KOINU) })],
      loading: false,
    });
    connectAs(PRINCIPAL_A, { ckDOGE: { raw: FIFTY_TWO_DOGE_KOINU, formatted: '52', usdValue: null } });
    fx.getPublicMinterActor.mockResolvedValue({
      get_doge_address: vi.fn(async () => 'DfeeBoundaryAddress000000000000001'),
      get_minter_info: vi.fn(async () => fakeMinterInfo()),
    });
    fx.updateDogeBalanceForOwner.mockResolvedValue(fakeMintedUtxoResult(9n, FIFTY_TWO_DOGE_KOINU));

    renderPage();
    await settle();
    // 52 DOGE at $0.08 is ~$4.16 of real collateral — borrow a small icUSD amount so the final
    // collateral ratio against the ACTUAL deposit stays comfortably above the 135% minimum
    // (the Step 1 default of 50 icUSD is sized for the hypothetical 1000 DOGE estimate, not a
    // real 52 DOGE deposit).
    setInputValue(q('#dbw-icusd') as HTMLInputElement, '1');
    await settle();
    findButtonByText('Continue with this loan')!.click();
    await settle();
    findButtonByText('I sent the DOGE')!.click();
    await settle();

    expect(host.textContent).toContain('Minted 52 DOGE worth of ckDOGE');
    // The displayed "ready to borrow" amount is honest about the reserved network fee up front —
    // never the full 52 ckDOGE the wallet holds, which would fail on submission.
    expect(host.textContent).toContain('Collateral ready for borrowing: 51.9998 ckDOGE');
    expect(host.textContent).toContain('network fee of 0.0002 ckDOGE reserved from your 52 ckDOGE');

    findButtonByText('Continue to confirm borrow')!.click();
    await settle();
    expect(host.textContent).not.toContain('Insufficient');

    // Step 1's hypothetical estimate (1000 DOGE / 1 icUSD) differs from the real 51.9998 ckDOGE
    // deposit, so the live-terms refresh flags a material change; acknowledge it like a real user
    // would before Confirm becomes available. This is orthogonal to the fee-reservation fix.
    const ackBtn = findButtonByText('I see the updated terms, continue');
    if (ackBtn) {
      ackBtn.click();
      await settle();
    }

    fx.getVaults.mockResolvedValueOnce([]).mockResolvedValueOnce([
      fakeRawVault({ vaultId: 99, collateralAmount: SAFE_COLLATERAL_KOINU, borrowedIcusd: 100_000_000n }),
    ]);
    fx.openVaultAndBorrowBound.mockResolvedValue({
      kind: 'dispatched_ok',
      vaultId: 99,
      blockIndex: 3,
      partialZeroDebtVaultId: null,
      errorMessage: null,
      approvalMayHaveMutated: true,
      submittedCollateralRaw: SAFE_COLLATERAL_KOINU,
      submittedIcusdRaw: 100_000_000n,
    });

    const confirmBtn = findButtonByText('Confirm and borrow')!;
    expect(confirmBtn.disabled).toBe(false);
    confirmBtn.click();
    await settle();

    expect(fx.openVaultAndBorrowBound).toHaveBeenCalledTimes(1);
    const [, submittedCollateralRaw] = fx.openVaultAndBorrowBound.mock.calls[0];
    // Exactly balance-minus-2x-fee — never the full 52 ckDOGE balance, and never a
    // float-rounded approximation of it.
    expect(submittedCollateralRaw).toBe(SAFE_COLLATERAL_KOINU);
    expect(host.textContent).toContain('Vault #99');
  });

  it('reads the LIVE ckDOGE ledger fee from collateral config, not a hardcoded page constant', async () => {
    // A distinctive fee, deliberately different from the 10_000 koinu used elsewhere in this
    // file and from apiClient.ts's own fallback default — proves +page.svelte wires
    // ckdogeInfo.ledgerFee through, rather than a constant that happens to match by coincidence.
    const DISTINCTIVE_FEE_KOINU = 250_000n;
    const FIFTY_TWO_DOGE_KOINU = 5_200_000_000n;
    const SAFE_COLLATERAL_KOINU = FIFTY_TWO_DOGE_KOINU - DISTINCTIVE_FEE_KOINU * 2n; // 51.5 DOGE

    fx.collateralState.set({
      collaterals: [fakeCollateralInfo({ ledgerFee: Number(DISTINCTIVE_FEE_KOINU) })],
      loading: false,
    });
    connectAs(PRINCIPAL_A, { ckDOGE: { raw: FIFTY_TWO_DOGE_KOINU, formatted: '52', usdValue: null } });
    fx.getPublicMinterActor.mockResolvedValue({
      get_doge_address: vi.fn(async () => 'DdistinctiveFeeAddress00000000001'),
      get_minter_info: vi.fn(async () => fakeMinterInfo()),
    });
    fx.updateDogeBalanceForOwner.mockResolvedValue(fakeMintedUtxoResult(11n, FIFTY_TWO_DOGE_KOINU));

    renderPage();
    await settle();
    setInputValue(q('#dbw-icusd') as HTMLInputElement, '1');
    await settle();
    findButtonByText('Continue with this loan')!.click();
    await settle();
    findButtonByText('I sent the DOGE')!.click();
    await settle();

    expect(host.textContent).toContain(`Collateral ready for borrowing: ${formatNumber(koinuToDoge(SAFE_COLLATERAL_KOINU), 8)} ckDOGE`);
    expect(host.textContent).toContain(`network fee of ${formatNumber(koinuToDoge(DISTINCTIVE_FEE_KOINU * 2n), 8)} ckDOGE reserved`);
  });

  it('never dispatches when the Web Locks API is unavailable (conservative fail-closed), and surfaces a clear message', async () => {
    connectAs(PRINCIPAL_A);
    fx.getPublicMinterActor.mockResolvedValue({
      get_doge_address: vi.fn(async () => 'DnoLocksAddress0000000000000000001'),
      get_minter_info: vi.fn(async () => fakeMinterInfo()),
    });
    fx.updateDogeBalanceForOwner.mockResolvedValue(fakeMintedUtxoResult(7n, 100_000_000_000n));

    renderPage();
    await settle();
    findButtonByText('Continue with this loan')!.click();
    await settle();
    findButtonByText('I sent the DOGE')!.click();
    await settle();
    findButtonByText('Continue to confirm borrow')!.click();
    await settle();

    // Simulate an environment without the Web Locks API.
    Object.defineProperty(navigator, 'locks', { value: undefined, configurable: true });

    const confirmBtn = findButtonByText('Confirm and borrow')!;
    confirmBtn.click();
    await settle();

    expect(fx.openVaultAndBorrowBound).not.toHaveBeenCalled();
    expect(host.textContent).toContain('does not support the safety lock');
  });

  it('fails closed before mutation when the pending intent cannot be durably saved', async () => {
    connectAs(PRINCIPAL_A);
    fx.getPublicMinterActor.mockResolvedValue({
      get_doge_address: vi.fn(async () => 'DdurabilityAddress000000000000001'),
      get_minter_info: vi.fn(async () => fakeMinterInfo()),
    });
    fx.updateDogeBalanceForOwner.mockResolvedValue(fakeMintedUtxoResult(7n, 100_000_000_000n));

    renderPage();
    await settle();
    findButtonByText('Continue with this loan')!.click();
    await settle();
    findButtonByText('I sent the DOGE')!.click();
    await settle();
    findButtonByText('Continue to confirm borrow')!.click();
    await settle();

    const originalSetItem = Storage.prototype.setItem;
    const setItemSpy = vi.spyOn(Storage.prototype, 'setItem').mockImplementation((key, value) => {
      if (key.startsWith('rumi_doge_borrow_intent_')) throw new Error('quota exceeded');
      originalSetItem.call(localStorage, key, value);
    });
    try {
      findButtonByText('Confirm and borrow')!.click();
      await settle();
    } finally {
      setItemSpy.mockRestore();
    }

    expect(fx.openVaultAndBorrowBound).not.toHaveBeenCalled();
    expect(host.textContent).toContain('could not be saved safely');
  });

  it('does not enable final consent when a strict collateral refresh fails despite warm cached terms', async () => {
    connectAs(PRINCIPAL_A);
    fx.getPublicMinterActor.mockResolvedValue({
      get_doge_address: vi.fn(async () => 'DfreshnessAddress000000000000001'),
      get_minter_info: vi.fn(async () => fakeMinterInfo()),
    });
    fx.updateDogeBalanceForOwner.mockResolvedValue(fakeMintedUtxoResult(7n, 100_000_000_000n));
    // beforeEach seeds a valid cached ckDOGE record. The strict refresh must
    // reject instead of allowing the route to mistake that cache for fresh terms.
    fx.fetchSupportedCollateral.mockImplementation(async (_force?: boolean, options?: { strict?: boolean }) => {
      if (options?.strict) throw new Error('collateral endpoint unavailable');
    });

    renderPage();
    await settle();
    findButtonByText('Continue with this loan')!.click();
    await settle();
    findButtonByText('I sent the DOGE')!.click();
    await settle();
    findButtonByText('Continue to confirm borrow')!.click();
    await settle();

    expect(fx.fetchSupportedCollateral).toHaveBeenCalledWith(true, { strict: true });
    expect(host.textContent).toContain('Could not refresh live borrowing terms');
    expect(findButtonByText('Confirm and borrow')).toBeNull();
    expect(fx.openVaultAndBorrowBound).not.toHaveBeenCalled();
  });

  it('discards a stale final-terms refresh after reconnect and waits for the new session refresh', async () => {
    connectAs(PRINCIPAL_A);
    saveIntent(localStorage, fakeIntentRecord({
      principal: PRINCIPAL_B_TEXT,
      step: 'confirm',
      collateralAmountDoge: 1000,
      icusdAmount: 50,
      vaultId: null,
      borrowConfirmed: false,
      mintedBlockIndices: ['7'],
      sessionMintedKoinu: '100000000000',
      submittedCollateralKoinu: '100000000000',
      submittedIcusdAmount: 50,
      now: Date.now(),
    }));
    fx.getPublicMinterActor.mockResolvedValue({
      get_doge_address: vi.fn(async () => 'DtermsAddress00000000000000001'),
      get_minter_info: vi.fn(async () => fakeMinterInfo()),
    });
    fx.updateDogeBalanceForOwner.mockResolvedValue(fakeMintedUtxoResult(7n, 100_000_000_000n));

    const staleCollateral = deferred<void>();
    const freshCollateral = deferred<void>();
    const staleProtocol = deferred<void>();
    const freshProtocol = deferred<void>();
    let strictRefreshes = 0;
    fx.fetchSupportedCollateral.mockImplementation(async (_force?: boolean, options?: { strict?: boolean }) => {
      if (!options?.strict) return;
      strictRefreshes += 1;
      return strictRefreshes === 1 ? staleCollateral.promise : freshCollateral.promise;
    });
    let protocolRefreshes = 0;
    fx.fetchProtocolStatus.mockImplementation(async (force?: boolean) => {
      if (!force) return;
      protocolRefreshes += 1;
      return protocolRefreshes === 1 ? staleProtocol.promise : freshProtocol.promise;
    });

    renderPage();
    await settle();
    findButtonByText('Continue with this loan')!.click();
    await settle();
    findButtonByText('I sent the DOGE')!.click();
    await settle();
    findButtonByText('Continue to confirm borrow')!.click();
    await settle();
    expect(strictRefreshes).toBe(1);

    // Reconnecting the same flow starts a new session and a second refresh.
    connectAs(PRINCIPAL_B);
    await settle();
    expect(strictRefreshes).toBeGreaterThanOrEqual(2);

    staleCollateral.resolve();
    staleProtocol.resolve();
    await settle();
    expect(findButtonByText('Confirm and borrow')).toBeNull();

    freshCollateral.resolve();
    freshProtocol.resolve();
    await settle();
    expect(findButtonByText('Confirm and borrow')).toBeTruthy();
    expect(findButtonByText('Confirm and borrow')!.disabled).toBe(false);
  });
});

describe('/doge/borrow — partial zero-debt recovery on reload', () => {
  it('never blindly resubmits open_vault_and_borrow for a saved intent that already created a zero-debt vault', async () => {
    saveIntent(
      localStorage,
      fakeIntentRecord({
        principal: PRINCIPAL_A_TEXT,
        step: 'confirm',
        collateralAmountDoge: 1000,
        icusdAmount: 50,
        vaultId: 99,
        borrowConfirmed: false,
        mintedBlockIndices: ['7'],
        sessionMintedKoinu: '100000000000',
        pendingAction: 'finish_borrow',
        partialBorrowAcknowledged: true,
        submittedCollateralKoinu: '100000000000',
        submittedIcusdAmount: 50,
        now: Date.now(),
      })
    );
    fx.getVaults.mockResolvedValue([
      fakeRawVault({ vaultId: 99, collateralAmount: 100_000_000_000n, borrowedIcusd: 0n }),
    ]);

    connectAs(PRINCIPAL_A);
    renderPage();
    await settle();

    expect(fx.openVaultAndBorrowBound).not.toHaveBeenCalled();
    expect(host.textContent).toContain('vault #99');
    const finishBtn = findButtonByText('Finish borrowing');
    expect(finishBtn).toBeTruthy();

    // The preflight check inside finishBorrowOnVault re-reads the vault before
    // ever calling borrowFromVault — still zero debt here, so it should call it.
    fx.borrowFromVaultBound.mockResolvedValue({
      kind: 'dispatched_err',
      vaultId: 99,
      blockIndex: null,
      feePaidRaw: null,
      errorMessage: 'unset',
      submittedIcusdRaw: 5_000_000_000n,
    });
    finishBtn!.click();
    await settle();

    expect(fx.borrowFromVaultBound).toHaveBeenCalledTimes(1);
    expect(fx.borrowFromVaultBound.mock.calls[0][1]).toBe(99);
    expect(fx.borrowFromVaultBound.mock.calls[0][2]).toBe(5_000_000_000n);
    // The one and only allowed recovery path is borrowFromVault on the exact
    // existing vault — a fresh open_vault_and_borrow must never fire.
    expect(fx.openVaultAndBorrowBound).not.toHaveBeenCalled();
  });

  it('never resubmits borrowFromVault when a fresh preflight shows the vault already has nonzero debt (late response landed already)', async () => {
    saveIntent(
      localStorage,
      fakeIntentRecord({
        principal: PRINCIPAL_A_TEXT,
        step: 'confirm',
        collateralAmountDoge: 1000,
        icusdAmount: 50,
        vaultId: 99,
        borrowConfirmed: false,
        pendingAction: 'finish_borrow',
        partialBorrowAcknowledged: true,
        submittedCollateralKoinu: '100000000000',
        submittedIcusdAmount: 50,
        now: Date.now(),
      })
    );
    // Reconcile's own vault lookup finds zero debt at mount time...
    fx.getVaults.mockResolvedValueOnce([
      fakeRawVault({ vaultId: 99, collateralAmount: 100_000_000_000n, borrowedIcusd: 0n }),
    ]);

    connectAs(PRINCIPAL_A);
    renderPage();
    await settle();
    const finishBtn = findButtonByText('Finish borrowing');
    expect(finishBtn).toBeTruthy();

    // ...but by the time the user clicks, the vault's fresh preflight (a SEPARATE
    // get_vaults call inside finishBorrowOnVault) shows the debt already landed
    // (e.g. a slow first borrow_from_vault call finally committed).
    fx.getVaults.mockResolvedValueOnce([
      fakeRawVault({ vaultId: 99, collateralAmount: 100_000_000_000n, borrowedIcusd: 50_00000000n }),
    ]);
    finishBtn!.click();
    await settle();

    expect(fx.borrowFromVaultBound).not.toHaveBeenCalled();
    expect(host.textContent).toContain('vault #99');
  });
});

describe('/doge/borrow — unresolved intent with no known vault id recovers read-only from the pre-action snapshot', () => {
  it('keeps an exact query candidate pending when the lost-response attempt returned no authoritative vault id', async () => {
    saveIntent(
      localStorage,
      fakeIntentRecord({
        principal: PRINCIPAL_A_TEXT,
        step: 'confirm',
        collateralAmountDoge: 1000,
        icusdAmount: 50,
        vaultId: null, // the confirm click's response never made it back to this frontend
        borrowConfirmed: false,
        mintedBlockIndices: ['7'],
        sessionMintedKoinu: '100000000000',
        pendingAction: 'open_and_borrow', // set BEFORE the mutating call, per beginPendingAction
        preActionVaultIds: [], // no ckDOGE vaults existed before this attempt
        submittedCollateralKoinu: '100000000000',
        submittedIcusdAmount: 50,
        now: Date.now(),
      })
    );
    // A vault matching this exact intent DOES exist on-chain, but query-only
    // evidence cannot attribute it to this request without the call's id.
    fx.getVaults.mockResolvedValue([
      fakeRawVault({ vaultId: 123, collateralAmount: 100_000_000_000n, borrowedIcusd: 50_00000000n }),
    ]);

    connectAs(PRINCIPAL_A);
    renderPage();
    await settle();

    expect(fx.getVaults).toHaveBeenCalled();
    expect(host.textContent).toContain('possible matching vault');
    expect(findButtonByText('Recheck on-chain')).toBeTruthy();
    expect(fx.openVaultAndBorrowBound).not.toHaveBeenCalled();

    const stored = JSON.parse(localStorage.getItem(storageKeyForPrincipal(PRINCIPAL_A_TEXT, TEST_NETWORK_SCOPE))!);
    expect(stored.vaultId).toBeNull();
    expect(stored.borrowConfirmed).toBe(false);
    expect(stored.pendingAction).toBe('open_and_borrow');
  });

  it('surfaces a read-only Recheck action (not another submit) when the same lost-response intent has no matching vault yet', async () => {
    saveIntent(
      localStorage,
      fakeIntentRecord({
        principal: PRINCIPAL_A_TEXT,
        step: 'confirm',
        collateralAmountDoge: 1000,
        icusdAmount: 50,
        vaultId: null,
        borrowConfirmed: false,
        pendingAction: 'open_and_borrow',
        preActionVaultIds: [],
        submittedCollateralKoinu: '100000000000',
        submittedIcusdAmount: 50,
        now: Date.now(),
      })
    );
    fx.getVaults.mockResolvedValue([]); // nothing has landed on-chain (yet, or ever)

    connectAs(PRINCIPAL_A);
    renderPage();
    await settle();

    expect(fx.openVaultAndBorrowBound).not.toHaveBeenCalled();
    const recheckBtn = findButtonByText('Recheck on-chain');
    expect(recheckBtn).toBeTruthy();
    expect(findButtonByText('Confirm and borrow')).toBeNull();
  });
});

describe('/doge/borrow — returning position management', () => {
  it("reuses the real VaultCard, filtered to this owner's ckDOGE vaults, on the done step, not a bare /vaults handoff", async () => {
    saveIntent(
      localStorage,
      fakeIntentRecord({
        principal: PRINCIPAL_A_TEXT,
        step: 'done',
        collateralAmountDoge: 1000,
        icusdAmount: 50,
        vaultId: 55,
        borrowConfirmed: true,
        now: Date.now(),
      })
    );
    // Two vaults belong to this account: the matching ckDOGE one, and an
    // unrelated ICP-collateral vault that must be filtered OUT of this view.
    // Pre-seeded (rather than populated inside the fetchUserVaults mock) so the
    // component's very first $userVaults notification already carries the
    // real data — mirroring how isConnected/principal are seeded in every
    // other scenario here, and avoiding a reactive-store-update-from-within-
    // a-reactive-block ordering edge case in this harness.
    fx.appDataState.update((s) => ({
      ...s,
      userVaults: [
        fakeVaultDto({ vaultId: 55, owner: PRINCIPAL_A_TEXT, collateralAmount: 1000, borrowedIcusd: 42 }),
        fakeVaultDto({
          vaultId: 77,
          owner: PRINCIPAL_A_TEXT,
          collateralType: 'ryjl3-tyaaa-aaaaa-aaaba-cai',
          collateralSymbol: 'ICP',
          collateralAmount: 500,
          borrowedIcusd: 10,
        }),
      ],
    }));

    connectAs(PRINCIPAL_A);
    renderPage();
    await settle();

    expect(fx.fetchUserVaults).toHaveBeenCalledTimes(1);
    const [calledOwner] = fx.fetchUserVaults.mock.calls[0];
    expect((calledOwner as { toText: () => string }).toText()).toBe(PRINCIPAL_A_TEXT);
    const cards = qAll('[data-testid="fake-vault-card"]');
    expect(cards).toHaveLength(1);
    expect(cards[0].getAttribute('data-vault-id')).toBe('55');
    expect(cards[0].getAttribute('data-collateral-type')).toBe(CKDOGE_LEDGER_TEXT);
    // The reconciled on-chain debt (42) is available on the position view via
    // the real VaultCard data, distinct from the locally-cached Step-1 estimate (50).
    expect(cards[0].getAttribute('data-borrowed-icusd')).toBe('42');
  });
});

describe('/doge/borrow — duplicate mint receipt and existing-balance protection', () => {
  it('does not double-count a re-observed Minted UTXO, and never silently uses pre-existing wallet ckDOGE without explicit opt-in', async () => {
    connectAs(PRINCIPAL_A, { ckDOGE: { raw: 50_000_000_000n, formatted: '500', usdValue: 40 } });
    fx.getPublicMinterActor.mockResolvedValue({
      get_doge_address: vi.fn(async () => 'DdupReceiptAddress000000000000001'),
      get_minter_info: vi.fn(async () => fakeMinterInfo()),
    });
    fx.updateDogeBalanceForOwner.mockResolvedValue(fakeMintedUtxoResult(7n, 100_000_000_000n));

    renderPage();
    await settle();
    findButtonByText('Continue with this loan')!.click();
    await settle();
    findButtonByText('I sent the DOGE')!.click();
    await settle();

    // Only the session's own 1000 DOGE mint counts — not the pre-existing 500 ckDOGE.
    expect(host.textContent).toContain('1000 DOGE');
    expect(host.textContent).not.toContain('1500');

    const record = JSON.parse(localStorage.getItem(storageKeyForPrincipal(PRINCIPAL_A_TEXT, TEST_NETWORK_SCOPE))!);
    expect(record.sessionMintedKoinu).toBe('100000000000');

    // Re-poll observes the SAME UTXO again (e.g. a second manual recheck
    // after this already stopped) — must dedup, not double the total.
    fx.updateDogeBalanceForOwner.mockResolvedValue(fakeMintedUtxoResult(7n, 100_000_000_000n));
    const recheckBtn = findButtonByText('Check again');
    if (recheckBtn) {
      recheckBtn.click();
      await settle();
    }
    const recordAfter = JSON.parse(localStorage.getItem(storageKeyForPrincipal(PRINCIPAL_A_TEXT, TEST_NETWORK_SCOPE))!);
    expect(recordAfter.sessionMintedKoinu).toBe('100000000000');
    expect(recordAfter.mintedBlockIndices).toEqual(['7']);
  });
});
