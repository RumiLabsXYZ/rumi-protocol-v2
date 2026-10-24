import { describe, expect, it, vi } from 'vitest';
import { Principal } from '@dfinity/principal';
import type { ReleaseView } from '$declarations/rumi_icusd_bridge/rumi_icusd_bridge.did';
import {
  SELECTORS,
  ZERO_BYTES32,
  encodeBridgeOut,
  encodeUint256,
  hexToBytes,
} from './abi';
import { formatE8s } from './amounts';
import {
  INITIAL_BASE_TO_ICP_STATE,
  checkIntentOnChain,
  classifyUnsentIntent,
  discardUnsentIntent,
  evaluateBaseToIcpDraft,
  notifyBridge,
  readBaseSnapshot,
  reduceBaseToIcp,
  sendBridgeOut,
  trackBaseToIcp,
  type BaseDraftInput,
  type BaseToIcpDeps,
  type BaseToIcpEvent,
  type BaseToIcpState,
} from './baseToIcp';
import { BASE_MAINNET } from './chains';
import { IntentStore, type BaseToIcpIntent } from './intents';
import { TEST_OWNER, createFakeProvider, createMemoryStorage, makeRelease, walletError, type FakeProvider } from './testSupport';
import type { BaseOutTracking } from './status';

const ME = TEST_OWNER.toText();
const ACCOUNT = '0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa';
const OTHER = '0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb';
const CONTRACT = '0x2222222222222222222222222222222222222222';
const TRANSFER_ID = `0x${'ee'.repeat(32)}`;
const NONCE_BYTE = 0x7a;
const NONCE = `0x${'7a'.repeat(32)}`;
const TX_HASH = `0x${'12'.repeat(32)}`;
const AMOUNT = 250_000_000n; // 2.5 icUSD
const fmt = (e8s: bigint) => formatE8s(e8s);

// ---------------------------------------------------------------------------
// Draft evaluation
// ---------------------------------------------------------------------------

function draft(overrides: Partial<BaseDraftInput> = {}): BaseDraftInput {
  return {
    walletAvailable: true,
    account: ACCOUNT,
    chainId: 8453,
    expectedChain: BASE_MAINNET,
    icpPrincipal: ME,
    icpPrincipalIsAnonymous: false,
    amountText: '2.5',
    balanceE8s: 1_000_000_000n,
    minimumE8s: 100_000_000n,
    ethBalanceWei: 10n ** 16n,
    gasCostWei: 10n ** 13n,
    preflightError: null,
    ...overrides,
  };
}

describe('evaluateBaseToIcpDraft', () => {
  it('is ready when everything checks out', () => {
    const result = evaluateBaseToIcpDraft(draft(), fmt);
    expect(result).toMatchObject({ state: 'ready', canSend: true, amountE8s: 250_000_000n, warnings: [] });
  });

  it('blocks sending on the wrong network and names the right one', () => {
    const result = evaluateBaseToIcpDraft(draft({ chainId: 1 }), fmt);
    expect(result).toMatchObject({ state: 'wrong_network', canSend: false });
    expect(result.message).toContain('Switch to Base');
    expect(evaluateBaseToIcpDraft(draft({ chainId: null }), fmt).state).toBe('wrong_network');
  });

  it('asks for a wallet, then a connection, before anything else', () => {
    expect(evaluateBaseToIcpDraft(draft({ walletAvailable: false, account: null }), fmt).state).toBe('no_wallet');
    expect(evaluateBaseToIcpDraft(draft({ account: null }), fmt).state).toBe('not_connected');
  });

  it('requires an ICP sign-in and rejects the anonymous principal', () => {
    expect(evaluateBaseToIcpDraft(draft({ icpPrincipal: null }), fmt).state).toBe('icp_signed_out');
    expect(evaluateBaseToIcpDraft(draft({ icpPrincipalIsAnonymous: true }), fmt).state).toBe('icp_signed_out');
  });

  it('validates the amount: format, zero and the contract minimum', () => {
    expect(evaluateBaseToIcpDraft(draft({ amountText: '' }), fmt).state).toBe('invalid_amount');
    expect(evaluateBaseToIcpDraft(draft({ amountText: '1.123456789' }), fmt).state).toBe('invalid_amount');
    expect(evaluateBaseToIcpDraft(draft({ amountText: '0' }), fmt).state).toBe('invalid_amount');
    const low = evaluateBaseToIcpDraft(draft({ amountText: '0.5' }), fmt);
    expect(low).toMatchObject({ state: 'below_minimum', canSend: false, amountE8s: 50_000_000n });
    expect(low.message).toContain('1 icUSD');
    expect(evaluateBaseToIcpDraft(draft({ amountText: '0.5', minimumE8s: null }), fmt).state).toBe('ready');
  });

  it('checks the amount against the Base balance', () => {
    const result = evaluateBaseToIcpDraft(draft({ amountText: '11', balanceE8s: 1_000_000_000n }), fmt);
    expect(result).toMatchObject({ state: 'insufficient_balance', canSend: false });
    expect(result.message).toContain('10 icUSD');
    expect(evaluateBaseToIcpDraft(draft({ amountText: '10', balanceE8s: 1_000_000_000n }), fmt).state).toBe('ready');
    expect(evaluateBaseToIcpDraft(draft({ balanceE8s: null }), fmt).state).toBe('balance_unknown');
  });

  it('shows "insufficient ETH for gas" when the estimated cost exceeds the ETH balance', () => {
    const result = evaluateBaseToIcpDraft(draft({ ethBalanceWei: 1_000n, gasCostWei: 2_000n }), fmt);
    expect(result).toMatchObject({ state: 'insufficient_gas', canSend: false });
    expect(result.message).toContain('Insufficient ETH for gas');
    expect(evaluateBaseToIcpDraft(draft({ ethBalanceWei: 2_000n, gasCostWei: 2_000n }), fmt).state).toBe('ready');
  });

  it('does not block when gas cannot be estimated, but says so', () => {
    const result = evaluateBaseToIcpDraft(draft({ gasCostWei: null }), fmt);
    expect(result).toMatchObject({ state: 'ready', canSend: true });
    expect(result.warnings[0]).toMatch(/Gas could not be estimated/);
  });

  it('surfaces a decoded revert from the gas estimate (for example a pause)', () => {
    const result = evaluateBaseToIcpDraft(draft({ preflightError: 'Bridging from Base to ICP is paused right now.' }), fmt);
    expect(result).toMatchObject({ state: 'preflight_failed', canSend: false });
    expect(result.message).toMatch(/paused/);
  });
});

