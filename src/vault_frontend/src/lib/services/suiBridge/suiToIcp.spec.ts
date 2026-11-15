import { Transaction } from '@mysten/sui/transactions';
import { fromBase64, toBase64 } from '@mysten/sui/utils';
import { Principal } from '@dfinity/principal';
import { describe, expect, it, vi } from 'vitest';
import { createMemoryStorage } from '../baseBridge/testSupport';
import { base58Encode, fromHex, toHex, type Bytes } from '../solanaBridge/bytes';
import { transferId } from './ids';
import { SuiIntentStore, type SuiReturnIntent } from './intents';
import {
  INITIAL_SUI_RETURN_STATE,
  continuePrepared,
  describeReturnProgress,
  discardReturn,
  evaluateReturnDraft,
  expectationOf,
  mayDiscard,
  notifyReturn,
  prepareReturn,
  rebroadcastReturn,
  reconcileReturn,
  reduceSuiReturn,
  startReturn,
  trackReturn,
  trackedReturns,
  unresolvedReturns,
  verdictOf,
  type ReturnDeps,
  type ReturnPlan,
  type SuiReturnEvent,
  type SuiReturnState,
} from './suiToIcp';
import {
  COIN_TYPE,
  KEYS,
  OTHER_KEYPAIR,
  SUI_BRIDGE_ID,
  TEST_OWNER_TEXT,
  coin,
  createFakeSui,
  landOn,
  makeCandidate,
  makeFakeWallet,
  makeSuiConfig,
  makeSuiRelease,
  makeSuiSigning,
  rejection,
  releaseFor,
  signLikeAWallet,
  type FakeSui,
  type FakeWallet,
} from './testSupport';
import { buildReturnTransaction, digestOf, type ReturnCallPlan } from './transaction';
import { formatMist } from './units';
import { SUI_MAINNET } from './networks';
import type { CandidateView, ReleaseView } from '$declarations/rumi_icusd_sui_bridge/rumi_icusd_sui_bridge.did';

const fill = (byte: number, n = 32): Bytes => new Uint8Array(n).fill(byte);
const E8 = 100_000_000n;

interface Harness {
  node: FakeSui;
  wallet: FakeWallet;
  storage: ReturnType<typeof createMemoryStorage>;
  store: SuiIntentStore;
  signing: ReturnType<typeof makeSuiSigning>;
  deps: ReturnDeps;
  events: SuiReturnEvent[];
  emit: (event: SuiReturnEvent) => void;
  views: { release: ReleaseView | null; candidate: CandidateView | null };
  session: { current: boolean; wallet: FakeWallet | null };
  notify: ReturnType<typeof vi.fn>;
}

function harness(options: { wallet?: Parameters<typeof makeFakeWallet>[0]; storage?: ReturnType<typeof createMemoryStorage>; coins?: ReturnType<typeof coin>[] } = {}): Harness {
  const config = makeSuiConfig();
  const node = createFakeSui(options.coins ? { coins: options.coins } : {});
  const wallet = makeFakeWallet(options.wallet);
  const storage = options.storage ?? createMemoryStorage();
  const store = new SuiIntentStore(storage, TEST_OWNER_TEXT);
  const signing = makeSuiSigning();
  const views = { release: null as ReleaseView | null, candidate: null as CandidateView | null };
  const session = { current: true, wallet: wallet as FakeWallet | null };
  const notify = vi.fn(async () => ({ Ok: null }) as never);
  let seed = 0;
  const deps: ReturnDeps = {
    config,
    signing,
    store,
    rpc: node,
    wallet: () => session.wallet,
    reads: { getRelease: async () => views.release, getCandidate: async () => views.candidate },
    bridge: { notifySuiReturn: notify },
    isCurrent: () => session.current,
    random: (n) => Uint8Array.from({ length: n }, (_, i) => (seed++ * 7 + i + 1) & 0xff),
    now: () => 1_700_000_000_000,
    sleep: async () => {},
  };
  const events: SuiReturnEvent[] = [];
  return { node, wallet, storage, store, signing, deps, events, emit: (e) => events.push(e), views, session, notify };
}

const DRAFT = { amountText: '3', principalText: TEST_OWNER_TEXT, subaccountText: '' };
const sendOnly = { signs: false, executes: true };

async function plan(h: Harness, input = DRAFT): Promise<ReturnPlan> {
  const result = await prepareReturn(h.deps, input);
  if (!result.ok) throw new Error(result.message);
  return result.plan;
}

const last = (h: Harness) => h.events[h.events.length - 1];
const only = (h: Harness) => h.store.list()[0] as SuiReturnIntent;

/** The call the plan reviewed, as plain parameters (so a spec can play a wallet that changes one of them). */
function callOf(p: ReturnPlan): ReturnCallPlan {
  return {
    packageId: p.config.packageId,
    gateId: p.config.gateId,
    gateInitialSharedVersion: p.config.gateInitialSharedVersion,
    sender: p.sender,
    coins: p.coins,
    amountE8s: p.amountE8s,
    principalBytes: Principal.fromText(p.icpPrincipal).toUint8Array(),
    subaccount: p.icpSubaccount ? fromHex(p.icpSubaccount)! : new Uint8Array(32),
    nonce: BigInt(p.nonce),
  };
}

