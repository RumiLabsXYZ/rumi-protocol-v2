import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import { CANISTER_IDS, CONFIG } from '$lib/config';
import { BITCOIN_ADDRESS, OWNER_A, OWNER_A_TEXT, OWNER_B, OWNER_B_TEXT, deferred, inputValue, settle } from '../../../tests/bitcoin-minter-fixtures/testData';

const fx = vi.hoisted(() => {
  function store<T>(value: T) {
    const subs = new Set<(v: T) => void>();
    return { subscribe(run: (v: T) => void) { subs.add(run); run(value); return () => subs.delete(run); }, set(next: T) { value = next; subs.forEach((run) => run(value)); } };
  }
  const connected = store(false);
  const principal = store<any>(null);
  return {
    connected, principal,
    minter: vi.fn(async () => ({
      get_minter_info: vi.fn(async () => ({ min_confirmations: 4, deposit_btc_min_amount: [10_000n], retrieve_btc_min_amount: 1_000n })),
      get_deposit_fee: vi.fn(async () => 10n),
      get_btc_address: vi.fn(async () => BITCOIN_ADDRESS),
      estimate_withdrawal_fee: vi.fn(async () => ({ bitcoin_fee: 100n, minter_fee: 200n })),
      retrieve_btc_status_v2: vi.fn(async () => ({ Pending: null })),
      retrieve_btc_status_v2_by_account: vi.fn(async () => []),
    })),
    ledger: vi.fn(async () => ({ icrc1_balance_of: vi.fn(async () => 1_000_000n) })),
    update: vi.fn(async () => ({ Ok: [] })),
    submit: vi.fn(async () => ({ kind: 'retrieve-uncertain', approveBlockIndex: 5n, message: 'response lost' })),
    fee: vi.fn(async () => 10n),
    qr: vi.fn(async () => 'data:image/png;base64,fake'),
  };
});

vi.mock('$lib/stores/wallet', () => ({ isConnected: { subscribe: fx.connected.subscribe }, principal: { subscribe: fx.principal.subscribe } }));
vi.mock('$lib/services/ckbtcMinterActors', () => ({ getPublicCkbtcMinterActor: fx.minter, getPublicCkbtcLedgerActor: fx.ledger,
  updateBtcBalanceForOwner: fx.update, submitCkbtcWithdrawal: fx.submit }));
vi.mock('$lib/services/ledgerFeeService', () => ({ fetchLedgerFeeStrict: fx.fee }));
vi.mock('qrcode', () => ({ default: { toDataURL: fx.qr } }));
import Page from '../../routes/bitcoin/+page.svelte';

let host: HTMLDivElement;
let instance: unknown;
function connect(owner: typeof OWNER_A | typeof OWNER_B = OWNER_A) { fx.principal.set(owner); fx.connected.set(true); }
function disconnect() { fx.connected.set(false); fx.principal.set(null); }
function button(text: string): HTMLButtonElement | null { return [...host.querySelectorAll('button')].find((b) => b.textContent?.includes(text)) as HTMLButtonElement ?? null; }
function render() { instance = mount(Page, { target: host }); flushSync(); }
function actorDefaults() {
  return { get_minter_info: vi.fn(async () => ({ min_confirmations: 4, deposit_btc_min_amount: [10_000n], retrieve_btc_min_amount: 1_000n })),
    get_deposit_fee: vi.fn(async () => 10n), get_btc_address: vi.fn(async () => BITCOIN_ADDRESS),
    estimate_withdrawal_fee: vi.fn(async () => ({ bitcoin_fee: 100n, minter_fee: 200n })),
    retrieve_btc_status_v2: vi.fn(async () => ({ Pending: null })), retrieve_btc_status_v2_by_account: vi.fn(async () => []) };
}
function withdrawalKey(owner: string) { return `rumi:ckbtc-withdrawal:${CONFIG.isLocal ? `local:${CONFIG.host}` : 'mainnet'}:${CANISTER_IDS.CKBTC_LEDGER}:${CANISTER_IDS.CKBTC_MINTER}:${owner}`; }
function installLocks() {
  Object.defineProperty(navigator, 'locks', { configurable: true, value: { request: async (_key: string, _opts: unknown, callback: (lock: unknown) => Promise<unknown>) => callback({}) } });
}
function reset() {
  disconnect(); localStorage.clear(); installLocks();
  for (const fn of [fx.minter, fx.ledger, fx.update, fx.submit, fx.fee, fx.qr]) fn.mockReset();
  fx.minter.mockResolvedValue(actorDefaults()); fx.ledger.mockResolvedValue({ icrc1_balance_of: vi.fn(async () => 1_000_000n) });
  fx.update.mockResolvedValue({ Ok: [] }); fx.submit.mockResolvedValue({ kind: 'retrieve-uncertain', approveBlockIndex: 5n, message: 'response lost' });
  fx.fee.mockResolvedValue(10n); fx.qr.mockResolvedValue('data:image/png;base64,fake');
}
beforeEach(() => { host = document.createElement('div'); document.body.appendChild(host); reset(); });
afterEach(() => { if (instance) { unmount(instance as any); instance = undefined; } host.remove(); localStorage.clear(); vi.restoreAllMocks(); });