// ---------------------------------------------------------------------------
// Fake chain
// ---------------------------------------------------------------------------

interface Chain {
  provider: FakeProvider;
  used: Set<string>;
  txs: Array<{ from: string; to: string; data: string; value: string }>;
}

function chain(over: { account?: string; chainId?: number; balance?: bigint; eth?: bigint; gas?: bigint; gasPrice?: bigint } = {}): Chain {
  const used = new Set<string>();
  const txs: Chain['txs'] = [];
  const provider = createFakeProvider({
    account: over.account ?? ACCOUNT,
    chainId: over.chainId ?? 8453,
    handlers: {
      eth_call: (params) => {
        const call = (params as [{ data: string }])[0];
        if (call.data.startsWith(SELECTORS.balanceOf)) return `0x${encodeUint256(over.balance ?? 1_000_000_000n)}`;
        if (call.data.startsWith(SELECTORS.computeTransferId)) return TRANSFER_ID;
        if (call.data.startsWith(SELECTORS.bridgeOutUsed)) {
          const id = `0x${call.data.slice(10)}`;
          return `0x${encodeUint256(used.has(id) ? 1n : 0n)}`;
        }
        throw new Error(`unexpected eth_call ${call.data}`);
      },
      eth_getBalance: () => `0x${(over.eth ?? 10n ** 16n).toString(16)}`,
      eth_estimateGas: () => `0x${(over.gas ?? 80_000n).toString(16)}`,
      eth_gasPrice: () => `0x${(over.gasPrice ?? 1_000_000n).toString(16)}`,
      eth_sendTransaction: (params) => {
        txs.push((params as [Chain['txs'][number]])[0]);
        return TX_HASH;
      },
    },
  });
  return { provider, used, txs };
}

interface SendHarness {
  deps: BaseToIcpDeps;
  chain: Chain;
  store: IntentStore;
  storage: ReturnType<typeof createMemoryStorage>;
  events: BaseToIcpEvent[];
  emit: (e: BaseToIcpEvent) => void;
  state: () => BaseToIcpState;
  getRelease: ReturnType<typeof vi.fn>;
  notify: ReturnType<typeof vi.fn>;
  current: { value: boolean };
}

function harness(over: { chain?: Chain; storage?: ReturnType<typeof createMemoryStorage>; principal?: Principal } = {}): SendHarness {
  const c = over.chain ?? chain();
  const storage = over.storage ?? createMemoryStorage();
  const store = new IntentStore(storage, ME);
  const current = { value: true };
  const getRelease = vi.fn(async () => null as ReleaseView | null);
  const notify = vi.fn(async () => ({ Ok: null }));
  const deps: BaseToIcpDeps = {
    provider: c.provider,
    chain: BASE_MAINNET,
    contract: CONTRACT,
    account: ACCOUNT,
    icpPrincipal: over.principal ?? TEST_OWNER,
    store,
    reads: { getRelease: getRelease as any },
    bridge: { notifyBaseBridgeOut: notify as any },
    isCurrent: () => current.value,
    random: (length) => new Uint8Array(length).fill(NONCE_BYTE),
    now: () => 1_800_000_000_000,
    sleep: async () => {},
  };
  const events: BaseToIcpEvent[] = [];
  let state: BaseToIcpState = reduceBaseToIcp(INITIAL_BASE_TO_ICP_STATE, { type: 'confirm', amountE8s: AMOUNT });
  const emit = (e: BaseToIcpEvent) => {
    events.push(e);
    state = reduceBaseToIcp(state, e);
  };
  return { deps, chain: c, store, storage, events, emit, state: () => state, getRelease, notify, current };
}

const expectedCalldata = (nonce = NONCE, amount = AMOUNT) =>
  encodeBridgeOut({ amountE8s: amount, icpPrincipal: TEST_OWNER.toUint8Array(), icpSubaccount: ZERO_BYTES32, clientNonce: nonce });

// ---------------------------------------------------------------------------
// Snapshot
// ---------------------------------------------------------------------------

describe('readBaseSnapshot', () => {
  it('reads the balance and ETH, and prices the exact calldata', async () => {
    const h = harness({ chain: chain({ balance: 777n, eth: 5_000_000n, gas: 100_000n, gasPrice: 3n }) });
    const snapshot = await readBaseSnapshot(h.deps, AMOUNT);
    expect(snapshot).toEqual({ balanceE8s: 777n, ethBalanceWei: 5_000_000n, gasCostWei: 300_000n, preflightError: null });
    const estimate = h.chain.provider.callsTo('eth_estimateGas')[0].params as [{ from: string; to: string; data: string }];
    expect(estimate[0]).toMatchObject({ from: ACCOUNT, to: CONTRACT });
    // Same shape as the real call: selector, amount, 0x80 offset, zero subaccount, a nonce, the signed-in principal.
    expect(estimate[0].data.startsWith(SELECTORS.bridgeOut + encodeUint256(AMOUNT) + encodeUint256(0x80n) + '00'.repeat(32))).toBe(true);
    expect(estimate[0].data).toContain(Buffer.from(TEST_OWNER.toUint8Array()).toString('hex'));
  });

  it('turns a reverting estimate into a plain message (bridge-out paused)', async () => {
    const h = harness();
    h.chain.provider.handlers.eth_estimateGas = () => {
      throw walletError(-32603, 'execution reverted', { data: '0x97221de2' });
    };
    const snapshot = await readBaseSnapshot(h.deps, AMOUNT);
    expect(snapshot.preflightError).toMatch(/paused/);
    expect(snapshot.gasCostWei).toBeNull();
  });

  it('leaves the gas cost unknown when the estimate fails for another reason', async () => {
    const h = harness();
    h.chain.provider.handlers.eth_estimateGas = () => {
      throw new Error('method not supported');
    };
    const snapshot = await readBaseSnapshot(h.deps, AMOUNT);
    expect(snapshot).toMatchObject({ gasCostWei: null, preflightError: null, balanceE8s: 1_000_000_000n });
  });

  it('reports each unreadable value as null instead of guessing', async () => {
    const h = harness();
    h.chain.provider.handlers.eth_getBalance = () => {
      throw new Error('rpc down');
    };
    const base = h.chain.provider.handlers.eth_call;
    h.chain.provider.handlers.eth_call = (params, call) => {
      const data = (params as [{ data: string }])[0].data;
      if (data.startsWith(SELECTORS.balanceOf)) throw new Error('rpc down');
      return base(params, call);
    };
    expect(await readBaseSnapshot(h.deps, AMOUNT)).toMatchObject({ balanceE8s: null, ethBalanceWei: null });
  });

  it('skips the gas estimate until there is an amount', async () => {
    const h = harness();
    await readBaseSnapshot(h.deps, null);
    expect(h.chain.provider.callsTo('eth_estimateGas')).toHaveLength(0);
  });
});

