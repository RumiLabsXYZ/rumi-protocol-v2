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
  const session = store(0);
  return {
    connected, principal, session,
    minter: vi.fn(async (): Promise<any> => ({
      get_minter_info: vi.fn(async () => ({ min_confirmations: 4, deposit_btc_min_amount: [10_000n], retrieve_btc_min_amount: 1_000n })),
      get_deposit_fee: vi.fn(async () => 10n),
      get_btc_address: vi.fn(async () => BITCOIN_ADDRESS),
      estimate_withdrawal_fee: vi.fn(async () => ({ bitcoin_fee: 100n, minter_fee: 200n })),
      retrieve_btc_status_v2: vi.fn(async () => ({ Pending: null })),
      retrieve_btc_status_v2_by_account: vi.fn(async () => []),
    })),
    ledger: vi.fn(async () => ({ icrc1_balance_of: vi.fn(async () => 1_000_000n) })),
    update: vi.fn(async (_owner?: any, _isLive?: () => boolean): Promise<any> => ({ Ok: [] })),
    submit: vi.fn(async () => ({ kind: 'retrieve-uncertain', approveBlockIndex: 5n, message: 'response lost' })),
    fee: vi.fn(async () => 10n),
    qr: vi.fn(async () => 'data:image/png;base64,fake'),
    beginAction: vi.fn(() => ({ ready: Promise.resolve(), close: vi.fn(async () => {}) })),
  };
});

vi.mock('$lib/stores/wallet', () => ({ isConnected: { subscribe: fx.connected.subscribe }, principal: { subscribe: fx.principal.subscribe } }));
vi.mock('$lib/services/ckbtcMinterActors', () => ({ getPublicCkbtcMinterActor: fx.minter, getPublicCkbtcLedgerActor: fx.ledger,
  updateBtcBalanceForOwner: fx.update, submitCkbtcWithdrawal: fx.submit }));
vi.mock('$lib/services/ledgerFeeService', () => ({ fetchLedgerFeeStrict: fx.fee }));
vi.mock('qrcode', () => ({ default: { toDataURL: fx.qr } }));
vi.mock('$lib/services/minterOisySigner', () => ({ beginMinterOisyAction: fx.beginAction }));
vi.mock('$lib/services/protocol/walletOperations', () => ({ isOisyWallet: () => true }));
vi.mock('$lib/services/auth', () => ({ walletSessionGeneration: fx.session }));
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
  disconnect(); fx.session.set(0); localStorage.clear(); installLocks();
  for (const fn of [fx.minter, fx.ledger, fx.update, fx.submit, fx.fee, fx.qr]) fn.mockReset();
  fx.minter.mockResolvedValue(actorDefaults()); fx.ledger.mockResolvedValue({ icrc1_balance_of: vi.fn(async () => 1_000_000n) });
  fx.update.mockResolvedValue({ Ok: [] }); fx.submit.mockResolvedValue({ kind: 'retrieve-uncertain', approveBlockIndex: 5n, message: 'response lost' });
  fx.fee.mockResolvedValue(10n); fx.qr.mockResolvedValue('data:image/png;base64,fake');
  fx.beginAction.mockClear();
}
beforeEach(() => { host = document.createElement('div'); document.body.appendChild(host); reset(); });
afterEach(() => { if (instance) { unmount(instance as any); instance = undefined; } host.remove(); localStorage.clear(); vi.useRealTimers(); vi.restoreAllMocks(); });

async function prepareWithdrawal() {
  connect(); render(); await settle();
  button('Redeem BTC')!.click(); await settle();
  const inputs = [...host.querySelectorAll('input')] as HTMLInputElement[];
  inputValue(inputs[0], BITCOIN_ADDRESS); inputValue(inputs[1], '0.001'); await settle();
  await new Promise((resolve) => setTimeout(resolve, 350)); await settle();
  return button('Approve and request BTC')!;
}

