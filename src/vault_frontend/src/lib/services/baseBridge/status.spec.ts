import { describe, expect, it, vi } from 'vitest';
import { Principal } from '@dfinity/principal';
import type { BridgeError } from '$declarations/rumi_icusd_bridge/rumi_icusd_bridge.did';
import {
  FINALITY_NOTE,
  abortableSleep,
  bridgeErrorKind,
  classifyBridgeError,
  deriveBaseOutTracking,
  describeDeposit,
  describeRelease,
  formatApproveError,
  formatBridgeError,
  loadBridgeHistory,
  mergeReleases,
  pollDelayMs,
  pollWithBackoff,
  shortAddress,
  shortHash,
} from './status';
import { TEST_OWNER, makeDeposit, makeRelease } from './testSupport';

const ME = TEST_OWNER.toText();
const HASH = `0x${'12'.repeat(32)}`;

describe('describeDeposit: every status maps to one user state', () => {
  it('PullPending is "Pulling icUSD"', () => {
    expect(describeDeposit(makeDeposit({ status: { PullPending: null } }))).toMatchObject({
      stage: 'pulling',
      label: 'Pulling icUSD',
      tone: 'pending',
      terminal: false,
      canRefund: false,
    });
  });

  it('PullUnknown reads as confirming, never as failure', () => {
    const d = describeDeposit(makeDeposit({ status: { PullUnknown: null } }));
    expect(d).toMatchObject({ stage: 'pull_unknown', terminal: false, tone: 'pending' });
    expect(d.detail).toMatch(/cannot be charged twice/);
  });

  it('PullFailed says no funds moved and is terminal', () => {
    const d = describeDeposit(makeDeposit({ status: { PullFailed: { reason: 'insufficient allowance' } } }));
    expect(d).toMatchObject({ stage: 'pull_failed', terminal: true, tone: 'error', canRefund: false });
    expect(d.label).toContain('no funds moved');
    expect(d.detail).toBe('insufficient allowance');
  });

  it('Escrowed is escrowed with a queued mint', () => {
    expect(describeDeposit(makeDeposit({ status: { Escrowed: null } }))).toMatchObject({ stage: 'escrowed', terminal: false });
  });

  it('MintInFlight without an included transaction is "Minting on Base"', () => {
    for (const mint_stage of [[], [{ Queued: null }], [{ Submitted: null }]] as const) {
      expect(describeDeposit(makeDeposit({ status: { MintInFlight: null }, mint_stage: mint_stage as any }))).toMatchObject({
        stage: 'minting',
        label: 'Minting on Base',
        terminal: false,
        finalized: false,
      });
    }
  });

  it('MintInFlight included shows the block and is not final', () => {
    const d = describeDeposit(makeDeposit({ status: { MintInFlight: null }, mint_stage: [{ Included: { block_number: 123n } }] }));
    expect(d).toMatchObject({ stage: 'included', blockNumber: 123n, finalized: false, terminal: false });
    expect(d.label).toBe('Included on Base (block 123)');
    expect(d.detail).toContain(FINALITY_NOTE);
  });

  it('a finalized mint that the canister has not yet marked Delivered is still not "Delivered"', () => {
    const d = describeDeposit(
      makeDeposit({ status: { MintInFlight: null }, mint_stage: [{ Finalized: { block_number: 9n, success: true } }] }),
    );
    expect(d).toMatchObject({ stage: 'included', finalized: false, terminal: false });
  });

  it('a reverted attempt stays "Minting" with a reassuring note', () => {
    const d = describeDeposit(
      makeDeposit({ status: { MintInFlight: null }, mint_stage: [{ Finalized: { block_number: 9n, success: false } }] }),
    );
    expect(d.stage).toBe('minting');
    expect(d.detail).toMatch(/did not succeed/);
  });

  it('Delivered is final and terminal, with the delivery block', () => {
    const d = describeDeposit(
      makeDeposit({
        status: { Delivered: null },
        delivered: [{ block_number: 77n, block_hash: '0x1', tx_hash: HASH }],
      }),
    );
    expect(d).toMatchObject({ stage: 'delivered', finalized: true, terminal: true, tone: 'success', blockNumber: 77n });
    expect(d.detail).toContain('24.9999 icUSD');
  });

  it('MintHeld shows the reason', () => {
    const d = describeDeposit(makeDeposit({ status: { MintHeld: { reason: 'mint paused' } } }), { principal: ME });
    expect(d).toMatchObject({ stage: 'held', holdReason: 'mint paused', tone: 'warning', terminal: false });
    expect(d.detail).toContain('mint paused');
  });

  it('Refunding and Refunded', () => {
    expect(describeDeposit(makeDeposit({ status: { Refunding: null } }))).toMatchObject({ stage: 'refunding', terminal: false });
    const done = describeDeposit(makeDeposit({ status: { Refunded: null }, refund_amount_e8s: [2_499_900_000n] }));
    expect(done).toMatchObject({ stage: 'refunded', terminal: true });
    expect(done.detail).toContain('24.999 icUSD');
  });
});