describe('the draft', () => {
  const config = makeSuiConfig();
  const holdings = [coin(KEYS.coinA, 500_000_000n)];

  it('accepts a good return and explains the first problem otherwise', () => {
    expect(evaluateReturnDraft(DRAFT, config, holdings)).toMatchObject({ ok: true, amountE8s: 300_000_000n, problem: null });
    const problem = (patch: Partial<typeof DRAFT>, list = holdings) => evaluateReturnDraft({ ...DRAFT, ...patch }, config, list).problem;
    expect(problem({ amountText: '' })).toMatch(/amount/i);
    expect(problem({ amountText: '1.123456789' })).toMatch(/decimal/i);
    expect(problem({ amountText: '0.0002' })).toBe('The amount must be larger than the release fee of 0.0002 icUSD.');
    expect(problem({ principalText: '' })).toMatch(/principal/i);
    expect(problem({ principalText: '2vxsx-fae' })).toMatch(/anonymous/i);
    expect(problem({ principalText: config.instanceCanisterId })).toMatch(/bridge canister itself/i);
    expect(problem({ subaccountText: 'AB' })).toMatch(/subaccount/i);
    expect(problem({ amountText: '6' })).toBe('Your Sui address does not hold that much icUSD.');
    // Beyond a u64 is refused, wherever the parser or the Gate rule catches it.
    expect(problem({ amountText: '184467440737.09551616' })).toMatch(/too large|larger than/);
    expect(problem({ amountText: '18446744073709.551616' })).toMatch(/too large|larger than/);
  });

  it('enforces the Gate minimum, and carries the all-zero subaccount as the default', () => {
    const gated = makeSuiConfig({ observed: { min_bridge_out: 50_000_000n } });
    expect(evaluateReturnDraft({ ...DRAFT, amountText: '0.4' }, gated, null).problem).toBe('The Gate accepts at least 0.5 icUSD in one return.');
    expect(evaluateReturnDraft({ ...DRAFT, subaccountText: '0'.repeat(64) }, config, null)).toMatchObject({ ok: true, subaccount: null });
    expect(evaluateReturnDraft({ ...DRAFT, subaccountText: 'AB'.repeat(32) }, config, null)).toMatchObject({ ok: true, subaccount: 'ab'.repeat(32) });
  });

  it('blocks on a halt and only warns when releases are paused', () => {
    expect(evaluateReturnDraft(DRAFT, makeSuiConfig({ config: { halted: ['stop'] } }), null)).toMatchObject({ ok: false, problem: 'The bridge is halted for safety: stop' });
    const paused = evaluateReturnDraft(DRAFT, makeSuiConfig({ config: { pauses: { mints: false, observer: false, releases: true, deposits: false } } }), null);
    expect(paused.ok).toBe(true);
    expect(paused.warning).toMatch(/Releases to the Internet Computer are paused/);
  });
});

describe('preparing the transaction', () => {
  it('builds the exact reviewed call: coins, nonce, destination and the Gate transfer id (never a digest)', async () => {
    const h = harness();
    const p = await plan(h);
    expect(p).toMatchObject({ sender: KEYS.holder, amountE8s: 300_000_000n, coinTotalE8s: 5_000_000_000n, icpPrincipal: TEST_OWNER_TEXT, icpSubaccount: null });
    expect(p.coins.map((c) => c.objectId)).toEqual([KEYS.coinA]);
    // 16 random bytes made the nonce; the transfer id is exactly the Gate's function of (chain, gate, sender, nonce).
    expect(p.transferId).toBe(transferId(fromHex(SUI_MAINNET.chainIdentifier)!, fromHex(KEYS.gateId.slice(2))!, fromHex(KEYS.holder.slice(2))!, BigInt(p.nonce)));
    expect(p.transferId).toMatch(/^[0-9a-f]{64}$/);
    // Preparing made no wallet call and saved nothing.
    expect(h.wallet.calls).toHaveLength(0);
    expect(h.store.list()).toHaveLength(0);
    expect(h.node.callsTo('listCoins')).toHaveLength(1);
    expect(h.node.callsTo('getSuiBalance')).toHaveLength(1);
  });

  it('keeps the gas balance (SUI, 9 decimals) apart from icUSD (8 decimals)', async () => {
    const h = harness();
    h.node.gas = 1_500_000_000n;
    const p = await plan(h);
    expect(p.gasBalanceMist).toBe(1_500_000_000n);
    expect(formatMist(p.gasBalanceMist!)).toBe('1.5');
    expect(formatMist(1n)).toBe('0.000000001');
    expect(formatMist(0n)).toBe('0');
    expect(formatMist(123_456_789_012n)).toBe('123.456789012');
    // An amount in icUSD is never scaled by the SUI factor.
    expect(p.amountE8s).toBe(3n * E8);
  });

  it('uses the u64 nonce boundaries as they are', async () => {
    const h = harness();
    h.deps.random = (n) => fill(0xff, n);
    expect((await plan(h)).nonce).toBe('18446744073709551615');
    h.deps.random = (n) => fill(0x00, n);
    expect((await plan(h)).nonce).toBe('0');
  });

  it('refuses to prepare without a wallet, on another network, with no gas, without enough icUSD, or when the node cannot be read', async () => {
    const h = harness();
    h.session.wallet = null;
    expect(await prepareReturn(h.deps, DRAFT)).toMatchObject({ ok: false, message: 'Connect your Sui wallet first.' });
    h.session.wallet = h.wallet;
    h.wallet.state.chains = ['sui:testnet'];
    expect(await prepareReturn(h.deps, DRAFT)).toMatchObject({ ok: false, message: expect.stringContaining('not on Sui') });
    h.wallet.state.chains = [SUI_MAINNET.chain];
    h.node.gas = 0n;
    expect(await prepareReturn(h.deps, DRAFT)).toMatchObject({ ok: false, message: expect.stringContaining('holds no SUI') });
    h.node.gas = 1n;
    expect(await prepareReturn(h.deps, { ...DRAFT, amountText: '60' })).toMatchObject({ ok: false, message: 'Your Sui address does not hold that much icUSD.' });
    h.node.failReads = new Error('node down');
    expect(await prepareReturn(h.deps, DRAFT)).toMatchObject({ ok: false, message: expect.stringContaining('node down') });
    expect(h.wallet.calls).toHaveLength(0);
  });

  it('splits one larger coin, merges small ones, and uses a coin of exactly the amount whole', async () => {
    const exact = harness({ coins: [coin(KEYS.coinA, 300_000_000n), coin(KEYS.coinB, 900_000_000n)] });
    expect((await plan(exact)).coins.map((c) => c.balance)).toEqual([300_000_000n]);
    const merged = harness({ coins: [coin(KEYS.coinA, 200_000_000n), coin(KEYS.coinB, 150_000_000n)] });
    const p = await plan(merged, { ...DRAFT, amountText: '3.5' });
    expect(p.coins.map((c) => c.balance)).toEqual([200_000_000n, 150_000_000n]);
    expect(p.coinTotalE8s).toBe(350_000_000n);
  });
});

