import { describe, expect, it, vi } from 'vitest';
import { NOT_SIGNED_NOTE } from '../baseBridge/signingGuard';
import { createMemoryStorage } from '../baseBridge/testSupport';
import { base58Encode, fromHex, pubkeyFromBase58, toHex, type Bytes } from './bytes';
import { returnId } from './ids';
import { SolanaIntentStore, type SolReturnIntent } from './intents';
import { tokenProgramBytes } from './programs';
import {
  INITIAL_SOL_RETURN_STATE,
  continuePrepared,
  describeReturnProgress,
  discardReturn,
  evaluateReturnDraft,
  mayDiscard,
  notifyReturn,
  prepareReturn,
  rebroadcastReturn,
  reconcileReturn,
  reduceSolReturn,
  startReturn,
  trackReturn,
  trackedReturns,
  unresolvedReturns,
  verdictOf,
  type ReturnDeps,
  type ReturnPlan,
  type SolReturnEvent,
  type SolReturnState,
} from './solanaToIcp';
import { buildReturnMessage } from './transaction';
import {
  KEYS,
  TEST_OWNER_TEXT,
  createFakeChain,
  landOn,
  makeFakeWallet,
  makeSolRelease,
  makeSolReturn,
  makeSolSigning,
  makeSolanaConfig,
  rejection,
  signWith,
  type FakeChain,
  type FakeWallet,
} from './testSupport';
import { SOLANA_MAINNET } from './networks';
import type { ReleaseView, ReturnView } from '$declarations/rumi_icusd_sol_bridge/rumi_icusd_sol_bridge.did';

const fill = (byte: number, n = 32): Bytes => new Uint8Array(n).fill(byte);

interface Harness {
  chain: FakeChain;
  wallet: FakeWallet;
  storage: ReturnType<typeof createMemoryStorage>;
  store: SolanaIntentStore;
  signing: ReturnType<typeof makeSolSigning>;
  deps: ReturnDeps;
  events: SolReturnEvent[];
  emit: (event: SolReturnEvent) => void;
  views: { ret: ReturnView | null; release: ReleaseView | null };
  session: { current: boolean; wallet: FakeWallet | null };
  notify: ReturnType<typeof vi.fn>;
}

function harness(options: { tokenProgram?: 'classic' | 'token2022'; wallet?: Parameters<typeof makeFakeWallet>[0]; storage?: ReturnType<typeof createMemoryStorage> } = {}): Harness {
  const config = makeSolanaConfig(options.tokenProgram ?? 'token2022');
  const chain = createFakeChain();
  const wallet = makeFakeWallet(options.wallet);
  const storage = options.storage ?? createMemoryStorage();
  const store = new SolanaIntentStore(storage, TEST_OWNER_TEXT);
  const signing = makeSolSigning();
  const views = { ret: null as ReturnView | null, release: null as ReleaseView | null };
  const session = { current: true, wallet: wallet as FakeWallet | null };
  const notify = vi.fn(async () => ({ Ok: null }) as never);
  let seed = 0;
  const deps: ReturnDeps = {
    config,
    signing,
    store,
    rpc: chain,
    wallet: () => session.wallet,
    reads: { getReturn: async () => views.ret, getRelease: async () => views.release },
    bridge: { notifySolanaReturn: notify },
    isCurrent: () => session.current,
    random: (n) => Uint8Array.from({ length: n }, (_, i) => (seed++ * 7 + i + 1) & 0xff),
    now: () => 1_700_000_000_000,
    sleep: async () => {},
  };
  const events: SolReturnEvent[] = [];
  return { chain, wallet, storage, store, signing, deps, events, emit: (e) => events.push(e), views, session, notify };
}

const DRAFT = { amountText: '3', principalText: TEST_OWNER_TEXT, subaccountText: '', sourceAccount: KEYS.holderSource };

async function plan(h: Harness, input = DRAFT): Promise<ReturnPlan> {
  const result = await prepareReturn(h.deps, input);
  if (!result.ok) throw new Error(result.message);
  return result.plan;
}

const last = (h: Harness) => h.events[h.events.length - 1];
const only = (h: Harness) => h.store.list()[0] as SolReturnIntent;

describe('the draft', () => {
  const config = makeSolanaConfig();
  const accounts = [{ address: KEYS.holderSource, amountE8s: 500_000_000n }];

  it('accepts a good return and explains the first problem otherwise', () => {
    expect(evaluateReturnDraft(DRAFT, config, accounts)).toMatchObject({ ok: true, amountE8s: 300_000_000n, problem: null });
    const problem = (patch: Partial<typeof DRAFT>, list = accounts) => evaluateReturnDraft({ ...DRAFT, ...patch }, config, list).problem;
    expect(problem({ amountText: '' })).toMatch(/amount/i);
    expect(problem({ amountText: '1.123456789' })).toMatch(/decimal/i);
    expect(problem({ amountText: '10.00000001' })).toBe('The bridge credits at most 10 icUSD in one return.');
    expect(problem({ amountText: '0.0002' })).toBe('The amount must be larger than the release fee of 0.0002 icUSD.');
    expect(problem({ principalText: '' })).toMatch(/principal/i);
    expect(problem({ principalText: '2vxsx-fae' })).toMatch(/anonymous/i);
    expect(problem({ principalText: config.instanceCanisterId })).toMatch(/bridge canister itself/i);
    expect(problem({ subaccountText: 'AB' })).toMatch(/subaccount/i);
    expect(problem({ sourceAccount: '' })).toMatch(/Choose the Solana token account/);
    expect(problem({ sourceAccount: KEYS.otherHolder })).toMatch(/does not belong/);
    expect(problem({ amountText: '6' })).toMatch(/holds 5 icUSD, less than 6/);
  });

  it('blocks on a halt and only warns when releases are paused', () => {
    expect(evaluateReturnDraft(DRAFT, { ...config, halted: 'stop' }, accounts)).toMatchObject({ ok: false, problem: 'The bridge is halted for safety: stop' });
    const paused = evaluateReturnDraft(DRAFT, { ...config, releasesPaused: true }, accounts);
    expect(paused.ok).toBe(true);
    expect(paused.warning).toMatch(/Releases to the Internet Computer are paused/);
  });
});