describe('describeDeposit: refund is offered only when held and owned', () => {
  const held = { MintHeld: { reason: 'cap reached' } };

  it('is true for the owner of a held user deposit', () => {
    expect(describeDeposit(makeDeposit({ status: held }), { principal: ME }).canRefund).toBe(true);
  });

  it('is false for another principal or when signed out', () => {
    expect(describeDeposit(makeDeposit({ status: held }), { principal: 'aaaaa-aa' }).canRefund).toBe(false);
    expect(describeDeposit(makeDeposit({ status: held }), { principal: null }).canRefund).toBe(false);
    expect(describeDeposit(makeDeposit({ status: held })).canRefund).toBe(false);
  });

  it('is false for a re-mint deposit even when held', () => {
    const remint = makeDeposit({ status: held, kind: { Remint: { transfer_id: HASH } } });
    expect(describeDeposit(remint, { principal: ME }).canRefund).toBe(false);
  });

  it('is false in every state other than MintHeld', () => {
    for (const status of [
      { PullPending: null },
      { PullUnknown: null },
      { PullFailed: { reason: 'x' } },
      { Escrowed: null },
      { MintInFlight: null },
      { Delivered: null },
      { Refunding: null },
      { Refunded: null },
    ] as const) {
      expect(describeDeposit(makeDeposit({ status: status as any }), { principal: ME }).canRefund, JSON.stringify(status)).toBe(false);
    }
  });
});

describe('describeRelease', () => {
  it('maps each canister status', () => {
    expect(describeRelease(makeRelease({ status: { Pending: null } }))).toMatchObject({ stage: 'release_pending', terminal: false });
    expect(describeRelease(makeRelease({ status: { TransferUnknown: null } }))).toMatchObject({ stage: 'confirming', label: 'Confirming', terminal: false });
    expect(describeRelease(makeRelease({ status: { Held: { reason: 'bad principal' } } }))).toMatchObject({
      stage: 'held',
      holdReason: 'bad principal',
      terminal: false,
    });
    expect(describeRelease(makeRelease({ status: { Reminting: { deposit_id: 4n } } }))).toMatchObject({ stage: 'reminting', terminal: false });
    expect(describeRelease(makeRelease({ status: { Reminted: { deposit_id: 4n } } }))).toMatchObject({ stage: 'reminted', terminal: true });
  });

  it('Released carries the ledger block index and the received amount', () => {
    const d = describeRelease(
      makeRelease({ status: { Released: null }, release_block_index: [555n], amount_to_receive_e8s: [99_990_000n] }),
    );
    expect(d).toMatchObject({ stage: 'released', terminal: true, tone: 'success', releaseBlockIndex: 555n });
    expect(d.label).toBe('Released (ledger block 555)');
    expect(d.detail).toContain('0.9999 icUSD');
  });
});