describe('signing with a wallet that returns signed bytes', () => {
  it('saves the attempt first, asks the wallet once, saves the validated bytes, signature and digest BEFORE broadcasting, then sends exactly those bytes', async () => {
    const h = harness();
    const p = await plan(h);
    const seen: string[] = [];
    const realSign = h.wallet.onSign;
    h.wallet.onSign = async (tx, chain) => {
      seen.push(`wallet: ${only(h).status}`);
      return realSign(tx, chain);
    };
    const realExecute = h.node.executeTransaction.bind(h.node);
    h.node.executeTransaction = async (bytes, signatures) => {
      const saved = only(h);
      seen.push(`execute: ${saved.status} ${saved.signedBytes === toBase64(bytes)} ${saved.digest === digestOf(bytes)}`);
      return realExecute(bytes, signatures);
    };
    const pending = startReturn(h.deps, p, h.emit);
    // Still inside the "click handler": the intent is saved and the wallet has already been asked.
    expect(h.wallet.calls).toHaveLength(1);
    expect(only(h).status).toBe('signing');
    await pending;
    expect(seen).toEqual(['wallet: signing', 'execute: signed true true']);
    expect(h.wallet.calls.map((c) => c.method)).toEqual(['signTransaction']);
    expect(h.wallet.calls[0].chain).toBe('sui:mainnet');
    const saved = only(h);
    expect(saved).toMatchObject({ status: 'sent', signMode: 'sign', sender: KEYS.holder, amountE8s: p.amountE8s, nonce: p.nonce, transferId: p.transferId, packageId: p.config.packageId, gateId: p.config.gateId, coinType: COIN_TYPE, destination: 'sui-mainnet' });
    expect(saved.coins).toEqual(p.coins);
    expect(h.node.executed).toHaveLength(1);
    expect(toBase64(h.node.executed[0].bytes)).toBe(saved.signedBytes);
    expect(h.node.executed[0].signatures).toEqual([saved.signature]);
    expect(saved.digest).toBe(digestOf(fromBase64(saved.signedBytes!)));
    // What the wallet was handed is the reviewed call, with the amount in e8s and the destination bytes.
    const json = JSON.parse(await h.wallet.calls[0].transaction.toJSON());
    expect(JSON.stringify(json)).toContain('bridge_out');
    expect(last(h)).toMatchObject({ type: 'sent' });
  });

  it('refuses to broadcast anything the wallet changed, and forgets the unsent attempt', async () => {
    const tampered: Array<[string, (p: ReturnPlan) => Promise<{ bytes: string; signature: string }>]> = [
      ['another amount', async (p) => pick(await signLikeAWallet(buildReturnTransaction({ ...callOf(p), amountE8s: p.amountE8s + 1n })))],
      ['another destination', async (p) => pick(await signLikeAWallet(buildReturnTransaction({ ...callOf(p), principalBytes: Principal.fromText('rrkah-fqaaa-aaaaa-aaaaq-cai').toUint8Array() })))],
      ['another nonce', async (p) => pick(await signLikeAWallet(buildReturnTransaction({ ...callOf(p), nonce: BigInt(p.nonce) + 1n })))],
      ['another subaccount', async (p) => pick(await signLikeAWallet(buildReturnTransaction({ ...callOf(p), subaccount: fill(0x07) })))],
      ['another Gate', async (p) => pick(await signLikeAWallet(buildReturnTransaction({ ...callOf(p), gateId: KEYS.mintCap })))],
      ['another package', async (p) => pick(await signLikeAWallet(buildReturnTransaction({ ...callOf(p), packageId: KEYS.gas })))],
      ['another coin', async (p) => pick(await signLikeAWallet(buildReturnTransaction({ ...callOf(p), coins: [coin(KEYS.coinB, 5_000_000_000n)] })))],
      ['an extra step', async (p) => pick(await signLikeAWallet(p.transaction, { mutate: (t) => void t.transferObjects([t.gas], KEYS.otherHolder) }))],
      ['another sender', async (p) => pick(await signLikeAWallet(p.transaction, { mutate: (t) => void t.setSender(KEYS.otherHolder) }))],
      ['a sponsor', async (p) => pick(await signLikeAWallet(p.transaction, { mutate: (t) => void t.setGasOwner(KEYS.otherHolder) }))],
      ['a signature from another key', async (p) => ({ bytes: (await signLikeAWallet(p.transaction)).bytes, signature: (await signLikeAWallet(p.transaction, { keypair: OTHER_KEYPAIR })).signature })],
      ['bytes that are not a transaction', async () => ({ bytes: toBase64(fill(1, 40)), signature: 'AAAA' })],
    ];
    for (const [name, change] of tampered) {
      const h = harness();
      const p = await plan(h);
      h.wallet.onSign = async () => change(p);
      await startReturn(h.deps, p, h.emit);
      expect(last(h), name).toMatchObject({ type: 'failed', message: expect.stringContaining('Nothing was sent') });
      expect(h.node.executed, name).toHaveLength(0);
      expect(h.store.list(), name).toHaveLength(0);
    }
  });

  it('declines cleanly on a rejection, and forgets the attempt on any other wallet error (nothing was sent either way)', async () => {
    const a = harness();
    a.wallet.onSign = async () => {
      throw rejection();
    };
    await startReturn(a.deps, await plan(a), a.emit);
    expect(last(a)).toEqual({ type: 'declined' });
    expect(a.store.list()).toHaveLength(0);
    const b = harness();
    b.wallet.onSign = async () => {
      throw new Error('wallet crashed');
    };
    await startReturn(b.deps, await plan(b), b.emit);
    expect(last(b)).toMatchObject({ type: 'failed', message: expect.stringContaining('Nothing was sent') });
    expect(b.store.list()).toHaveLength(0);
    expect(b.node.executed).toHaveLength(0);
  });

  it('keeps the signed bytes when the broadcast result is unknown, and resends the SAME bytes without asking the wallet again', async () => {
    const h = harness();
    const p = await plan(h);
    h.node.failExecute = new Error('gateway timeout');
    await startReturn(h.deps, p, h.emit);
    expect(last(h)).toMatchObject({ type: 'broadcast_uncertain' });
    const signed = only(h);
    expect(signed).toMatchObject({ status: 'signed' });
    expect(h.wallet.calls).toHaveLength(1);
    h.node.failExecute = null;
    await rebroadcastReturn(h.deps, signed, h.emit);
    expect(h.wallet.calls).toHaveLength(1);
    expect(h.node.executed).toHaveLength(1);
    expect(toBase64(h.node.executed[0].bytes)).toBe(signed.signedBytes);
    expect(h.node.executed[0].signatures).toEqual([signed.signature]);
    expect(only(h)).toMatchObject({ status: 'sent', nonce: p.nonce, digest: signed.digest });
    // A second explicit resend is the same bytes again, never a rebuilt transaction.
    await rebroadcastReturn(h.deps, only(h), h.emit);
    expect(h.node.executed.map((e) => toBase64(e.bytes))).toEqual([signed.signedBytes, signed.signedBytes]);
    expect(h.wallet.calls).toHaveLength(1);
  });

  it('treats a node answer with a different digest as unconfirmed, never as sent', async () => {
    const h = harness();
    h.node.executeTransaction = async () => ({ digest: base58Encode(fill(0x31)), success: true, checkpoint: null, epoch: null, error: null });
    await startReturn(h.deps, await plan(h), h.emit);
    expect(last(h)).toMatchObject({ type: 'broadcast_uncertain', message: expect.stringContaining('different digest') });
    expect(only(h).status).toBe('signed');
  });
});