describe('preparing the transaction', () => {
  it('builds exactly the backend reference return for Token-2022 and for the classic program', async () => {
    for (const tokenProgram of ['token2022', 'classic'] as const) {
      const h = harness({ tokenProgram });
      const p = await plan(h);
      const reference = buildReturnMessage({
        tokenProgram,
        mint: fill(0x44),
        returnOwner: fill(0x33),
        roleKeys: [fill(0x11), fill(0x22)],
        holder: pubkeyFromBase58(KEYS.holder)!,
        sourceTokenAccount: pubkeyFromBase58(KEYS.holderSource)!,
        amount: 300_000_000n,
        memo: p.memo,
        recentBlockhash: fill(0x77),
      });
      expect(toHex(p.message)).toBe(toHex(reference.message));
      expect(p.returnAccount).toBe(base58Encode(reference.returnAta));
      expect(p.returnAccount).toBe(h.deps.config.returnAccount);
      // The token program is the one the canister reports: its id is in the message accounts.
      expect(toHex(p.message)).toContain(toHex(tokenProgramBytes(tokenProgram)));
      const other = tokenProgram === 'token2022' ? 'classic' : 'token2022';
      expect(toHex(p.message)).not.toContain(toHex(tokenProgramBytes(other)));
      expect(p.memo).toBe(`rumi1|r|${TEST_OWNER_TEXT}|-|${p.nonce}`);
      expect(p).toMatchObject({ holder: KEYS.holder, sourceAccount: KEYS.holderSource, amountE8s: 300_000_000n, icpPrincipal: TEST_OWNER_TEXT, icpSubaccount: null });
      // An ordinary recent blockhash, from the node.
      expect(h.chain.callsTo('getLatestBlockhash')).toHaveLength(1);
      expect(toHex(p.message)).toContain(toHex(fill(0x77)));
      // Preparing signs, sends and saves nothing.
      expect(h.wallet.calls).toHaveLength(0);
      expect(h.chain.sent).toHaveLength(0);
      expect(h.store.list()).toHaveLength(0);
    }
  });

  it('writes the destination subaccount and a 64-bit nonce into the memo', async () => {
    const h = harness();
    const sub = `${'00'.repeat(31)}01`;
    const p = await plan(h, { ...DRAFT, subaccountText: sub });
    expect(p.memo).toBe(`rumi1|r|${TEST_OWNER_TEXT}|${sub}|${p.nonce}`);
    expect(BigInt(p.nonce) <= 18_446_744_073_709_551_615n).toBe(true);
  });

  it('refuses before reading anything when the wallet is missing, on another network, or the draft is wrong', async () => {
    const h = harness();
    h.session.wallet = null;
    expect(await prepareReturn(h.deps, DRAFT)).toEqual({ ok: false, message: 'Connect your Solana wallet first.' });
    h.session.wallet = makeFakeWallet({ chains: ['solana:devnet'] });
    expect(await prepareReturn(h.deps, DRAFT)).toMatchObject({ ok: false, message: expect.stringContaining('not on Solana') });
    h.session.wallet = h.wallet;
    expect(await prepareReturn(h.deps, { ...DRAFT, amountText: '0' })).toMatchObject({ ok: false });
    expect(h.chain.calls).toHaveLength(0);
  });

  it('reports a node failure plainly and checks the source account against the chain', async () => {
    const h = harness();
    h.chain.failReads = new Error('node down');
    expect(await prepareReturn(h.deps, DRAFT)).toMatchObject({ ok: false, message: expect.stringContaining('node down') });
    h.chain.failReads = null;
    h.chain.tokenAccounts = [{ address: KEYS.holderSource, amountE8s: 100_000_000n }];
    expect(await prepareReturn(h.deps, DRAFT)).toMatchObject({ ok: false, message: expect.stringContaining('holds 1 icUSD') });
  });
});

