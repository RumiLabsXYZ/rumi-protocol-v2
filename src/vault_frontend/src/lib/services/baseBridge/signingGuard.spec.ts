import { describe, expect, it, vi } from 'vitest';
import { Principal } from '@dfinity/principal';
import type { DepositView } from '$declarations/rumi_icusd_bridge/rumi_icusd_bridge.did';
import { SELECTORS, encodeUint256 } from './abi';
import { BASE_MAINNET, resolveBridgeConfig } from './chains';
import { baseDestination } from './destinations';
import {
  bridgeAfterApproval,
  buildQuote,
  requestDepositRefund,
  retryIcpToBase,
  startIcpToBase,
  type IcpToBaseDeps,
  type IcpToBaseEvent,
} from './icpToBase';
import { sendBridgeOut, notifyBridge, type BaseToIcpDeps, type BaseToIcpEvent } from './baseToIcp';
import { IntentStore, type BaseToIcpIntent, type IcpToBaseIntent } from './intents';
import { NOT_SIGNED_NOTE, SigningBlockedError, assertCanSign, signingBlockMessage } from './signingGuard';
import {
  TEST_BRIDGE_ID,
  TEST_OWNER,
  createFakeProvider,
  createMemoryStorage,
  makeBridgeInfo,
  makeDeposit,
  makeDestination,
  makeDisabledDestination,
  makeSigning,
} from './testSupport';

const ME = TEST_OWNER.toText();
const NOW = 1_800_000_000_000;
const RECIPIENT = '0x1111111111111111111111111111111111111111';
const ACCOUNT = '0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa';
const CONTRACT = '0x2222222222222222222222222222222222222222';
const TRANSFER_ID = `0x${'ee'.repeat(32)}`;
const TX_HASH = `0x${'12'.repeat(32)}`;
const AMOUNT = 250_000_000n;

/** A destination whose releases AND deposits are paused and whose bridge is halted: still enabled. */
function pausedBase() {
  const result = resolveBridgeConfig(
    makeBridgeInfo({ pauses: { mints: true, observer: false, releases: true, deposits: true }, halted: ['coverage mismatch'] }),
  );
  return baseDestination(result, TEST_BRIDGE_ID);
}

describe('assertCanSign', () => {
  it('lets an enabled destination sign, on either wallet', () => {
    const destination = makeDestination();
    expect(() => assertCanSign(destination, { wallet: 'icp_signer' }, { status: 'enabled', reason: null })).not.toThrow();
    expect(() => assertCanSign(destination, { wallet: 'evm', chainId: 8453 }, { status: 'enabled', reason: null })).not.toThrow();
  });

  it('throws for a DISABLED destination, whatever the newest status says, and tells the user nothing was signed', () => {
    const destination = makeDisabledDestination();
    for (const fresh of [{ status: 'enabled', reason: null }, { status: 'disabled', reason: 'x' }, null] as const) {
      for (const wallet of ['icp_signer', 'evm'] as const) {
        const thrown = (() => {
          try {
            assertCanSign(destination, { wallet }, fresh);
          } catch (error) {
            return error;
          }
        })();
        expect(thrown).toBeInstanceOf(SigningBlockedError);
        expect((thrown as SigningBlockedError).code).toBe('destination_disabled');
        expect((thrown as Error).message).toContain('Not available yet');
        expect((thrown as Error).message).toContain(NOT_SIGNED_NOTE);
      }
    }
  });

  it('throws when the newest read has turned an open destination off, or no longer reports it', () => {
    const destination = makeDestination();
    expect(() => assertCanSign(destination, { wallet: 'icp_signer' }, { status: 'disabled', reason: 'The Base contract is not bound yet.' })).toThrowError(
      /The Base contract is not bound yet\./,
    );
    try {
      assertCanSign(destination, { wallet: 'icp_signer' }, null);
      throw new Error('should have thrown');
    } catch (error) {
      expect((error as SigningBlockedError).code).toBe('destination_unknown');
    }
  });

  it('checks the EVM wallet network only when the caller already read it, and never reads one itself', () => {
    const destination = makeDestination();
    const fresh = { status: 'enabled', reason: null } as const;
    expect(() => assertCanSign(destination, { wallet: 'evm', chainId: 1 }, fresh)).toThrowError(/different network than Base \(chain 8453\)/);
    expect(() => assertCanSign(destination, { wallet: 'evm', chainId: '8453' }, fresh)).not.toThrow();
    expect(() => assertCanSign(destination, { wallet: 'evm', chainId: null }, fresh)).not.toThrow();
    expect(() => assertCanSign(destination, { wallet: 'evm' }, fresh)).not.toThrow();
    expect(() => assertCanSign(destination, { wallet: 'icp_signer', chainId: 1 }, fresh)).not.toThrow();
  });

  it('keeps DISABLED and PAUSED apart: a paused, halted Base is enabled and may sign', () => {
    const destination = pausedBase();
    expect(destination.status).toBe('enabled');
    expect(destination.config).toMatchObject({ depositsPaused: true, releasesPaused: true, halted: 'coverage mismatch' });
    expect(() => assertCanSign(destination, { wallet: 'icp_signer' }, { status: destination.status, reason: destination.reason })).not.toThrow();
    expect(() => assertCanSign(destination, { wallet: 'evm', chainId: 8453 }, { status: 'enabled', reason: null })).not.toThrow();
  });

  it('signingBlockMessage returns the text, or null, and rethrows anything unexpected', () => {
    expect(signingBlockMessage(makeSigning(), { wallet: 'icp_signer' })).toBeNull();
    expect(signingBlockMessage(makeSigning(makeDisabledDestination()), { wallet: 'icp_signer' })).toContain(NOT_SIGNED_NOTE);
    const broken = { destination: makeDestination(), freshStatus: () => { throw new TypeError('boom'); } };
    expect(() => signingBlockMessage(broken, { wallet: 'icp_signer' })).toThrow(TypeError);
  });
});

