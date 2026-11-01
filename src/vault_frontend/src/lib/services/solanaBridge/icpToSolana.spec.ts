import { Principal } from '@dfinity/principal';
import * as flows from './icpToSolana';
import { describe, expect, it, vi } from 'vitest';
import type { ApproveArgs } from '$declarations/icusd_ledger/icusd_ledger.did';
import type { BridgeToSolanaArgs, DepositView, Result_2 } from '$declarations/rumi_icusd_sol_bridge/rumi_icusd_sol_bridge.did';
import { formatE8s } from '../baseBridge/amounts';
import { NOT_SIGNED_NOTE } from '../baseBridge/signingGuard';
import { createMemoryStorage } from '../baseBridge/testSupport';
import { SolanaIntentStore, type SolDepositIntent } from './intents';
import {
  INITIAL_SOL_DEPOSIT_STATE,
  bridgeAfterApproval,
  bridgeToSolanaArgs,
  buildQuote,
  checkSolQuote,
  findDepositByRequestId,
  prepareIcpToSolana,
  reduceSolDeposit,
  requestSolDepositRefund,
  resumeIcpToSolana,
  retryIcpToSolana,
  solanaQuoteLimits,
  startIcpToSolana,
  trackSolDeposit,
  unresolvedDeposits,
  type SolDepositDeps,
  type SolDepositEvent,
  type SolDepositState,
} from './icpToSolana';
import { KEYS, SOL_BRIDGE_ID, TEST_OWNER_TEXT, makeSolDeposit, makeSolSigning, makeSolanaConfig } from './testSupport';

const OWNER = Principal.fromText(TEST_OWNER_TEXT);
const config = makeSolanaConfig();

function quote(overrides: Partial<Parameters<typeof buildQuote>[0]> = {}) {
  return buildQuote({
    amountE8s: 200_000_000n,
    recipient: KEYS.holder,
    ledgerFeeE8s: 10_000n,
    depositFeeE8s: 10_000_000n,
    allowanceE8s: 0n,
    allowanceExpiresAtNs: null,
    balanceE8s: 5_000_000_000n,
    nowMs: 1_700_000_000_000,
    ...overrides,
  });
}

interface Harness {
  deps: SolDepositDeps;
  store: SolanaIntentStore;
  events: SolDepositEvent[];
  emit: (e: SolDepositEvent) => void;
  approve: ReturnType<typeof vi.fn>;
  bridge: ReturnType<typeof vi.fn>;
  signing: ReturnType<typeof makeSolSigning>;
  session: { current: boolean };
  lookup: { value: DepositView | null | 'throw' };
}

function harness(options: { separateApproval?: boolean } = {}): Harness {
  const store = new SolanaIntentStore(createMemoryStorage(), TEST_OWNER_TEXT);
  const signing = makeSolSigning();
  const session = { current: true };
  const lookup: Harness['lookup'] = { value: null };
  const approve = vi.fn(async (_a: ApproveArgs) => ({ Ok: 1n }) as never);
  const bridge = vi.fn(async (_a: BridgeToSolanaArgs): Promise<Result_2> => ({ Ok: makeSolDeposit() }));
  let seed = 0;
  const deps: SolDepositDeps = {
    config,
    owner: OWNER,
    store,
    bridgeCanister: Principal.fromText(SOL_BRIDGE_ID),
    ledger: { approve: approve as never },
    bridge: { bridgeToSolana: bridge as never, requestDepositRefund: vi.fn(async () => ({ Ok: makeSolDeposit({ status: { Refunding: null } }) })) as never },
    reads: {
      getDeposit: async () => makeSolDeposit(),
      getDepositByRequest: async () => {
        if (lookup.value === 'throw') throw new Error('query failed');
        return lookup.value;
      },
    },
    separateApproval: options.separateApproval ?? false,
    isCurrent: () => session.current,
    signing,
    random: (n) => Uint8Array.from({ length: n }, (_, i) => (seed++ + i + 1) & 0xff),
    now: () => 1_700_000_000_000,
    sleep: async () => {},
  };
  const events: SolDepositEvent[] = [];
  return { deps, store, events, emit: (e) => events.push(e), approve, bridge, signing, session, lookup };
}