describe('signing with a wallet that returns signed bytes', () => {
  it('saves the attempt before the wallet is asked, the signed bytes and signature before anything is broadcast, and sends once', async () => {
    const h = harness();
    const p = await plan(h);
    const seen: string[] = [];
    h.wallet.onSign = async (tx, chain) => {
      // At the moment of the wallet call the attempt is already on disk, in the `signing` state, with its exact message.
      const saved = only(h);
      seen.push(`wallet: ${saved.status} ${saved.messageHex === toHex(p.message)} ${chain}`);
      return signWith(fill(0xe5, 64))(tx);
    };
    const originalSend = h.chain.sendTransaction.bind(h.chain);
    h.chain.sendTransaction = async (wire) => {
      const saved = only(h);
      seen.push(`send: ${saved.status} ${saved.signedHex === toHex(wire)} ${saved.signature === base58Encode(wire.slice(1, 65))}`);
      return originalSend(wire);
    };
    await startReturn(h.deps, p, h.emit);
    expect(seen).toEqual(['wallet: signing true solana:mainnet', 'send: signed true true']);
    expect(h.wallet.calls).toHaveLength(1);
    expect(h.wallet.calls[0].method).toBe('signTransaction');
    expect(h.chain.sent).toHaveLength(1);
    const saved = only(h);
    expect(saved).toMatchObject({ status: 'sent', signature: base58Encode(fill(0xe5, 64)), destination: 'solana-mainnet', genesisHash: SOLANA_MAINNET.genesisHash, mint: KEYS.mint, tokenProgram: 'token2022', wallet: KEYS.holder, sourceAccount: KEYS.holderSource, icpPrincipal: TEST_OWNER_TEXT, nonce: p.nonce, blockhash: p.blockhash.blockhash, lastValidBlockHeight: '1000', signMode: 'sign' });
    expect(h.events.map((e) => e.type)).toEqual(['signing', 'signed', 'sent']);
    // The wallet is asked for the unsigned wire: one empty signature slot, then our message.
    expect(h.wallet.calls[0].transaction.length).toBe(1 + 64 + p.message.length);
  });

  it('does not broadcast, and forgets the attempt, when the user declines', async () => {
    const h = harness();
    h.wallet.onSign = async () => {
      throw rejection();
    };
    await startReturn(h.deps, await plan(h), h.emit);
    expect(last(h)).toEqual({ type: 'declined' });
    expect(h.chain.sent).toHaveLength(0);
    expect(h.store.list()).toHaveLength(0);
  });

  it('does not broadcast, and forgets the attempt, when the wallet fails without signing', async () => {
    const h = harness();
    h.wallet.onSign = async () => {
      throw new Error('Unexpected wallet crash');
    };
    await startReturn(h.deps, await plan(h), h.emit);
    expect(last(h)).toMatchObject({ type: 'failed', message: expect.stringContaining('Nothing was sent') });
    expect(h.chain.sent).toHaveLength(0);
    expect(h.store.list()).toHaveLength(0);
  });

  it('never broadcasts a transaction the wallet changed, because the bridge would read it as a stray', async () => {
    const h = harness();
    h.wallet.onSign = async (tx) => Uint8Array.from([...(await signWith(fill(0xe5, 64))(tx)), 0xff]);
    await startReturn(h.deps, await plan(h), h.emit);
    expect(last(h)).toMatchObject({ type: 'failed', message: expect.stringContaining('changed the transaction') });
    expect(h.chain.sent).toHaveLength(0);
    expect(h.store.list()).toHaveLength(0);
    // And a wallet that returns it unsigned.
    const g = harness();
    g.wallet.onSign = async (tx) => tx;
    await startReturn(g.deps, await plan(g), g.emit);
    expect(last(g)).toMatchObject({ type: 'failed' });
    expect(g.chain.sent).toHaveLength(0);
  });

  it('keeps the signed bytes when the broadcast is uncertain and sends the SAME bytes again, never a new transaction', async () => {
    const h = harness();
    const p = await plan(h);
    h.chain.failSend = new Error('timeout');
    await startReturn(h.deps, p, h.emit);
    expect(last(h)).toMatchObject({ type: 'broadcast_uncertain' });
    const saved = only(h);
    expect(saved.status).toBe('signed');
    expect(saved.signedHex).toBeTruthy();
    expect(h.wallet.calls).toHaveLength(1);
    // The node recovers; the click sends the identical saved bytes.
    h.chain.failSend = null;
    await rebroadcastReturn(h.deps, saved, h.emit);
    expect(h.chain.sent).toHaveLength(1);
    expect(toHex(h.chain.sent[0])).toBe(saved.signedHex);
    expect(only(h).status).toBe('sent');
    expect(h.wallet.calls).toHaveLength(1);
    expect(h.chain.callsTo('getLatestBlockhash')).toHaveLength(1);
  });

  it('refuses to send again when there are no saved signed bytes or the destination is off', async () => {
    const h = harness();
    await startReturn(h.deps, await plan(h), h.emit);
    const sent = only(h);
    await rebroadcastReturn(h.deps, sent, h.emit);
    expect(last(h)).toMatchObject({ type: 'blocked', message: 'There is no saved signed transaction to send again.' });
    h.chain.failSend = new Error('x');
    const g = harness();
    g.chain.failSend = new Error('x');
    await startReturn(g.deps, await plan(g), g.emit);
    g.chain.failSend = null;
    g.signing.fresh.value = { status: 'disabled', reason: 'Switched off.' };
    await rebroadcastReturn(g.deps, only(g), g.emit);
    expect(last(g)).toMatchObject({ type: 'blocked', message: `Switched off. ${NOT_SIGNED_NOTE}` });
    expect(g.chain.sent).toHaveLength(0);
  });
});