describe('signing with a wallet that signs and executes', () => {
  it('saves the semantic attempt before the prompt, validates what the wallet executed and records the digest', async () => {
    const h = harness({ wallet: sendOnly });
    const p = await plan(h);
    const seen: string[] = [];
    const real = h.wallet.onSignAndExecute;
    h.wallet.onSignAndExecute = async (tx, chain) => {
      seen.push(only(h).status);
      return real(tx, chain);
    };
    await startReturn(h.deps, p, h.emit);
    expect(seen).toEqual(['signing']);
    expect(h.wallet.calls.map((c) => c.method)).toEqual(['signAndExecuteTransaction']);
    expect(only(h)).toMatchObject({ status: 'sent', signMode: 'sign_and_execute', transferId: p.transferId, nonce: p.nonce });
    expect(only(h).digest).toBe(digestOf(fromBase64(only(h).signedBytes!)));
    expect(h.node.executed).toHaveLength(0);
  });

  it('clears the attempt only on the explicit rejection code, whatever its message says', async () => {
    for (const message of ['User rejected the request.', 'closed', 'Transaction cancelled']) {
      const h = harness({ wallet: sendOnly });
      h.wallet.onSignAndExecute = async () => {
        throw rejection(message);
      };
      await startReturn(h.deps, await plan(h), h.emit);
      expect(last(h), message).toEqual({ type: 'declined' });
      expect(h.store.list(), message).toHaveLength(0);
    }
  });

  it('HOLDS the original under its identity when the wallet executes and then throws transport text, and never signs again', async () => {
    for (const text of ['connection closed', 'Transaction cancelled by the relay', 'request denied: HTTP 429', 'popup closed unexpectedly', 'RPC timeout while confirming']) {
      const h = harness({ wallet: sendOnly });
      const p = await plan(h);
      h.wallet.onSignAndExecute = async (tx) => {
        await signLikeAWallet(tx); // the wallet really executed it, then lost its connection
        throw new Error(text);
      };
      await startReturn(h.deps, p, h.emit);
      expect(last(h), text).toMatchObject({ type: 'held' });
      const held = only(h);
      expect(held, text).toMatchObject({ status: 'held', signMode: 'sign_and_execute', nonce: p.nonce, transferId: p.transferId, amountE8s: p.amountE8s });
      expect(new SuiIntentStore(h.storage, TEST_OWNER_TEXT).list()[0], text).toMatchObject({ id: held.id, status: 'held', nonce: p.nonce });
      expect(h.wallet.calls, text).toHaveLength(1);
      expect(h.node.executed, text).toHaveLength(0);
      // A new transfer cannot start while it is held, and does not touch the wallet.
      await startReturn(h.deps, await plan(h), h.emit);
      expect(last(h), text).toMatchObject({ type: 'blocked', message: expect.stringContaining('no recorded outcome') });
      expect(h.wallet.calls, text).toHaveLength(1);
      expect(h.store.list(), text).toHaveLength(1);
    }
  });

  it('holds when the answer is unusable, changed from the reviewed call, or its digest does not match its bytes', async () => {
    const answers: Array<[string, (p: ReturnPlan) => Promise<{ bytes: string; signature: string; digest: string; effects: string }>]> = [
      ['unusable bytes', async () => ({ bytes: '***', signature: 'AAAA', digest: 'x', effects: '' })],
      ['another amount', async (p) => signLikeAWallet(buildReturnTransaction({ ...callOf(p), amountE8s: p.amountE8s + 1n }))],
      ['a digest that is not the bytes\'', async (p) => ({ ...(await signLikeAWallet(p.transaction)), digest: base58Encode(fill(0x31)) })],
    ];
    for (const [name, answer] of answers) {
      const h = harness({ wallet: sendOnly });
      const p = await plan(h);
      h.wallet.onSignAndExecute = async () => answer(p);
      await startReturn(h.deps, p, h.emit);
      expect(last(h), name).toMatchObject({ type: 'held' });
      expect(only(h), name).toMatchObject({ status: 'held', nonce: p.nonce, transferId: p.transferId });
      expect(h.wallet.calls, name).toHaveLength(1);
    }
  });
});

describe('the wallet account and the network', () => {
  it('asks nothing when the account changed after the review, the wallet left the network or the destination is disabled', async () => {
    const h = harness();
    const p = await plan(h);
    h.wallet.state.address = KEYS.otherHolder;
    await startReturn(h.deps, p, h.emit);
    expect(last(h)).toMatchObject({ type: 'blocked', message: expect.stringContaining('account changed') });
    h.wallet.state.address = KEYS.holder;
    h.wallet.state.chains = ['sui:testnet'];
    await startReturn(h.deps, p, h.emit);
    expect(last(h)).toMatchObject({ type: 'blocked', message: expect.stringContaining('not on Sui') });
    h.wallet.state.chains = [SUI_MAINNET.chain];
    h.signing.fresh.value = { status: 'disabled', reason: 'Not available yet.' };
    await startReturn(h.deps, p, h.emit);
    expect(last(h)).toMatchObject({ type: 'blocked', message: expect.stringContaining('Not available yet.') });
    h.signing.fresh.value = { status: 'enabled', reason: null };
    h.session.wallet = null;
    await startReturn(h.deps, p, h.emit);
    expect(last(h)).toMatchObject({ type: 'blocked', message: expect.stringContaining('not connected') });
    h.session.current = false;
    await startReturn(h.deps, p, h.emit);
    expect(last(h)).toEqual({ type: 'session_changed' });
    expect(h.wallet.calls).toHaveLength(0);
    expect(h.store.list()).toHaveLength(0);
    expect(h.node.executed).toHaveLength(0);
  });

  it('keeps an old attempt exactly as it was when the account changes, and will not continue it elsewhere', async () => {
    const h = harness();
    const p = await plan(h);
    await startReturn(h.deps, p, h.emit);
    const sent = only(h);
    h.wallet.state.address = KEYS.otherHolder;
    expect(await reconcileReturn(h.deps, sent)).toMatchObject({ digest: sent.digest });
    expect(only(h)).toEqual(sent);
    // A never-asked attempt cannot be continued from another account.
    const prepared: SuiReturnIntent = { ...sent, id: '33'.repeat(16), status: 'prepared', signedBytes: undefined, signature: undefined, digest: undefined, createdAt: 5 };
    delete prepared.signedBytes;
    delete prepared.signature;
    delete prepared.digest;
    h.store.put(prepared);
    await continuePrepared(h.deps, prepared, h.emit);
    expect(last(h)).toMatchObject({ type: 'blocked', message: expect.stringContaining('wallet account this attempt was prepared for') });
    expect(h.wallet.calls).toHaveLength(1);
  });
});