const types = (h: Harness) => h.events.map((e) => e.type);

describe('the quote and its limits', () => {
  it('reuses the Base arithmetic: amount plus ledger fee, fees apart, the deposit fee taken from the mint', () => {
    const q = quote();
    expect(q).toMatchObject({ requiredAllowanceE8s: 200_010_000n, mintAmountE8s: 190_000_000n, needsApproval: true, approvalFeeE8s: 10_000n, totalDebitE8s: 200_020_000n });
    expect(quote({ allowanceE8s: 200_010_000n, allowanceExpiresAtNs: null })).toMatchObject({ needsApproval: false, totalDebitE8s: 200_010_000n });
  });

  it('words the problems for Solana and checks them in the order a user would fix them', () => {
    const limits = solanaQuoteLimits(config);
    const fmt = (v: bigint) => formatE8s(v);
    expect(checkSolQuote(quote(), limits, fmt)).toBeNull();
    expect(checkSolQuote(quote(), { ...limits, halted: 'stop' }, fmt)?.message).toBe('The bridge is halted for safety: stop');
    expect(checkSolQuote(quote(), { ...limits, depositsPaused: true }, fmt)?.message).toBe('Deposits to Solana are paused right now. Your icUSD has not moved.');
    expect(checkSolQuote(quote({ amountE8s: 1n }), limits, fmt)?.code).toBe('below_minimum');
    expect(checkSolQuote(quote({ amountE8s: 2_000_000_000n }), limits, fmt)?.code).toBe('above_maximum');
    expect(checkSolQuote(quote({ amountE8s: 10_000_000n, depositFeeE8s: 10_000_000n }), { ...limits, minDepositE8s: 1n }, fmt)?.code).toBe('fee_exceeds_amount');
    expect(checkSolQuote(quote({ balanceE8s: 1n }), limits, fmt)?.code).toBe('insufficient_balance');
  });

  it('prepares from anonymous reads only', async () => {
    const reads = {
      fetchConfig: vi.fn(async () => ({ depositFeeE8s: 10_000_000n, bridgeLedgerFeeE8s: null, limits: solanaQuoteLimits(config) })),
      fetchLedgerFee: vi.fn(async () => 10_000n),
      readAllowance: vi.fn(async () => ({ allowance: 0n, expiresAtNs: null })),
      readBalance: vi.fn(async () => 5_000_000_000n),
      now: () => 1_700_000_000_000,
    };
    const prepared = await prepareIcpToSolana(reads, { amountE8s: 200_000_000n, recipient: KEYS.holder }, formatE8s);
    expect(prepared.problem).toBeNull();
    expect(prepared.quote.recipient).toBe(KEYS.holder);
  });
});