// ---------------------------------------------------------------------------
// ICP to Base: a disabled destination reaches no signer
// ---------------------------------------------------------------------------

const DEPOSIT = makeDeposit({ status: { Escrowed: null }, client_request_id: new Uint8Array(16).fill(0xcd) });

function icpHarness(signing = makeSigning(), over: { separateApproval?: boolean } = {}) {
  const store = new IntentStore(createMemoryStorage(), ME);
  const approve = vi.fn(async () => ({ Ok: 11n }));
  const bridgeToBase = vi.fn(async () => ({ Ok: DEPOSIT }));
  const refund = vi.fn(async () => ({ Ok: makeDeposit({ status: { Refunding: null } }) }));
  const deps: IcpToBaseDeps = {
    owner: TEST_OWNER,
    store,
    bridgeCanister: Principal.fromText(TEST_BRIDGE_ID),
    ledger: { approve: approve as never },
    bridge: { bridgeToBase: bridgeToBase as never, requestDepositRefund: refund as never },
    reads: { getDeposit: async () => DEPOSIT as DepositView | null, getDepositByRequest: async () => null as DepositView | null },
    separateApproval: over.separateApproval ?? false,
    isCurrent: () => true,
    signing,
    random: (length) => new Uint8Array(length).fill(0xcd),
    now: () => NOW,
    sleep: async () => {},
  };
  const events: IcpToBaseEvent[] = [];
  return { deps, store, approve, bridgeToBase, refund, events, emit: (event: IcpToBaseEvent) => void events.push(event), signing };
}

const quote = (needsApproval: boolean) =>
  buildQuote({
    amountE8s: 2_500_000_000n,
    recipient: RECIPIENT,
    ledgerFeeE8s: 100_000n,
    depositFeeE8s: 10_000n,
    allowanceE8s: needsApproval ? 0n : 10n ** 12n,
    allowanceExpiresAtNs: null,
    balanceE8s: 10_000_000_000n,
    nowMs: NOW,
  });

function approvedIntent(destination = 'base-mainnet'): IcpToBaseIntent {
  return {
    v: 2,
    destination,
    kind: 'icp_to_base',
    clientRequestId: `0x${'cd'.repeat(16)}`,
    amountE8s: 2_500_000_000n,
    recipient: RECIPIENT,
    expectedDepositFee: 10_000n,
    createdAt: NOW,
    status: 'approved',
  };
}

