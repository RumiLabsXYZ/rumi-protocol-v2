import { beforeEach, describe, expect, it, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import {
  formatDropsAsXrp,
  parseXrpToDrops,
  tryParseXrpToDrops,
  buildXrpPaymentUri,
} from '$lib/utils/nativeXrpBorrowFlow';
import {
  createXamanAdapter,
  payloadToSubmission,
  type XamanSdkLike,
} from './xrplWalletService';

describe('exact XRP money parsing', () => {
  it('parses 12.345678 XRP to 12345678 drops', () => {
    expect(parseXrpToDrops('12.345678')).toBe(12_345_678n);
    expect(parseXrpToDrops('0.000001')).toBe(1n);
    expect(parseXrpToDrops('5')).toBe(5_000_000n);
    expect(parseXrpToDrops(' 1.5 ')).toBe(1_500_000n);
  });

  it('rejects seven or more decimals, negatives, empty and garbage', () => {
    for (const bad of ['12.3456789', '0.0000001', '-1', '-0.5', '', '  ', 'abc', '1e3', '1,5', '.', '1.', '1.2.3', '+1', '0x10', 'NaN', 'Infinity']) {
      expect(tryParseXrpToDrops(bad), bad).toBeNull();
      expect(() => parseXrpToDrops(bad), bad).toThrow();
    }
  });

  it('rejects amounts beyond the XRPL maximum supply', () => {
    expect(tryParseXrpToDrops('100000000001')).toBeNull();
    expect(tryParseXrpToDrops('100000000000')).toBe(100_000_000_000_000_000n);
  });

  it('formats drops back to exact decimal text without float math', () => {
    expect(formatDropsAsXrp(12_345_678n)).toBe('12.345678');
    expect(formatDropsAsXrp(1_000_000n)).toBe('1');
    expect(formatDropsAsXrp(1n)).toBe('0.000001');
    expect(formatDropsAsXrp(100_000_000_000_000_000n)).toBe('100000000000');
  });

  it('formats the ripple: URI amount from integer drops exactly', () => {
    expect(buildXrpPaymentUri('rAddr', 12_345_678n)).toBe('ripple:rAddr?amount=12.345678');
    // A float that is not exactly a drop count is refused, not rounded.
    expect(buildXrpPaymentUri('rAddr', 0.1 + 0.2)).toBe('');
    expect(buildXrpPaymentUri('rAddr', 2.5)).toBe('ripple:rAddr?amount=2.5');
  });
});

// ---------------------------------------------------------------------------

function makeSdk(overrides: Partial<Record<string, any>> = {}) {
  const listeners: Record<string, Array<(...a: any[]) => void>> = {};
  const state = { account: 'rUser', networkId: 1, networkType: 'TESTNET' };
  const payloads = new Map<string, any>();
  let n = 0;
  const sdk: any = {
    ctorArgs: [] as unknown[],
    on: vi.fn((ev: string, fn: any) => { (listeners[ev] ??= []).push(fn); }),
    off: vi.fn(),
    authorize: vi.fn(async () => ({ me: { account: state.account } })),
    logout: vi.fn(async () => {}),
    user: {
      get account() { return Promise.resolve(state.account); },
      get networkId() { return Promise.resolve(state.networkId); },
      get networkType() { return Promise.resolve(state.networkType); },
    },
    payload: {
      create: vi.fn(async (body: any) => {
        const uuid = `uuid-${++n}`;
        payloads.set(uuid, { uuid, body });
        return {
          uuid,
          next: { always: `https://xumm.app/sign/${uuid}` },
          refs: { qr_png: `https://xumm.app/sign/${uuid}_q.png` },
        };
      }),
      get: vi.fn(async (uuid: string) => payloads.get(uuid)?.response ?? null),
    },
    ...overrides,
  };
  return { sdk, listeners, state, payloads };
}

function respond(payloads: Map<string, any>, uuid: string, meta: any, response: any = {}) {
  payloads.get(uuid).response = {
    meta: { exists: true, uuid, resolved: false, signed: false, cancelled: false, expired: false, ...meta },
    response: { txid: null, account: 'rUser', ...response },
  };
}

function makeAdapter(h: ReturnType<typeof makeSdk>, extra: Record<string, any> = {}) {
  const ctor = vi.fn();
  const loadSdk = vi.fn(async (apiKey: string): Promise<XamanSdkLike> => {
    ctor(apiKey);
    return h.sdk;
  });
  const store = new Map<string, string>();
  const storage = {
    getItem: (k: string) => store.get(k) ?? null,
    setItem: (k: string, v: string) => { store.set(k, v); },
    removeItem: (k: string) => { store.delete(k); },
  };
  const adapter = createXamanAdapter({
    apiKey: 'pub-key',
    expectedNetwork: 'testnet',
    loadSdk,
    storage,
    sleep: async () => {},
    pollIntervalMs: 1,
    maxWaitMs: 5,
    ...extra,
  });
  return { adapter, loadSdk, ctor, store, storage };
}

describe('Xaman adapter (PKCE, public key only)', () => {
  beforeEach(() => vi.restoreAllMocks());

  it('reports unavailable without an API key and never loads the SDK', async () => {
    const h = makeSdk();
    const loadSdk = vi.fn(async () => h.sdk);
    const adapter = createXamanAdapter({ apiKey: '', expectedNetwork: 'testnet', loadSdk });
    expect(adapter.availability().available).toBe(false);
    await expect(adapter.connect()).rejects.toThrow(/not configured/i);
    expect(loadSdk).not.toHaveBeenCalled();
  });

  it('lazy-loads the SDK with only the public API key (no secret)', async () => {
    const h = makeSdk();
    const { adapter, loadSdk, ctor } = makeAdapter(h);
    expect(loadSdk).not.toHaveBeenCalled();
    const conn = await adapter.connect();
    expect(loadSdk).toHaveBeenCalledTimes(1);
    expect(ctor).toHaveBeenCalledWith('pub-key');
    expect(loadSdk.mock.calls[0]).toHaveLength(1);
    expect(conn).toEqual({ address: 'rUser', network: 'testnet' });
    expect(adapter.getConnection()).toEqual({ address: 'rUser', network: 'testnet' });
  });

  it('never references an API secret in the adapter source', () => {
    const src = readFileSync(resolve(process.cwd(), 'src/lib/services/xrplWalletService.ts'), 'utf8');
    expect(src).not.toMatch(/API_SECRET|apiSecret|XAMAN_SECRET/i);
    expect(src).toMatch(/VITE_XAMAN_API_KEY/);
  });

  it('refuses a wallet on the wrong network and logs it out', async () => {
    const h = makeSdk();
    h.state.networkId = 0;
    h.state.networkType = 'MAINNET';
    const { adapter } = makeAdapter(h);
    await expect(adapter.connect()).rejects.toThrow(/network/i);
    expect(h.sdk.logout).toHaveBeenCalled();
    expect(adapter.getConnection()).toBeNull();
  });

  it('proposes a Payment pinned to the expected network and exposes the deep link and QR', async () => {
    const h = makeSdk();
    const { adapter } = makeAdapter(h, { maxWaitMs: 1_000_000, sleep: () => new Promise((r) => setTimeout(r, 1)) });
    await adapter.connect();
    const prompts: any[] = [];
    adapter.onPrompt((p) => prompts.push(p));
    const tx = { TransactionType: 'Payment' as const, Destination: 'rCustody', Amount: '12345678' };
    const p = adapter.signAndSubmit(tx, { key: 'vault-7' });
    await vi.waitFor(() => expect(h.sdk.payload.create).toHaveBeenCalledTimes(1));
    const body = h.sdk.payload.create.mock.calls[0][0];
    expect(body.txjson).toEqual(tx);
    expect(body.options.force_network).toBe('TESTNET');
    expect(prompts[0]).toMatchObject({ requestId: 'uuid-1', deepLink: 'https://xumm.app/sign/uuid-1' });
    respond(h.payloads, 'uuid-1', { resolved: true, signed: true }, { txid: 'ABC123' });
    await expect(p).resolves.toEqual({ requestId: 'uuid-1', status: 'submitted', signed: true, hash: 'ABC123' });
  });

  it('maps rejected, expired and cancelled payloads to rejected with no hash', () => {
    const base = { meta: { exists: true, resolved: true, signed: false, cancelled: false, expired: false }, response: { txid: null } };
    expect(payloadToSubmission('r', { ...base })).toMatchObject({ status: 'rejected', signed: false, hash: null });
    expect(payloadToSubmission('r', { ...base, meta: { ...base.meta, resolved: false, expired: true } })).toMatchObject({ status: 'rejected' });
    expect(payloadToSubmission('r', { ...base, meta: { ...base.meta, resolved: false, cancelled: true } })).toMatchObject({ status: 'rejected' });
  });

  it('keeps a signed payload without a txid pending, never delivered', () => {
    const p = { meta: { exists: true, resolved: true, signed: true, cancelled: false, expired: false }, response: { txid: null } };
    expect(payloadToSubmission('r', p)).toEqual({ requestId: 'r', status: 'pending', signed: true, hash: null });
    expect(payloadToSubmission('r', null)).toEqual({ requestId: 'r', status: 'pending', signed: false, hash: null });
  });

  it('resumes the same request for the same key instead of creating a second payment', async () => {
    const h = makeSdk();
    const { adapter, store } = makeAdapter(h);
    await adapter.connect();
    const tx = { TransactionType: 'Payment' as const, Destination: 'rCustody', Amount: '5000000' };
    const first = await adapter.signAndSubmit(tx, { key: 'vault-9' }); // wait budget elapses: still pending
    expect(first.status).toBe('pending');
    expect([...store.values()].join()).toContain('uuid-1');
    const second = await adapter.signAndSubmit(tx, { key: 'vault-9' });
    expect(second.requestId).toBe('uuid-1');
    expect(h.sdk.payload.create).toHaveBeenCalledTimes(1);
    respond(h.payloads, 'uuid-1', { resolved: true, signed: true }, { txid: 'HASH' });
    await expect(adapter.resumeRequest('uuid-1')).resolves.toMatchObject({ status: 'submitted', hash: 'HASH' });
  });

  it('survives localStorage throwing', async () => {
    const h = makeSdk();
    const storage = { getItem: () => { throw new Error('x'); }, setItem: () => { throw new Error('x'); }, removeItem: () => { throw new Error('x'); } };
    const { adapter } = makeAdapter(h, { storage });
    await adapter.connect();
    const res = await adapter.signAndSubmit({ TransactionType: 'Payment', Destination: 'r', Amount: '1' }, { key: 'k' });
    expect(res.status).toBe('pending');
  });

  it('notifies listeners on logout/disconnect', async () => {
    const h = makeSdk();
    const { adapter } = makeAdapter(h);
    await adapter.connect();
    const seen: any[] = [];
    const off = adapter.onChange((c) => seen.push(c));
    adapter.disconnect();
    expect(seen.at(-1)).toBeNull();
    off();
    expect(adapter.getConnection()).toBeNull();
  });
});