describe('signing with a wallet that signs and sends', () => {
  const sendOnly = { signs: false, sendsAndSigns: true };

  it('saves the signature the wallet reports and tracks it, without treating it as proof', async () => {
    const h = harness({ wallet: sendOnly });
    await startReturn(h.deps, await plan(h), h.emit);
    expect(h.wallet.calls.map((c) => c.method)).toEqual(['signAndSendTransaction']);
    expect(only(h)).toMatchObject({ status: 'sent', signMode: 'sign_and_send', signature: base58Encode(fill(0xe5, 64)) });
    expect(h.chain.sent).toHaveLength(0);
  });

  it('declines cleanly on a rejection', async () => {
    const h = harness({ wallet: sendOnly });
    h.wallet.onSignAndSend = async () => {
      throw rejection();
    };
    await startReturn(h.deps, await plan(h), h.emit);
    expect(last(h)).toEqual({ type: 'declined' });
    expect(h.store.list()).toHaveLength(0);
  });

  it('keeps the original when the wallet broadcasts and then throws a transport error that only sounds like a refusal', async () => {
    for (const text of ['connection closed', 'Transaction cancelled by the relay', 'request denied: HTTP 429 from the node', 'popup closed unexpectedly', 'WebSocket was closed before the confirmation arrived']) {
      const h = harness({ wallet: sendOnly });
      const p = await plan(h);
      h.wallet.onSignAndSend = async () => {
        // The wallet really broadcast the transaction, then lost its connection.
        landOn(h.chain, Uint8Array.from([1, ...fill(0xe5, 64), ...p.message]));
        throw new Error(text);
      };
      await startReturn(h.deps, p, h.emit);
      expect(last(h), text).toMatchObject({ type: 'held' });
      const held = only(h);
      expect(held, text).toMatchObject({ status: 'held', signMode: 'sign_and_send', messageHex: toHex(p.message), nonce: p.nonce, blockhash: p.blockhash.blockhash });
      // The record is durable: a fresh store over the same storage sees the same original.
      expect(new SolanaIntentStore(h.storage, TEST_OWNER_TEXT).list()[0], text).toMatchObject({ id: held.id, status: 'held', messageHex: toHex(p.message) });
      // Zero fresh signing, nothing rebuilt.
      expect(h.wallet.calls, text).toHaveLength(1);
      expect(h.chain.callsTo('getLatestBlockhash'), text).toHaveLength(1);
      expect(h.chain.sent, text).toHaveLength(0);
    }
  });

  it('still clears cleanly on an explicit user rejection, whatever its message says', async () => {
    for (const message of ['User rejected the request.', 'closed', 'Transaction cancelled']) {
      const h = harness({ wallet: sendOnly });
      h.wallet.onSignAndSend = async () => {
        throw rejection(message);
      };
      await startReturn(h.deps, await plan(h), h.emit);
      expect(last(h), message).toEqual({ type: 'declined' });
      expect(h.store.list(), message).toHaveLength(0);
    }
  });

  it('HOLDS the original intent on an ambiguous answer and never rebuilds or signs again', async () => {
    for (const ambiguous of [
      async () => {
        throw new Error('RPC timeout while confirming');
      },
      async () => new Uint8Array(10),
      async () => new Uint8Array(64),
    ]) {
      const h = harness({ wallet: sendOnly });
      const p = await plan(h);
      h.wallet.onSignAndSend = ambiguous;
      await startReturn(h.deps, p, h.emit);
      expect(last(h)).toMatchObject({ type: 'held' });
      const held = only(h);
      expect(held).toMatchObject({ status: 'held', messageHex: toHex(p.message), blockhash: p.blockhash.blockhash, nonce: p.nonce });
      expect(held.signature).toBeUndefined();
      // Nothing was rebuilt (one blockhash read, at preparation) and the wallet was asked exactly once.
      expect(h.chain.callsTo('getLatestBlockhash')).toHaveLength(1);
      expect(h.wallet.calls).toHaveLength(1);
      // A new transfer cannot start while it is held, and does not touch the wallet.
      const next = await plan(h);
      await startReturn(h.deps, next, h.emit);
      expect(last(h)).toMatchObject({ type: 'blocked', message: expect.stringContaining('no recorded outcome') });
      expect(h.wallet.calls).toHaveLength(1);
      expect(h.store.list()).toHaveLength(1);
    }
  });
});

describe('the wallet account and the network', () => {
  it('asks nothing when the account changed after the review, or the wallet left the network', async () => {
    const h = harness();
    const p = await plan(h);
    h.wallet.state.address = KEYS.otherHolder;
    await startReturn(h.deps, p, h.emit);
    expect(last(h)).toMatchObject({ type: 'blocked', message: expect.stringContaining('account changed') });
    h.wallet.state.address = KEYS.holder;
    h.wallet.state.chains = ['solana:devnet'];
    await startReturn(h.deps, p, h.emit);
    expect(last(h)).toMatchObject({ type: 'blocked', message: expect.stringContaining('not on Solana') });
    h.session.wallet = null;
    await startReturn(h.deps, p, h.emit);
    expect(last(h)).toMatchObject({ type: 'blocked', message: expect.stringContaining('not connected') });
    expect(h.wallet.calls).toHaveLength(0);
    expect(h.store.list()).toHaveLength(0);
    expect(h.chain.sent).toHaveLength(0);
  });

  it('keeps an old attempt exactly as it was when the account or network changes, and will not continue it elsewhere', async () => {
    const h = harness();
    const p = await plan(h);
    // Save an attempt the wallet was never asked about.
    h.wallet.onSign = async () => {
      throw new Error('never reached');
    };
    h.store.put({ v: 1, kind: 'solana_to_icp', id: '11'.repeat(16), destination: 'solana-mainnet', genesisHash: SOLANA_MAINNET.genesisHash, instance: h.deps.config.instanceCanisterId, mint: KEYS.mint, tokenProgram: 'token2022', returnAccount: p.returnAccount, wallet: KEYS.holder, sourceAccount: KEYS.holderSource, icpPrincipal: TEST_OWNER_TEXT, icpSubaccount: null, amountE8s: p.amountE8s, nonce: p.nonce, memo: p.memo, blockhash: p.blockhash.blockhash, lastValidBlockHeight: '1000', messageHex: toHex(p.message), signMode: 'sign', createdAt: 5, status: 'prepared' });
    const before = JSON.stringify(h.storage.map.get(h.store.key));
    h.wallet.state.address = KEYS.otherHolder;
    await continuePrepared(h.deps, h.store.get('11'.repeat(16)) as SolReturnIntent, h.emit);
    expect(last(h)).toMatchObject({ type: 'blocked', message: expect.stringContaining('Connect the wallet account this attempt was prepared for') });
    expect(h.wallet.calls).toHaveLength(0);
    expect(JSON.stringify(h.storage.map.get(h.store.key))).toBe(before);
  });
});