// ---------------------------------------------------------------------------
// Sending
// ---------------------------------------------------------------------------

describe('sendBridgeOut', () => {
  it('persists the intent before the wallet prompt, then sends the exact calldata', async () => {
    const h = harness();
    let savedAtSend: unknown = null;
    h.chain.provider.handlers.eth_sendTransaction = (params) => {
      savedAtSend = h.store.get(TRANSFER_ID);
      h.chain.txs.push((params as [Chain['txs'][number]])[0]);
      return TX_HASH;
    };
    const outcome = await sendBridgeOut(h.deps, { amountE8s: AMOUNT }, h.emit);

    expect(savedAtSend).toMatchObject({
      v: 1,
      kind: 'base_to_icp',
      clientNonce: NONCE,
      transferId: TRANSFER_ID,
      from: ACCOUNT,
      amountE8s: AMOUNT,
      destinationPrincipal: ME,
      chainId: 8453,
      contract: CONTRACT,
      status: 'prepared',
    });
    expect(h.chain.txs).toEqual([{ from: ACCOUNT, to: CONTRACT, data: expectedCalldata(), value: '0x0' }]);
    expect(outcome).toMatchObject({ kind: 'sent', txHash: TX_HASH });
    expect(h.store.get(TRANSFER_ID)).toMatchObject({ status: 'sent', txHash: TX_HASH });
    expect(h.state()).toMatchObject({ kind: 'tracking', tracking: { stage: 'submitted' } });
  });

  it('asks the contract for the transfer id, then re-checks chain, account and bridgeOutUsed right before sending', async () => {
    const h = harness();
    await sendBridgeOut(h.deps, { amountE8s: AMOUNT }, h.emit);
    const methods = h.chain.provider.calls.map((call) => call.method);
    const send = methods.indexOf('eth_sendTransaction');
    expect(methods.slice(0, send)).toEqual(['eth_call', 'eth_chainId', 'eth_accounts', 'eth_call']);
    const calls = h.chain.provider.callsTo('eth_call').map((call) => (call.params as [{ data: string }])[0].data);
    expect(calls[0].startsWith(SELECTORS.computeTransferId)).toBe(true);
    expect(calls[0]).toContain(ACCOUNT.slice(2));
    expect(calls[0]).toContain(NONCE.slice(2));
    expect(calls[1]).toBe(SELECTORS.bridgeOutUsed + TRANSFER_ID.slice(2));
  });

  it('emits the wallet prompt before the send and removes nothing on success', async () => {
    const h = harness();
    await sendBridgeOut(h.deps, { amountE8s: AMOUNT }, h.emit);
    expect(h.events.map((e) => e.type)).toEqual(['wallet_prompt', 'sent']);
  });

  it('returns to the draft without a trace when the user declines (4001)', async () => {
    const h = harness();
    h.chain.provider.handlers.eth_sendTransaction = () => {
      throw walletError(4001, 'User rejected the request.');
    };
    const outcome = await sendBridgeOut(h.deps, { amountE8s: AMOUNT }, h.emit);
    expect(outcome).toEqual({ kind: 'rejected' });
    expect(h.state()).toEqual({ kind: 'rejected' });
    expect(h.store.list()).toEqual([]);
    // The user can start again from the confirmation step.
    expect(reduceBaseToIcp(h.state(), { type: 'confirm', amountE8s: AMOUNT }).kind).toBe('confirming');
  });

  it('aborts, without sending, if the wallet changed network after the review', async () => {
    const h = harness();
    h.chain.provider.state.chainId = 1;
    const outcome = await sendBridgeOut(h.deps, { amountE8s: AMOUNT }, h.emit);
    expect(outcome).toEqual({ kind: 'aborted', reason: 'chain_changed' });
    expect(h.chain.provider.callsTo('eth_sendTransaction')).toHaveLength(0);
    expect(h.chain.txs).toEqual([]);
    expect(h.state()).toMatchObject({ kind: 'aborted', reason: 'chain_changed' });
    expect(h.store.list()).toEqual([]);
  });

  it('aborts, without sending, if the selected account changed after the review', async () => {
    const h = harness();
    h.chain.provider.state.account = OTHER;
    const outcome = await sendBridgeOut(h.deps, { amountE8s: AMOUNT }, h.emit);
    expect(outcome).toEqual({ kind: 'aborted', reason: 'account_changed' });
    expect(h.chain.provider.callsTo('eth_sendTransaction')).toHaveLength(0);
    expect(h.store.list()).toEqual([]);
  });

  it('aborts if the wallet has no selected account any more', async () => {
    const h = harness({ chain: chain({ account: ACCOUNT }) });
    h.chain.provider.state.account = null;
    expect(await sendBridgeOut(h.deps, { amountE8s: AMOUNT }, h.emit)).toMatchObject({ kind: 'aborted', reason: 'account_changed' });
  });

  it('does not send when bridgeOutUsed is already true; it switches to tracking', async () => {
    const h = harness();
    h.chain.used.add(TRANSFER_ID);
    const outcome = await sendBridgeOut(h.deps, { amountE8s: AMOUNT }, h.emit);
    expect(outcome.kind).toBe('already_used');
    expect(h.chain.provider.callsTo('eth_sendTransaction')).toHaveLength(0);
    expect(h.state()).toMatchObject({ kind: 'tracking', tracking: { stage: 'awaiting_finality' } });
    expect(h.store.get(TRANSFER_ID)).toMatchObject({ status: 'sent' });
    expect(h.store.get(TRANSFER_ID)).not.toHaveProperty('txHash');
  });

  it('makes no wallet call when the intent cannot be persisted', async () => {
    const h = harness();
    h.storage.setItem = () => {
      throw new Error('quota');
    };
    const outcome = await sendBridgeOut(h.deps, { amountE8s: AMOUNT }, h.emit);
    expect(outcome.kind).toBe('blocked');
    expect(h.chain.provider.callsTo('eth_sendTransaction')).toHaveLength(0);
    expect(h.state()).toMatchObject({ kind: 'blocked' });
  });

  it('makes no wallet call when the transfer id cannot be read', async () => {
    const h = harness();
    const base = h.chain.provider.handlers.eth_call;
    h.chain.provider.handlers.eth_call = (params, call) => {
      if ((params as [{ data: string }])[0].data.startsWith(SELECTORS.computeTransferId)) throw new Error('rpc down');
      return base(params, call);
    };
    expect((await sendBridgeOut(h.deps, { amountE8s: AMOUNT }, h.emit)).kind).toBe('blocked');
    expect(h.store.list()).toEqual([]);
    expect(h.chain.provider.callsTo('eth_sendTransaction')).toHaveLength(0);
  });

  it('refuses a second send while the first is still being prepared or signed', async () => {
    const h = harness();
    let release!: (hash: string) => void;
    let holdFirst = true;
    h.chain.provider.handlers.eth_sendTransaction = (params) => {
      h.chain.txs.push((params as [Chain['txs'][number]])[0]);
      if (!holdFirst) return TX_HASH;
      holdFirst = false;
      return new Promise<string>((resolve) => (release = resolve));
    };
    const first = sendBridgeOut(h.deps, { amountE8s: AMOUNT }, h.emit);
    const second = await sendBridgeOut(h.deps, { amountE8s: AMOUNT }, h.emit);
    expect(second).toMatchObject({ kind: 'blocked' });
    // Let the first reach the wallet prompt, then try again while the prompt is open.
    await vi.waitFor(() => expect(release).toBeTypeOf('function'));
    expect(await sendBridgeOut(h.deps, { amountE8s: AMOUNT }, h.emit)).toMatchObject({ kind: 'blocked' });
    release(TX_HASH);
    expect(await first).toMatchObject({ kind: 'sent' });
    expect(h.chain.txs).toHaveLength(1);
    expect(h.store.list()).toHaveLength(1);
    // Once the first finishes, a deliberate new send is possible again.
    expect(await sendBridgeOut(h.deps, { amountE8s: AMOUNT }, () => {})).toMatchObject({ kind: 'sent' });
  });

  it('does nothing when the ICP session is no longer current', async () => {
    const h = harness();
    h.current.value = false;
    expect(await sendBridgeOut(h.deps, { amountE8s: AMOUNT }, h.emit)).toEqual({ kind: 'aborted', reason: 'session_changed' });
    expect(h.chain.provider.calls).toHaveLength(0);
  });

  it('stops before sending if the ICP session changes during the pre-send checks', async () => {
    const h = harness();
    h.chain.provider.handlers.eth_accounts = () => {
      h.current.value = false;
      return [ACCOUNT];
    };
    const outcome = await sendBridgeOut(h.deps, { amountE8s: AMOUNT }, h.emit);
    expect(outcome).toEqual({ kind: 'aborted', reason: 'session_changed' });
    expect(h.chain.provider.callsTo('eth_sendTransaction')).toHaveLength(0);
    expect(h.store.list()).toEqual([]);
  });

  it('refuses the anonymous ICP principal', async () => {
    const h = harness({ principal: Principal.anonymous() });
    expect(await sendBridgeOut(h.deps, { amountE8s: AMOUNT }, h.emit)).toEqual({ kind: 'aborted', reason: 'check_failed' });
    expect(h.chain.provider.calls).toHaveLength(0);
  });

  it('keeps the intent and calls the result uncertain when the wallet fails without proof it did not send', async () => {
    const h = harness();
    h.chain.provider.handlers.eth_sendTransaction = () => {
      throw new Error('socket closed');
    };
    const outcome = await sendBridgeOut(h.deps, { amountE8s: AMOUNT }, h.emit);
    expect(outcome).toMatchObject({ kind: 'failed', uncertain: true });
    expect(h.store.get(TRANSFER_ID)).toMatchObject({ status: 'prepared', clientNonce: NONCE });
    expect(h.state()).toMatchObject({ kind: 'failed', uncertain: true });
    // An uncertain send cannot simply be restarted from the form.
    expect(reduceBaseToIcp(h.state(), { type: 'confirm', amountE8s: AMOUNT }).kind).toBe('failed');
    expect(reduceBaseToIcp(h.state(), { type: 'reset' }).kind).toBe('failed');
  });

  it('treats a wallet simulation revert as a clean failure and forgets the intent', async () => {
    const h = harness();
    h.chain.provider.handlers.eth_sendTransaction = () => {
      throw walletError(-32603, 'execution reverted', { data: '0x97221de2' });
    };
    const outcome = await sendBridgeOut(h.deps, { amountE8s: AMOUNT }, h.emit);
    expect(outcome).toMatchObject({ kind: 'failed', uncertain: false });
    expect((outcome as { message: string }).message).toMatch(/paused/);
    expect(h.store.list()).toEqual([]);
  });

  it('does not trust a malformed hash from the wallet', async () => {
    const h = harness();
    h.chain.provider.handlers.eth_sendTransaction = () => 'pending';
    const outcome = await sendBridgeOut(h.deps, { amountE8s: AMOUNT }, h.emit);
    expect(outcome).toMatchObject({ kind: 'failed', uncertain: true });
    expect(h.store.get(TRANSFER_ID)).toMatchObject({ status: 'prepared' });
    expect(h.store.get(TRANSFER_ID)).not.toHaveProperty('txHash');
  });
});

