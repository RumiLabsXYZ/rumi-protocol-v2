import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';

/**
 * Component-level scenarios for the XRPL icUSD panel (plan Task 6, last bullet):
 * approval, rejection, reload recovery and network switch.
 *
 * This is the jsdom equivalent of a browser run: the REAL panel, controller and
 * service wrappers are mounted; only the external boundaries are faked (backend
 * actor, Xaman adapter, XRPL JSON-RPC via global fetch, app stores). A real
 * browser run against Playwright is NOT part of this repo's tooling yet.
 */

const h = vi.hoisted(() => {
  function store<T>(initial: T) {
    let v = initial;
    const subs = new Set<(x: T) => void>();
    return {
      subscribe(run: (x: T) => void) { subs.add(run); run(v); return () => { subs.delete(run); }; },
      set(x: T) { v = x; subs.forEach((s) => s(x)); },
      get: () => v,
    };
  }
  return { store, actor: {} as Record<string, any>, wallet: {} as any, debt: 50_00000000n as bigint | null };
});

vi.mock('$lib/stores/wallet', () => ({
  walletStore: h.store({ principal: { toText: () => 'test-principal' }, isConnected: true }),
}));
vi.mock('$lib/stores/vaultStore', () => ({ vaultStore: h.store({ vaults: [] }) }));
vi.mock('$lib/stores/collateralStore', () => ({ collateralStore: { getCollateralInfo: () => ({ borrowingFee: 0.005 }) } }));
vi.mock('$lib/services/auth', () => ({
  currentWalletType: h.store('internet-identity'),
  WALLET_TYPES: { OISY: 'oisy', INTERNET_IDENTITY: 'internet-identity' },
}));
vi.mock('$lib/config', () => ({ CONFIG: { currentCanisterId: 'backend', currentIcusdLedgerId: 'ledger' } }));
vi.mock('$lib/services/protocol/walletOperations', () => ({ walletOperations: { approveIcusdTransfer: vi.fn(async () => ({ success: true })) } }));
vi.mock('$lib/services/ledgerFeeService', () => ({ fetchLedgerFeeStrict: vi.fn(async () => 10_000n) }));
vi.mock('$lib/components/borrow/xrpDepositFlow', () => ({ xrplRpcEndpoints: () => ['https://rpc.test'] }));
vi.mock('$lib/services/xrplWalletService', () => ({
  getXrplWalletAdapter: () => h.wallet,
  expectedXrplNetwork: () => 'testnet',
}));
vi.mock('$lib/services/xrplIcusdService', async () => {
  const actual = await vi.importActual<typeof import('$lib/services/xrplIcusdService')>('$lib/services/xrplIcusdService');
  return {
    ...actual,
    fetchVaultDebtE8s: async () => h.debt,
    createDefaultXrplIcusdApi: async () =>
      actual.createXrplIcusdApi({ getActor: async () => h.actor as any, getPublicActor: async () => h.actor as any }),
  };
});

import XrplIcusdPanel from './XrplIcusdPanel.svelte';

const ISSUER = 'rIssuer1111111111111111111111111';
const CURRENCY = '6963555344000000000000000000000000000000';
const USER = 'rUser1111111111111111111111111111';
const VAULTS = [{ vaultId: 7, borrowedIcusd: 50, collateralType: 'xrp' }];

const flush = async (n = 8) => { for (let i = 0; i < n; i++) { await Promise.resolve(); await new Promise((r) => setTimeout(r, 0)); flushSync(); } };

function opView(over: Record<string, unknown> = {}) {
  return {
    observed_exact_amount: [], kind: { Repayment: null }, net_e8s: 0n, recipient: [], network: { Testnet: null }, fee_e8s: 0n,
    xrpl_tx_hashes: [], updated_at_ns: 1n, operation_id: 9n, vault_id: [7n], ic_ledger_blocks: [], resolution: [],
    created_at_ns: 1n, held_exact_amount: [], next_action: { Wait: null }, notes: [], debt_applied_e8s: 0n,
    phase: { IntentPrepared: null }, gross_e8s: 10_00000000n, settlement_cutoff_ns: [], ...over,
  };
}