describe('a destination that is not enabled', () => {
  it('asks no wallet, saves nothing and sends nothing, in every state of the page', async () => {
    for (const fresh of [{ status: 'disabled' as const, reason: 'Not available yet: there is no icUSD bridge to Solana at the moment.' }, null]) {
      const h = harness();
      const p = await plan(h);
      h.signing.fresh.value = fresh;
      await startReturn(h.deps, p, h.emit);
      expect(last(h)).toMatchObject({ type: 'blocked', message: expect.stringContaining(NOT_SIGNED_NOTE) });
      expect(h.wallet.calls).toHaveLength(0);
      expect(h.chain.sent).toHaveLength(0);
      expect(h.store.list()).toHaveLength(0);
    }
    // A destination that was disabled from the start (the usual state today).
    const d = harness();
    const q = await plan(d);
    d.signing.destination.status = 'disabled' as never;
    (d.signing as { destination: { status: string } }).destination = { ...d.signing.destination, status: 'disabled' };
    await startReturn(d.deps, q, d.emit);
    expect(last(d)).toMatchObject({ type: 'blocked' });
    expect(d.wallet.calls).toHaveLength(0);
  });

  it('refuses a saved attempt that belongs to another network without touching it', async () => {
    const h = harness();
    await startReturn(h.deps, await plan(h), h.emit);
    h.chain.failSend = new Error('x');
    const g = harness();
    g.chain.failSend = new Error('x');
    await startReturn(g.deps, await plan(g), g.emit);
    const stray = { ...only(g), destination: 'solana-devnet' };
    await rebroadcastReturn(g.deps, stray, g.emit);
    expect(last(g)).toMatchObject({ type: 'blocked', message: expect.stringContaining('another destination') });
    expect(g.chain.sent).toHaveLength(0);
  });

  it('stops when the ICP wallet session changed', async () => {
    const h = harness();
    const p = await plan(h);
    h.session.current = false;
    await startReturn(h.deps, p, h.emit);
    expect(last(h)).toEqual({ type: 'session_changed' });
    expect(h.wallet.calls).toHaveLength(0);
  });
});

describe('storage that cannot keep the attempt', () => {
  it('asks no wallet when the attempt cannot be saved first', async () => {
    const h = harness();
    const p = await plan(h);
    const broken = { getItem: () => null, setItem: () => { throw new Error('quota'); }, removeItem: () => {} };
    (h.deps as { store: SolanaIntentStore }).store = new SolanaIntentStore(broken as never, TEST_OWNER_TEXT);
    await startReturn(h.deps, p, h.emit);
    expect(last(h)).toMatchObject({ type: 'blocked', message: expect.stringContaining('nothing was sent') });
    expect(h.wallet.calls).toHaveLength(0);
  });
});