describe('reload recovery: resuming with the same nonce', () => {
  async function unsent(): Promise<{ storage: ReturnType<typeof createMemoryStorage>; first: SendHarness }> {
    const storage = createMemoryStorage();
    const first = harness({ storage });
    first.chain.provider.handlers.eth_sendTransaction = () => {
      throw new Error('tab crashed');
    };
    await sendBridgeOut(first.deps, { amountE8s: AMOUNT }, first.emit);
    return { storage, first };
  }

  it('finds the unsent intent after a reload and resends it with the SAME nonce and transfer id', async () => {
    const { storage } = await unsent();
    // New page load: a fresh store over the same storage, a fresh wallet session.
    const second = harness({ storage });
    const stored = second.store.get(TRANSFER_ID) as BaseToIcpIntent;
    expect(stored).toMatchObject({ status: 'prepared', clientNonce: NONCE });

    const check = await checkIntentOnChain(second.chain.provider, stored);
    expect(check).toBe('unused');
    expect(classifyUnsentIntent(stored, { walletAvailable: true, account: ACCOUNT, chainId: 8453, used: false })).toEqual({ action: 'resume' });

    // A different random source proves the nonce is reused, not regenerated.
    second.deps.random = (length) => new Uint8Array(length).fill(0x11);
    reduceBaseToIcp(INITIAL_BASE_TO_ICP_STATE, { type: 'confirm', amountE8s: stored.amountE8s });
    const outcome = await sendBridgeOut(second.deps, { amountE8s: stored.amountE8s, resume: stored }, second.emit);
    expect(outcome).toMatchObject({ kind: 'sent', txHash: TX_HASH });
    expect(second.chain.txs).toEqual([{ from: ACCOUNT, to: CONTRACT, data: expectedCalldata(NONCE), value: '0x0' }]);
    // No new transfer id was requested and still only one intent exists.
    const idCalls = second.chain.provider
      .callsTo('eth_call')
      .filter((call) => (call.params as [{ data: string }])[0].data.startsWith(SELECTORS.computeTransferId));
    expect(idCalls).toHaveLength(0);
    expect(second.store.list()).toHaveLength(1);
    expect(second.store.get(TRANSFER_ID)).toMatchObject({ status: 'sent', txHash: TX_HASH });
  });

  it('goes to tracking instead of resending when the first attempt actually went through', async () => {
    const { storage } = await unsent();
    const second = harness({ storage });
    second.chain.used.add(TRANSFER_ID);
    const stored = second.store.get(TRANSFER_ID) as BaseToIcpIntent;
    expect(await checkIntentOnChain(second.chain.provider, stored)).toBe('used');
    expect(classifyUnsentIntent(stored, { walletAvailable: true, account: ACCOUNT, chainId: 8453, used: true })).toEqual({ action: 'track' });
    // Even if the user presses Resume, the pre-send check prevents a second send.
    const outcome = await sendBridgeOut(second.deps, { amountE8s: stored.amountE8s, resume: stored }, second.emit);
    expect(outcome.kind).toBe('already_used');
    expect(second.chain.txs).toEqual([]);
  });

  it('keeps the intent when the user declines a resumed send', async () => {
    const { storage } = await unsent();
    const second = harness({ storage });
    second.chain.provider.handlers.eth_sendTransaction = () => {
      throw walletError(4001);
    };
    const stored = second.store.get(TRANSFER_ID) as BaseToIcpIntent;
    expect(await sendBridgeOut(second.deps, { amountE8s: stored.amountE8s, resume: stored }, second.emit)).toEqual({ kind: 'rejected' });
    expect(second.store.get(TRANSFER_ID)).toMatchObject({ status: 'prepared' });
  });

  it('cannot be sent from a different Base account (the transfer id depends on the sender)', async () => {
    const { storage } = await unsent();
    const second = harness({ storage, chain: chain({ account: OTHER }) });
    second.deps.account = OTHER;
    const stored = second.store.get(TRANSFER_ID) as BaseToIcpIntent;
    const outcome = await sendBridgeOut(second.deps, { amountE8s: stored.amountE8s, resume: stored }, second.emit);
    expect(outcome).toEqual({ kind: 'aborted', reason: 'intent_mismatch' });
    expect(second.chain.provider.callsTo('eth_sendTransaction')).toHaveLength(0);
    expect(second.store.get(TRANSFER_ID)).toBeDefined();
  });

  it('refuses to resend onto another network or a different ICP principal', async () => {
    const { storage } = await unsent();
    const wrongChain = harness({ storage });
    wrongChain.deps.chain = { ...BASE_MAINNET, chainId: 84532 };
    const stored = wrongChain.store.get(TRANSFER_ID) as BaseToIcpIntent;
    expect(await sendBridgeOut(wrongChain.deps, { amountE8s: stored.amountE8s, resume: stored }, wrongChain.emit)).toMatchObject({ reason: 'intent_mismatch' });

    const otherPrincipal = harness({ storage, principal: Principal.fromText('aaaaa-aa') });
    expect(await sendBridgeOut(otherPrincipal.deps, { amountE8s: stored.amountE8s, resume: stored }, otherPrincipal.emit)).toMatchObject({ reason: 'intent_mismatch' });
  });
});

