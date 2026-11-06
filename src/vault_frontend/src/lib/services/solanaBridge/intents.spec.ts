import { describe, expect, it } from 'vitest';
import { createMemoryStorage } from '../baseBridge/testSupport';
import { MAX_RESOLVED_SOLANA_INTENTS, SOLANA_INTENT_PREFIX, SolIntentStoreError, SolanaIntentStore, solIntentStorageKey, type SolDepositIntent, type SolReturnIntent } from './intents';
import { KEYS, SOL_BRIDGE_ID } from './testSupport';
import { SOLANA_MAINNET } from './networks';

const PRINCIPAL = 'zegjz-jpi6k-qkand-c2bgf-qw6za-xk4si-nz3gx-qzzia-fk6fg-snepb-tae';

function returnIntent(overrides: Partial<SolReturnIntent> = {}): SolReturnIntent {
  return {
    v: 1,
    kind: 'solana_to_icp',
    id: 'ab'.repeat(16),
    destination: 'solana-mainnet',
    genesisHash: SOLANA_MAINNET.genesisHash,
    instance: SOL_BRIDGE_ID,
    mint: KEYS.mint,
    tokenProgram: 'token2022',
    returnAccount: 'RetAcc',
    wallet: KEYS.holder,
    sourceAccount: KEYS.holderSource,
    icpPrincipal: PRINCIPAL,
    icpSubaccount: null,
    amountE8s: 300_000_000n,
    nonce: '42',
    memo: `rumi1|r|${PRINCIPAL}|-|42`,
    blockhash: 'BH',
    lastValidBlockHeight: '1000',
    messageHex: '0102',
    signMode: 'sign',
    createdAt: 1_000,
    status: 'prepared',
    ...overrides,
  };
}

function depositIntent(overrides: Partial<SolDepositIntent> = {}): SolDepositIntent {
  return {
    v: 1,
    kind: 'icp_to_solana',
    id: 'cd'.repeat(16),
    destination: 'solana-mainnet',
    genesisHash: SOLANA_MAINNET.genesisHash,
    instance: SOL_BRIDGE_ID,
    mint: KEYS.mint,
    tokenProgram: 'token2022',
    recipient: KEYS.holder,
    amountE8s: 200_000_000n,
    expectedDepositFee: 10_000n,
    createdAt: 2_000,
    status: 'prepared',
    ...overrides,
  };
}

