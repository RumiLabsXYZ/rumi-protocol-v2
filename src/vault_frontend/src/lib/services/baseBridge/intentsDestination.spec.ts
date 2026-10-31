import { describe, expect, it, vi } from 'vitest';
import { Principal } from '@dfinity/principal';
import { startIcpToBase, buildQuote, unresolvedIntents, type IcpToBaseDeps } from './icpToBase';
import {
  INTENT_STORAGE_PREFIX,
  INTENT_VERSION,
  IntentStore,
  LEGACY_DESTINATION_ID,
  LEGACY_INTENT_VERSION,
  intentDestination,
  intentStorageKey,
  type BaseToIcpIntent,
  type IcpToBaseIntent,
} from './intents';
import { TEST_BRIDGE_ID, TEST_OWNER, createMemoryStorage, makeDeposit, makeDestination, makeSigning } from './testSupport';

const ALICE = 'zegjz-jpi6k-qkand-c2bgf-qw6za-xk4si-nz3gx-qzzia-fk6fg-snepb-tae';
const RECIPIENT = '0x1111111111111111111111111111111111111111';
const CONTRACT = '0x2222222222222222222222222222222222222222';
const NOW = 1_800_000_000_000;

/** A record exactly as the Base-only app (version 1) wrote it: no destination anywhere. */
function legacyRecord(extra: unknown[] = []) {
  return JSON.stringify({
    v: 1,
    principal: ALICE,
    intents: [
      {
        v: 1,
        kind: 'icp_to_base',
        clientRequestId: `0x${'ab'.repeat(16)}`,
        amountE8s: '2500000000',
        recipient: RECIPIENT,
        expectedDepositFee: '10000',
        createdAt: 1_000,
        status: 'prepared',
      },
      {
        v: 1,
        kind: 'base_to_icp',
        clientNonce: `0x${'cd'.repeat(32)}`,
        transferId: `0x${'ef'.repeat(32)}`,
        from: RECIPIENT,
        amountE8s: '100000000',
        destinationPrincipal: ALICE,
        chainId: 8453,
        contract: CONTRACT,
        createdAt: 2_000,
        status: 'prepared',
      },
      ...extra,
    ],
  });
}

function icpDeps(store: IntentStore, signing = makeSigning()): IcpToBaseDeps {
  return {
    owner: TEST_OWNER,
    store,
    bridgeCanister: Principal.fromText(TEST_BRIDGE_ID),
    ledger: { approve: vi.fn(async () => ({ Ok: 1n })) as never },
    bridge: { bridgeToBase: vi.fn(async () => ({ Ok: makeDeposit({ status: { Escrowed: null }, client_request_id: new Uint8Array(16).fill(0xcd) }) })) as never, requestDepositRefund: vi.fn() as never },
    reads: { getDeposit: async () => null, getDepositByRequest: async () => null },
    separateApproval: false,
    isCurrent: () => true,
    signing,
    random: (length) => new Uint8Array(length).fill(0xcd),
    now: () => NOW,
    sleep: async () => {},
  };
}

const quote = () =>
  buildQuote({
    amountE8s: 2_500_000_000n,
    recipient: RECIPIENT,
    ledgerFeeE8s: 100_000n,
    depositFeeE8s: 10_000n,
    allowanceE8s: 10n ** 12n,
    allowanceExpiresAtNs: null,
    balanceE8s: 10_000_000_000n,
    nowMs: NOW,
  });

describe('versions and keys', () => {
  it('bumps the version, keeps the v1 storage key, and binds legacy records to Base', () => {
    expect(INTENT_VERSION).toBe(2);
    expect(LEGACY_INTENT_VERSION).toBe(1);
    expect(LEGACY_DESTINATION_ID).toBe('base');
    expect(INTENT_STORAGE_PREFIX).toBe('rumi:base-bridge:v1:');
    expect(intentStorageKey(ALICE)).toBe(`rumi:base-bridge:v1:${ALICE}`);
  });
});

