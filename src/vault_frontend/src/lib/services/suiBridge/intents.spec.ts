import { describe, expect, it } from 'vitest';
import { createMemoryStorage } from '../baseBridge/testSupport';
import { SUI_INTENT_PREFIX, SuiIntentStore, SuiIntentStoreError, suiIntentStorageKey, type SuiDepositIntent, type SuiReturnIntent } from './intents';
import { COIN_TYPE, KEYS, SUI_BRIDGE_ID, TEST_OWNER_TEXT, coin } from './testSupport';
import { SUI_MAINNET } from './networks';

const ME = TEST_OWNER_TEXT;
const OTHER = 'rrkah-fqaaa-aaaaa-aaaaq-cai';

function returnIntent(overrides: Partial<SuiReturnIntent> = {}): SuiReturnIntent {
  return {
    v: 1,
    kind: 'sui_to_icp',
    id: '11'.repeat(16),
    destination: 'sui-mainnet',
    chainIdentifier: SUI_MAINNET.chainIdentifier,
    instance: SUI_BRIDGE_ID,
    packageId: KEYS.packageId,
    gateId: KEYS.gateId,
    coinType: COIN_TYPE,
    sender: KEYS.holder,
    coins: [coin(KEYS.coinA, 5_000_000_000n)],
    icpPrincipal: ME,
    icpSubaccount: null,
    amountE8s: 300_000_000n,
    nonce: '18446744073709551615',
    transferId: 'aa'.repeat(32),
    signMode: 'sign',
    createdAt: 5,
    status: 'prepared',
    ...overrides,
  };
}

function depositIntent(overrides: Partial<SuiDepositIntent> = {}): SuiDepositIntent {
  return {
    v: 1,
    kind: 'icp_to_sui',
    id: '22'.repeat(16),
    destination: 'sui-mainnet',
    chainIdentifier: SUI_MAINNET.chainIdentifier,
    instance: SUI_BRIDGE_ID,
    packageId: KEYS.packageId,
    gateId: KEYS.gateId,
    coinType: COIN_TYPE,
    recipient: KEYS.holder,
    amountE8s: 200_000_000n,
    expectedDepositFee: 10_000_000n,
    createdAt: 6,
    status: 'prepared',
    ...overrides,
  };
}