async function prepareWithdrawal() {
  connect(); render(); await settle();
  button('Redeem BTC')!.click(); await settle();
  const inputs = [...host.querySelectorAll('input')] as HTMLInputElement[];
  inputValue(inputs[0], BITCOIN_ADDRESS); inputValue(inputs[1], '0.001'); await settle();
  button('Refresh fee estimate')!.click(); await settle();
  return button('Approve and request BTC')!;
}

describe('/bitcoin minter mounted flow', () => {
  it('does not paint an old owner QR result after the connected account changes', async () => {
    connect(OWNER_A);
    const qr = deferred<string>(); fx.qr.mockReturnValue(qr.promise);
    fx.minter.mockResolvedValue({ ...actorDefaults(), get_btc_address: vi.fn(async () => BITCOIN_ADDRESS) });
    render(); await settle(); button('Get Bitcoin deposit address')!.click(); await settle();
    expect(fx.qr).toHaveBeenCalledTimes(1);
    connect(OWNER_B); await settle();
    qr.resolve('data:image/png;base64,stale-owner-a'); await settle();
    expect(host.textContent).not.toContain(OWNER_A_TEXT);
    expect(host.textContent).not.toContain('stale-owner-a');
    expect(host.querySelector('img.qr')).toBeNull();
    expect(button('Get Bitcoin deposit address')).toBeTruthy();
  });

  it('persists an uncertain withdrawal, blocks repeat dispatch, and restores the block after remount', async () => {
    const submit = await prepareWithdrawal();
    expect(submit.disabled).toBe(false);
    submit.click(); await settle();
    expect(fx.submit).toHaveBeenCalledTimes(1);
    expect(host.textContent).toContain('Previous result needs reconciliation');
    const savedKey = withdrawalKey(OWNER_A_TEXT);
    expect(JSON.parse(localStorage.getItem(savedKey)!).state).toBe('uncertain');
    expect(button('Resolve previous request first')?.disabled).toBe(true);
    unmount(instance as any); instance = undefined;
    render(); await settle(); button('Redeem BTC')!.click(); await settle();
    expect(host.textContent).toContain('uncertain result');
    expect(button('Resolve previous request first')?.disabled).toBe(true);
    expect(fx.submit).toHaveBeenCalledTimes(1);
  });

  it('restores an accepted request by status block without dispatching another withdrawal', async () => {
    const key = withdrawalKey(OWNER_A_TEXT);
    localStorage.setItem(key, JSON.stringify({ owner: OWNER_A_TEXT, address: BITCOIN_ADDRESS, amount: '100000', startedAt: Date.now(), attemptId: 'accepted-fixture-45', state: 'accepted', blockIndex: '45' }));
    const status = vi.fn(async ({ block_index }: { block_index: bigint }) => { expect(block_index).toBe(45n); return { Pending: null }; });
    fx.minter.mockResolvedValue({ ...actorDefaults(), retrieve_btc_status_v2: status });
    connect(); render(); await settle(); button('Redeem BTC')!.click(); await settle();
    expect(host.textContent).toContain('Request block 45');
    expect(host.textContent).toContain('Queued by the minter');
    expect(status).toHaveBeenCalledTimes(1);
    expect(fx.submit).not.toHaveBeenCalled();
    expect(button('Resolve previous request first')?.disabled).toBe(true);
  });

  it('expires a fee quote at 60 seconds and disables withdrawal until refreshed', async () => {
    let now = 1_800_000_000_000;
    vi.spyOn(Date, 'now').mockImplementation(() => now);
    const submit = await prepareWithdrawal();
    expect(submit.disabled).toBe(false);
    now += 60_001;
    await new Promise((resolve) => setTimeout(resolve, 1_100));
    await settle();
    expect(host.textContent).toContain('Enter a valid address and amount, then refresh.');
    expect(button('Approve and request BTC')!.disabled).toBe(true);
    expect(fx.submit).not.toHaveBeenCalled();
  });

  it('invalidates an in-flight quote after disconnect and reconnect of the same principal', async () => {
    connect(); render(); await settle(); button('Redeem BTC')!.click(); await settle();
    const estimate = deferred<{ bitcoin_fee: bigint; minter_fee: bigint }>();
    fx.minter.mockResolvedValue({ ...actorDefaults(), estimate_withdrawal_fee: vi.fn(() => estimate.promise) });
    const inputs = [...host.querySelectorAll('input')] as HTMLInputElement[];
    inputValue(inputs[0], BITCOIN_ADDRESS); inputValue(inputs[1], '0.001'); await settle();
    button('Refresh fee estimate')!.click(); await settle();
    disconnect(); await settle(); connect(OWNER_A); await settle();
    estimate.resolve({ bitcoin_fee: 100n, minter_fee: 200n }); await settle();
    expect(button('Approve and request BTC')!.disabled).toBe(true);
    expect(host.textContent).toContain('Enter a valid address and amount, then refresh.');
  });
});