describe('the journal gates every wallet call', () => {
  it('asks no wallet when the recovery record cannot be saved (storage unavailable, or written by a newer app)', async () => {
    const h = harness();
    const p = await plan(h);
    h.deps.store = new SuiIntentStore(null, TEST_OWNER_TEXT);
    await startReturn(h.deps, p, h.emit);
    expect(last(h)).toMatchObject({ type: 'blocked', message: expect.stringContaining('nothing was sent') });
    expect(h.wallet.calls).toHaveLength(0);
    const newer = createMemoryStorage();
    newer.setItem(`rumi:sui-bridge:v1:${TEST_OWNER_TEXT}`, JSON.stringify({ v: 2, principal: TEST_OWNER_TEXT, intents: [] }));
    h.deps.store = new SuiIntentStore(newer, TEST_OWNER_TEXT);
    await startReturn(h.deps, p, h.emit);
    expect(last(h)).toMatchObject({ type: 'blocked', message: expect.stringContaining('newer version') });
    expect(h.wallet.calls).toHaveLength(0);
    expect(h.node.executed).toHaveLength(0);
  });
});

describe('reload without resubmission', () => {
  it('shows each saved status without asking a wallet or the node, and tracks a sent attempt from its digest', async () => {
    const h = harness();
    await startReturn(h.deps, await plan(h), h.emit);
    const sent = only(h);
    const g = harness({ storage: h.storage });
    const at = (status: SuiReturnIntent['status'], extra: Partial<SuiReturnIntent> = {}): SuiReturnState =>
      reduceSuiReturn(INITIAL_SUI_RETURN_STATE, { type: 'resume', intent: { ...(g.store.list()[0] as SuiReturnIntent), status, ...extra } });
    expect(at('prepared').kind).toBe('prepared');
    expect(at('signing').kind).toBe('held');
    expect(at('held').kind).toBe('held');
    expect(at('signed').kind).toBe('broadcast_uncertain');
    expect(at('sent').kind).toBe('tracking');
    expect(unresolvedReturns(g.store)).toEqual([]);
    expect(trackedReturns(g.store, 'sui-mainnet')).toHaveLength(1);
    expect(g.wallet.calls).toHaveLength(0);
    expect(g.node.executed).toHaveLength(0);
    expect(sent.status).toBe('sent');
  });

  it('re-asks the wallet for a never-asked attempt only with the SAME call (coins, nonce, transfer id), on the same account', async () => {
    const h = harness();
    const p = await plan(h);
    h.store.put({
      v: 1, kind: 'sui_to_icp', id: '44'.repeat(16), destination: 'sui-mainnet', chainIdentifier: SUI_MAINNET.chainIdentifier, instance: SUI_BRIDGE_ID,
      packageId: p.config.packageId, gateId: p.config.gateId, coinType: COIN_TYPE, sender: p.sender, coins: p.coins, icpPrincipal: p.icpPrincipal, icpSubaccount: null,
      amountE8s: p.amountE8s, nonce: p.nonce, transferId: p.transferId, signMode: 'sign', createdAt: 5, status: 'prepared',
    });
    await continuePrepared(h.deps, h.store.get('44'.repeat(16)) as SuiReturnIntent, h.emit);
    expect(h.wallet.calls).toHaveLength(1);
    const json = await h.wallet.calls[0].transaction.toJSON();
    expect(json).toBe(await p.transaction.toJSON());
    expect(only(h)).toMatchObject({ status: 'sent', nonce: p.nonce, transferId: p.transferId });
  });
});

describe('what the evidence may and may not allow', () => {
  async function sentAttempt(h: Harness) {
    await startReturn(h.deps, await plan(h), h.emit);
    return only(h);
  }

  it('finds the bridge record through the predicted transfer id, and the observer\'s hint as only a hint', async () => {
    const h = harness({ wallet: sendOnly });
    h.wallet.onSignAndExecute = async () => {
      throw new Error('timeout');
    };
    const p = await plan(h);
    await startReturn(h.deps, p, h.emit);
    const held = only(h);
    let evidence = await reconcileReturn(h.deps, held);
    expect(evidence).toMatchObject({ digest: null, chain: { kind: 'no_digest' }, release: null, deviation: null, candidate: null });
    expect(verdictOf(evidence)).toBe('unknown');
    expect(discardReturn(h.deps, held, verdictOf(evidence))).toMatchObject({ ok: false });
    h.views.candidate = makeCandidate({ client_nonce: BigInt(p.nonce) });
    evidence = await reconcileReturn(h.deps, held);
    expect(describeReturnProgress(evidence).label).toBe('Seen by the bridge, verifying');
    expect(verdictOf(evidence)).toBe('unknown');
    const asked: string[] = [];
    h.views.release = releaseFor(held);
    h.deps.reads.getRelease = async (id) => (asked.push(id), h.views.release);
    evidence = await reconcileReturn(h.deps, held);
    expect(asked).toEqual([p.transferId]);
    expect(verdictOf(evidence)).toBe('credited');
    expect(describeReturnProgress(evidence).label).toBe('Burn observed, release pending');
  });

  it('keeps a sent but unindexed attempt unknown, however long ago, and never lets it be forgotten', async () => {
    const h = harness();
    const sent = await sentAttempt(h);
    const evidence = await reconcileReturn(h.deps, sent);
    expect(evidence.chain).toEqual({ kind: 'not_found' });
    expect(verdictOf(evidence)).toBe('unknown');
    expect(mayDiscard(sent, verdictOf(evidence))).toBe(false);
    expect(discardReturn(h.deps, sent, verdictOf(evidence))).toMatchObject({ ok: false });
    expect(h.store.list()).toHaveLength(1);
    expect(h.wallet.calls).toHaveLength(1);
    expect(describeReturnProgress(evidence).detail).toMatch(/not proof/i);
  });

  it('does not treat a failure that is not yet in a checkpoint as definitive', async () => {
    const h = harness();
    const sent = await sentAttempt(h);
    h.node.statuses.set(sent.digest!, { success: false, checkpoint: null, epoch: 123n, error: 'abort 9' });
    const early = await reconcileReturn(h.deps, sent);
    expect(early.chain).toEqual({ kind: 'failed_on_chain', finalized: false });
    expect(verdictOf(early)).not.toBe('failed_on_chain');
    expect(discardReturn(h.deps, sent, verdictOf(early))).toMatchObject({ ok: false });
    h.node.statuses.set(sent.digest!, { success: true, checkpoint: 9n, epoch: 123n, error: null });
    const later = await reconcileReturn(h.deps, sent);
    expect(verdictOf(later)).toBe('landed');
    expect(mayDiscard(sent, verdictOf(later))).toBe(false);
  });

  it('lets an attempt go only after a FINALIZED failure of the exact saved digest, or when the wallet was never asked', async () => {
    const h = harness();
    const sent = await sentAttempt(h);
    h.node.statuses.set(sent.digest!, { success: false, checkpoint: 12n, epoch: 123n, error: 'MoveAbort 6' });
    const evidence = await reconcileReturn(h.deps, sent);
    expect(evidence.chain).toEqual({ kind: 'failed_on_chain', finalized: true });
    expect(verdictOf(evidence)).toBe('failed_on_chain');
    expect(describeReturnProgress(evidence)).toMatchObject({ label: 'The transaction failed on Sui', terminal: true });
    expect(discardReturn(h.deps, sent, verdictOf(evidence))).toEqual({ ok: true });
    expect(h.store.list()).toHaveLength(0);
    // Never asked: always removable.
    const never: SuiReturnIntent = { ...sent, id: '55'.repeat(16), status: 'prepared' };
    h.store.put(never);
    expect(discardReturn(h.deps, never, null)).toEqual({ ok: true });
  });

  it('reads an unreachable node as unavailable, never as "not found"', async () => {
    const h = harness();
    const sent = await sentAttempt(h);
    h.node.failReads = new Error('down');
    const evidence = await reconcileReturn(h.deps, sent);
    expect(evidence.chain).toMatchObject({ kind: 'unavailable' });
    expect(verdictOf(evidence)).toBe('unknown');
    expect(describeReturnProgress(evidence).label).toBe('Checking');
  });
});