describe('deriveBaseOutTracking: the canister decides, the wallet only hints', () => {
  const included = { transactionHash: HASH, blockNumber: 4242n, success: true };
  const failed = { transactionHash: HASH, blockNumber: 4242n, success: false };

  it('shows "submitted" while nothing is included yet', () => {
    expect(deriveBaseOutTracking({ receipt: null, release: null, transferUsed: false })).toMatchObject({ stage: 'submitted', terminal: false });
    expect(deriveBaseOutTracking({ receipt: null, release: null, transferUsed: null })).toMatchObject({ stage: 'submitted' });
  });

  it('never reports released from a successful Base receipt alone', () => {
    const t = deriveBaseOutTracking({ receipt: included, release: null, transferUsed: true });
    expect(t.stage).toBe('awaiting_finality');
    expect(t.stage).not.toBe('released');
    expect(t.terminal).toBe(false);
    expect(t.includedBlock).toBe(4242n);
    expect(t.detail).toContain('block 4242');
    expect(t.detail).toContain(FINALITY_NOTE);
  });

  it('treats bridgeOutUsed=true without a receipt as awaiting finality', () => {
    const t = deriveBaseOutTracking({ receipt: null, release: null, transferUsed: true });
    expect(t.stage).toBe('awaiting_finality');
    expect(t.includedBlock).toBeUndefined();
  });

  it('reports a reverted receipt as failed only when the contract has no record of the transfer', () => {
    expect(deriveBaseOutTracking({ receipt: failed, release: null, transferUsed: false })).toMatchObject({ stage: 'reverted', terminal: true, tone: 'error' });
    expect(deriveBaseOutTracking({ receipt: failed, release: null, transferUsed: true }).stage).toBe('awaiting_finality');
  });

  it('follows the canister release record once it exists', () => {
    const cases: Array<[any, string, boolean]> = [
      [{ Pending: null }, 'release_pending', false],
      [{ TransferUnknown: null }, 'confirming', false],
      [{ Released: null }, 'released', true],
      [{ Held: { reason: 'x' } }, 'held', false],
      [{ Reminting: { deposit_id: 1n } }, 'reminting', false],
      [{ Reminted: { deposit_id: 1n } }, 'reminted', true],
    ];
    for (const [status, stage, terminal] of cases) {
      const t = deriveBaseOutTracking({ receipt: included, release: makeRelease({ status }), transferUsed: true });
      expect(t.stage).toBe(stage);
      expect(t.terminal).toBe(terminal);
      expect(t.release).toBeDefined();
    }
  });

  it('prefers the canister over a contradicting failed receipt', () => {
    const t = deriveBaseOutTracking({ receipt: failed, release: makeRelease({ status: { Released: null } }), transferUsed: true });
    expect(t.stage).toBe('released');
  });
});

describe('bridge errors', () => {
  const cases: Array<[BridgeError, string, 'no_effect' | 'uncertain', RegExp]> = [
    [{ Rpc: 'timeout' }, 'Rpc', 'uncertain', /could not reach Base/],
    [{ Internal: 'boom' }, 'Internal', 'uncertain', /internal error/],
    [{ Anonymous: null }, 'Anonymous', 'no_effect', /Sign in/],
    [{ Paused: 'deposits are paused' }, 'Paused', 'no_effect', /paused/],
    [{ AmountBelowMinimum: { min_e8s: 100_000_000n } }, 'AmountBelowMinimum', 'no_effect', /minimum amount is 1 icUSD/],
    [{ AmountAboveMaximum: { max_e8s: 5_000_000_000n } }, 'AmountAboveMaximum', 'no_effect', /maximum amount per transfer is 50 icUSD/],
    [{ CapacityExceeded: { available_e8s: 150_000_000n } }, 'CapacityExceeded', 'no_effect', /1\.5 icUSD more/],
    [{ NotFound: null }, 'NotFound', 'uncertain', /no record/],
    [{ TooManyActiveOperations: null }, 'TooManyActiveOperations', 'no_effect', /in progress/],
    [{ Unauthorized: null }, 'Unauthorized', 'no_effect', /not allowed/],
    [{ NotOwner: null }, 'NotOwner', 'uncertain', /Only the account/],
    [{ InvalidRecipient: 'zero address' }, 'InvalidRecipient', 'no_effect', /zero address/],
    [{ RateLimited: { window_resets_at_ns: 1_700_000_000_000_000_000n, available_e8s: 0n } }, 'RateLimited', 'no_effect', /rolling limit/],
    [{ NotConfigured: 'Base contract not bound' }, 'NotConfigured', 'no_effect', /not fully configured/],
    [{ InvalidArgument: 'bad' }, 'InvalidArgument', 'no_effect', /rejected the request: bad/],
    [{ Ledger: 'BadFee' }, 'Ledger', 'uncertain', /ledger reported/],
    [{ Halted: 'coverage' }, 'Halted', 'no_effect', /halted/],
    [{ RequestIdConflict: null }, 'RequestIdConflict', 'no_effect', /different bridge request/],
    [{ InvalidState: 'not held' }, 'InvalidState', 'uncertain', /not available in the current state/],
    [{ FeeChanged: { current_fee_e8s: 20_000n } }, 'FeeChanged', 'no_effect', /fee changed to 0\.0002 icUSD/],
    [
      { InsufficientAllowance: { allowance_e8s: 1_000_000n, required_e8s: 2_500_100_000n } },
      'InsufficientAllowance',
      'no_effect',
      /allowance for the bridge is 0\.01 icUSD, but 25\.001 icUSD is needed\. Approve again/,
    ],
    [
      { InsufficientFunds: { balance_e8s: 5_000_000n, required_e8s: 2_500_100_000n } },
      'InsufficientFunds',
      'no_effect',
      /balance is 0\.05 icUSD, but 25\.001 icUSD is needed/,
    ],
  ];

  it.each(cases)('formats and classifies variant #%#', (error, kind, classification, pattern) => {
    expect(bridgeErrorKind(error)).toBe(kind);
    expect(classifyBridgeError(error)).toBe(classification);
    expect(formatBridgeError(error)).toMatch(pattern);
  });

  it('covers every BridgeError variant in the Candid interface', () => {
    // Guards against a new variant silently falling through to the generic message.
    const covered = new Set(cases.map(([, kind]) => kind));
    expect(covered.size).toBe(22);
  });

  it('formats ICRC-2 approve errors', () => {
    expect(formatApproveError({ InsufficientFunds: { balance: 5_000n } })).toMatch(/balance \(0\.00005\)/);
    expect(formatApproveError({ BadFee: { expected_fee: 20_000n } })).toMatch(/now 0\.0002 icUSD/);
    expect(formatApproveError({ TemporarilyUnavailable: null })).toMatch(/busy/);
    expect(formatApproveError({ GenericError: { message: 'nope', error_code: 1n } })).toMatch(/nope/);
    expect(formatApproveError({ Expired: { ledger_time: 1n } })).toMatch(/device clock/);
    expect(formatApproveError(undefined)).toMatch(/did not accept/);
  });
});