describe('ICP to Base: a DISABLED destination gets no signing request', () => {
  for (const separateApproval of [false, true]) {
    for (const needsApproval of [true, false]) {
      it(`startIcpToBase signs nothing and saves nothing (popup signer: ${separateApproval}, approval needed: ${needsApproval})`, async () => {
        const h = icpHarness(makeSigning(makeDisabledDestination()), { separateApproval });
        await startIcpToBase(h.deps, quote(needsApproval), h.emit);
        expect(h.approve).not.toHaveBeenCalled();
        expect(h.bridgeToBase).not.toHaveBeenCalled();
        expect(h.refund).not.toHaveBeenCalled();
        expect(h.store.list()).toEqual([]);
        expect(h.events).toEqual([{ type: 'blocked', message: expect.stringContaining('Not available yet') }]);
        expect(h.events[0]).toMatchObject({ message: expect.stringContaining(NOT_SIGNED_NOTE) });
      });
    }
  }

  it('bridgeAfterApproval (the second click) and retryIcpToBase send nothing', async () => {
    const h = icpHarness(makeSigning(makeDisabledDestination()));
    h.store.put(approvedIntent());
    await bridgeAfterApproval(h.deps, approvedIntent(), h.emit);
    await retryIcpToBase(h.deps, approvedIntent(), h.emit);
    expect(h.bridgeToBase).not.toHaveBeenCalled();
    expect(h.approve).not.toHaveBeenCalled();
    expect(h.events.filter((event) => event.type === 'blocked')).toHaveLength(2);
  });

  it('requestDepositRefund signs nothing and says why', async () => {
    const h = icpHarness(makeSigning(makeDisabledDestination()));
    const held = makeDeposit({ status: { MintHeld: { reason: 'paused' } }, owner: TEST_OWNER });
    const result = await requestDepositRefund(h.deps, held);
    expect(h.refund).not.toHaveBeenCalled();
    expect(result).toMatchObject({ ok: false, message: expect.stringContaining('Not available yet') });
  });

  it('refuses when the newest read turned an open destination off', async () => {
    const h = icpHarness();
    h.signing.fresh.value = { status: 'disabled', reason: 'The Base contract is not bound yet.' };
    await startIcpToBase(h.deps, quote(true), h.emit);
    expect(h.approve).not.toHaveBeenCalled();
    expect(h.store.list()).toEqual([]);
    expect(h.events).toEqual([{ type: 'blocked', message: expect.stringContaining('The Base contract is not bound yet.') }]);
  });

  it('refuses when the destination is no longer reported at all', async () => {
    const h = icpHarness();
    h.signing.fresh.value = null;
    await startIcpToBase(h.deps, quote(false), h.emit);
    expect(h.bridgeToBase).not.toHaveBeenCalled();
    expect(h.events[0]).toMatchObject({ type: 'blocked' });
  });

  it('checks again after a slow approval: a destination turned off meanwhile gets no transfer, and the approved request is kept', async () => {
    const h = icpHarness();
    h.approve.mockImplementation(async () => {
      h.signing.fresh.value = { status: 'disabled', reason: 'The Base contract is not bound yet.' };
      return { Ok: 11n };
    });
    await startIcpToBase(h.deps, quote(true), h.emit);
    expect(h.approve).toHaveBeenCalledTimes(1);
    expect(h.bridgeToBase).not.toHaveBeenCalled();
    expect(h.events.map((event) => event.type)).toEqual(['confirmed', 'approval_ok', 'blocked']);
    // The approval stands; the request waits, recoverable, until the destination is back.
    expect(h.store.list()).toMatchObject([{ status: 'approved', destination: 'base-mainnet' }]);
  });
});