describe('the transfer', () => {
  it('saves the attempt first, approves exactly amount plus fee, then calls bridge_to_solana with the same request id (one click)', async () => {
    const h = harness();
    const seen: string[] = [];
    h.approve.mockImplementation(async (a: ApproveArgs) => {
      seen.push(`approve: ${h.store.list()[0].status} ${a.amount}`);
      return { Ok: 1n } as never;
    });
    h.bridge.mockImplementation(async (a: BridgeToSolanaArgs) => {
      seen.push(`bridge: ${h.store.list()[0].status}`);
      expect(a.solana_recipient).toBe(KEYS.holder);
      return { Ok: makeSolDeposit() };
    });
    await startIcpToSolana(h.deps, quote(), h.emit);
    expect(seen).toEqual(['approve: prepared 200010000', 'bridge: approved']);
    expect(types(h)).toEqual(['confirmed', 'approval_ok', 'submit_ok']);
    const intent = h.store.list()[0] as SolDepositIntent;
    expect(intent).toMatchObject({ destination: 'solana-mainnet', genesisHash: config.network.genesisHash, instance: SOL_BRIDGE_ID, mint: KEYS.mint, tokenProgram: 'token2022', recipient: KEYS.holder, amountE8s: 200_000_000n, expectedDepositFee: 10_000_000n, status: 'submitted', depositId: 7n });
    const args = h.bridge.mock.calls[0][0];
    expect(args.expected_deposit_fee_e8s).toBe(10_000_000n);
    expect(Array.from(args.client_request_id as Uint8Array)).toEqual(Array.from(bridgeToSolanaArgs(intent).client_request_id as Uint8Array));
    expect((args.client_request_id as Uint8Array).length).toBe(16);
  });

  it('does the approval and the transfer as two clicks for a popup signer, keeping the intent between them', async () => {
    const h = harness({ separateApproval: true });
    await startIcpToSolana(h.deps, quote(), h.emit);
    expect(types(h)).toEqual(['confirmed', 'approval_confirmed']);
    expect(h.bridge).not.toHaveBeenCalled();
    const intent = h.store.list()[0] as SolDepositIntent;
    expect(intent.status).toBe('approved');
    // A reload leaves it approved; the second click is the transfer's first signer call.
    await bridgeAfterApproval(h.deps, intent, h.emit);
    expect(h.approve).toHaveBeenCalledTimes(1);
    expect(h.bridge).toHaveBeenCalledTimes(1);
    expect(types(h).slice(-2)).toEqual(['bridge_started', 'submit_ok']);
  });

  it('skips the approval when the allowance already covers it', async () => {
    const h = harness();
    await startIcpToSolana(h.deps, quote({ allowanceE8s: 1_000_000_000n }), h.emit);
    expect(h.approve).not.toHaveBeenCalled();
    expect(h.bridge).toHaveBeenCalledTimes(1);
  });

  it('forgets the attempt when the approval is declined or fails, and tells the two apart', async () => {
    const h = harness();
    h.approve.mockRejectedValueOnce(new Error('User rejected the request.'));
    await startIcpToSolana(h.deps, quote(), h.emit);
    expect(h.events[h.events.length - 1]).toMatchObject({ type: 'approval_failed', declined: true });
    expect(h.store.list()).toHaveLength(0);
    expect(h.bridge).not.toHaveBeenCalled();
    h.approve.mockResolvedValueOnce({ Err: { InsufficientFunds: { balance: 1n } } } as never);
    await startIcpToSolana(h.deps, quote(), h.emit);
    expect(h.events[h.events.length - 1]).toMatchObject({ type: 'approval_failed', declined: false });
    expect(h.store.list()).toHaveLength(0);
  });

  it('resolves an uncertain result by looking the request up, and a retry reuses the SAME request id', async () => {
    const h = harness();
    h.bridge.mockRejectedValueOnce(new Error('network lost'));
    h.lookup.value = null;
    await startIcpToSolana(h.deps, quote({ allowanceE8s: 1_000_000_000n }), h.emit);
    expect(types(h)).toEqual(['confirmed', 'submit_uncertain', 'lookup_not_found']);
    const intent = h.store.list()[0] as SolDepositIntent;
    expect(intent.status).toBe('prepared');
    await retryIcpToSolana(h.deps, intent, h.emit);
    expect(h.bridge).toHaveBeenCalledTimes(2);
    expect(Array.from(h.bridge.mock.calls[1][0].client_request_id as Uint8Array)).toEqual(Array.from(h.bridge.mock.calls[0][0].client_request_id as Uint8Array));
    // The lookup that finds the deposit settles it without sending again.
    const g = harness();
    g.bridge.mockRejectedValueOnce(new Error('network lost'));
    g.lookup.value = makeSolDeposit({ client_request_id: g.deps.random!(16) });
    const idBytes = (): Uint8Array => g.bridge.mock.calls[0][0].client_request_id as Uint8Array;
    g.lookup.value = null;
    await startIcpToSolana(g.deps, quote({ allowanceE8s: 1_000_000_000n }), g.emit);
    g.lookup.value = makeSolDeposit({ client_request_id: idBytes() });
    await resumeIcpToSolana(g.deps, g.store.list()[0] as SolDepositIntent, g.emit);
    expect(g.events[g.events.length - 1]).toMatchObject({ type: 'lookup_found' });
    expect(g.bridge).toHaveBeenCalledTimes(1);
  });

  it('never treats an unreadable or a foreign answer as "not found"', async () => {
    const h = harness();
    h.lookup.value = 'throw';
    expect(await findDepositByRequestId(h.deps, 'ab'.repeat(16))).toBe('unavailable');
    h.lookup.value = makeSolDeposit({ client_request_id: new Uint8Array(16).fill(1) });
    expect(await findDepositByRequestId(h.deps, 'ab'.repeat(16))).toBe('unavailable');
    h.lookup.value = makeSolDeposit({ client_request_id: new Uint8Array(16).fill(0xab), owner: Principal.fromText('aaaaa-aa') });
    expect(await findDepositByRequestId(h.deps, 'ab'.repeat(16))).toBe('unavailable');
    h.lookup.value = makeSolDeposit({ client_request_id: new Uint8Array(16).fill(0xab) });
    expect(await findDepositByRequestId(h.deps, 'ab'.repeat(16))).toMatchObject({ id: 7n });
  });

  it('maps a rejected request to a plain message, with a re-approval offer when the allowance is short', async () => {
    const h = harness();
    h.bridge.mockResolvedValueOnce({ Err: { InsufficientAllowance: { allowance_e8s: 1n, required_e8s: 2n } } });
    await startIcpToSolana(h.deps, quote({ allowanceE8s: 1_000_000_000n }), h.emit);
    expect(h.events[h.events.length - 1]).toMatchObject({ type: 'submit_rejected', reapprove: true });
    expect(h.store.list()[0]).toMatchObject({ status: 'failed', failure: 'InsufficientAllowance' });
    const g = harness();
    g.bridge.mockResolvedValueOnce({ Err: { FeeChanged: { current_fee_e8s: 20_000_000n } } });
    await startIcpToSolana(g.deps, quote({ allowanceE8s: 1_000_000_000n }), g.emit);
    expect(g.events[g.events.length - 1]).toMatchObject({ type: 'submit_fee_changed', currentFeeE8s: 20_000_000n });
    const r = harness();
    r.bridge.mockResolvedValueOnce({ Err: { InvalidRecipient: 'a program id' } });
    await startIcpToSolana(r.deps, quote({ allowanceE8s: 1_000_000_000n }), r.emit);
    expect(r.events[r.events.length - 1]).toMatchObject({ type: 'submit_rejected', message: 'That Solana recipient is not valid: a program id' });
    // An internal error may have created the deposit: it is looked up, not called rejected.
    const u = harness();
    u.bridge.mockResolvedValueOnce({ Err: { Internal: 'trap' } });
    await startIcpToSolana(u.deps, quote({ allowanceE8s: 1_000_000_000n }), u.emit);
    expect(types(u)).toEqual(['confirmed', 'submit_uncertain', 'lookup_not_found']);
  });
});