describe('/bitcoin minter mounted flow', () => {
  it.each([
    { label: 'network error', failure: new Error('connection interrupted') },
    { label: 'temporarily unavailable minter', result: { Err: { TemporarilyUnavailable: 'retry later' } } },
    { label: 'generic minter error', result: { Err: { GenericError: { error_message: 'deposit check unavailable' } } } },
  ])('retries a $label automatically and replaces the transient state with deposit progress', async (scenario) => {
    vi.useFakeTimers();
    if ('failure' in scenario) fx.update.mockRejectedValueOnce(scenario.failure);
    else fx.update.mockResolvedValueOnce(scenario.result);
    fx.update.mockResolvedValueOnce({ Err: { NoNewUtxos: { current_confirmations: [1], required_confirmations: 4 } } });
    fx.update.mockResolvedValue({ Ok: [{ Minted: { minted_amount: 100_000n } }] });
    connect(); render(); await settle();
    expect(host.textContent).toContain('temporarily unavailable');
    expect(button('Resume deposit checks')).toBeNull();
    await vi.advanceTimersByTimeAsync(60_000); await settle();
    expect(fx.update).toHaveBeenCalledTimes(2);
    expect(host.textContent).toContain('Bitcoin confirmations: 1 of 4');
    expect(host.textContent).not.toContain('temporarily unavailable');
    await vi.advanceTimersByTimeAsync(60_000); await settle();
    expect(fx.update).toHaveBeenCalledTimes(3);
    expect(host.textContent).toContain('Minted 0.001 ckBTC');
    expect(host.querySelector('.error')).toBeNull();
    expect(fx.beginAction).not.toHaveBeenCalled();
    expect(fx.submit).not.toHaveBeenCalled();
  });

  it('cancels a scheduled transient retry when the wallet disconnects', async () => {
    vi.useFakeTimers();
    fx.update.mockRejectedValueOnce(new Error('connection interrupted'));
    connect(); render(); await settle();
    expect(fx.update).toHaveBeenCalledTimes(1);
    disconnect(); await settle();
    await vi.advanceTimersByTimeAsync(60_000); await settle();
    expect(fx.update).toHaveBeenCalledTimes(1);
    expect(host.textContent).not.toContain('temporarily unavailable');
    expect(fx.beginAction).not.toHaveBeenCalled();
    expect(fx.submit).not.toHaveBeenCalled();
  });

  it('does not request a deposit check or wallet action while disconnected', async () => {
    render(); await settle();
    expect(fx.update).not.toHaveBeenCalled();
    expect(fx.beginAction).not.toHaveBeenCalled();
    expect(fx.submit).not.toHaveBeenCalled();
  });

  it.each([
    ['corrupt JSON', '{broken-json'],
    ['invalid shape', JSON.stringify({ owner: OWNER_A_TEXT, state: 'uncertain', attemptId: 'missing-request-fields' })],
    ['owner mismatch', JSON.stringify({ owner: OWNER_B_TEXT, state: 'uncertain', attemptId: 'other-owner', address: BITCOIN_ADDRESS, amount: '100000', startedAt: 1 })],
    ['invalid accepted block', JSON.stringify({ owner: OWNER_A_TEXT, state: 'accepted', attemptId: 'invalid-block', address: BITCOIN_ADDRESS, amount: '100000', startedAt: 1, blockIndex: 'not-a-block' })],
    ['missing accepted block', JSON.stringify({ owner: OWNER_A_TEXT, state: 'accepted', attemptId: 'missing-block', address: BITCOIN_ADDRESS, amount: '100000', startedAt: 1 })],
    ['nonempty null record', 'null'],
  ])('preserves %s and prevents wallet spending while public deposit reads remain available', async (_case, raw) => {
    const key = withdrawalKey(OWNER_A_TEXT);
    localStorage.setItem(key, raw);
    connect(); render(); await settle();
    expect(host.textContent).toContain(BITCOIN_ADDRESS);
    expect(fx.update).toHaveBeenCalledTimes(1);
    button('Redeem BTC')!.click(); await settle();
    expect(host.textContent).toContain('Saved withdrawal record preserved');
    expect(host.textContent).toContain('Contact support');
    expect(button('Resolve previous request first')?.disabled).toBe(true);
    button('Resolve previous request first')?.click(); await settle();
    expect(localStorage.getItem(key)).toBe(raw);
    expect(fx.beginAction).not.toHaveBeenCalled();
    expect(fx.submit).not.toHaveBeenCalled();
    expect(localStorage.getItem(`${key}:receipt`)).toBeNull();
  });

  it('detects a newly unreadable record before opening the signer from an otherwise ready withdrawal', async () => {
    const submit = await prepareWithdrawal();
    expect(submit.disabled).toBe(false);
    const key = withdrawalKey(OWNER_A_TEXT);
    const raw = '{record-written-before-click';
    localStorage.setItem(key, raw);
    submit.click(); await settle();
    expect(host.textContent).toContain('Saved withdrawal record preserved');
    expect(localStorage.getItem(key)).toBe(raw);
    expect(fx.beginAction).not.toHaveBeenCalled();
    expect(fx.submit).not.toHaveBeenCalled();
  });
  it('restores the reusable address and checks deposits automatically after reload', async () => {
    connect(); render(); await settle();
    expect(host.textContent).toContain(BITCOIN_ADDRESS);
    expect(fx.update).toHaveBeenCalledTimes(1);
    expect(fx.update.mock.calls[0][0].toText()).toBe(OWNER_A_TEXT);
    unmount(instance as any); instance = undefined;
    render(); await settle();
    expect(host.textContent).toContain(BITCOIN_ADDRESS);
    expect(fx.update).toHaveBeenCalledTimes(2);
    expect(fx.submit).not.toHaveBeenCalled();
  });

  it('retires a confirmed accepted request automatically and restores a collapsed receipt on reload', async () => {
    const key = withdrawalKey(OWNER_A_TEXT);
    localStorage.setItem(key, JSON.stringify({ owner: OWNER_A_TEXT, address: BITCOIN_ADDRESS, amount: '100000', startedAt: Date.now(), attemptId: 'terminal-45', state: 'accepted', blockIndex: '45' }));
    fx.minter.mockResolvedValue({ ...actorDefaults(), retrieve_btc_status_v2: vi.fn(async () => ({ Confirmed: { txid: [1, 2] } })) });
    connect(); render(); await settle(); button('Redeem BTC')!.click(); await settle();
    expect(localStorage.getItem(key)).toBeNull();
    expect(host.querySelector('details.withdrawal-receipt')?.hasAttribute('open')).toBe(false);
    expect(host.textContent).toContain('Confirmed on Bitcoin');
    unmount(instance as any); instance = undefined; render(); await settle(); button('Redeem BTC')!.click(); await settle();
    expect(host.textContent).toContain('Confirmed on Bitcoin');
    expect(host.textContent).toContain('Request block 45');
    expect(fx.submit).not.toHaveBeenCalled();
  });

  it('never retires a newer journal when an old terminal status reply arrives', async () => {
    const key = withdrawalKey(OWNER_A_TEXT);
    const original = { owner: OWNER_A_TEXT, address: BITCOIN_ADDRESS, amount: '100000', startedAt: Date.now(), attemptId: 'old-45', state: 'accepted', blockIndex: '45' };
    localStorage.setItem(key, JSON.stringify(original));
    const reply = deferred<any>();
    fx.minter.mockResolvedValue({ ...actorDefaults(), retrieve_btc_status_v2: vi.fn(() => reply.promise) });
    connect(); render(); await settle();
    const newer = { ...original, attemptId: 'new-46', blockIndex: '46' };
    localStorage.setItem(key, JSON.stringify(newer));
    reply.resolve({ Confirmed: { txid: [1, 2] } }); await settle();
    expect(JSON.parse(localStorage.getItem(key)!)).toEqual(newer);
    expect(host.querySelector('details.withdrawal-receipt')).toBeNull();
    expect(fx.submit).not.toHaveBeenCalled();
  });

  it('rechecks the exact record after waiting for the retirement Web Lock', async () => {
    const key = withdrawalKey(OWNER_A_TEXT);
    const original = { owner: OWNER_A_TEXT, address: BITCOIN_ADDRESS, amount: '100000', startedAt: Date.now(), attemptId: 'old-45', state: 'accepted', blockIndex: '45' };
    localStorage.setItem(key, JSON.stringify(original));
    const release = deferred<void>();
    const lock = vi.fn(async (_key: string, _opts: unknown, callback: () => Promise<unknown>) => { await release.promise; return callback(); });
    Object.defineProperty(navigator, 'locks', { configurable: true, value: { request: lock } });
    fx.minter.mockResolvedValue({ ...actorDefaults(), retrieve_btc_status_v2: vi.fn(async () => ({ Confirmed: { txid: [1, 2] } })) });
    connect(); render(); await settle();
    expect(lock).toHaveBeenCalledTimes(1);
    const newer = { ...original, attemptId: 'new-46', blockIndex: '46' };
    localStorage.setItem(key, JSON.stringify(newer));
    release.resolve(); await settle();
    expect(JSON.parse(localStorage.getItem(key)!)).toEqual(newer);
    expect(localStorage.getItem(`${key}:receipt`)).toBeNull();
  });

  it('keeps tracking a pending reimbursement until reimbursement is terminal', async () => {
    vi.useFakeTimers();
    const key = withdrawalKey(OWNER_A_TEXT);
    localStorage.setItem(key, JSON.stringify({ owner: OWNER_A_TEXT, address: BITCOIN_ADDRESS, amount: '100000', startedAt: Date.now(), attemptId: 'reimburse-45', state: 'accepted', blockIndex: '45' }));
    const status = vi.fn().mockResolvedValueOnce({ WillReimburse: {} }).mockResolvedValue({ Reimbursed: {} });
    fx.minter.mockResolvedValue({ ...actorDefaults(), retrieve_btc_status_v2: status });
    connect(); render(); await settle(); button('Redeem BTC')!.click(); await settle();
    expect(host.textContent).toContain('preparing a reimbursement');
    expect(localStorage.getItem(key)).not.toBeNull();
    await vi.advanceTimersByTimeAsync(60_000); await settle();
    expect(status).toHaveBeenCalledTimes(2);
    expect(localStorage.getItem(key)).toBeNull();
    expect(host.textContent).toContain('reimbursed this request');
    expect(fx.submit).not.toHaveBeenCalled();
  });

  it.each(['AmountTooLow', 'Unknown'])('retains the exact accepted journal for %s status and keeps checking for reimbursement', async (state) => {
    vi.useFakeTimers();
    const key = withdrawalKey(OWNER_A_TEXT);
    const saved = JSON.stringify({ owner: OWNER_A_TEXT, address: BITCOIN_ADDRESS, amount: '100000', startedAt: Date.now(), attemptId: `held-${state}-45`, state: 'accepted', blockIndex: '45' });
    localStorage.setItem(key, saved);
    const status = vi.fn().mockResolvedValue({ [state]: null });
    fx.minter.mockResolvedValue({ ...actorDefaults(), retrieve_btc_status_v2: status });
    connect(); render(); await settle(); button('Redeem BTC')!.click(); await settle();
    expect(localStorage.getItem(key)).toBe(saved);
    expect(localStorage.getItem(`${key}:receipt`)).toBeNull();
    expect(button('Withdrawal in progress')?.disabled).toBe(true);
    if (state === 'AmountTooLow') expect(host.textContent).toContain('checking for reimbursement');
    await vi.advanceTimersByTimeAsync(60_000); await settle();
    expect(status).toHaveBeenCalledTimes(2);
    expect(localStorage.getItem(key)).toBe(saved);
    expect(button('Withdrawal in progress')?.disabled).toBe(true);
    expect(fx.beginAction).not.toHaveBeenCalled();
    expect(fx.submit).not.toHaveBeenCalled();
    status.mockResolvedValue({ Reimbursed: {} });
    await vi.advanceTimersByTimeAsync(60_000); await settle();
    expect(localStorage.getItem(key)).toBeNull();
    expect(host.textContent).toContain('reimbursed this request');
  });

  it('synchronously opens the wallet action before requesting the submission lock and closes it on a tab conflict', async () => {
    const submit = await prepareWithdrawal();
    const close = vi.fn(async () => {});
    fx.beginAction.mockReturnValueOnce({ ready: Promise.resolve(), close });
    Object.defineProperty(navigator, 'locks', { configurable: true, value: { request: async (_key: string, _opts: unknown, callback: (lock: unknown) => Promise<unknown>) => {
      expect(fx.beginAction).toHaveBeenCalledTimes(1);
      return callback(null);
    } } });
    submit.click();
    expect(fx.beginAction).toHaveBeenCalledTimes(1);
    await settle();
    expect(close).toHaveBeenCalledTimes(1);
    expect(fx.submit).not.toHaveBeenCalled();
    expect(host.textContent).toContain('Another tab');
  });

  it('invalidates a quote when the same owner changes wallet session', async () => {
    const submit = await prepareWithdrawal();
    expect(submit.disabled).toBe(false);
    fx.session.set(1); fx.session.set(2);
    await settle();
    expect(button('Approve and request BTC')?.disabled).toBe(true);
    expect(host.querySelectorAll('input')[0].value).toBe('');
    expect(fx.submit).not.toHaveBeenCalled();
  });

  it('binds automatic estimates to the newest amount and recipient after overlapping requests', async () => {
    vi.useFakeTimers();
    connect(); render(); await settle(); button('Redeem BTC')!.click(); await settle();
    const oldEstimate = deferred<any>();
    const estimate = vi.fn().mockReturnValueOnce(oldEstimate.promise).mockResolvedValue({ bitcoin_fee: 400n, minter_fee: 500n });
    fx.minter.mockResolvedValue({ ...actorDefaults(), estimate_withdrawal_fee: estimate });
    const inputs = [...host.querySelectorAll('input')] as HTMLInputElement[];
    inputValue(inputs[0], BITCOIN_ADDRESS); inputValue(inputs[1], '0.001'); await settle();
    await vi.advanceTimersByTimeAsync(300); await settle();
    inputValue(inputs[0], '1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa'); inputValue(inputs[1], '0.002'); await settle();
    await vi.advanceTimersByTimeAsync(300); await settle();
    expect(estimate).toHaveBeenCalledTimes(2);
    expect(estimate.mock.calls[1][0]).toEqual({ amount: [200_000n] });
    expect(host.textContent).toContain('0.000004 BTC');
    oldEstimate.resolve({ bitcoin_fee: 100n, minter_fee: 200n }); await settle();
    expect(host.textContent).toContain('0.000004 BTC');
    expect(host.textContent).not.toContain('0.000001 BTC');
    expect(fx.submit).not.toHaveBeenCalled();
  });

  it('refreshes the next estimate automatically after a confirmed submitted withdrawal', async () => {
    const submit = await prepareWithdrawal();
    fx.submit.mockResolvedValueOnce({ kind: 'success', approveBlockIndex: 5n, withdrawalBlockIndex: 45n } as any);
    fx.minter.mockResolvedValue({ ...actorDefaults(), retrieve_btc_status_v2: vi.fn(async () => ({ Confirmed: { txid: [1, 2] } })) });
    submit.click(); await settle();
    expect(localStorage.getItem(withdrawalKey(OWNER_A_TEXT))).toBeNull();
    expect(host.querySelector('details.withdrawal-receipt')).not.toBeNull();
    await new Promise((resolve) => setTimeout(resolve, 350)); await settle();
    expect(button('Approve and request BTC')?.disabled).toBe(false);
    expect(fx.submit).toHaveBeenCalledTimes(1);
  });

  it('ignores a deposit reply from an earlier same-owner wallet session', async () => {
    const reply = deferred<any>();
    fx.update.mockReturnValueOnce(reply.promise);
    connect(); render(); await settle();
    fx.session.set(1); await settle();
    reply.resolve({ Ok: [{ Minted: { minted_amount: 777n } }] }); await settle();
    expect(host.textContent).not.toContain('Minted 0.00000777');
    expect(fx.update).toHaveBeenCalledTimes(2);
  });
  it('does not paint an old owner QR result after the connected account changes', async () => {
    connect(OWNER_A);
    const qr = deferred<string>(); fx.qr.mockReturnValueOnce(qr.promise);
    fx.minter.mockResolvedValue({ ...actorDefaults(), get_btc_address: vi.fn(async () => BITCOIN_ADDRESS) });
    render(); await settle();
    expect(fx.qr).toHaveBeenCalledTimes(1);
    connect(OWNER_B); await settle();
    qr.resolve('data:image/png;base64,stale-owner-a'); await settle();
    expect(host.textContent).not.toContain(OWNER_A_TEXT);
    expect(host.textContent).not.toContain('stale-owner-a');
    expect(host.querySelector('img.qr')?.getAttribute('src')).toBe('data:image/png;base64,fake');
    expect(host.textContent).toContain(OWNER_B_TEXT);
  });

  it('persists an uncertain withdrawal, blocks repeat dispatch, and restores the block after remount', async () => {
    const submit = await prepareWithdrawal();
    expect(submit.disabled).toBe(false);
    submit.click(); await settle();
    expect(fx.submit).toHaveBeenCalledTimes(1);
    const [submitted] = fx.submit.mock.calls[0] as unknown as [{
      approveArgs: { amount: bigint; fee: [] | [bigint] };
      retrieveArgs: { amount: bigint; address: string };
    }];
    expect(submitted.approveArgs.amount).toBe(100_000n);
    expect(submitted.approveArgs.fee).toEqual([10n]);
    expect(submitted.retrieveArgs.amount).toBe(100_000n);
    expect(submitted.retrieveArgs.address).toBe(BITCOIN_ADDRESS);
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
    expect(button('Withdrawal in progress')?.disabled).toBe(true);
  });

  it('refreshes an expiring fee quote automatically without dispatching a withdrawal', async () => {
    let now = 1_800_000_000_000;
    vi.spyOn(Date, 'now').mockImplementation(() => now);
    const submit = await prepareWithdrawal();
    expect(submit.disabled).toBe(false);
    now += 60_001;
    await new Promise((resolve) => setTimeout(resolve, 1_100));
    await settle();
    expect(host.textContent).toContain('Estimated Bitcoin received');
    expect(button('Approve and request BTC')!.disabled).toBe(false);
    expect(fx.submit).not.toHaveBeenCalled();
  });

  it('invalidates an in-flight quote after disconnect and reconnect of the same principal', async () => {
    connect(); render(); await settle(); button('Redeem BTC')!.click(); await settle();
    const estimate = deferred<{ bitcoin_fee: bigint; minter_fee: bigint }>();
    fx.minter.mockResolvedValue({ ...actorDefaults(), estimate_withdrawal_fee: vi.fn(() => estimate.promise) });
    const inputs = [...host.querySelectorAll('input')] as HTMLInputElement[];
    inputValue(inputs[0], BITCOIN_ADDRESS); inputValue(inputs[1], '0.001'); await settle();
    await new Promise((resolve) => setTimeout(resolve, 350)); await settle();
    disconnect(); await settle(); connect(OWNER_A); await settle();
    estimate.resolve({ bitcoin_fee: 100n, minter_fee: 200n }); await settle();
    expect(button('Approve and request BTC')!.disabled).toBe(true);
    expect(host.textContent).toContain('Fees are estimated automatically.');
  });
});
