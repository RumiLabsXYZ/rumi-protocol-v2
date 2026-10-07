import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import { Principal } from '@dfinity/principal';
import {
  CKBTC_LEDGER_TEXT, PRINCIPAL_A, PRINCIPAL_B, deferred, fakeCollateralInfo, fakeMinterInfo, fakeMinted,
  fakeRawVault, settle, setInputValue,
} from '../../../tests/bitcoin-borrow-fixtures/testData';

const fx = vi.hoisted(() => {
  function store<T>(value: T) {
    const subs = new Set<(v: T) => void>();
    return { subscribe(run: (v: T) => void) { subs.add(run); run(value); return () => subs.delete(run); },
      set(next: T) { value = next; subs.forEach((run) => run(value)); } };
  }
  const wallet = store<any>({ isConnected: false, principal: null, balance: null, error: null, loading: false, icon: '', tokenBalances: {} });
  const collateral = store<any>({ collaterals: [], loading: false, lastFetch: Date.now(), error: null });
  const app = store<any>({ protocolStatus: null, userVaults: [] });
  const derived = (s: any, f: (x: any) => any) => ({ subscribe(run: (v: any) => void) { return s.subscribe((x: any) => run(f(x))); } });
  return {
    wallet, collateral, app, derived,
    connect: vi.fn(async () => {}), disconnect: vi.fn(async () => {}), refreshBalance: vi.fn(async () => {}),
    fetchCollateral: vi.fn(async () => {}), fetchStatus: vi.fn(async () => {}), refreshAll: vi.fn(async () => {}), fetchVaults: vi.fn(async () => []),
    openV2: vi.fn(async () => ({ kind: 'ambiguous_transport', status: null, errorMessage: 'unset', approvalMayHaveMutated: false })),
    getIngressState: vi.fn(async () => ({ next_request_id: 1n, active_request: [], latest_result: [] })),
    getIngress: vi.fn(async () => null),
    borrow: vi.fn(async () => ({})), getVaults: vi.fn(async () => []),
    getActor: vi.fn(async () => ({ get_btc_address: vi.fn(async () => 'bc1qfixtureaddress000000000000000000000000000000'), get_minter_info: vi.fn(async () => fakeMinterInfo()) })),
    update: vi.fn(async () => ({ Ok: [] })), qr: vi.fn(async () => 'data:image/png;base64,fake'),
  };
});

vi.mock('$lib/stores/wallet', () => ({ walletStore: { subscribe: fx.wallet.subscribe, connect: fx.connect, disconnect: fx.disconnect, refreshBalance: fx.refreshBalance },
  isConnected: fx.derived(fx.wallet, (s: any) => s.isConnected), principal: fx.derived(fx.wallet, (s: any) => s.principal) }));
vi.mock('$lib/stores/collateralStore', () => ({ collateralStore: { subscribe: fx.collateral.subscribe, fetchSupportedCollateral: fx.fetchCollateral } }));
vi.mock('$lib/stores/appDataStore', () => ({ appDataStore: { subscribe: fx.app.subscribe, fetchProtocolStatus: fx.fetchStatus, refreshAll: fx.refreshAll,
  fetchUserVaults: fx.fetchVaults }, protocolStatus: fx.derived(fx.app, (s: any) => s.protocolStatus), userVaults: fx.derived(fx.app, (s: any) => s.userVaults) }));
vi.mock('$lib/services/protocol', () => ({ protocolService: {
  openVaultV2Bound: fx.openV2,
  getCollateralIngressStateBound: fx.getIngressState,
  getCollateralIngressBound: fx.getIngress,
  borrowFromVaultBound: fx.borrow,
} }));
vi.mock('$lib/services/protocol/apiClient', () => ({ publicActor: { get_vaults: fx.getVaults } }));
vi.mock('$lib/services/auth', () => ({ WALLET_TYPES: { INTERNET_IDENTITY: 'internet-identity', OISY: 'oisy' } }));
vi.mock('$lib/services/ckbtcMinterActors', () => ({ getPublicCkbtcMinterActor: fx.getActor, updateBtcBalanceForOwner: fx.update }));
vi.mock('qrcode', () => ({ default: { toDataURL: fx.qr } }));
vi.mock('$lib/components/vault/VaultCard.svelte', async () => ({ default: (await import('../../../tests/doge-borrow-fixtures/FakeVaultCard.svelte')).default }));
import Page from '../../routes/bitcoin/borrow/+page.svelte';