describe('what is refused before any signer call', () => {
  it('asks no signer and saves nothing for a disabled destination, a bad recipient or an unresolved attempt', async () => {
    const h = harness();
    h.signing.fresh.value = { status: 'disabled', reason: 'Not available yet.' };
    await startIcpToSolana(h.deps, quote(), h.emit);
    expect(h.events[0]).toMatchObject({ type: 'blocked', message: `Not available yet. ${NOT_SIGNED_NOTE}` });
    h.signing.fresh.value = null;
    await startIcpToSolana(h.deps, quote(), h.emit);
    expect(h.events[1]).toMatchObject({ type: 'blocked' });
    h.signing.fresh.value = { status: 'enabled', reason: null };
    await startIcpToSolana(h.deps, quote({ recipient: config.mint }), h.emit);
    expect(h.events[2]).toMatchObject({ type: 'blocked', message: expect.stringContaining('program or one of the bridge') });
    expect(h.approve).not.toHaveBeenCalled();
    expect(h.bridge).not.toHaveBeenCalled();
    expect(h.store.list()).toHaveLength(0);
    // An unresolved earlier attempt blocks a new one.
    h.store.put({ v: 1, kind: 'icp_to_solana', id: '55'.repeat(16), destination: 'solana-mainnet', genesisHash: config.network.genesisHash, instance: SOL_BRIDGE_ID, mint: KEYS.mint, tokenProgram: 'token2022', recipient: KEYS.holder, amountE8s: 1n, expectedDepositFee: 0n, createdAt: 1, status: 'approved' });
    expect(unresolvedDeposits(h.store, 'solana-mainnet')).toHaveLength(1);
    await startIcpToSolana(h.deps, quote(), h.emit);
    expect(h.events[3]).toMatchObject({ type: 'blocked', message: expect.stringContaining('no recorded outcome') });
    expect(h.approve).not.toHaveBeenCalled();
    h.session.current = false;
    h.store.remove('55'.repeat(16));
    await startIcpToSolana(h.deps, quote(), h.emit);
    expect(h.events[4]).toEqual({ type: 'session_changed' });
  });

  it('refuses a request saved for another network without sending or rewriting it', async () => {
    const h = harness({ separateApproval: true });
    await startIcpToSolana(h.deps, quote(), h.emit);
    const intent = { ...(h.store.list()[0] as SolDepositIntent), destination: 'solana-devnet' };
    await bridgeAfterApproval(h.deps, intent, h.emit);
    expect(h.events[h.events.length - 1]).toMatchObject({ type: 'blocked', message: expect.stringContaining('another destination') });
    expect(h.bridge).not.toHaveBeenCalled();
  });
});

