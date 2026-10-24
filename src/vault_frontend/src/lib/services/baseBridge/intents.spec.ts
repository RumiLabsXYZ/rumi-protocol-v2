import { describe, expect, it } from 'vitest';
import {
  INTENT_STORAGE_PREFIX,
  IntentStore,
  IntentStoreError,
  MAX_RESOLVED_INTENTS,
  intentId,
  intentStorageKey,
  isResolved,
  newClientNonce,
  newClientRequestId,
  type BaseToIcpIntent,
  type IcpToBaseIntent,
} from './intents';
import { createMemoryStorage } from './testSupport';

const ALICE = 'zegjz-jpi6k-qkand-c2bgf-qw6za-xk4si-nz3gx-qzzia-fk6fg-snepb-tae';
const BOB = 'aaaaa-aa';
const RECIPIENT = '0x1111111111111111111111111111111111111111';
const CONTRACT = '0x2222222222222222222222222222222222222222';

function icpIntent(overrides: Partial<IcpToBaseIntent> = {}): IcpToBaseIntent {
  return {
    v: 1,
    kind: 'icp_to_base',
    clientRequestId: `0x${'ab'.repeat(16)}`,
    amountE8s: 2_500_000_000n,
    recipient: RECIPIENT,
    expectedDepositFee: 10_000n,
    createdAt: 1_000,
    status: 'prepared',
    ...overrides,
  };
}

function baseIntent(overrides: Partial<BaseToIcpIntent> = {}): BaseToIcpIntent {
  return {
    v: 1,
    kind: 'base_to_icp',
    clientNonce: `0x${'cd'.repeat(32)}`,
    transferId: `0x${'ef'.repeat(32)}`,
    from: RECIPIENT,
    amountE8s: 100_000_000n,
    destinationPrincipal: ALICE,
    chainId: 8453,
    contract: CONTRACT,
    createdAt: 2_000,
    status: 'prepared',
    ...overrides,
  };
}

describe('storage key', () => {
  it('is scoped per principal under the pinned prefix', () => {
    expect(INTENT_STORAGE_PREFIX).toBe('rumi:base-bridge:v1:');
    expect(intentStorageKey(ALICE)).toBe(`rumi:base-bridge:v1:${ALICE}`);
  });
});

describe('persist and load', () => {
  it('round-trips both intent kinds with exact bigint values', () => {
    const storage = createMemoryStorage();
    const store = new IntentStore(storage, ALICE);
    const a = icpIntent({ amountE8s: 18_446_744_073_709_551_615n, depositId: 7n, status: 'submitted' });
    const b = baseIntent({ txHash: `0x${'12'.repeat(32)}`, status: 'sent', notified: true });
    store.put(a);
    store.put(b);

    const reopened = new IntentStore(storage, ALICE);
    expect(reopened.loadStatus).toBe('ok');
    expect(reopened.get(intentId(a))).toEqual(a);
    expect(reopened.get(intentId(b))).toEqual(b);
    expect(reopened.list().map((intent) => intent.kind)).toEqual(['base_to_icp', 'icp_to_base']); // newest first
  });

  it('writes a versioned, principal-tagged record with decimal-string amounts', () => {
    const storage = createMemoryStorage();
    new IntentStore(storage, ALICE).put(icpIntent());
    const raw = JSON.parse(storage.map.get(`rumi:base-bridge:v1:${ALICE}`)!);
    expect(raw).toMatchObject({ v: 1, principal: ALICE });
    expect(raw.intents[0]).toMatchObject({
      v: 1,
      kind: 'icp_to_base',
      clientRequestId: `0x${'ab'.repeat(16)}`,
      amountE8s: '2500000000',
      expectedDepositFee: '10000',
    });
  });

  it('replaces an intent with the same id instead of duplicating it', () => {
    const store = new IntentStore(createMemoryStorage(), ALICE);
    store.put(icpIntent());
    store.put(icpIntent({ status: 'submitted', depositId: 3n }));
    expect(store.list()).toHaveLength(1);
    expect(store.list()[0]).toMatchObject({ status: 'submitted', depositId: 3n });
  });

  it('patches and removes by id, case-insensitively', () => {
    const store = new IntentStore(createMemoryStorage(), ALICE);
    store.put(baseIntent());
    const patched = store.patch<BaseToIcpIntent>(baseIntent().transferId.toUpperCase().replace('0X', '0x'), {
      txHash: `0x${'99'.repeat(32)}`,
      status: 'sent',
    });
    expect(patched).toMatchObject({ status: 'sent', txHash: `0x${'99'.repeat(32)}` });
    store.remove(baseIntent().transferId);
    expect(store.list()).toEqual([]);
    expect(() => store.patch(baseIntent().transferId, { status: 'done' })).toThrow(IntentStoreError);
  });

  it('merges a write made by another tab rather than overwriting it', () => {
    const storage = createMemoryStorage();
    const tabA = new IntentStore(storage, ALICE);
    const tabB = new IntentStore(storage, ALICE);
    tabA.put(icpIntent());
    tabB.put(baseIntent());
    expect(new IntentStore(storage, ALICE).list()).toHaveLength(2);
  });

  it('classifies resolved intents', () => {
    expect(isResolved(icpIntent({ status: 'done' }))).toBe(true);
    expect(isResolved(icpIntent({ status: 'failed' }))).toBe(true);
    expect(isResolved(icpIntent({ status: 'prepared' }))).toBe(false);
    expect(isResolved(baseIntent({ status: 'done' }))).toBe(true);
    expect(isResolved(baseIntent({ status: 'sent' }))).toBe(false);
  });

  it('prunes old resolved intents but never an unresolved one', () => {
    const store = new IntentStore(createMemoryStorage(), ALICE);
    const unresolved = icpIntent({ clientRequestId: `0x${'01'.repeat(16)}`, createdAt: 1 });
    store.put(unresolved);
    for (let i = 0; i < MAX_RESOLVED_INTENTS + 5; i++) {
      store.put(icpIntent({ clientRequestId: `0x${(i + 16).toString(16).padStart(2, '0').repeat(16)}`, createdAt: 100 + i, status: 'done' }));
    }
    const ids = store.list().map(intentId);
    expect(ids).toContain(unresolved.clientRequestId);
    expect(store.list().filter(isResolved)).toHaveLength(MAX_RESOLVED_INTENTS);
  });
});