function makeWallet(network = 'testnet') {
  let conn: { address: string; network: string } | null = { address: USER, network };
  const handlers = new Set<(c: any) => void>();
  const prompts = new Set<(p: any) => void>();
  return {
    id: 'xaman', availability: () => ({ available: true }),
    connect: vi.fn(async () => conn!), getConnection: () => conn,
    signAndSubmit: vi.fn(async () => ({ requestId: 'req-1', status: 'submitted', signed: true, hash: 'HASH1' })),
    resumeRequest: vi.fn(async (id: string) => ({ requestId: id, status: 'submitted', signed: true, hash: 'HASH1' })),
    disconnect: vi.fn(), restore: vi.fn(async () => conn),
    onChange: (fn: (c: any) => void) => { handlers.add(fn); return () => handlers.delete(fn); },
    onPrompt: (fn: (p: any) => void) => { prompts.add(fn); return () => prompts.delete(fn); },
    emitPrompt(p: any) { prompts.forEach((fn) => fn(p)); },
    setConn(c: any) { conn = c; handlers.forEach((fn) => fn(c)); },
  };
}

function makeActor() {
  const ops: any[] = [];
  const a = {
    get_xrpl_icusd_config: vi.fn(async () => ({
      enabled: true, ready: true, issuance_paused: false, settlement_paused: false, issuer_address: [ISSUER], currency_hex: CURRENCY,
      network: [{ Testnet: null }], per_operation_cap_e8s: 1_000_000_00000000n, outstanding_cap_e8s: 1_000_000_00000000n, config_version: 1n,
    })),
    get_my_xrpl_operations: vi.fn(async () => ({ items: [...ops], next_cursor: [] })),
    prepare_xrpl_repayment: vi.fn(async (arg: any) => {
      ops.length = 0;
      ops.push(opView());
      return { Ok: {
        operation_id: 9n, issuer: ISSUER, currency: CURRENCY, payer: arg.payer, destination_tag: 777, max_e8s: arg.max_e8s,
        network: arg.network, quote_time_ns: BigInt(Date.now()) * 1_000_000n,
        payment_json: JSON.stringify({ TransactionType: 'Payment', Account: arg.payer, Destination: ISSUER, DestinationTag: 777,
          Amount: { currency: CURRENCY, issuer: ISSUER, value: '10' } }),
      } };
    }),
    observe_xrpl_repayment: vi.fn(async () => {
      const done = opView({ phase: { Complete: null }, observed_exact_amount: ['10'], debt_applied_e8s: 10_00000000n });
      ops.length = 0; ops.push(done);
      return { Ok: done };
    }),
    borrow_to_xrpl: vi.fn(), issue_xrpl_from_canonical: vi.fn(), resume_xrpl_operation: vi.fn(), resolve_xrpl_claim: vi.fn(),
    ops,
  };
  return a;
}

let target: HTMLElement;
let comp: ReturnType<typeof mount> | null = null;

function render() {
  comp = mount(XrplIcusdPanel, { target, props: { vaults: VAULTS } });
}
function mountWithVaults(vaults: any[]) {
  comp = mount(XrplIcusdPanel, { target, props: { vaults } });
}
function teardown() { if (comp) { unmount(comp); comp = null; } }
const text = () => target.textContent ?? '';
function button(label: RegExp): HTMLButtonElement {
  const b = Array.from(target.querySelectorAll('button')).find((x) => label.test(x.textContent ?? ''));
  if (!b) throw new Error(`no button ${label}; page: ${text().replace(/\s+/g, ' ').slice(0, 600)}`);
  return b as HTMLButtonElement;
}
async function type(sel: string, value: string, idx = 0) {
  const el = target.querySelectorAll<HTMLInputElement>(sel)[idx];
  el.value = value;
  el.dispatchEvent(new Event('input', { bubbles: true }));
  await flush(2);
}
async function click(label: RegExp) { button(label).click(); await flush(); }

function stubRpc() {
  vi.stubGlobal('fetch', vi.fn(async (_u: string, init: any) => {
    const m = JSON.parse(init.body).method;
    const result = m === 'account_lines'
      ? { status: 'success', validated: true, lines: [{ account: ISSUER, currency: CURRENCY, limit: '1000000', balance: '0' }] }
      : { status: 'success', state: { validated_ledger: { reserve_inc: 200000 } } };
    return { ok: true, json: async () => ({ result }) } as any;
  }));
}