describe('an unanswered bridge request is never forgotten', () => {
  const hex = (bytes: ArrayLike<number>) => Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');

  it('offers no way to forget a request that may have been submitted', () => {
    expect(flows).not.toHaveProperty('discardDeposit');
  });

  it('keeps the original under its own id: delayed old update, absent query, blocked fresh start, same-id retry, no second request', async () => {
    const h = harness();
    // The bridge call is lost: the ingress is still pending when the page asks.
    h.bridge.mockRejectedValueOnce(new Error('ingress timed out'));
    await startIcpToSolana(h.deps, quote({ allowanceE8s: 1_000_000_000n }), h.emit);
    expect(types(h)).toContain('lookup_not_found');
    const original = h.store.list()[0] as SolDepositIntent;
    expect(h.bridge).toHaveBeenCalledTimes(1);

    // The user tries a fresh start with a new request id: refused, nothing is sent, nothing is forgotten.
    await startIcpToSolana(h.deps, quote({ allowanceE8s: 1_000_000_000n }), h.emit);
    expect(h.events[h.events.length - 1]).toMatchObject({ type: 'blocked' });
    expect(h.bridge).toHaveBeenCalledTimes(1);
    expect(h.store.list()).toHaveLength(1);

    // The old update executes late. Retrying under the SAME id returns the existing deposit.
    h.bridge.mockImplementationOnce(async (args: BridgeToSolanaArgs) => ({ Ok: makeSolDeposit({ client_request_id: args.client_request_id }) }));
    await retryIcpToSolana(h.deps, original, h.emit);
    expect(h.events[h.events.length - 1]).toMatchObject({ type: 'submit_ok' });
    const ids = h.bridge.mock.calls.map((call) => hex((call[0] as BridgeToSolanaArgs).client_request_id));
    expect(ids).toEqual([original.id, original.id]);
    expect(h.approve).not.toHaveBeenCalled();
    expect(h.store.list()).toHaveLength(1);
    expect(h.store.list()[0]).toMatchObject({ id: original.id, status: 'submitted' });
  });
});