describe('classifyUnsentIntent', () => {
  const unsent: BaseToIcpIntent = {
    v: 1,
    kind: 'base_to_icp',
    clientNonce: NONCE,
    transferId: TRANSFER_ID,
    from: ACCOUNT,
    amountE8s: AMOUNT,
    destinationPrincipal: ME,
    chainId: 8453,
    contract: CONTRACT,
    createdAt: 1,
    status: 'prepared',
  };
  const ctx = (over: Partial<Parameters<typeof classifyUnsentIntent>[1]> = {}) => ({
    walletAvailable: true,
    account: ACCOUNT as string | null,
    chainId: 8453 as number | null,
    used: false as boolean | null,
    ...over,
  });

  it('tracks anything with a stored hash or a used transfer id', () => {
    expect(classifyUnsentIntent({ ...unsent, status: 'sent', txHash: TX_HASH }, ctx({ walletAvailable: false, account: null, used: null }))).toEqual({ action: 'track' });
    expect(classifyUnsentIntent(unsent, ctx({ used: true, account: OTHER }))).toEqual({ action: 'track' });
  });

  it('resumes an unsent intent for the same account on the right network', () => {
    expect(classifyUnsentIntent(unsent, ctx())).toEqual({ action: 'resume' });
    expect(classifyUnsentIntent(unsent, ctx({ account: ACCOUNT.toUpperCase().replace('0X', '0x') }))).toEqual({ action: 'resume' });
  });

  it('shows a mismatch when another account is connected', () => {
    expect(classifyUnsentIntent(unsent, ctx({ account: OTHER }))).toEqual({ action: 'account_mismatch', expected: ACCOUNT });
  });

  it('asks for the wallet, a connection or the right network when it cannot check', () => {
    expect(classifyUnsentIntent(unsent, ctx({ walletAvailable: false, account: null, used: null }))).toEqual({ action: 'needs_wallet', reason: 'no_wallet' });
    expect(classifyUnsentIntent(unsent, ctx({ account: null, used: null }))).toEqual({ action: 'needs_wallet', reason: 'not_connected' });
    expect(classifyUnsentIntent(unsent, ctx({ chainId: 1, used: null }))).toEqual({ action: 'needs_wallet', reason: 'wrong_network' });
  });

  it('is unknown when bridgeOutUsed could not be read', () => {
    expect(classifyUnsentIntent(unsent, ctx({ used: null }))).toEqual({ action: 'unknown' });
  });
});