describe('status comes from the canister records only', () => {
  it('walks nothing yet, observed, burn verified and release pending, to Released, marking the intent done only from the release', async () => {
    const h = harness();
    await startReturn(h.deps, await plan(h), h.emit);
    const sent = only(h);
    landOn(h.node, fromBase64(sent.signedBytes!));
    const labels: string[] = [];
    const script: Array<() => void> = [
      () => undefined,
      () => (h.views.candidate = makeCandidate({ client_nonce: BigInt(sent.nonce), verified: false, waiting: ['waiting for finality'] })),
      () => (h.views.release = releaseFor(sent, { status: { Pending: null } })),
      () => (h.views.release = releaseFor(sent, { status: { TransferUnknown: null } })),
      () => (h.views.release = releaseFor(sent, { status: { Released: null }, release_block_index: [77n], amount_to_receive_e8s: [299_980_000n] })),
    ];
    let step = 0;
    h.deps.sleep = async () => {
      expect(only(h).status, 'a callback or a digest never marks it done').toBe('sent');
      script[++step]?.();
    };
    const result = await trackReturn(h.deps, sent, (e) => labels.push(describeReturnProgress(e).label));
    expect(result).toBe('done');
    expect(labels).toEqual(['Final on Sui', 'Seen by the bridge, verifying', 'Burn observed, release pending', 'Confirming', 'Released (ledger block 77)']);
    expect(only(h).status).toBe('done');
  });

  it('shows held, re-minting and re-minted from the release record', () => {
    expect(describeReturnProgress({ digest: null, chain: { kind: 'no_digest' }, release: makeSuiRelease({ status: { Held: { reason: 'bad account' } } }), candidate: null, deviation: null })).toMatchObject({ label: 'Held', terminal: false });
    expect(describeReturnProgress({ digest: null, chain: { kind: 'no_digest' }, release: makeSuiRelease({ status: { Reminting: { deposit_id: 4n } } }), candidate: null, deviation: null }).terminal).toBe(false);
    expect(describeReturnProgress({ digest: null, chain: { kind: 'no_digest' }, release: makeSuiRelease({ status: { Reminted: { deposit_id: 4n } } }), candidate: null, deviation: null })).toMatchObject({ label: 'Re-minted to your Sui address', terminal: true });
    expect(describeReturnProgress(null).label).toBe('Sent to Sui');
  });
});