let host: HTMLDivElement;
let instance: unknown;
function connect(principal = PRINCIPAL_A, tokenBalances: Record<string, { raw: bigint; formatted: string; usdValue: number | null }> = {}) { fx.wallet.set({ isConnected: true, principal, balance: null, error: null, loading: false, icon: '', tokenBalances }); }
function button(label: string): HTMLButtonElement | null { return [...host.querySelectorAll('button')].find((b) => b.textContent?.includes(label)) as HTMLButtonElement ?? null; }
function render() { instance = mount(Page, { target: host }); flushSync(); }
function installLocks() {
  Object.defineProperty(navigator, 'locks', { configurable: true, value: { request: async (_name: string, _opts: unknown, fn: (lock: unknown) => Promise<unknown>) => fn({}) } });
}
function completeOpenStatus(vaultId: number, amountRaw: bigint, requestId = 1n) {
  return {
    request_id: requestId,
    owner: Principal.fromText(PRINCIPAL_A.toText()),
    ledger: Principal.fromText(CKBTC_LEDGER_TEXT),
    operation: { Open: { collateral_type: Principal.fromText(CKBTC_LEDGER_TEXT) } },
    phase: { Complete: null },
    amount_raw: amountRaw,
    fee_raw: 10_000n,
    memo: [],
    created_at_time_ns: 1n,
    candidate_block_index: [9n],
    result: [{ Open: { vault_id: BigInt(vaultId), block_index: 9n } }],
    had_ambiguous_attempt: false,
    last_error: [],
  };
}
function reset() {
  fx.wallet.set({ isConnected: false, principal: null, balance: null, error: null, loading: false, icon: '', tokenBalances: {} });
  fx.collateral.set({ collaterals: [fakeCollateralInfo()], loading: false, lastFetch: Date.now(), error: null });
  fx.app.set({ protocolStatus: { globalIcusdMintCap: 1_000_000, borrowingFeeCurveResolved: [] }, userVaults: [] });
  for (const fn of [fx.connect, fx.disconnect, fx.refreshBalance, fx.fetchCollateral, fx.fetchStatus, fx.refreshAll, fx.fetchVaults, fx.openV2, fx.getIngressState, fx.getIngress, fx.borrow, fx.getVaults, fx.getActor, fx.update, fx.qr]) fn.mockReset();
  fx.connect.mockResolvedValue(undefined); fx.refreshBalance.mockResolvedValue(undefined); fx.fetchCollateral.mockResolvedValue(undefined);
  fx.fetchStatus.mockResolvedValue(undefined); fx.refreshAll.mockResolvedValue(undefined); fx.fetchVaults.mockResolvedValue([]); fx.getVaults.mockResolvedValue([]);
  fx.openV2.mockResolvedValue({ kind: 'ambiguous_transport', status: null, errorMessage: 'uncertain', approvalMayHaveMutated: true });
  fx.getIngressState.mockResolvedValue({ next_request_id: 1n, active_request: [], latest_result: [] });
  fx.getIngress.mockResolvedValue(null);
  fx.getActor.mockResolvedValue({ get_btc_address: vi.fn(async () => 'bc1qfixtureaddress000000000000000000000000000000'), get_minter_info: vi.fn(async () => fakeMinterInfo()) });
  fx.update.mockResolvedValue({ Ok: [] }); fx.qr.mockResolvedValue('data:image/png;base64,fake');
  localStorage.clear(); installLocks();
}
beforeEach(() => { host = document.createElement('div'); document.body.appendChild(host); reset(); });
afterEach(() => { if (instance) { unmount(instance as any); instance = undefined; } host.remove(); localStorage.clear(); });