describe('new records carry the destination key', () => {
  it('an ICP to Base request names its destination, in memory and on disk', async () => {
    const storage = createMemoryStorage();
    const store = new IntentStore(storage, ALICE);
    await startIcpToBase(icpDeps(store), quote(), () => {});

    const [saved] = store.list() as IcpToBaseIntent[];
    expect(saved).toMatchObject({ v: 2, destination: 'base', kind: 'icp_to_base' });
    const raw = JSON.parse(storage.map.get(`rumi:base-bridge:v1:${ALICE}`)!);
    expect(raw.v).toBe(2);
    expect(raw.intents[0]).toMatchObject({ v: 2, destination: 'base' });
  });

  it('the key follows the destination the flow was started for', async () => {
    const store = new IntentStore(createMemoryStorage(), ALICE);
    await startIcpToBase(icpDeps(store, makeSigning(makeDestination({ id: 'arbitrum', name: 'Arbitrum One', chainId: '42161' }))), quote(), () => {});
    expect((store.list()[0] as IcpToBaseIntent).destination).toBe('arbitrum');
  });

  it('round-trips through storage and refuses a version 2 intent that names no destination', () => {
    const storage = createMemoryStorage();
    const store = new IntentStore(storage, ALICE);
    const intent: IcpToBaseIntent = {
      v: 2,
      destination: 'base',
      kind: 'icp_to_base',
      clientRequestId: `0x${'ab'.repeat(16)}`,
      amountE8s: 5n,
      recipient: RECIPIENT,
      expectedDepositFee: 1n,
      createdAt: 1,
      status: 'prepared',
    };
    store.put(intent);
    expect(new IntentStore(storage, ALICE).get(intent.clientRequestId)).toEqual(intent);

    for (const bad of [{ destination: undefined }, { destination: '' }, { destination: 'Base Chain!' }, { destination: 8453 }]) {
      const raw = JSON.stringify({ v: 2, principal: ALICE, intents: [{ ...JSON.parse(JSON.stringify({ ...intent, amountE8s: '5', expectedDepositFee: '1' })), ...bad }] });
      const reopened = new IntentStore(createMemoryStorage({ [intentStorageKey(ALICE)]: raw }), ALICE);
      expect(reopened.list(), JSON.stringify(bad)).toEqual([]);
      expect(reopened.loadStatus).toBe('recovered');
    }
  });
});