describe('principal scoping', () => {
  it('does not show one principal the intents of another', () => {
    const storage = createMemoryStorage();
    new IntentStore(storage, ALICE).put(icpIntent());
    const bob = new IntentStore(storage, BOB);
    expect(bob.list()).toEqual([]);
    bob.put(baseIntent());
    expect(new IntentStore(storage, ALICE).list()).toHaveLength(1);
    expect(new IntentStore(storage, BOB).list()).toHaveLength(1);
  });

  it('treats a record filed under the wrong principal as unreadable and preserves it', () => {
    const storage = createMemoryStorage();
    new IntentStore(storage, BOB).put(icpIntent());
    // Copy Bob's record under Alice's key.
    storage.map.set(intentStorageKey(ALICE), storage.map.get(intentStorageKey(BOB))!);
    const alice = new IntentStore(storage, ALICE, { now: () => 42 });
    expect(alice.list()).toEqual([]);
    expect(alice.loadStatus).toBe('recovered');
    expect(storage.map.get(`${intentStorageKey(ALICE)}:unreadable:42`)).toContain(`"principal":"${BOB}"`);
  });
});

describe('corrupt and future records', () => {
  it('preserves corrupt JSON in a quarantine key before starting fresh', () => {
    const storage = createMemoryStorage({ [intentStorageKey(ALICE)]: '{not json' });
    const store = new IntentStore(storage, ALICE, { now: () => 777 });
    expect(store.loadStatus).toBe('recovered');
    expect(store.quarantineKey).toBe(`${intentStorageKey(ALICE)}:unreadable:777`);
    expect(storage.map.get(store.quarantineKey!)).toBe('{not json');
    expect(store.list()).toEqual([]);
    // The store is usable again and the original text is still there.
    store.put(icpIntent());
    expect(store.list()).toHaveLength(1);
    expect(store.loadStatus).toBe('recovered');
    expect(storage.map.get(store.quarantineKey!)).toBe('{not json');
  });

  it('keeps readable entries and quarantines the whole record when only some are invalid', () => {
    const good = icpIntent();
    const raw = JSON.stringify({
      v: 1,
      principal: ALICE,
      intents: [
        { ...good, amountE8s: good.amountE8s.toString(), expectedDepositFee: '10000' },
        { v: 1, kind: 'icp_to_base', clientRequestId: 'garbage' },
        { v: 1, kind: 'mystery' },
      ],
    });
    const storage = createMemoryStorage({ [intentStorageKey(ALICE)]: raw });
    const store = new IntentStore(storage, ALICE, { now: () => 5 });
    expect(store.loadStatus).toBe('recovered');
    expect(store.list()).toHaveLength(1);
    expect(storage.map.get(`${intentStorageKey(ALICE)}:unreadable:5`)).toBe(raw);
  });

  it('rejects intents with invalid hex, addresses, amounts or statuses', () => {
    const base = { ...icpIntent(), amountE8s: '1', expectedDepositFee: '0' };
    const make = (patch: Record<string, unknown>) =>
      JSON.stringify({ v: 1, principal: ALICE, intents: [{ ...base, ...patch }] });
    for (const patch of [
      { clientRequestId: '0x12' },
      { recipient: '0x12' },
      { amountE8s: '-1' },
      { amountE8s: '1.5' },
      { status: 'unknown' },
      { createdAt: 'yesterday' },
      { depositId: 'x' },
    ]) {
      const store = new IntentStore(createMemoryStorage({ [intentStorageKey(ALICE)]: make(patch) }), ALICE);
      expect(store.list(), JSON.stringify(patch)).toEqual([]);
      expect(store.loadStatus).toBe('recovered');
    }
  });

  it('never overwrites a record written by a newer version, and becomes read-only', () => {
    const future = JSON.stringify({ v: 2, principal: ALICE, intents: [{ anything: true }] });
    const storage = createMemoryStorage({ [intentStorageKey(ALICE)]: future });
    const store = new IntentStore(storage, ALICE);
    expect(store.loadStatus).toBe('newer_version');
    expect(store.readOnly).toBe(true);
    expect(() => store.put(icpIntent())).toThrowError(expect.objectContaining({ code: 'read_only' }));
    expect(storage.map.get(intentStorageKey(ALICE))).toBe(future);
    expect([...storage.map.keys()]).toEqual([intentStorageKey(ALICE)]);
  });
});