describe('ICP to Base: a saved request is sent only to its own destination', () => {
  /** A request made for another destination, as a later build could have saved it. */
  const foreign = (status: IcpToBaseIntent['status'] = 'approved'): IcpToBaseIntent => ({ ...approvedIntent('arbitrum-mainnet'), status });

  /** The same request as the Base-only app saved it: version 1, no destination key. */
  const legacy = (status: IcpToBaseIntent['status'] = 'approved'): IcpToBaseIntent => {
    const { destination: _destination, ...rest } = approvedIntent();
    return { ...rest, v: 1, status };
  };

  for (const separateApproval of [false, true]) {
    it(`the second click does not send another destination's request to enabled Base (popup signer: ${separateApproval})`, async () => {
      const h = icpHarness(makeSigning(), { separateApproval });
      h.store.put(foreign());
      await bridgeAfterApproval(h.deps, foreign(), h.emit);
      expect(h.bridgeToBase).not.toHaveBeenCalled();
      expect(h.approve).not.toHaveBeenCalled();
      expect(h.events.at(-1)).toMatchObject({ type: 'blocked', message: expect.stringContaining('arbitrum-mainnet') });
      expect(h.events.at(-1)).toMatchObject({ message: expect.stringContaining(NOT_SIGNED_NOTE) });
      // The same durable request, untouched: its recovery stays with its own destination.
      expect(h.store.list()).toEqual([foreign()]);
    });
  }

  for (const status of ['prepared', 'approved', 'submitted'] as const) {
    it(`a retry of another destination's ${status} request sends nothing and leaves it as it was`, async () => {
      const h = icpHarness(makeSigning());
      h.store.put(foreign(status));
      await retryIcpToBase(h.deps, foreign(status), h.emit);
      expect(h.bridgeToBase).not.toHaveBeenCalled();
      expect(h.approve).not.toHaveBeenCalled();
      expect(h.events.at(-1)).toMatchObject({ type: 'blocked', message: expect.stringContaining(NOT_SIGNED_NOTE) });
      expect(h.store.list()).toEqual([foreign(status)]);
    });
  }

  it('refuses before the signer is reached, with no await in front of the check', () => {
    const h = icpHarness(makeSigning());
    h.store.put(foreign());
    const promise = bridgeAfterApproval(h.deps, foreign(), h.emit);
    // Synchronously, before any microtask: the refusal is already out, and no signer was touched.
    expect(h.events.map((event) => event.type)).toContain('blocked');
    expect(h.bridgeToBase).not.toHaveBeenCalled();
    return promise;
  });

  it('a legacy Base request (no key) is sent on the second click, with its own request id', async () => {
    const h = icpHarness(makeSigning());
    h.store.put(legacy());
    const promise = bridgeAfterApproval(h.deps, legacy(), h.emit);
    // The first signer call of the click is still synchronous.
    expect(h.bridgeToBase).toHaveBeenCalledTimes(1);
    await promise;
    expect(h.bridgeToBase).toHaveBeenCalledWith(expect.objectContaining({ client_request_id: new Uint8Array(16).fill(0xcd) }));
    expect(h.events.map((event) => event.type)).toEqual(['bridge_started', 'submit_ok']);
  });

  it('a legacy Base request (no key) is sent on retry, with its own request id', async () => {
    const h = icpHarness(makeSigning());
    h.store.put(legacy('submitted'));
    await retryIcpToBase(h.deps, legacy('submitted'), h.emit);
    expect(h.bridgeToBase).toHaveBeenCalledTimes(1);
    expect(h.bridgeToBase).toHaveBeenCalledWith(expect.objectContaining({ client_request_id: new Uint8Array(16).fill(0xcd) }));
    expect(h.events.map((event) => event.type)).toEqual(['retry', 'submit_ok']);
  });

  it('a request keyed to another NETWORK of the same profile is not sent: base-sepolia never goes to base-mainnet', async () => {
    const h = icpHarness(makeSigning());
    h.store.put(approvedIntent('base-sepolia'));
    await bridgeAfterApproval(h.deps, approvedIntent('base-sepolia'), h.emit);
    await retryIcpToBase(h.deps, approvedIntent('base-sepolia'), h.emit);
    expect(h.bridgeToBase).not.toHaveBeenCalled();
    expect(h.store.list()).toEqual([approvedIntent('base-sepolia')]);
  });

  it('a legacy keyless request is never sent by a non-Base network', async () => {
    const arbitrum = makeDestination({ id: 'arbitrum-mainnet', profileId: 'arbitrum', name: 'Arbitrum One', chainId: '42161' });
    const h = icpHarness(makeSigning(arbitrum));
    h.store.put(legacy());
    await bridgeAfterApproval(h.deps, legacy(), h.emit);
    await retryIcpToBase(h.deps, legacy('submitted'), h.emit);
    expect(h.bridgeToBase).not.toHaveBeenCalled();
    expect(h.events.at(-1)).toMatchObject({ type: 'blocked', message: expect.stringContaining('Base') });
    expect(h.store.list()).toEqual([legacy()]);
  });

  it('a legacy keyless request binds to the Base network the page is configured for, testnet included', async () => {
    const sepolia = makeDestination({ id: 'base-sepolia', name: 'Base Sepolia', chainId: '84532', network: 'testnet' });
    const h = icpHarness(makeSigning(sepolia));
    h.store.put(legacy());
    await bridgeAfterApproval(h.deps, legacy(), h.emit);
    expect(h.bridgeToBase).toHaveBeenCalledTimes(1);
  });

  it('a request that names Base is sent too', async () => {
    const h = icpHarness(makeSigning());
    h.store.put(approvedIntent('base-mainnet'));
    await bridgeAfterApproval(h.deps, approvedIntent('base-mainnet'), h.emit);
    expect(h.bridgeToBase).toHaveBeenCalledTimes(1);
  });
});