describe('discarding an unsent intent', () => {
  function unsentIntent(h: SendHarness): BaseToIcpIntent {
    const intent: BaseToIcpIntent = {
      v: 1,
      kind: 'base_to_icp',
      clientNonce: NONCE,
      transferId: TRANSFER_ID,
      from: ACCOUNT,
      amountE8s: AMOUNT,
      destinationPrincipal: ME,
      chainId: 8453,
      contract: CONTRACT,
      createdAt: 1,
      status: 'prepared',
    };
    h.store.put(intent);
    return intent;
  }

  it('removes the record only after the contract shows it was never sent', async () => {
    const h = harness();
    const intent = unsentIntent(h);
    expect(await discardUnsentIntent({ store: h.store, provider: h.chain.provider }, intent)).toBe('discarded');
    expect(h.store.list()).toEqual([]);
  });

  it('refuses when the transfer exists on chain', async () => {
    const h = harness();
    const intent = unsentIntent(h);
    h.chain.used.add(TRANSFER_ID);
    expect(await discardUnsentIntent({ store: h.store, provider: h.chain.provider }, intent)).toBe('sent_on_chain');
    expect(h.store.list()).toHaveLength(1);
  });

  it('refuses a sent intent outright', async () => {
    const h = harness();
    const intent = { ...unsentIntent(h), status: 'sent' as const, txHash: TX_HASH };
    expect(await discardUnsentIntent({ store: h.store, provider: h.chain.provider }, intent)).toBe('sent_on_chain');
    expect(h.store.list()).toHaveLength(1);
  });

  it('refuses when it cannot verify (wrong network, no wallet, unreadable)', async () => {
    const h = harness();
    const intent = unsentIntent(h);
    h.chain.provider.state.chainId = 1;
    expect(await discardUnsentIntent({ store: h.store, provider: h.chain.provider }, intent)).toBe('cannot_verify');
    expect(await discardUnsentIntent({ store: h.store, provider: null }, intent)).toBe('cannot_verify');
    h.chain.provider.state.chainId = 8453;
    h.chain.provider.handlers.eth_call = () => {
      throw new Error('rpc down');
    };
    expect(await discardUnsentIntent({ store: h.store, provider: h.chain.provider }, intent)).toBe('cannot_verify');
    expect(h.store.list()).toHaveLength(1);
  });
});

// ---------------------------------------------------------------------------
// State machine
// ---------------------------------------------------------------------------