describe('the Sui journal', () => {
  it('lives under its own key, scoped to one principal', () => {
    expect(suiIntentStorageKey(ME)).toBe(`${SUI_INTENT_PREFIX}${ME}`);
    expect(SUI_INTENT_PREFIX).toBe('rumi:sui-bridge:v1:');
    const storage = createMemoryStorage();
    new SuiIntentStore(storage, ME).put(returnIntent());
    expect(new SuiIntentStore(storage, OTHER).list()).toEqual([]);
    expect(new SuiIntentStore(storage, ME).list()).toHaveLength(1);
  });

  it('round-trips every field exactly: bigints, the nonce at the u64 boundary, the coins and the signed evidence', () => {
    const storage = createMemoryStorage();
    const store = new SuiIntentStore(storage, ME);
    const signed = returnIntent({ status: 'signed', signedBytes: 'AAEC', signature: 'c2ln', digest: '4vJ9JU1bJJE96FWSJKvHsmmFADCg4gpZQff4P3bkLKi', note: 'n' });
    store.put(signed);
    store.put(depositIntent({ status: 'submitted', depositId: 99n }));
    const reopened = new SuiIntentStore(storage, ME);
    expect(reopened.get(signed.id)).toEqual(signed);
    expect(reopened.get('22'.repeat(16))).toMatchObject({ kind: 'icp_to_sui', depositId: 99n, expectedDepositFee: 10_000_000n });
    expect(reopened.listFor('sui-mainnet')).toHaveLength(2);
    expect(reopened.listFor('sui-testnet')).toHaveLength(0);
  });

  it('patches in place and removes by id, never touching the others', () => {
    const store = new SuiIntentStore(createMemoryStorage(), ME);
    store.put(returnIntent());
    store.put(returnIntent({ id: '33'.repeat(16), createdAt: 7 }));
    store.patch<SuiReturnIntent>('11'.repeat(16), { status: 'sent', digest: 'D' });
    expect(store.get('11'.repeat(16))).toMatchObject({ status: 'sent', digest: 'D', nonce: '18446744073709551615' });
    store.remove('33'.repeat(16));
    expect(store.list().map((i) => i.id)).toEqual(['11'.repeat(16)]);
    expect(() => store.patch('99'.repeat(16), {})).toThrow(SuiIntentStoreError);
  });

  it('goes read-only for a record from a newer app, and never overwrites it', () => {
    const storage = createMemoryStorage();
    storage.setItem(suiIntentStorageKey(ME), JSON.stringify({ v: 2, principal: ME, intents: [] }));
    const store = new SuiIntentStore(storage, ME);
    expect(store.loadStatus).toBe('newer_version');
    expect(store.readOnly).toBe(true);
    expect(() => store.put(returnIntent())).toThrow(expect.objectContaining({ code: 'read_only' }));
    expect(JSON.parse(storage.getItem(suiIntentStorageKey(ME))!).v).toBe(2);
  });

  it('refuses to save when storage is unavailable, so nothing is sent without a record', () => {
    const store = new SuiIntentStore(null, ME);
    expect(store.loadStatus).toBe('unavailable');
    expect(() => store.put(returnIntent())).toThrow(expect.objectContaining({ code: 'persist_failed' }));
    const throwing = {
      getItem: () => {
        throw new Error('blocked');
      },
      setItem: () => {
        throw new Error('blocked');
      },
      removeItem: () => {},
    };
    expect(new SuiIntentStore(throwing as never, ME).loadStatus).toBe('unavailable');
  });

  it('copies an unreadable record aside and restores what is readable', () => {
    const storage = createMemoryStorage();
    storage.setItem(suiIntentStorageKey(ME), '{not json');
    const store = new SuiIntentStore(storage, ME, { now: () => 1234 });
    expect(store.loadStatus).toBe('recovered');
    expect(store.quarantineKey).toBe(`${suiIntentStorageKey(ME)}:unreadable:1234`);
    expect(storage.getItem(`${suiIntentStorageKey(ME)}:unreadable:1234`)).toBe('{not json');
    // A record with one bad entry keeps the good one and the original text.
    const good = returnIntent();
    const mixed = createMemoryStorage();
    mixed.setItem(suiIntentStorageKey(ME), JSON.stringify({ v: 1, principal: ME, intents: [{ ...good, amountE8s: good.amountE8s.toString(), coins: [{ ...good.coins[0], balance: '5000000000' }] }, { kind: 'sui_to_icp', id: 'bad' }] }));
    const recovered = new SuiIntentStore(mixed, ME, { now: () => 9 });
    expect(recovered.loadStatus).toBe('recovered');
    expect(recovered.list().map((i) => i.id)).toEqual([good.id]);
  });

  it('rejects an entry whose addresses, coins or ids are malformed', () => {
    const storage = createMemoryStorage();
    const good = returnIntent();
    const serial = (patch: Record<string, unknown>) => ({ ...good, amountE8s: '1', coins: [{ ...good.coins[0], balance: '1' }], ...patch });
    for (const bad of [serial({ sender: '0x2' }), serial({ coins: [] }), serial({ transferId: 'zz' }), serial({ nonce: '-1' }), serial({ chainIdentifier: 'ab' }), serial({ signMode: 'other' }), serial({ id: 'short' })]) {
      storage.setItem(suiIntentStorageKey(ME), JSON.stringify({ v: 1, principal: ME, intents: [bad] }));
      expect(new SuiIntentStore(storage, ME, { now: () => 1 }).list(), JSON.stringify(bad).slice(0, 60)).toEqual([]);
    }
  });

  it('keeps every unresolved attempt while pruning only old resolved ones', () => {
    const store = new SuiIntentStore(createMemoryStorage(), ME);
    for (let i = 0; i < 45; i++) store.put(returnIntent({ id: i.toString(16).padStart(32, '0'), status: 'done', createdAt: i }));
    store.put(returnIntent({ id: 'ff'.repeat(16), status: 'held', createdAt: 1 }));
    expect(store.list().filter((i) => i.status === 'done')).toHaveLength(40);
    expect(store.get('ff'.repeat(16))?.status).toBe('held');
  });
});