beforeEach(() => {
  localStorage.clear();
  h.debt = 50_00000000n;
  target = document.createElement('div');
  document.body.appendChild(target);
  h.actor = makeActor();
  h.wallet = makeWallet();
  stubRpc();
});
afterEach(() => {
  teardown();
  target.remove();
  vi.unstubAllGlobals();
});

async function prepareTen() {
  await type('[data-testid="xrpl-icusd-repay"] input:not([type="checkbox"])', '10');
  await click(/Prepare repayment/);
}

describe('XRPL icUSD panel (mocked actor, Xaman adapter and XRPL RPC)', () => {
  it('approval: prepare, pay in wallet, tell Rumi, and show only what the backend verified', async () => {
    render();
    await flush();
    expect(text()).toContain('icUSD is enabled');
    await prepareTen();
    expect(h.actor.prepare_xrpl_repayment).toHaveBeenCalledTimes(1);
    const arg = h.actor.prepare_xrpl_repayment.mock.calls[0][0];
    expect(arg.request_nonce).toBeInstanceOf(Uint8Array);
    expect(arg.request_nonce.length).toBe(32);
    expect(arg).toMatchObject({ vault_id: 7n, max_e8s: 10_00000000n, payer: USER, network: { Testnet: null }, full_close: false, top_up_operation: [] });
    expect(text()).toContain('777');

    await click(/Pay in wallet/);
    const [tx] = h.wallet.signAndSubmit.mock.calls[0];
    expect(tx).toMatchObject({ TransactionType: 'Payment', Destination: ISSUER, DestinationTag: 777 });
    // Wallet "submitted" is only a hint: the panel then asks the backend, whose operation view decides the outcome.
    expect(h.actor.observe_xrpl_repayment).toHaveBeenCalledTimes(1);
    expect(h.actor.observe_xrpl_repayment).toHaveBeenCalledWith({ operation_id: 9n, tx_hash: 'HASH1' });
    expect(text()).toMatch(/Repayment complete/);
    // The repay card shows the exact amounts the backend verified.
    const amounts = target.querySelector('[data-testid="xrpl-icusd-repay-amounts"]')?.textContent ?? '';
    expect(amounts).toMatch(/Received by Rumi\s*10 icUSD/);
    expect(amounts).toMatch(/Applied to debt\s*10\.00 icUSD/);
  });

  it('rejection: the user declines in the wallet, nothing is sent to Rumi, and they can retry', async () => {
    h.wallet.signAndSubmit.mockResolvedValueOnce({ requestId: 'req-x', status: 'rejected', signed: false, hash: null });
    render();
    await flush();
    await prepareTen();
    await click(/Pay in wallet/);
    expect(h.actor.observe_xrpl_repayment).not.toHaveBeenCalled();
    expect(text()).toMatch(/rejected or expired/);
    expect(text()).toMatch(/debt is unchanged/);
    expect(() => button(/Prepare repayment/)).not.toThrow();
  });

  it('reload recovery: a remounted panel shows the backend operation and reuses no stale wallet state', async () => {
    render();
    await flush();
    await prepareTen();
    expect(text()).toMatch(/Pay in wallet/);
    teardown();
    target.innerHTML = '';
    // Page reload: new panel instance, same backend and storage.
    render();
    await flush();
    expect(h.actor.get_my_xrpl_operations).toHaveBeenCalled();
    expect(text()).toMatch(/Payment pending/);
    expect(h.actor.observe_xrpl_repayment).not.toHaveBeenCalled();
  });

  it('network switch: a wallet on the wrong network blocks actions until it switches back', async () => {
    h.wallet = makeWallet('mainnet');
    render();
    await flush();
    expect(text()).toMatch(/Your wallet is on mainnet; XRPL icUSD runs on testnet/);
    await type('[data-testid="xrpl-icusd-repay"] input:not([type="checkbox"])', '10');
    expect(button(/Prepare repayment/).disabled).toBe(true);
    expect(button(/Borrow to XRPL/).disabled).toBe(true);

    h.wallet.setConn({ address: USER, network: 'testnet' });
    await flush();
    expect(text()).not.toMatch(/Your wallet is on mainnet/);
    await type('[data-testid="xrpl-icusd-repay"] input:not([type="checkbox"])', '10');
    expect(button(/Prepare repayment/).disabled).toBe(false);
    expect(h.actor.prepare_xrpl_repayment).not.toHaveBeenCalled();
  });

  it('disabled lane: shows an explanation and offers no actions', async () => {
    h.actor.get_xrpl_icusd_config.mockResolvedValue({
      enabled: false, ready: false, issuance_paused: false, settlement_paused: false, issuer_address: [], currency_hex: CURRENCY,
      network: [], per_operation_cap_e8s: 0n, outstanding_cap_e8s: 0n, config_version: 0n,
    });
    render();
    await flush();
    expect(target.querySelector('[data-testid="xrpl-icusd-gate"]')?.textContent).toMatch(/not enabled/);
    expect(target.querySelector('[data-testid="xrpl-icusd-repay"]')).toBeNull();
  });
});