describe('tracking and refund', () => {
  it('follows the deposit to its final state and marks the intent done', async () => {
    const h = harness();
    await startIcpToSolana(h.deps, quote({ allowanceE8s: 1_000_000_000n }), h.emit);
    const states = [makeSolDeposit({ status: { Escrowed: null } }), makeSolDeposit({ status: { MintInFlight: null } }), makeSolDeposit({ status: { Delivered: null }, delivered: [{ signature: 'DeliveredSig', slot: 5n }] })];
    let i = 0;
    h.deps.reads.getDeposit = async () => states[Math.min(i++, states.length - 1)];
    const seen: string[] = [];
    expect(await trackSolDeposit(h.deps, 7n, (d) => seen.push(Object.keys(d.status)[0]))).toBe('done');
    expect(seen).toEqual(['Escrowed', 'MintInFlight', 'Delivered']);
    expect(h.store.list()[0].status).toBe('done');
  });

  it('offers a refund only to the owner of a held deposit, through the ICP signer, and not for a disabled destination', async () => {
    const h = harness();
    const held = makeSolDeposit({ status: { MintHeld: { reason: 'mints paused' } } });
    expect(await requestSolDepositRefund(h.deps, held)).toMatchObject({ ok: true });
    expect(await requestSolDepositRefund(h.deps, makeSolDeposit({ status: { Delivered: null } }))).toMatchObject({ ok: false });
    expect(await requestSolDepositRefund(h.deps, makeSolDeposit({ status: { MintHeld: { reason: 'x' } }, owner: Principal.fromText('aaaaa-aa') }))).toMatchObject({ ok: false });
    h.signing.fresh.value = { status: 'disabled', reason: 'off' };
    expect(await requestSolDepositRefund(h.deps, held)).toMatchObject({ ok: false, message: `off ${NOT_SIGNED_NOTE}` });
  });
});

describe('the state machine', () => {
  it('moves through the same states as the Base flow and keeps in-flight work on screen', () => {
    let s: SolDepositState = INITIAL_SOL_DEPOSIT_STATE;
    const intent = { id: 'x' } as SolDepositIntent;
    s = reduceSolDeposit(s, { type: 'quote_started' });
    expect(s.kind).toBe('quoting');
    s = reduceSolDeposit(s, { type: 'quote_ready', quote: quote(), problem: null });
    expect(s.kind).toBe('review');
    s = reduceSolDeposit(s, { type: 'confirmed', quote: quote(), intent, needsApproval: true });
    expect(s.kind).toBe('approving');
    expect(reduceSolDeposit(s, { type: 'edit' })).toBe(s);
    s = reduceSolDeposit(s, { type: 'approval_confirmed' });
    expect(s.kind).toBe('approved');
    s = reduceSolDeposit(s, { type: 'bridge_started' });
    expect(s.kind).toBe('submitting');
    s = reduceSolDeposit(s, { type: 'submit_uncertain', message: 'm' });
    expect(s).toMatchObject({ kind: 'uncertain', lookup: 'looking' });
    expect(reduceSolDeposit(s, { type: 'retry' })).toBe(s);
    s = reduceSolDeposit(s, { type: 'lookup_not_found' });
    expect(reduceSolDeposit(s, { type: 'retry' }).kind).toBe('submitting');
    expect(reduceSolDeposit({ kind: 'draft' }, { type: 'submit_fee_changed', currentFeeE8s: 5n }).kind).toBe('draft');
    expect(reduceSolDeposit({ kind: 'draft' }, { type: 'blocked', message: 'm' })).toEqual({ kind: 'error', message: 'm' });
  });
});