describe('a legacy record is read as Base', () => {
  it('reads every keyless intent and binds it explicitly to Base', () => {
    const store = new IntentStore(createMemoryStorage({ [intentStorageKey(ALICE)]: legacyRecord() }), ALICE);
    expect(store.loadStatus).toBe('ok');
    expect(store.readOnly).toBe(false);
    const all = store.list();
    expect(all).toHaveLength(2);
    expect(all.map(intentDestination)).toEqual(['base', 'base']);
    expect(all.every((intent) => intent.v === 1 && intent.destination === undefined)).toBe(true);
    expect(store.listFor('base')).toHaveLength(2);
    expect(store.listFor('ethereum')).toEqual([]);
  });

  it('hides a Base request from another destination, and the other way round', () => {
    const storage = createMemoryStorage({ [intentStorageKey(ALICE)]: legacyRecord() });
    const store = new IntentStore(storage, ALICE);
    expect(unresolvedIntents(store, 'base')).toHaveLength(1);
    expect(unresolvedIntents(store, 'arbitrum')).toEqual([]);
    // The guard in startIcpToBase still sees every destination's unresolved request.
    expect(unresolvedIntents(store)).toHaveLength(1);
    store.put({
      v: 2,
      destination: 'arbitrum',
      kind: 'icp_to_base',
      clientRequestId: `0x${'11'.repeat(16)}`,
      amountE8s: 1n,
      recipient: RECIPIENT,
      expectedDepositFee: 1n,
      createdAt: 3_000,
      status: 'approved',
    });
    expect(unresolvedIntents(store, 'arbitrum')).toHaveLength(1);
    expect(unresolvedIntents(store, 'base')).toHaveLength(1);
  });

  it('keeps legacy intents untouched when a new one is written, and upgrades only the record version', () => {
    const storage = createMemoryStorage({ [intentStorageKey(ALICE)]: legacyRecord() });
    const store = new IntentStore(storage, ALICE);
    store.put({
      v: 2,
      destination: 'base',
      kind: 'icp_to_base',
      clientRequestId: `0x${'22'.repeat(16)}`,
      amountE8s: 1n,
      recipient: RECIPIENT,
      expectedDepositFee: 1n,
      createdAt: 4_000,
      status: 'done',
    });
    const raw = JSON.parse(storage.map.get(intentStorageKey(ALICE))!);
    expect(raw.v).toBe(2);
    expect(raw.intents).toHaveLength(3);
    const legacy = raw.intents.filter((entry: { v: number }) => entry.v === 1);
    expect(legacy).toHaveLength(2);
    for (const entry of legacy) expect('destination' in entry).toBe(false);

    const reopened = new IntentStore(storage, ALICE);
    expect(reopened.loadStatus).toBe('ok');
    expect(reopened.list().map(intentDestination)).toEqual(['base', 'base', 'base']);
    expect(reopened.list().filter((intent) => intent.v === 1)).toHaveLength(2);
  });

  it('a legacy Base to ICP transfer can still be resumed from Base', () => {
    const store = new IntentStore(createMemoryStorage({ [intentStorageKey(ALICE)]: legacyRecord() }), ALICE);
    const unsent = store.list().find((intent): intent is BaseToIcpIntent => intent.kind === 'base_to_icp')!;
    expect(intentDestination(unsent)).toBe('base');
    expect(unsent.chainId).toBe(8453);
  });
});

describe('a newer record stays read-only', () => {
  for (const version of [3, 4, 99]) {
    it(`version ${version} is never overwritten`, () => {
      const future = JSON.stringify({ v: version, principal: ALICE, intents: [{ anything: true }] });
      const storage = createMemoryStorage({ [intentStorageKey(ALICE)]: future });
      const store = new IntentStore(storage, ALICE);
      expect(store.loadStatus).toBe('newer_version');
      expect(store.readOnly).toBe(true);
      expect(store.list()).toEqual([]);
      expect(() =>
        store.put({
          v: 2,
          destination: 'base',
          kind: 'icp_to_base',
          clientRequestId: `0x${'33'.repeat(16)}`,
          amountE8s: 1n,
          recipient: RECIPIENT,
          expectedDepositFee: 1n,
          createdAt: 1,
          status: 'prepared',
        }),
      ).toThrowError(expect.objectContaining({ code: 'read_only' }));
      expect(storage.map.get(intentStorageKey(ALICE))).toBe(future);
    });
  }

  it('an app that only knows version 1 would see what this app writes as newer, so it goes read-only', () => {
    const storage = createMemoryStorage();
    new IntentStore(storage, ALICE).put({
      v: 2,
      destination: 'base',
      kind: 'icp_to_base',
      clientRequestId: `0x${'44'.repeat(16)}`,
      amountE8s: 1n,
      recipient: RECIPIENT,
      expectedDepositFee: 1n,
      createdAt: 1,
      status: 'prepared',
    });
    const written = JSON.parse(storage.map.get(intentStorageKey(ALICE))!);
    expect(written.v).toBeGreaterThan(LEGACY_INTENT_VERSION);
  });

  it('still treats a record of no known version as unreadable (copied aside, not read-only)', () => {
    const raw = JSON.stringify({ v: 0, principal: ALICE, intents: [] });
    const storage = createMemoryStorage({ [intentStorageKey(ALICE)]: raw });
    const store = new IntentStore(storage, ALICE, { now: () => 9 });
    expect(store.loadStatus).toBe('recovered');
    expect(storage.map.get(`${intentStorageKey(ALICE)}:unreadable:9`)).toBe(raw);
  });
});