describe('/bitcoin/borrow mounted boundary', () => {
  it('fails closed unless active ckBTC collateral has live risk terms', async () => {
    fx.collateral.set({ collaterals: [fakeCollateralInfo({ principal: 'not-ckbtc' })], loading: false, lastFetch: Date.now(), error: null });
    render(); await settle();
    const next = button('Continue with this loan')!;
    expect(next.disabled).toBe(true);
    expect(host.textContent).toContain('ckBTC collateral is not available');
    expect(fx.getActor).not.toHaveBeenCalled();
    fx.collateral.set({ collaterals: [fakeCollateralInfo({ status: 'Inactive' })], loading: false, lastFetch: Date.now(), error: null }); await settle();
    expect(button('Continue with this loan')!.disabled).toBe(true);
  });

  it('uses the public ckBTC minter address/info boundary with the connected owner and displays four confirmations', async () => {
    connect();
    const getAddress = vi.fn(async (account?: unknown) => { expect(account).toEqual({ owner: [PRINCIPAL_A], subaccount: [] }); return 'bc1qfixtureaddress000000000000000000000000000000'; });
    fx.getActor.mockResolvedValue({ get_btc_address: getAddress, get_minter_info: vi.fn(async () => ({ min_confirmations: 4, deposit_btc_min_amount: [10_000n], retrieve_btc_min_amount: 10_000n })) });
    render(); await settle();
    button('Continue with this loan')!.click(); await settle();
    expect(host.textContent).toContain('Send BTC to your address');
    expect(host.textContent).toContain('4');
    expect(fx.getActor).toHaveBeenCalledTimes(1);
    expect(getAddress).toHaveBeenCalledTimes(1);
    expect(fx.update).not.toHaveBeenCalled();
  });

  it('opens with exact ckBTC raw amount, then borrows only after a separate explicit click', async () => {
    connect();
    fx.getActor.mockResolvedValue({ get_btc_address: vi.fn(async () => 'bc1qfixtureaddress000000000000000000000000000000'), get_minter_info: vi.fn(async () => fakeMinterInfo()) });
    fx.update.mockResolvedValue(fakeMinted(1_000_000n, 77n) as any);
    fx.getVaults.mockResolvedValueOnce([] as any)
      .mockResolvedValueOnce([] as any)
      .mockResolvedValueOnce([fakeRawVault(41, 999_980n, 0n)] as any)
      .mockResolvedValueOnce([fakeRawVault(41, 999_980n, 0n)] as any)
      .mockResolvedValueOnce([fakeRawVault(41, 999_980n, 5_000_000_000n)] as any);
    fx.openV2.mockResolvedValue({ kind: 'dispatched_ok', status: completeOpenStatus(41, 999_980n), errorMessage: null,
      approvalMayHaveMutated: true } as any);
    render(); await settle(); button('Continue with this loan')!.click(); await settle();
    button('I sent the BTC')!.click(); await settle();
    expect(fx.update).toHaveBeenCalledWith(PRINCIPAL_A, expect.any(Function));
    expect(host.textContent).toContain('Minted 0.01 BTC worth of ckBTC');
    button('Continue to confirm borrow')!.click(); await settle();
    const confirm = button('Confirm and open vault'); expect(confirm).toBeTruthy();
    confirm!.click(); await settle();
    expect(fx.openV2).toHaveBeenCalledTimes(1);
    expect(fx.borrow).not.toHaveBeenCalled();
    const [, requestId, satoshi, collateralPrincipal] = fx.openV2.mock.calls[0] as unknown as [unknown, bigint, bigint, string];
    expect(requestId).toBe(1n);
    expect(satoshi).toBe(999_980n);
    expect(collateralPrincipal).toBe(CKBTC_LEDGER_TEXT);
    expect(host.textContent).toContain('Vault #41');
    const borrow = button('Finish borrowing 50 icUSD'); expect(borrow).toBeTruthy();
    fx.borrow.mockResolvedValue({ kind: 'dispatched_ok', vaultId: 41, blockIndex: 10, feePaidRaw: 1n,
      errorMessage: null, submittedIcusdRaw: 5_000_000_000n } as any);
    borrow!.click(); await settle();
    expect(fx.borrow).toHaveBeenCalledWith(expect.anything(), 41, 5_000_000_000n);
    expect(host.textContent).toContain('Vault #41 opened');
    expect(host.textContent).toContain('Debt on the vault is');
  });

  it('does not redispatch after an ambiguous open response and retains its principal-scoped pending intent', async () => {
    connect();
    fx.getActor.mockResolvedValue({ get_btc_address: vi.fn(async () => 'bc1qfixtureaddress000000000000000000000000000000'), get_minter_info: vi.fn(async () => fakeMinterInfo()) });
    fx.update.mockResolvedValue(fakeMinted(1_000_000n, 88n) as any);
    fx.openV2.mockResolvedValue({ kind: 'ambiguous_transport', status: null,
      errorMessage: 'response lost', approvalMayHaveMutated: true });
    render(); await settle(); button('Continue with this loan')!.click(); await settle(); button('I sent the BTC')!.click(); await settle();
    button('Continue to confirm borrow')!.click(); await settle(); button('Confirm and open vault')!.click(); await settle();
    expect(fx.openV2).toHaveBeenCalledTimes(1);
    const entries = Object.keys(localStorage).filter((key) => key.includes(PRINCIPAL_A.toText()));
    expect(entries).toHaveLength(1);
    expect(JSON.parse(localStorage.getItem(entries[0])!).pendingAction).toBe('open_and_borrow');
    expect(JSON.parse(localStorage.getItem(entries[0])!).openRequestId).toBe('1');
    expect(button('Confirm and open vault')).toBeNull();
    expect(fx.borrow).not.toHaveBeenCalled();
    expect(host.textContent).toMatch(/uncertain|recheck|check/i);
  });

  it('refreshes stale borrowing terms and restores calculator readiness', async () => {
    fx.collateral.set({ collaterals: [fakeCollateralInfo()], loading: false, lastFetch: Date.now() - 31_000, error: null });
    let calls = 0;
    fx.fetchCollateral.mockImplementation(async () => {
      calls++;
      if (calls > 1) fx.collateral.set({ collaterals: [fakeCollateralInfo()], loading: false, lastFetch: Date.now(), error: null });
    });
    render(); await settle();
    expect(host.textContent).toMatch(/stale|fresh backend data/i);
    expect(button('Continue with this loan')!.disabled).toBe(true);
    button('Refresh borrowing terms')!.click(); await settle();
    expect(host.textContent).not.toContain('Live price or borrowing terms need a refresh.');
    expect(button('Refresh borrowing terms')).toBeNull();
    expect(button('Continue with this loan')!.disabled).toBe(false);
    expect(fx.fetchCollateral).toHaveBeenCalledWith(true, { strict: true });
  });

  it('requires explicit opt-in before using pre-existing ckBTC and submits only that balance minus fees', async () => {
    const walletBalance = 2_000_000n;
    connect(PRINCIPAL_A, { ckBTC: { raw: walletBalance, formatted: '0.02', usdValue: 1200 } });
    fx.getActor.mockResolvedValue({ get_btc_address: vi.fn(async () => 'bc1qfixtureaddress000000000000000000000000000000'), get_minter_info: vi.fn(async () => fakeMinterInfo()) });
    fx.update.mockResolvedValue({ Err: { NoNewUtxos: { current_confirmations: [0], required_confirmations: 4, pending_utxos: [[]] } } } as any);
    fx.getVaults.mockResolvedValueOnce([] as any).mockResolvedValueOnce([] as any)
      .mockResolvedValueOnce([fakeRawVault(62, 1_999_980n, 0n)] as any);
    fx.openV2.mockResolvedValue({ kind: 'dispatched_ok', status: completeOpenStatus(62, 1_999_980n), errorMessage: null,
      approvalMayHaveMutated: true } as any);
    render(); await settle(); button('Continue with this loan')!.click(); await settle();
    button('I sent the BTC')!.click(); await settle();
    expect(host.textContent).not.toContain('Collateral ready for borrowing:');
    expect(button('Use existing ckBTC balance')).toBeTruthy();
    button('Use existing ckBTC balance')!.click(); await settle();
    expect(host.textContent).toContain('from your explicitly selected wallet balance');
    expect(host.textContent).toContain('0.0199998 ckBTC');
    button('Continue to confirm borrow')!.click(); await settle();
    const ack = button('I see the updated terms, continue'); if (ack) { ack.click(); await settle(); }
    button('Confirm and open vault')!.click(); await settle();
    expect(fx.openV2).toHaveBeenCalledTimes(1);
    const [, , submittedCollateral] = fx.openV2.mock.calls[0] as unknown as [unknown, bigint, bigint];
    expect(submittedCollateral).toBe(walletBalance - 20n);
  });

  it('keeps a zero-debt vault ambiguous when the borrow error does not prove mint dispatch was skipped', async () => {
    connect();
    fx.getActor.mockResolvedValue({ get_btc_address: vi.fn(async () => 'bc1qfixtureaddress000000000000000000000000000000'), get_minter_info: vi.fn(async () => fakeMinterInfo()) });
    fx.update.mockResolvedValue(fakeMinted(1_000_000n, 99n) as any);
    fx.getVaults.mockResolvedValueOnce([] as any).mockResolvedValueOnce([] as any)
      .mockResolvedValueOnce([fakeRawVault(53, 999_980n, 0n)] as any);
    fx.openV2.mockResolvedValue({ kind: 'ambiguous_transport', status: null,
      errorMessage: 'response lost', approvalMayHaveMutated: true } as any);
    render(); await settle(); button('Continue with this loan')!.click(); await settle();
    button('I sent the BTC')!.click(); await settle(); button('Continue to confirm borrow')!.click(); await settle();
    button('Confirm and open vault')!.click(); await settle();
    expect(fx.openV2).toHaveBeenCalledTimes(1);
    expect(host.textContent).toMatch(/recheck|uncertain|confirm/i);
    const persisted = Object.values(localStorage).map((raw) => JSON.parse(String(raw))).find((r: any) => r?.principal === PRINCIPAL_A.toText());
    expect(persisted.vaultId).toBeNull();
    expect(persisted.pendingAction).toBe('open_and_borrow');
    expect(persisted.partialBorrowAcknowledged).toBe(false);
    expect(button('Finish borrowing')).toBeNull();
    expect(fx.borrow).not.toHaveBeenCalled();
  });

  it('does not commit a delayed minter address or QR result after page teardown', async () => {
    const address = deferred<string>();
    fx.getActor.mockResolvedValue({ get_btc_address: vi.fn(() => address.promise), get_minter_info: vi.fn(async () => fakeMinterInfo()) });
    connect(); render(); await settle(); button('Continue with this loan')!.click(); await settle();
    unmount(instance as any); instance = undefined;
    address.resolve('bc1qafterteardown0000000000000000000000000000000');
    await settle();
    expect(host.textContent).not.toContain('bc1qafterteardown');
  });

  it('ignores an old address result after the connected principal changes', async () => {
    const oldAddress = deferred<string>();
    const getAddress = vi.fn().mockReturnValueOnce(oldAddress.promise).mockResolvedValueOnce('bc1qfreshaddress0000000000000000000000000000000');
    fx.getActor.mockResolvedValue({ get_btc_address: getAddress, get_minter_info: vi.fn(async () => fakeMinterInfo()) });
    connect(PRINCIPAL_A); render(); await settle(); button('Continue with this loan')!.click(); await settle();
    connect(PRINCIPAL_B); await settle();
    oldAddress.resolve('bc1qstaleaddress0000000000000000000000000000000'); await settle();
    expect(host.textContent).not.toContain('bc1qstaleaddress');
    expect(host.textContent).toContain('bc1qfreshaddress');
  });
});