describe('reload without resubmission', () => {
  function reopened(h: Harness): Harness {
    const next = harness({ storage: h.storage });
    next.chain = h.chain;
    next.deps.rpc = h.chain;
    return next;
  }

  it('shows every kind of unfinished attempt as saved, and signs and sends nothing', async () => {
    const h = harness({ wallet: { signs: false, sendsAndSigns: true } });
    h.wallet.onSignAndSend = async () => {
      throw new Error('timeout');
    };
    await startReturn(h.deps, await plan(h), h.emit);
    const g = reopened(h);
    expect(g.store.list()).toHaveLength(1);
    expect(unresolvedReturns(g.store).map((i) => i.status)).toEqual(['held']);
    const state = reduceSolReturn(INITIAL_SOL_RETURN_STATE, { type: 'resume', intent: g.store.list()[0] as SolReturnIntent });
    expect(state.kind).toBe('held');
    expect(g.wallet.calls).toHaveLength(0);
    expect(g.chain.sent).toHaveLength(0);
    expect(g.chain.callsTo('getLatestBlockhash')).toHaveLength(1);
    // Each status maps to its own state.
    const at = (status: SolReturnIntent['status'], extra: Partial<SolReturnIntent> = {}): SolReturnState =>
      reduceSolReturn(INITIAL_SOL_RETURN_STATE, { type: 'resume', intent: { ...(g.store.list()[0] as SolReturnIntent), status, ...extra } });
    expect(at('prepared').kind).toBe('prepared');
    expect(at('signing').kind).toBe('held');
    expect(at('signed', { signedHex: 'aa' }).kind).toBe('broadcast_uncertain');
    expect(at('sent').kind).toBe('tracking');
    expect(trackedReturns(g.store, 'solana-mainnet')).toEqual([]);
  });

  it('finds an ambiguous attempt by the EXACT saved message, and the bridge record through its return id', async () => {
    const h = harness({ wallet: { signs: false, sendsAndSigns: true } });
    h.wallet.onSignAndSend = async () => {
      throw new Error('timeout');
    };
    const p = await plan(h);
    await startReturn(h.deps, p, h.emit);
    const held = only(h);
    // Nothing on chain yet, the blockhash can still land: unknown, and the original may NOT be discarded.
    let evidence = await reconcileReturn(h.deps, held);
    expect(evidence).toMatchObject({ signature: null, chain: { kind: 'not_found', blockhashExpired: false } });
    expect(verdictOf(evidence)).toBe('unknown');
    expect(discardReturn(h.deps, held, verdictOf(evidence))).toMatchObject({ ok: false });
    expect(h.store.list()).toHaveLength(1);
    // A look-alike (same shape, other blockhash) is not ours.
    const other = buildReturnMessage({ tokenProgram: 'token2022', mint: fill(0x44), returnOwner: fill(0x33), roleKeys: [fill(0x11), fill(0x22)], holder: pubkeyFromBase58(KEYS.holder)!, sourceTokenAccount: pubkeyFromBase58(KEYS.holderSource)!, amount: 300_000_000n, memo: p.memo, recentBlockhash: fill(0x78) });
    landOn(h.chain, Uint8Array.from([1, ...fill(0xd1, 64), ...other.message]));
    evidence = await reconcileReturn(h.deps, held);
    expect(evidence.signature).toBeNull();
    // Ours, landed and finalized: recovered by its exact message.
    const signature = landOn(h.chain, Uint8Array.from([1, ...fill(0xe7, 64), ...p.message]));
    evidence = await reconcileReturn(h.deps, held);
    expect(evidence).toMatchObject({ signature, recovered: true, chain: { kind: 'finalized' }, ret: null });
    expect(verdictOf(evidence)).toBe('landed');
    expect(mayDiscard(held, 'landed')).toBe(false);
    // The bridge's record is found through the return id derived from the genesis hash and the signature.
    const id = returnId(pubkeyFromBase58(held.genesisHash)!, fromHex(toHex(pubkeyFromBase58(signature) ?? new Uint8Array()))! && Uint8Array.from(fill(0xe7, 64)));
    const asked: string[] = [];
    h.deps.reads.getReturn = async (rid) => (asked.push(rid), makeSolReturn({ return_id: rid }));
    evidence = await reconcileReturn(h.deps, held);
    expect(asked).toEqual([id]);
    expect(verdictOf(evidence)).toBe('credited');
  });

  it('allows discarding only a never-asked attempt or a FINALIZED failure; absence and expiry prove nothing', async () => {
    const h = harness({ wallet: { signs: false, sendsAndSigns: true } });
    h.wallet.onSignAndSend = async () => {
      throw new Error('timeout');
    };
    await startReturn(h.deps, await plan(h), h.emit);
    const held = only(h);
    // The blockhash expired and the node knows nothing of it. That is NOT proof that it never executed.
    h.chain.height = 1_001n;
    const expired = await reconcileReturn(h.deps, held);
    expect(expired.chain).toEqual({ kind: 'not_found', blockhashExpired: true });
    expect(verdictOf(expired)).toBe('unknown');
    expect(discardReturn(h.deps, held, verdictOf(expired))).toMatchObject({ ok: false });
    expect(h.store.list()).toHaveLength(1);

    // A saved signature whose failure is FINALIZED moved no icUSD.
    const g = harness();
    await startReturn(g.deps, await plan(g), g.emit);
    const sent = only(g);
    const sentSnapshot = { ...sent };
    g.chain.statuses.set(sent.signature!, { confirmation: 'finalized', failed: true, slot: 9n });
    const failed = await reconcileReturn(g.deps, sent);
    expect(verdictOf(failed)).toBe('failed_on_chain');
    expect(discardReturn(g.deps, sent, 'failed_on_chain')).toEqual({ ok: true });

    // An attempt the wallet was never asked about can always go; one whose evidence is unreadable cannot.
    const k = harness();
    k.store.put({ ...sentSnapshot, id: '22'.repeat(16) });
    const p = await plan(k);
    k.store.put({ v: 1, kind: 'solana_to_icp', id: '33'.repeat(16), destination: 'solana-mainnet', genesisHash: SOLANA_MAINNET.genesisHash, instance: 'x', mint: KEYS.mint, tokenProgram: 'token2022', returnAccount: p.returnAccount, wallet: KEYS.holder, sourceAccount: KEYS.holderSource, icpPrincipal: TEST_OWNER_TEXT, icpSubaccount: null, amountE8s: 1n, nonce: '1', memo: p.memo, blockhash: 'BH', lastValidBlockHeight: '1000', messageHex: toHex(p.message), signMode: 'sign', createdAt: 5, status: 'prepared' });
    expect(discardReturn(k.deps, k.store.get('33'.repeat(16)) as SolReturnIntent, null)).toEqual({ ok: true });
    k.chain.failReads = new Error('down');
    const unreadable = await reconcileReturn(k.deps, { ...(k.store.get('22'.repeat(16)) as SolReturnIntent), status: 'held', signature: undefined });
    expect(unreadable.chain).toMatchObject({ kind: 'unavailable' });
    expect(verdictOf(unreadable)).toBe('unknown');
  });

  it('re-asks the wallet for a never-asked attempt only with the saved message, on the same account', async () => {
    const h = harness();
    const p = await plan(h);
    h.store.put({ v: 1, kind: 'solana_to_icp', id: '44'.repeat(16), destination: 'solana-mainnet', genesisHash: SOLANA_MAINNET.genesisHash, instance: h.deps.config.instanceCanisterId, mint: KEYS.mint, tokenProgram: 'token2022', returnAccount: p.returnAccount, wallet: KEYS.holder, sourceAccount: KEYS.holderSource, icpPrincipal: TEST_OWNER_TEXT, icpSubaccount: null, amountE8s: p.amountE8s, nonce: p.nonce, memo: p.memo, blockhash: p.blockhash.blockhash, lastValidBlockHeight: '1000', messageHex: toHex(p.message), signMode: 'sign', createdAt: 5, status: 'prepared' });
    await continuePrepared(h.deps, h.store.get('44'.repeat(16)) as SolReturnIntent, h.emit);
    expect(h.wallet.calls).toHaveLength(1);
    expect(toHex(h.wallet.calls[0].transaction.slice(65))).toBe(toHex(p.message));
    expect(h.chain.callsTo('getLatestBlockhash')).toHaveLength(1);
    expect(h.chain.sent).toHaveLength(1);
    // A second attempt to continue the same (now sent) record is refused.
    await continuePrepared(h.deps, h.store.get('44'.repeat(16)) as SolReturnIntent, h.emit);
    expect(last(h)).toMatchObject({ type: 'blocked', message: expect.stringContaining('never asked') });
    expect(h.wallet.calls).toHaveLength(1);
  });
});