describe('polling', () => {
  it('backs off from 2s, growing by half each time, capped at 30s', () => {
    expect([0, 1, 2, 3].map((n) => pollDelayMs(n))).toEqual([2_000, 3_000, 4_500, 6_750]);
    expect(pollDelayMs(50)).toBe(30_000);
    expect(pollDelayMs(2, { baseMs: 100, maxMs: 150 })).toBe(150);
    expect(pollDelayMs(-3)).toBe(2_000);
  });

  it('polls until done, sleeping with the backoff schedule', async () => {
    const values = ['a', 'b', 'c', 'done'];
    const seen: string[] = [];
    const sleeps: number[] = [];
    let i = 0;
    const result = await pollWithBackoff({
      fetch: async () => values[i++],
      isDone: (v) => v === 'done',
      onUpdate: (v) => seen.push(v),
      sleep: async (ms) => void sleeps.push(ms),
    });
    expect(result).toBe('done');
    expect(seen).toEqual(['a', 'b', 'c', 'done']);
    expect(sleeps).toEqual([2_000, 3_000, 4_500]);
  });

  it('reports errors and keeps polling', async () => {
    const onError = vi.fn();
    let calls = 0;
    const result = await pollWithBackoff({
      fetch: async () => {
        calls += 1;
        if (calls < 3) throw new Error('offline');
        return 'ok';
      },
      isDone: () => true,
      onUpdate: () => {},
      onError,
      sleep: async () => {},
    });
    expect(result).toBe('done');
    expect(onError).toHaveBeenCalledTimes(2);
  });

  it('stops when aborted, even mid-fetch, and drops the late result', async () => {
    const controller = new AbortController();
    const onUpdate = vi.fn();
    const result = await pollWithBackoff({
      fetch: async () => {
        controller.abort();
        return 'late';
      },
      isDone: () => false,
      onUpdate,
      sleep: async () => {},
      signal: controller.signal,
    });
    expect(result).toBe('aborted');
    expect(onUpdate).not.toHaveBeenCalled();
  });

  it('gives up after maxAttempts', async () => {
    const fetch = vi.fn(async () => 'x');
    const result = await pollWithBackoff({ fetch, isDone: () => false, onUpdate: () => {}, sleep: async () => {}, maxAttempts: 3 });
    expect(result).toBe('exhausted');
    expect(fetch).toHaveBeenCalledTimes(3);
  });

  it('abortableSleep wakes early on abort', async () => {
    vi.useFakeTimers();
    try {
      const controller = new AbortController();
      let woke = false;
      const sleeping = abortableSleep(60_000, controller.signal).then(() => (woke = true));
      await Promise.resolve();
      expect(woke).toBe(false);
      controller.abort();
      await sleeping;
      expect(woke).toBe(true);
      await abortableSleep(10, controller.signal); // already aborted: resolves at once
      const timed = abortableSleep(1_000);
      vi.advanceTimersByTime(1_000);
      await timed;
    } finally {
      vi.useRealTimers();
    }
  });
});

describe('formatting helpers', () => {
  it('shortens hashes and addresses', () => {
    expect(shortHash(HASH)).toBe('0x12121212...121212');
    expect(shortHash('0x1234')).toBe('0x1234');
    expect(shortAddress('0x1111111111111111111111111111111111111111')).toBe('0x1111...1111');
    expect(Principal.fromText(ME).toText()).toBe(ME);
  });
});