describe('reduceBaseToIcp', () => {
  const intent: BaseToIcpIntent = {
    v: 1,
    kind: 'base_to_icp',
    clientNonce: NONCE,
    transferId: TRANSFER_ID,
    from: ACCOUNT,
    amountE8s: AMOUNT,
    destinationPrincipal: ME,
    chainId: 8453,
    contract: CONTRACT,
    createdAt: 1,
    status: 'prepared',
  };
  const run = (events: BaseToIcpEvent[], from: BaseToIcpState = INITIAL_BASE_TO_ICP_STATE) => events.reduce(reduceBaseToIcp, from);

  it('walks draft -> confirming -> awaiting wallet -> tracking', () => {
    expect(run([{ type: 'confirm', amountE8s: AMOUNT }])).toEqual({ kind: 'confirming', amountE8s: AMOUNT });
    expect(run([{ type: 'confirm', amountE8s: AMOUNT }, { type: 'wallet_prompt', intent }]).kind).toBe('awaiting_wallet');
    const sent = run([{ type: 'confirm', amountE8s: AMOUNT }, { type: 'wallet_prompt', intent }, { type: 'sent', intent: { ...intent, txHash: TX_HASH } }]);
    expect(sent).toMatchObject({ kind: 'tracking', tracking: { stage: 'submitted' } });
    const tracking: BaseOutTracking = { stage: 'awaiting_finality', label: 'x', detail: 'y', tone: 'pending', terminal: false };
    expect(reduceBaseToIcp(sent, { type: 'tracking_update', tracking })).toMatchObject({ tracking });
  });

  it('can go back from the confirmation step, and not from elsewhere', () => {
    const confirming = run([{ type: 'confirm', amountE8s: AMOUNT }]);
    expect(reduceBaseToIcp(confirming, { type: 'back' })).toEqual({ kind: 'draft' });
    const awaiting = reduceBaseToIcp(confirming, { type: 'wallet_prompt', intent });
    expect(reduceBaseToIcp(awaiting, { type: 'back' })).toBe(awaiting);
  });

  it('handles every send outcome', () => {
    const awaiting = run([{ type: 'confirm', amountE8s: AMOUNT }, { type: 'wallet_prompt', intent }]);
    expect(reduceBaseToIcp(awaiting, { type: 'rejected' })).toEqual({ kind: 'rejected' });
    expect(reduceBaseToIcp(awaiting, { type: 'aborted', reason: 'chain_changed', message: 'm' })).toMatchObject({ kind: 'aborted', reason: 'chain_changed' });
    expect(reduceBaseToIcp(awaiting, { type: 'failed', message: 'm', intent, uncertain: true })).toMatchObject({ kind: 'failed', uncertain: true });
    expect(reduceBaseToIcp(awaiting, { type: 'already_used', intent })).toMatchObject({ kind: 'tracking', tracking: { stage: 'awaiting_finality' } });
    expect(reduceBaseToIcp(awaiting, { type: 'blocked', message: 'no storage' })).toMatchObject({ kind: 'blocked' });
  });

  it('ignores a wallet outcome when no prompt is open', () => {
    expect(reduceBaseToIcp(INITIAL_BASE_TO_ICP_STATE, { type: 'rejected' })).toBe(INITIAL_BASE_TO_ICP_STATE);
    expect(reduceBaseToIcp(INITIAL_BASE_TO_ICP_STATE, { type: 'sent', intent })).toBe(INITIAL_BASE_TO_ICP_STATE);
    expect(reduceBaseToIcp(INITIAL_BASE_TO_ICP_STATE, { type: 'wallet_prompt', intent })).toBe(INITIAL_BASE_TO_ICP_STATE);
  });

  it('lets an uncertain send be resumed or dismissed, but not replaced by a new transfer', () => {
    const awaiting = run([{ type: 'confirm', amountE8s: AMOUNT }, { type: 'wallet_prompt', intent }]);
    const uncertain = reduceBaseToIcp(awaiting, { type: 'failed', message: 'm', intent, uncertain: true });
    expect(reduceBaseToIcp(uncertain, { type: 'confirm', amountE8s: AMOUNT })).toBe(uncertain);
    expect(reduceBaseToIcp(uncertain, { type: 'confirm', amountE8s: AMOUNT, resume: true })).toEqual({ kind: 'confirming', amountE8s: AMOUNT });
    expect(reduceBaseToIcp(uncertain, { type: 'dismiss' })).toEqual({ kind: 'draft' });
    // A prompt that is still open, or a transfer being tracked, cannot be dismissed.
    expect(reduceBaseToIcp(awaiting, { type: 'dismiss' })).toBe(awaiting);
    const tracking = run([{ type: 'track', intent }]);
    expect(reduceBaseToIcp(tracking, { type: 'dismiss' })).toBe(tracking);
  });

  it('does not reset while a wallet prompt is open', () => {
    const awaiting = run([{ type: 'confirm', amountE8s: AMOUNT }, { type: 'wallet_prompt', intent }]);
    expect(reduceBaseToIcp(awaiting, { type: 'reset' })).toBe(awaiting);
  });

  it('keeps tracking through an ICP session change, and clears everything else', () => {
    const tracking = run([{ type: 'track', intent: { ...intent, txHash: TX_HASH, status: 'sent' } }]);
    expect(reduceBaseToIcp(tracking, { type: 'session_changed' })).toBe(tracking);
    expect(reduceBaseToIcp(run([{ type: 'confirm', amountE8s: AMOUNT }]), { type: 'session_changed' })).toEqual({ kind: 'draft' });
  });

  it('starts tracking an existing transfer from a recovery action', () => {
    expect(run([{ type: 'track', intent }])).toMatchObject({ kind: 'tracking', tracking: { stage: 'awaiting_finality' } });
    expect(run([{ type: 'track', intent: { ...intent, txHash: TX_HASH } }])).toMatchObject({ tracking: { stage: 'submitted' } });
  });
});

// ---------------------------------------------------------------------------
// Tracking
// ---------------------------------------------------------------------------