describe('recovery review fixes (panel)', () => {
  it('shows the Open-in-Xaman link and QR while a repay request is open', async () => {
    let finish: (v: any) => void = () => {};
    h.wallet.signAndSubmit.mockImplementation(() => new Promise((r) => { finish = r; }));
    render();
    await flush();
    await prepareTen();
    await click(/Pay in wallet/);
    h.wallet.emitPrompt({ requestId: 'req-open', deepLink: 'https://xumm.app/sign/req-open', qrPng: 'https://xumm.app/sign/req-open_q.png' });
    await flush();
    const box = target.querySelector('[data-testid="xrpl-icusd-prompt"]')!;
    expect(box.querySelector('a')?.getAttribute('href')).toBe('https://xumm.app/sign/req-open');
    expect(box.querySelector('img')?.getAttribute('src')).toBe('https://xumm.app/sign/req-open_q.png');
    finish({ requestId: 'req-open', status: 'rejected', signed: false, hash: null });
    await flush();
    expect(target.querySelector('[data-testid="xrpl-icusd-prompt"]')).toBeNull();
  });

  it('Pay the whole debt sends the exact bigint e8s from the backend, not a float', async () => {
    // The vault card float cannot represent this debt; the backend e8s value can.
    h.debt = 123456789012345678n;
    const exact = '1234567890.12345678';
    mountWithVaults([{ vaultId: 7, borrowedIcusd: 1234567890.1234567, collateralType: 'xrp' }]);
    await flush();
    expect(text()).toContain(`${exact} icUSD now`);
    const box = target.querySelector<HTMLInputElement>('[data-testid="xrpl-icusd-repay"] input[type="checkbox"]')!;
    box.click();
    await flush(2);
    await click(/Prepare repayment/);
    const arg = h.actor.prepare_xrpl_repayment.mock.calls[0][0];
    expect(arg.max_e8s).toBe(123456789012345678n);
    expect(arg.full_close).toBe(true);
  });

  it('full close is blocked with a message when the exact debt cannot be read', async () => {
    h.debt = null;
    render();
    await flush();
    target.querySelector<HTMLInputElement>('[data-testid="xrpl-icusd-repay"] input[type="checkbox"]')!.click();
    await flush(2);
    expect(button(/Prepare repayment/).disabled).toBe(true);
    expect(text()).toMatch(/Could not read the exact debt/);
  });

  it('refund pending with Wait says Rumi is arranging it, and a held payment shows its settlement cutoff', async () => {
    h.actor.ops.push(opView({ operation_id: 3n, kind: { Refund: null }, phase: { RefundPending: null }, next_action: { Wait: null } }));
    h.actor.ops.push(opView({ operation_id: 4n, phase: { Held: null }, next_action: { ResolveClaim: null }, held_exact_amount: ['5'], settlement_cutoff_ns: [1_800_000_000_000_000_000n] }));
    render();
    await flush();
    expect(text()).toMatch(/Rumi is arranging your refund/);
    expect(text()).not.toMatch(/Choose how to receive your funds/);
    expect(target.querySelector('[data-testid="xrpl-icusd-cutoff"]')?.textContent).toMatch(/Rumi holds this until/);
  });

  it('status and error messages are announced (role=status / role=alert)', async () => {
    h.wallet.signAndSubmit.mockResolvedValueOnce({ requestId: 'req-x', status: 'rejected', signed: false, hash: null, warning: 'The XRP Ledger refused this transaction (tecPATH_DRY). Nothing was applied.' });
    render();
    await flush();
    await prepareTen();
    await click(/Pay in wallet/);
    const alert = target.querySelector('[role="alert"]');
    expect(alert?.textContent).toMatch(/tecPATH_DRY/);
    expect(target.querySelector('[role="status"]')).not.toBeNull();
  });
});