describe('the Solana journal', () => {
  it('lives under its own key, so the Base journal is never touched', () => {
    expect(solIntentStorageKey(PRINCIPAL)).toBe(`${SOLANA_INTENT_PREFIX}${PRINCIPAL}`);
    expect(SOLANA_INTENT_PREFIX).not.toBe('rumi:base-bridge:v1:');
    const storage = createMemoryStorage({ [`rumi:base-bridge:v1:${PRINCIPAL}`]: 'base record' });
    const store = new SolanaIntentStore(storage, PRINCIPAL);
    store.put(returnIntent());
    expect(storage.map.get(`rumi:base-bridge:v1:${PRINCIPAL}`)).toBe('base record');
  });

  it('persists both kinds with every field of the attempt and reads them back after a reload', () => {
    const storage = createMemoryStorage();
    const store = new SolanaIntentStore(storage, PRINCIPAL);
    store.put(returnIntent({ signedHex: 'aabb', signature: 'Sig', status: 'sent', note: 'n' }));
    store.put(depositIntent({ status: 'submitted', depositId: 9n, failure: undefined }));
    const reopened = new SolanaIntentStore(storage, PRINCIPAL);
    expect(reopened.loadStatus).toBe('ok');
    expect(reopened.list().map((i) => i.id)).toEqual(['cd'.repeat(16), 'ab'.repeat(16)]);
    const r = reopened.get('ab'.repeat(16)) as SolReturnIntent;
    expect(r).toMatchObject({ amountE8s: 300_000_000n, nonce: '42', messageHex: '0102', signedHex: 'aabb', signature: 'Sig', status: 'sent', lastValidBlockHeight: '1000', icpSubaccount: null, signMode: 'sign' });
    expect(reopened.get('cd'.repeat(16))).toMatchObject({ depositId: 9n, expectedDepositFee: 10_000n });
    // Bigints are stored as decimal strings.
    expect(JSON.parse(storage.map.get(solIntentStorageKey(PRINCIPAL))!).intents[0].amountE8s).toBeTypeOf('string');
  });

  it('is scoped to the principal and to the network', () => {
    const storage = createMemoryStorage();
    new SolanaIntentStore(storage, PRINCIPAL).put(returnIntent());
    expect(new SolanaIntentStore(storage, 'aaaaa-aa').list()).toEqual([]);
    const store = new SolanaIntentStore(storage, PRINCIPAL);
    store.put(returnIntent({ id: 'ef'.repeat(16), destination: 'solana-devnet' }));
    expect(store.listFor('solana-mainnet').map((i) => i.id)).toEqual(['ab'.repeat(16)]);
    expect(store.listFor('solana-devnet').map((i) => i.id)).toEqual(['ef'.repeat(16)]);
  });

  it('patches an attempt in place and removes it', () => {
    const store = new SolanaIntentStore(createMemoryStorage(), PRINCIPAL);
    store.put(returnIntent());
    expect(store.patch<SolReturnIntent>('ab'.repeat(16), { status: 'signing' }).status).toBe('signing');
    expect(() => store.patch('nope', { status: 'signing' })).toThrow(SolIntentStoreError);
    store.remove('ab'.repeat(16));
    expect(store.list()).toEqual([]);
  });

  it('keeps a copy of an unreadable record and restores what is readable', () => {
    const good = returnIntent();
    const storage = createMemoryStorage();
    const first = new SolanaIntentStore(storage, PRINCIPAL);
    first.put(good);
    const record = JSON.parse(storage.map.get(solIntentStorageKey(PRINCIPAL))!);
    record.intents.push({ kind: 'solana_to_icp', id: 'not hex' });
    storage.map.set(solIntentStorageKey(PRINCIPAL), JSON.stringify(record));
    const reopened = new SolanaIntentStore(storage, PRINCIPAL, { now: () => 5 });
    expect(reopened.loadStatus).toBe('recovered');
    expect(reopened.list().map((i) => i.id)).toEqual([good.id]);
    expect(reopened.quarantineKey).toBe(`${solIntentStorageKey(PRINCIPAL)}:unreadable:5`);
    expect(storage.map.get(reopened.quarantineKey!)).toContain('not hex');
    // Garbage JSON too.
    storage.map.set(solIntentStorageKey(PRINCIPAL), '{nope');
    const third = new SolanaIntentStore(storage, PRINCIPAL, { now: () => 6 });
    expect(third.loadStatus).toBe('recovered');
    expect(storage.map.get(`${solIntentStorageKey(PRINCIPAL)}:unreadable:6`)).toBe('{nope');
  });

  it('goes read-only for a newer version and refuses to write without storage', () => {
    const storage = createMemoryStorage({ [solIntentStorageKey(PRINCIPAL)]: JSON.stringify({ v: 2, principal: PRINCIPAL, intents: [] }) });
    const newer = new SolanaIntentStore(storage, PRINCIPAL);
    expect(newer.loadStatus).toBe('newer_version');
    expect(newer.readOnly).toBe(true);
    expect(() => newer.put(returnIntent())).toThrowError(/newer version/);
    expect(storage.map.get(solIntentStorageKey(PRINCIPAL))).toContain('"v":2');
    const none = new SolanaIntentStore(null, PRINCIPAL);
    expect(none.loadStatus).toBe('unavailable');
    expect(() => none.put(returnIntent())).toThrowError(/nothing was sent/);
  });

  it('prunes old resolved attempts but never an unresolved one', () => {
    const store = new SolanaIntentStore(createMemoryStorage(), PRINCIPAL);
    store.put(returnIntent({ id: '00'.repeat(16), createdAt: 1, status: 'held' }));
    for (let i = 0; i < MAX_RESOLVED_SOLANA_INTENTS + 5; i++) {
      store.put(returnIntent({ id: (i + 1).toString(16).padStart(32, '0'), createdAt: 10 + i, status: 'done' }));
    }
    expect(store.list().filter((i) => i.status === 'done')).toHaveLength(MAX_RESOLVED_SOLANA_INTENTS);
    expect(store.get('00'.repeat(16))?.status).toBe('held');
  });
});