describe('unavailable storage', () => {
  it('refuses to persist when there is no storage, so the caller never reaches the wallet', () => {
    const store = new IntentStore(null, ALICE);
    expect(store.loadStatus).toBe('unavailable');
    expect(store.readOnly).toBe(true);
    expect(() => store.put(icpIntent())).toThrowError(expect.objectContaining({ code: 'persist_failed' }));
  });

  it('refuses when a write throws (quota or blocked storage) and keeps the old state', () => {
    const storage = createMemoryStorage();
    const store = new IntentStore(storage, ALICE);
    store.put(icpIntent());
    storage.setItem = () => {
      throw new Error('QuotaExceededError');
    };
    expect(() => store.put(baseIntent())).toThrowError(expect.objectContaining({ code: 'persist_failed' }));
    expect(store.list()).toHaveLength(1);
  });

  it('refuses when reads throw', () => {
    const storage = createMemoryStorage();
    storage.getItem = () => {
      throw new Error('SecurityError');
    };
    const store = new IntentStore(storage, ALICE);
    expect(store.loadStatus).toBe('unavailable');
    expect(() => store.put(icpIntent())).toThrow(IntentStoreError);
  });

  it('does not touch the original record if it cannot be copied aside', () => {
    const storage = createMemoryStorage({ [intentStorageKey(ALICE)]: '{broken' });
    const realSet = storage.setItem;
    storage.setItem = (key: string, value: string) => {
      if (key.includes(':unreadable:')) throw new Error('quota');
      realSet(key, value);
    };
    const store = new IntentStore(storage, ALICE);
    expect(store.loadStatus).toBe('unavailable');
    expect(storage.map.get(intentStorageKey(ALICE))).toBe('{broken');
    expect(() => store.put(icpIntent())).toThrow(IntentStoreError);
  });
});

describe('random ids', () => {
  it('builds a 16 byte request id and a 32 byte nonce from injected randomness', () => {
    const counter = (length: number) => Uint8Array.from({ length }, (_, i) => i + 1);
    expect(newClientRequestId(counter)).toBe('0x0102030405060708090a0b0c0d0e0f10');
    expect(newClientNonce(counter)).toHaveLength(66);
  });

  it('uses crypto randomness by default', () => {
    expect(newClientRequestId()).toMatch(/^0x[0-9a-f]{32}$/);
    expect(newClientNonce()).toMatch(/^0x[0-9a-f]{64}$/);
    expect(newClientNonce()).not.toBe(newClientNonce());
  });
});