describe('ICP to Base: PAUSED is not DISABLED', () => {
  it('a paused, halted Base still reaches the signer (the form and the canister gate it, as before)', async () => {
    const h = icpHarness(makeSigning(pausedBase()));
    await startIcpToBase(h.deps, quote(true), h.emit);
    expect(h.approve).toHaveBeenCalledTimes(1);
    expect(h.bridgeToBase).toHaveBeenCalledTimes(1);
    expect(h.events.map((event) => event.type)).toEqual(['confirmed', 'approval_ok', 'submit_ok']);
  });
});

// ---------------------------------------------------------------------------
// Base to ICP: a disabled destination reaches no wallet, not even for a read
// ---------------------------------------------------------------------------

function evmHarness(signing = makeSigning()) {
  const store = new IntentStore(createMemoryStorage(), ME);
  const txs: unknown[] = [];
  const used = new Set<string>();
  const provider = createFakeProvider({
    account: ACCOUNT,
    chainId: 8453,
    handlers: {
      eth_call: (params) => {
        const data = (params as [{ data: string }])[0].data;
        if (data.startsWith(SELECTORS.computeTransferId)) return TRANSFER_ID;
        if (data.startsWith(SELECTORS.bridgeOutUsed)) return `0x${encodeUint256(used.has(TRANSFER_ID) ? 1n : 0n)}`;
        throw new Error('unexpected eth_call');
      },
      eth_sendTransaction: (params) => {
        txs.push((params as unknown[])[0]);
        used.add(TRANSFER_ID);
        return TX_HASH;
      },
    },
  });
  const notify = vi.fn(async () => ({ Ok: null }));
  const deps: BaseToIcpDeps = {
    provider,
    chain: BASE_MAINNET,
    contract: CONTRACT,
    account: ACCOUNT,
    icpPrincipal: TEST_OWNER,
    store,
    reads: { getRelease: async () => null },
    bridge: { notifyBaseBridgeOut: notify as never },
    isCurrent: () => true,
    signing,
    random: (length) => new Uint8Array(length).fill(0x07),
    now: () => NOW,
    sleep: async () => {},
  };
  const events: BaseToIcpEvent[] = [];
  return { deps, store, provider, txs, notify, events, emit: (event: BaseToIcpEvent) => void events.push(event), signing };
}

function unsentIntent(): BaseToIcpIntent {
  return {
    v: 2,
    destination: 'base-mainnet',
    kind: 'base_to_icp',
    clientNonce: `0x${'07'.repeat(32)}`,
    transferId: TRANSFER_ID,
    from: ACCOUNT,
    amountE8s: AMOUNT,
    destinationPrincipal: ME,
    chainId: 8453,
    contract: CONTRACT,
    createdAt: NOW,
    status: 'prepared',
  };
}