describe('mergeReleases', () => {
  const id = (byte: string) => `0x${byte.repeat(32)}`;

  it('keeps one row per transfer id, newest first', () => {
    const a = makeRelease({ transfer_id: id('aa'), created_at_ns: 10n });
    const b = makeRelease({ transfer_id: id('bb'), created_at_ns: 30n });
    const c = makeRelease({ transfer_id: id('cc'), created_at_ns: 20n });
    const merged = mergeReleases([a, b], [c, { ...a }]);
    expect(merged.map((r) => r.transfer_id)).toEqual([id('bb'), id('cc'), id('aa')]);
  });

  it('prefers the more recently updated record when listings disagree', () => {
    const stale = makeRelease({ transfer_id: id('aa'), status: { Held: { reason: 'bad principal' } }, updated_at_ns: 5n });
    const fresh = makeRelease({ transfer_id: id('aa'), status: { Reminting: { deposit_id: 1n } }, updated_at_ns: 9n });
    expect(mergeReleases([fresh], [stale])[0].status).toEqual({ Reminting: { deposit_id: 1n } });
    expect(mergeReleases([stale], [fresh])[0].status).toEqual({ Reminting: { deposit_id: 1n } });
  });

  it('matches transfer ids case-insensitively', () => {
    const lower = makeRelease({ transfer_id: `0x${'ab'.repeat(32)}` });
    const upper = makeRelease({ transfer_id: `0x${'AB'.repeat(32)}` });
    expect(mergeReleases([lower], [upper])).toHaveLength(1);
  });
});

describe('loadBridgeHistory (public queries only)', () => {
  const SENDER = '0x1111111111111111111111111111111111111111';
  function reader() {
    return {
      list_deposits_for_owner: vi.fn(async () => [makeDeposit({ id: 1n }), makeDeposit({ id: 3n }), makeDeposit({ id: 2n })]),
      list_releases_for_owner: vi.fn(async () => [makeRelease({ transfer_id: `0x${'aa'.repeat(32)}`, created_at_ns: 10n })]),
      list_releases_for_base_sender: vi.fn(async () => [
        makeRelease({ transfer_id: `0x${'aa'.repeat(32)}`, created_at_ns: 10n }),
        makeRelease({ transfer_id: `0x${'bb'.repeat(32)}`, status: { Held: { reason: 'invalid destination' } }, created_at_ns: 20n, destination: [] }),
      ]),
    };
  }

  it('lists deposits and releases for the owner with the documented arguments', async () => {
    const r = reader();
    const history = await loadBridgeHistory(r as any, { owner: TEST_OWNER, baseSender: null });
    expect(r.list_deposits_for_owner).toHaveBeenCalledWith({ owner: TEST_OWNER, start_after: [], limit: 20 });
    expect(r.list_releases_for_owner).toHaveBeenCalledWith({ owner: TEST_OWNER, start_after_sequence: [], limit: 20 });
    expect(r.list_releases_for_base_sender).not.toHaveBeenCalled();
    expect(history.deposits.map((d) => d.id)).toEqual([3n, 2n, 1n]);
    expect(history.releases).toHaveLength(1);
    expect(history.senderListingFailed).toBe(false);
  });

  it('adds releases sent from the connected Base address and dedupes by transfer id (held claims appear)', async () => {
    const r = reader();
    const history = await loadBridgeHistory(r as any, { owner: TEST_OWNER, baseSender: SENDER.toUpperCase().replace('0X', '0x') });
    expect(r.list_releases_for_base_sender).toHaveBeenCalledWith({ base_sender: SENDER, start_after_sequence: [], limit: 20 });
    expect(history.releases.map((x) => x.transfer_id)).toEqual([`0x${'bb'.repeat(32)}`, `0x${'aa'.repeat(32)}`]);
    expect(history.releases[0].status).toEqual({ Held: { reason: 'invalid destination' } });
  });

  it('keeps the owner history when only the sender listing fails, and says so', async () => {
    const r = reader();
    r.list_releases_for_base_sender.mockRejectedValue(new Error('offline'));
    const history = await loadBridgeHistory(r as any, { owner: TEST_OWNER, baseSender: SENDER });
    expect(history.releases).toHaveLength(1);
    expect(history.senderListingFailed).toBe(true);
  });

  it('fails when the owner listings fail', async () => {
    const r = reader();
    r.list_deposits_for_owner.mockRejectedValue(new Error('offline'));
    await expect(loadBridgeHistory(r as any, { owner: TEST_OWNER, baseSender: null })).rejects.toThrow('offline');
  });
});