describe('status: pending, then burn, then final release', () => {
  it('follows the chain and the bridge record without calling anything settled early', async () => {
    const h = harness();
    await startReturn(h.deps, await plan(h), h.emit);
    const sent = only(h);
    const seen: string[] = [];
    const script: Array<() => void> = [
      () => h.chain.statuses.set(sent.signature!, { confirmation: 'confirmed', failed: false, slot: 5n }),
      () => h.chain.statuses.set(sent.signature!, { confirmation: 'finalized', failed: false, slot: 5n }),
      () => (h.views.ret = makeSolReturn({ status: { Custodied: null } })),
      () => (h.views.ret = makeSolReturn({ status: { Burning: { burn_op_id: 1n } } })),
      () => (h.views.release = makeSolRelease({ status: { Pending: null } })),
      () => (h.views.release = makeSolRelease({ status: { Released: null }, release_block_index: [88n], amount_to_receive_e8s: [299_000_000n] })),
    ];
    let step = 0;
    h.deps.sleep = async () => {
      script[Math.min(step++, script.length - 1)]();
    };
    const outcome = await trackReturn(h.deps, sent, (e) => seen.push(describeReturnProgress(e).label));
    expect(outcome).toBe('done');
    expect(seen).toEqual([
      'Not found yet',
      'Waiting for finality',
      'Finalized on Solana',
      'Credited by the bridge',
      'Burning your returned icUSD',
      'Release pending',
      'Released (ledger block 88)',
    ]);
    // Only the bridge's own release record settled it.
    expect(only(h).status).toBe('done');
  });

  it('does not mark a return done from a chain finality or a wallet answer alone', async () => {
    const h = harness();
    await startReturn(h.deps, await plan(h), h.emit);
    const sent = only(h);
    h.chain.statuses.set(sent.signature!, { confirmation: 'finalized', failed: false, slot: 5n });
    h.views.ret = makeSolReturn({ status: { Burned: { burn_op_id: 1n } } });
    const controller = new AbortController();
    let polls = 0;
    h.deps.sleep = async () => {
      if (++polls >= 3) controller.abort();
    };
    expect(await trackReturn(h.deps, sent, () => {}, controller.signal)).toBe('aborted');
    expect(only(h).status).toBe('sent');
  });

  it('saves a signature recovered by searching, so a reload does not search again', async () => {
    const h = harness({ wallet: { signs: false, sendsAndSigns: true } });
    h.wallet.onSignAndSend = async () => {
      throw new Error('timeout');
    };
    const p = await plan(h);
    await startReturn(h.deps, p, h.emit);
    const held = only(h);
    const signature = landOn(h.chain, Uint8Array.from([1, ...fill(0xe7, 64), ...p.message]));
    h.views.release = makeSolRelease({ status: { Released: null } });
    expect(await trackReturn(h.deps, held, () => {})).toBe('done');
    expect(only(h)).toMatchObject({ status: 'done', signature });
  });

  it('describes each stage of the evidence in plain words', () => {
    const base = { signature: 'S', recovered: false, ret: null, release: null };
    expect(describeReturnProgress(null).label).toBe('Sent to Solana');
    expect(describeReturnProgress({ ...base, chain: { kind: 'failed_on_chain', finalized: true } })).toMatchObject({ label: 'The transaction failed on Solana', terminal: true });
    expect(describeReturnProgress({ ...base, chain: { kind: 'failed_on_chain', finalized: false } })).toMatchObject({ label: 'Failed, but not final yet', terminal: false });
    const expiredText = describeReturnProgress({ ...base, chain: { kind: 'not_found', blockhashExpired: true } });
    expect(expiredText).toMatchObject({ label: 'Not found, outcome unknown', terminal: false });
    expect(expiredText.detail).not.toMatch(/never landed|No icUSD moved/);
    expect(expiredText.detail).toMatch(/not proof/i);
    expect(describeReturnProgress({ ...base, chain: { kind: 'unavailable', message: 'x' } }).label).toBe('Checking');
  });
});