describe('a release is bound to the EXACT reviewed attempt (not just its transfer id)', () => {
  const OTHER_OWNER = Principal.fromText('rrkah-fqaaa-aaaaa-aaaaq-cai');

  /** A sign-and-execute wallet that changes one thing of the reviewed call but keeps the sender and the nonce. */
  async function changedByWallet(change: (p: ReturnPlan) => Partial<ReturnCallPlan>) {
    const h = harness({ wallet: sendOnly });
    const p = await plan(h);
    h.wallet.onSignAndExecute = async () => signLikeAWallet(buildReturnTransaction({ ...callOf(p), ...change(p) }));
    await startReturn(h.deps, p, h.emit);
    expect(last(h)).toMatchObject({ type: 'held' });
    return { h, p, held: only(h) };
  }

  it('holds a wallet-changed DESTINATION, and a record for the changed operation under the same id is a deviation: never credited, never done', async () => {
    const { h, p, held } = await changedByWallet(() => ({ principalBytes: OTHER_OWNER.toUint8Array() }));
    const before = JSON.stringify(h.store.list(), (_k, v) => (typeof v === 'bigint' ? v.toString() : v));
    // The canister recorded what actually happened on Sui: the same sender and nonce, another ICP owner.
    h.views.release = releaseFor(held, { destination: { owner: OTHER_OWNER, subaccount: [] }, status: { Released: null } });
    const evidence = await reconcileReturn(h.deps, held);
    expect(evidence.release, 'the record is not the reviewed transfer').toBeNull();
    expect(evidence.deviation?.differences.join(' ')).toContain(OTHER_OWNER.toText());
    expect(verdictOf(evidence)).toBe('deviation');
    expect(verdictOf(evidence)).not.toBe('credited');
    const progress = describeReturnProgress(evidence);
    expect(progress).toMatchObject({ label: 'The bridge recorded a different transfer', terminal: false });
    expect(progress.detail).toContain(OTHER_OWNER.toText());
    expect(progress.detail).toContain('3 icUSD');
    expect(mayDiscard(held, verdictOf(evidence))).toBe(false);
    // Tracking (even of the original) never marks it done, and the original journal is exactly as it was.
    expect(await trackReturn(h.deps, held, () => {})).toBe('done');
    expect(JSON.stringify(h.store.list(), (_k, v) => (typeof v === 'bigint' ? v.toString() : v))).toBe(before);
    expect(only(h)).toMatchObject({ status: 'held', nonce: p.nonce, transferId: p.transferId, icpPrincipal: TEST_OWNER_TEXT });
    expect(h.wallet.calls).toHaveLength(1);
    expect(h.node.executed).toHaveLength(0);
  });

  it('holds a wallet-changed AMOUNT, and a record for the changed operation under the same id is a deviation showing its actual amount', async () => {
    const { h, held } = await changedByWallet((p) => ({ amountE8s: p.amountE8s + 1n }));
    h.views.release = releaseFor(held, { amount_e8s: held.amountE8s + 1n, status: { Released: null } });
    const evidence = await reconcileReturn(h.deps, held);
    expect(evidence.release).toBeNull();
    expect(verdictOf(evidence)).toBe('deviation');
    expect(describeReturnProgress(evidence).detail).toContain('3.00000001 icUSD');
    await trackReturn(h.deps, held, () => {});
    expect(only(h).status).toBe('held');
  });

  it('never marks the journal done from a record that is not the reviewed attempt', async () => {
    const { h, held } = await changedByWallet((p) => ({ amountE8s: p.amountE8s + 1n }));
    h.views.release = releaseFor(held, { amount_e8s: held.amountE8s + 1n, status: { Released: null } });
    await trackReturn(h.deps, held, () => {});
    expect(only(h).status, 'a Released record for another amount must leave the original held').toBe('held');
  });

  it('still succeeds, credited and done, for the exact matching record (and the default and the explicit all-zero subaccount are the same account)', async () => {
    const h = harness({ wallet: sendOnly });
    const p = await plan(h);
    await startReturn(h.deps, p, h.emit);
    const sent = only(h);
    expect(sent.status).toBe('sent');
    h.views.release = releaseFor(sent, { status: { Pending: null }, destination: { owner: Principal.fromText(TEST_OWNER_TEXT), subaccount: [new Uint8Array(32)] } });
    const pending = await reconcileReturn(h.deps, sent);
    expect(pending.deviation).toBeNull();
    expect(verdictOf(pending)).toBe('credited');
    expect(describeReturnProgress(pending).label).toBe('Burn observed, release pending');
    h.views.release = releaseFor(sent, { status: { Released: null } });
    expect(await trackReturn(h.deps, sent, () => {})).toBe('done');
    expect(only(h).status).toBe('done');
  });

  it('compares a non-default subaccount exactly: another subaccount, sender or nonce is a deviation', async () => {
    const h = harness({ wallet: sendOnly });
    const p = await plan(h, { ...DRAFT, subaccountText: 'ab'.repeat(32) });
    await startReturn(h.deps, p, h.emit);
    const sent = only(h);
    expect(sent.icpSubaccount).toBe('ab'.repeat(32));
    h.views.release = releaseFor(sent);
    expect(verdictOf(await reconcileReturn(h.deps, sent))).toBe('credited');
    for (const [name, change] of [
      ['another subaccount', { destination: { owner: Principal.fromText(TEST_OWNER_TEXT), subaccount: [fill(0xcd)] } }],
      ['the default subaccount', { destination: { owner: Principal.fromText(TEST_OWNER_TEXT), subaccount: [] } }],
      ['another sender', { sui_sender: KEYS.otherHolder }],
      ['another nonce', { client_nonce: BigInt(sent.nonce) + 1n }],
      ['another transfer id', { return_id: 'bb'.repeat(32) }],
    ] as const) {
      h.views.release = releaseFor(sent, change as never);
      const evidence = await reconcileReturn(h.deps, sent);
      expect(verdictOf(evidence), name).toBe('deviation');
      expect(evidence.release, name).toBeNull();
    }
  });

  it('catches a saved-digest mismatch: the record names another burn transaction than the one this attempt saved', async () => {
    const h = harness();
    await startReturn(h.deps, await plan(h), h.emit);
    const sent = only(h);
    expect(sent.digest).toBeTruthy();
    h.views.release = releaseFor(sent, { tx_digest: base58Encode(fill(0x31)), status: { Released: null } });
    const evidence = await reconcileReturn(h.deps, sent);
    expect(verdictOf(evidence)).toBe('deviation');
    expect(evidence.deviation?.differences.join(' ')).toContain(base58Encode(fill(0x31)));
    expect(await trackReturn(h.deps, sent, () => {})).toBe('done');
    expect(only(h).status).toBe('sent');
    // With no saved digest (an ambiguous attempt) the digest is not compared, every other field still is.
    const g = harness({ wallet: sendOnly });
    g.wallet.onSignAndExecute = async () => {
      throw new Error('timeout');
    };
    await startReturn(g.deps, await plan(g), g.emit);
    const held = only(g);
    expect(held.digest).toBeUndefined();
    g.views.release = releaseFor(held, { tx_digest: base58Encode(fill(0x32)) });
    expect(verdictOf(await reconcileReturn(g.deps, held))).toBe('credited');
  });
});