describe('trackBaseToIcp: the canister decides', () => {
  const sentIntent: BaseToIcpIntent = {
    v: 1,
    kind: 'base_to_icp',
    clientNonce: NONCE,
    transferId: TRANSFER_ID,
    from: ACCOUNT,
    amountE8s: AMOUNT,
    destinationPrincipal: ME,
    chainId: 8453,
    contract: CONTRACT,
    createdAt: 1,
    status: 'sent',
    txHash: TX_HASH,
  };

  function track(h: SendHarness, signal?: AbortSignal) {
    const seen: BaseOutTracking[] = [];
    const done = trackBaseToIcp({ provider: h.chain.provider, reads: h.deps.reads, store: h.store, sleep: h.deps.sleep }, sentIntent, (t) => seen.push(t), signal);
    return { seen, done };
  }

  it('never reports released from a successful Base receipt alone', async () => {
    const h = harness();
    h.store.put(sentIntent);
    h.chain.provider.handlers.eth_getTransactionReceipt = () => ({ transactionHash: TX_HASH, blockNumber: '0x64', status: '0x1' });
    h.chain.used.add(TRANSFER_ID);
    const controller = new AbortController();
    let polls = 0;
    h.getRelease.mockImplementation(async () => {
      polls += 1;
      if (polls >= 3) controller.abort();
      return null;
    });
    const { seen, done } = track(h, controller.signal);
    await done;
    expect(seen.length).toBeGreaterThan(0);
    for (const update of seen) {
      expect(update.stage).toBe('awaiting_finality');
      expect(update.stage).not.toBe('released');
      expect(update.terminal).toBe(false);
    }
    expect(seen[0].includedBlock).toBe(100n);
    expect(h.store.get(TRANSFER_ID)).toMatchObject({ status: 'sent' });
  });

  it('follows the canister release record through to Released and then marks the intent done', async () => {
    const h = harness();
    h.store.put(sentIntent);
    h.chain.provider.handlers.eth_getTransactionReceipt = () => ({ transactionHash: TX_HASH, blockNumber: '0x64', status: '0x1' });
    const sequence: Array<ReleaseView | null> = [
      null,
      makeRelease({ status: { Pending: null } }),
      makeRelease({ status: { TransferUnknown: null } }),
      makeRelease({ status: { Released: null }, release_block_index: [901n], amount_to_receive_e8s: [249_990_000n] }),
    ];
    let i = 0;
    h.getRelease.mockImplementation(async () => sequence[Math.min(i++, sequence.length - 1)]);
    const { seen, done } = track(h);
    expect(await done).toBe('done');
    expect(seen.map((s) => s.stage)).toEqual(['awaiting_finality', 'release_pending', 'confirming', 'released']);
    expect(seen[3]).toMatchObject({ terminal: true, release: { releaseBlockIndex: 901n } });
    expect(h.store.get(TRANSFER_ID)).toMatchObject({ status: 'done' });
  });

  it('shows Held and re-minted outcomes', async () => {
    const h = harness();
    h.store.put(sentIntent);
    const sequence = [
      makeRelease({ status: { Held: { reason: 'destination rejected' } } }),
      makeRelease({ status: { Reminting: { deposit_id: 3n } } }),
      makeRelease({ status: { Reminted: { deposit_id: 3n } } }),
    ];
    let i = 0;
    h.getRelease.mockImplementation(async () => sequence[i++]);
    const { seen, done } = track(h);
    await done;
    expect(seen.map((s) => s.stage)).toEqual(['held', 'reminting', 'reminted']);
    expect(seen[0].release?.holdReason).toBe('destination rejected');
  });

  it('a wallet receipt failure does not hide the canister answer', async () => {
    const h = harness();
    h.store.put(sentIntent);
    h.chain.provider.handlers.eth_getTransactionReceipt = () => {
      throw new Error('rpc down');
    };
    h.getRelease.mockResolvedValue(makeRelease({ status: { Released: null } }));
    const { seen, done } = track(h);
    await done;
    expect(seen[0].stage).toBe('released');
  });

  it('works without a connected wallet, from the canister alone', async () => {
    const h = harness();
    h.store.put(sentIntent);
    h.getRelease.mockResolvedValue(makeRelease({ status: { Pending: null } }));
    const seen: BaseOutTracking[] = [];
    const controller = new AbortController();
    const done = trackBaseToIcp({ provider: null, reads: h.deps.reads, store: h.store, sleep: async () => controller.abort() }, sentIntent, (t) => seen.push(t), controller.signal);
    expect(await done).toBe('aborted');
    expect(seen[0].stage).toBe('release_pending');
  });

  it('does not read a receipt while the wallet is on another network', async () => {
    const h = harness();
    h.store.put(sentIntent);
    h.chain.provider.state.chainId = 1;
    h.getRelease.mockResolvedValue(makeRelease({ status: { Released: null } }));
    const handler = vi.fn(() => ({ transactionHash: TX_HASH, blockNumber: '0x64', status: '0x1' }));
    h.chain.provider.handlers.eth_getTransactionReceipt = handler;
    await track(h).done;
    expect(handler).not.toHaveBeenCalled();
  });

  it('reports a failed Base transaction only when the contract has no record of the transfer', async () => {
    const h = harness();
    h.store.put(sentIntent);
    h.chain.provider.handlers.eth_getTransactionReceipt = () => ({ transactionHash: TX_HASH, blockNumber: '0x64', status: '0x0' });
    const { seen, done } = track(h);
    await done;
    expect(seen[0]).toMatchObject({ stage: 'reverted', terminal: true });
  });
});

// ---------------------------------------------------------------------------
// Tell the bridge
// ---------------------------------------------------------------------------

describe('notifyBridge', () => {
  const sentIntent: BaseToIcpIntent = {
    v: 1,
    kind: 'base_to_icp',
    clientNonce: NONCE,
    transferId: TRANSFER_ID,
    from: ACCOUNT,
    amountE8s: AMOUNT,
    destinationPrincipal: ME,
    chainId: 8453,
    contract: CONTRACT,
    createdAt: 1,
    status: 'sent',
    txHash: TX_HASH,
  };

  it('reports the hash only when asked, and remembers that it did', async () => {
    const h = harness();
    h.store.put(sentIntent);
    const result = await notifyBridge(h.deps, sentIntent);
    expect(h.notify).toHaveBeenCalledWith(TX_HASH);
    expect(result).toMatchObject({ ok: true, intent: { notified: true } });
    expect(h.store.get(TRANSFER_ID)).toMatchObject({ notified: true });
  });

  it('never calls the bridge by itself while sending', async () => {
    const h = harness();
    await sendBridgeOut(h.deps, { amountE8s: AMOUNT }, h.emit);
    expect(h.notify).not.toHaveBeenCalled();
  });

  it('refuses without a hash or after a session change, and reports canister errors', async () => {
    const h = harness();
    expect(await notifyBridge(h.deps, { ...sentIntent, txHash: undefined })).toMatchObject({ ok: false });
    h.current.value = false;
    expect(await notifyBridge(h.deps, sentIntent)).toMatchObject({ ok: false });
    h.current.value = true;
    h.notify.mockResolvedValueOnce({ Err: { TooManyActiveOperations: null } });
    expect(await notifyBridge(h.deps, sentIntent)).toMatchObject({ ok: false });
    h.notify.mockRejectedValueOnce(new Error('timeout'));
    expect(await notifyBridge(h.deps, sentIntent)).toMatchObject({ ok: false });
    expect(h.notify).toHaveBeenCalledTimes(2);
  });

  it('hex helpers used by the page agree with the contract encoding', () => {
    expect(hexToBytes(NONCE)).toHaveLength(32);
  });
});