describe('evidence never authorises forgetting an uncertain transfer', () => {
  it('keeps a sent but unindexed transfer held after its blockhash expires', async () => {
    const h = harness();
    await startReturn(h.deps, await plan(h), h.emit);
    const sent = only(h);
    expect(sent.status).toBe('sent');
    // The node has not indexed it (no status, nothing in the listing) and the blockhash is now too old to land.
    h.chain.height = 1_001n;
    const evidence = await reconcileReturn(h.deps, sent);
    expect(evidence.chain).toEqual({ kind: 'not_found', blockhashExpired: true });
    expect(verdictOf(evidence)).toBe('unknown');
    expect(mayDiscard(sent, verdictOf(evidence))).toBe(false);
    expect(discardReturn(h.deps, sent, verdictOf(evidence))).toMatchObject({ ok: false });
    // The original identity is retained and nothing was signed or sent again.
    expect(h.store.list()).toHaveLength(1);
    expect(h.store.get(sent.id)).toMatchObject({ id: sent.id, messageHex: sent.messageHex, signature: sent.signature, nonce: sent.nonce });
    expect(h.wallet.calls).toHaveLength(1);
    expect(h.chain.sent).toHaveLength(1);
  });

  it('keeps a wallet-sent transfer held when it fell outside the 25 signatures that are searched', async () => {
    const h = harness({ wallet: { signs: false, sendsAndSigns: true } });
    const p = await plan(h);
    h.wallet.onSignAndSend = async () => {
      // The wallet really broadcast it, then lost its reply.
      landOn(h.chain, Uint8Array.from([1, ...fill(0xe5, 64), ...p.message]));
      throw new Error('timeout');
    };
    await startReturn(h.deps, p, h.emit);
    const held = only(h);
    expect(held.status).toBe('held');
    // Thirty newer signatures push it out of the search window, and the blockhash has expired.
    for (let i = 0; i < 30; i++) h.chain.listing.unshift({ signature: `Newer${i}`, failed: false });
    h.chain.height = 1_001n;
    const evidence = await reconcileReturn(h.deps, held);
    expect(evidence.signature).toBeNull();
    expect(verdictOf(evidence)).toBe('unknown');
    expect(discardReturn(h.deps, held, verdictOf(evidence))).toMatchObject({ ok: false });
    expect(h.store.list()).toHaveLength(1);
    expect(h.wallet.calls).toHaveLength(1);
  });

  it.each(['processed', 'confirmed'] as const)('does not treat a %s failure as definitive: the same signature can still finalize successfully', async (confirmation) => {
    const h = harness();
    await startReturn(h.deps, await plan(h), h.emit);
    const sent = only(h);
    h.chain.statuses.set(sent.signature!, { confirmation, failed: true, slot: 9n });
    const early = await reconcileReturn(h.deps, sent);
    expect(verdictOf(early)).not.toBe('failed_on_chain');
    expect(mayDiscard(sent, verdictOf(early))).toBe(false);
    expect(discardReturn(h.deps, sent, verdictOf(early))).toMatchObject({ ok: false });
    expect(h.store.list()).toHaveLength(1);
    // The fork it failed on is dropped and the transfer finalizes successfully.
    h.chain.statuses.set(sent.signature!, { confirmation: 'finalized', failed: false, slot: 12n });
    const later = await reconcileReturn(h.deps, sent);
    expect(verdictOf(later)).toBe('landed');
    expect(mayDiscard(sent, verdictOf(later))).toBe(false);
    expect(discardReturn(h.deps, sent, verdictOf(later))).toMatchObject({ ok: false });
    expect(h.store.list()).toHaveLength(1);
  });

  it('counts a finalized failure of a signature the wallet REPORTED only when that transaction carries the saved message', async () => {
    const h = harness({ wallet: { signs: false, sendsAndSigns: true } });
    const p = await plan(h);
    await startReturn(h.deps, p, h.emit);
    const sent = only(h);
    expect(sent).toMatchObject({ status: 'sent', signMode: 'sign_and_send' });
    h.chain.statuses.set(sent.signature!, { confirmation: 'finalized', failed: true, slot: 9n });
    // The transaction behind the reported signature is unknown, or is some other message: not proof about ours.
    let evidence = await reconcileReturn(h.deps, sent);
    expect(evidence.chain.kind).toBe('unavailable');
    expect(verdictOf(evidence)).toBe('unknown');
    const other = buildReturnMessage({ tokenProgram: 'token2022', mint: fill(0x44), returnOwner: fill(0x33), roleKeys: [fill(0x11), fill(0x22)], holder: pubkeyFromBase58(KEYS.holder)!, sourceTokenAccount: pubkeyFromBase58(KEYS.holderSource)!, amount: 300_000_000n, memo: p.memo, recentBlockhash: fill(0x78) });
    h.chain.landed.set(sent.signature!, Uint8Array.from([1, ...fill(0xe5, 64), ...other.message]));
    evidence = await reconcileReturn(h.deps, sent);
    expect(verdictOf(evidence)).toBe('unknown');
    expect(discardReturn(h.deps, sent, verdictOf(evidence))).toMatchObject({ ok: false });
    expect(h.store.list()).toHaveLength(1);
    // Carrying exactly the saved message, the finalized failure is definitive.
    h.chain.landed.set(sent.signature!, Uint8Array.from([1, ...fill(0xe5, 64), ...p.message]));
    evidence = await reconcileReturn(h.deps, sent);
    expect(evidence.chain).toEqual({ kind: 'failed_on_chain', finalized: true });
    expect(discardReturn(h.deps, sent, verdictOf(evidence))).toEqual({ ok: true });
  });

  it('allows a discard once the failure of the exact saved attempt is FINALIZED', async () => {
    const h = harness();
    await startReturn(h.deps, await plan(h), h.emit);
    const sent = only(h);
    h.chain.statuses.set(sent.signature!, { confirmation: 'finalized', failed: true, slot: 9n });
    const evidence = await reconcileReturn(h.deps, sent);
    expect(evidence.chain).toEqual({ kind: 'failed_on_chain', finalized: true });
    expect(verdictOf(evidence)).toBe('failed_on_chain');
    expect(discardReturn(h.deps, sent, verdictOf(evidence))).toEqual({ ok: true });
    expect(h.store.list()).toHaveLength(0);
  });
});

describe('telling the bridge', () => {
  it('is a hint through the ICP wallet, refused for a disabled destination or a changed session', async () => {
    const h = harness();
    expect(await notifyReturn(h.deps, 'Sig')).toEqual({ ok: true });
    expect(h.notify).toHaveBeenCalledWith('Sig');
    h.signing.fresh.value = { status: 'disabled', reason: 'off' };
    expect(await notifyReturn(h.deps, 'Sig')).toMatchObject({ ok: false });
    expect(h.notify).toHaveBeenCalledTimes(1);
    h.signing.fresh.value = { status: 'enabled', reason: null };
    h.session.current = false;
    expect(await notifyReturn(h.deps, 'Sig')).toMatchObject({ ok: false, message: expect.stringContaining('session changed') });
    h.session.current = true;
    h.notify.mockResolvedValueOnce({ Err: { NotFound: null } } as never);
    expect(await notifyReturn(h.deps, 'Sig')).toMatchObject({ ok: false });
    h.notify.mockRejectedValueOnce(new Error('lost'));
    expect(await notifyReturn(h.deps, 'Sig')).toMatchObject({ ok: false, message: expect.stringContaining('lost') });
  });
});

describe('the state machine', () => {
  it('keeps a held, signed or in-flight attempt on screen until it is resolved', () => {
    const intent = { id: 'x' } as SolReturnIntent;
    for (const state of [
      { kind: 'signing', intent },
      { kind: 'signed', intent },
      { kind: 'held', intent, message: 'm' },
      { kind: 'broadcast_uncertain', intent, message: 'm' },
    ] as SolReturnState[]) {
      expect(reduceSolReturn(state, { type: 'reset' })).toBe(state);
      expect(reduceSolReturn(state, { type: 'preparing' })).toBe(state);
    }
    expect(reduceSolReturn({ kind: 'declined' }, { type: 'reset' })).toEqual({ kind: 'draft' });
    expect(reduceSolReturn({ kind: 'draft' }, { type: 'preparing' })).toEqual({ kind: 'preparing' });
    expect(reduceSolReturn({ kind: 'draft' }, { type: 'session_changed' })).toEqual({ kind: 'session_changed' });
  });
});