describe('a saved attempt is bound to the EXACT bridge that made it (not only the destination slug)', () => {
  const OTHER_INSTANCE = 'ryjl3-tyaaa-aaaaa-aaaba-cai';
  const identities: Array<[string, Partial<SuiReturnIntent>]> = [
    ['another bridge canister', { instance: OTHER_INSTANCE }],
    ['another Gate', { gateId: KEYS.mintCap }],
    ['another package', { packageId: KEYS.gas, coinType: `${KEYS.gas}::rumi_icusd_gate::RUMI_ICUSD_GATE` }],
    ['another coin type', { coinType: `${KEYS.packageId}::other::COIN` }],
    ['another full chain identifier', { chainIdentifier: '4c78adacf2a2f5ad80f27ed7d54aa69d3a78f1ca67fdef9ecf5754f5b8bb77b0' }],
  ];

  async function signedAttempt() {
    const h = harness();
    h.node.failExecute = new Error('gateway timeout');
    await startReturn(h.deps, await plan(h), h.emit);
    h.node.failExecute = null;
    expect(only(h).status).toBe('signed');
    return { h, signed: only(h) };
  }

  it('refuses to re-ask the wallet for a PREPARED attempt of an earlier bridge, and keeps the record', async () => {
    for (const [name, change] of identities) {
      const h = harness();
      const p = await plan(h);
      const prepared: SuiReturnIntent = { v: 1, kind: 'sui_to_icp', id: '44'.repeat(16), destination: 'sui-mainnet', chainIdentifier: SUI_MAINNET.chainIdentifier, instance: SUI_BRIDGE_ID, packageId: p.config.packageId, gateId: p.config.gateId, coinType: COIN_TYPE, sender: p.sender, coins: p.coins, icpPrincipal: p.icpPrincipal, icpSubaccount: null, amountE8s: p.amountE8s, nonce: p.nonce, transferId: p.transferId, signMode: 'sign', createdAt: 5, status: 'prepared', ...change };
      h.store.put(prepared);
      await continuePrepared(h.deps, h.store.get(prepared.id) as SuiReturnIntent, h.emit);
      expect(last(h), name).toMatchObject({ type: 'blocked', message: expect.stringContaining('earlier Sui bridge') });
      expect(h.wallet.calls, name).toHaveLength(0);
      expect(h.store.get(prepared.id), name).toEqual({ ...prepared });
    }
  });

  it('refuses to rebroadcast a SIGNED attempt of an earlier bridge: nothing is sent, and the record is untouched', async () => {
    for (const [name, change] of identities) {
      const { h, signed } = await signedAttempt();
      const old: SuiReturnIntent = { ...signed, ...change };
      h.store.put(old);
      const executedBefore = h.node.callsTo('executeTransaction').length;
      await rebroadcastReturn(h.deps, old, h.emit);
      expect(last(h), name).toMatchObject({ type: 'blocked', message: expect.stringContaining('earlier Sui bridge') });
      expect(h.node.callsTo('executeTransaction').length, name).toBe(executedBefore);
      expect(h.wallet.calls, name).toHaveLength(1);
      expect(h.store.get(old.id), name).toEqual(old);
    }
  });

  it('names the original bridge, and never reads the new canister or the node to settle an old claim', async () => {
    const { h, signed } = await signedAttempt();
    const old: SuiReturnIntent = { ...signed, instance: OTHER_INSTANCE, gateId: KEYS.mintCap };
    h.store.put(old);
    const asked: string[] = [];
    h.deps.reads.getRelease = async (id) => (asked.push(id), releaseFor(old, { status: { Released: null } }));
    const statusCalls = h.node.callsTo('getTransactionStatus').length;
    const evidence = await reconcileReturn(h.deps, old);
    expect(asked).toEqual([]);
    expect(h.node.callsTo('getTransactionStatus')).toHaveLength(statusCalls);
    expect(evidence.chain).toMatchObject({ kind: 'wrong_instance', message: expect.stringContaining(OTHER_INSTANCE) });
    expect((evidence.chain as { message: string }).message).toContain(KEYS.mintCap);
    expect(evidence.release).toBeNull();
    expect(verdictOf(evidence)).toBe('unknown');
    expect(mayDiscard(old, verdictOf(evidence))).toBe(false);
    expect(describeReturnProgress(evidence).label).toBe('Belongs to an earlier bridge');
    expect(await trackReturn(h.deps, old, () => {})).toBe('done');
    expect(h.store.get(old.id)).toEqual(old);
  });

  it('refuses to sign a plan that was reviewed against another bridge than the one now in use', async () => {
    const h = harness();
    const p = await plan(h);
    h.deps.config = { ...h.deps.config, gateId: KEYS.mintCap };
    await startReturn(h.deps, p, h.emit);
    expect(last(h)).toMatchObject({ type: 'blocked', message: expect.stringContaining('bridge changed') });
    expect(h.wallet.calls).toHaveLength(0);
    expect(h.store.list()).toHaveLength(0);
  });

  it('still continues the same operation when the identity is unchanged', async () => {
    const { h, signed } = await signedAttempt();
    await rebroadcastReturn(h.deps, signed, h.emit);
    expect(last(h)).toMatchObject({ type: 'sent' });
    expect(h.node.executed).toHaveLength(1);
    expect(toBase64(h.node.executed[0].bytes)).toBe(signed.signedBytes);
    expect(only(h)).toMatchObject({ id: signed.id, nonce: signed.nonce, transferId: signed.transferId });
  });
});

describe('telling the bridge', () => {
  it('is an explicit ICP signer click, refused for a disabled destination or a changed session, and never made by the flow itself', async () => {
    const h = harness();
    await startReturn(h.deps, await plan(h), h.emit);
    landOn(h.node, fromBase64(only(h).signedBytes!));
    await trackReturn(h.deps, only(h), () => {}, AbortSignal.abort());
    await reconcileReturn(h.deps, only(h));
    // Neither signing, tracking nor reading ever asked the ICP signer to notify the bridge.
    expect(h.notify).not.toHaveBeenCalled();
    expect(await notifyReturn(h.deps, 'Digest')).toEqual({ ok: true });
    expect(h.notify).toHaveBeenCalledWith('Digest');
    h.signing.fresh.value = { status: 'disabled', reason: 'off' };
    expect(await notifyReturn(h.deps, 'Digest')).toMatchObject({ ok: false });
    expect(h.notify).toHaveBeenCalledTimes(1);
    h.signing.fresh.value = { status: 'enabled', reason: null };
    h.session.current = false;
    expect(await notifyReturn(h.deps, 'Digest')).toMatchObject({ ok: false });
    expect(await notifyReturn({ ...h.deps, bridge: undefined }, 'Digest')).toMatchObject({ ok: false });
  });
});

describe('what a saved attempt is checked against', () => {
  it('is derived from the saved intent alone, so a reload checks the same thing', async () => {
    const h = harness();
    const p = await plan(h);
    await startReturn(h.deps, p, h.emit);
    const expected = expectationOf(h.deps.config, only(h));
    expect(expected).toMatchObject({ sender: KEYS.holder, packageId: p.config.packageId, gateId: p.config.gateId, gateInitialSharedVersion: '42', amountE8s: p.amountE8s, coinTotalE8s: 5_000_000_000n });
    expect(expected.nonce).toBe(BigInt(p.nonce));
    expect(toHex(expected.subaccount)).toBe('00'.repeat(32));
  });
});
function pick(signed: { bytes: string; signature: string }) {
  return { bytes: signed.bytes, signature: signed.signature };
}