describe('Base to ICP: a DISABLED destination gets no wallet call at all', () => {
  it('sendBridgeOut makes no call to the wallet (not even a read), saves nothing and sends nothing', async () => {
    const h = evmHarness(makeSigning(makeDisabledDestination({ id: 'base-mainnet', profileId: 'base', name: 'Base', chainId: '8453' })));
    const outcome = await sendBridgeOut(h.deps, { amountE8s: AMOUNT }, h.emit);
    expect(outcome.kind).toBe('blocked');
    expect(h.provider.calls).toEqual([]);
    expect(h.txs).toEqual([]);
    expect(h.store.list()).toEqual([]);
    expect(h.events).toEqual([{ type: 'blocked', message: expect.stringContaining(NOT_SIGNED_NOTE) }]);
  });

  it('a saved unsent transfer cannot be resumed either', async () => {
    const h = evmHarness(makeSigning(makeDisabledDestination({ id: 'base-mainnet', profileId: 'base', name: 'Base', chainId: '8453' })));
    h.store.put(unsentIntent());
    const outcome = await sendBridgeOut(h.deps, { amountE8s: AMOUNT, resume: unsentIntent() }, h.emit);
    expect(outcome.kind).toBe('blocked');
    expect(h.provider.calls).toEqual([]);
    expect(h.store.get(TRANSFER_ID)).toMatchObject({ status: 'prepared' });
  });

  it('notifyBridge makes no ICP update call', async () => {
    const h = evmHarness(makeSigning(makeDisabledDestination()));
    const result = await notifyBridge(h.deps, { ...unsentIntent(), status: 'sent', txHash: TX_HASH });
    expect(h.notify).not.toHaveBeenCalled();
    expect(result).toMatchObject({ ok: false, message: expect.stringContaining('Not available yet') });
  });

  it('refuses when the newest read has turned the destination off, before any wallet call', async () => {
    const h = evmHarness();
    h.signing.fresh.value = { status: 'disabled', reason: 'The Base contract is not bound yet.' };
    const outcome = await sendBridgeOut(h.deps, { amountE8s: AMOUNT }, h.emit);
    expect(outcome.kind).toBe('blocked');
    expect(h.provider.calls).toEqual([]);
  });

  it('looks once more just before the wallet prompt: a destination turned off during the reads is never sent', async () => {
    const h = evmHarness();
    const usedHandler = h.provider.handlers.eth_call;
    h.provider.handlers.eth_call = (params, call) => {
      const result = usedHandler(params, call);
      if ((params as [{ data: string }])[0].data.startsWith(SELECTORS.bridgeOutUsed)) {
        h.signing.fresh.value = { status: 'disabled', reason: 'The Base contract is not bound yet.' };
      }
      return result;
    };
    const outcome = await sendBridgeOut(h.deps, { amountE8s: AMOUNT }, h.emit);
    expect(outcome).toEqual({ kind: 'aborted', reason: 'check_failed' });
    expect(h.provider.callsTo('eth_sendTransaction')).toHaveLength(0);
    expect(h.txs).toEqual([]);
    // Nothing was sent, so the unsent record is not left behind.
    expect(h.store.list()).toEqual([]);
    expect(h.events.at(-1)).toMatchObject({ type: 'aborted', reason: 'check_failed', message: expect.stringContaining('The Base contract is not bound yet.') });
  });
});

describe('Base to ICP: PAUSED releases are not DISABLED', () => {
  it('a Base with releases paused (and a halt) still sends: the form warns, the wallet is asked', async () => {
    const h = evmHarness(makeSigning(pausedBase()));
    const outcome = await sendBridgeOut(h.deps, { amountE8s: AMOUNT }, h.emit);
    expect(outcome).toMatchObject({ kind: 'sent', txHash: TX_HASH });
    expect(h.txs).toHaveLength(1);
    expect(h.events.map((event) => event.type)).toEqual(['wallet_prompt', 'sent']);
  });
});
