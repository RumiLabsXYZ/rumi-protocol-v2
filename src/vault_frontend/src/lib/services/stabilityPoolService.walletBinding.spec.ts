import { beforeEach, describe, expect, it, vi } from 'vitest';
import { Principal } from '@dfinity/principal';

const mocks = vi.hoisted(() => ({
  state: { isConnected: true, principal: null as any, icon: '/wallets/plug.svg', loading: false },
  sessionGeneration: 0,
  getActor: vi.fn(),
  isOisyWallet: vi.fn(() => false),
  lockBusy: false,
}));

vi.mock('../stores/wallet', () => ({
  walletStore: {
    subscribe(run: (value: typeof mocks.state) => void) {
      run(mocks.state);
      return () => {};
    },
    getActor: mocks.getActor,
  },
}));
vi.mock('./pnp', () => ({ pnp: {}, canisterIDLs: { stability_pool: {} } }));
vi.mock('../config', () => ({
  CANISTER_IDS: { STABILITY_POOL: 'aaaaa-aa' },
  CONFIG: { icusd_ledgerIDL: {}, host: 'http://localhost', isLocal: false },
}));
vi.mock('./protocol/walletOperations', () => ({ isOisyWallet: mocks.isOisyWallet }));
vi.mock('./auth', () => ({
  walletSessionGeneration: {
    subscribe(run: (value: number) => void) {
      run(mocks.sessionGeneration);
      return () => {};
    },
  },
}));
vi.mock('./oisySigner', () => ({
  getOisySignerAgent: vi.fn(),
  createOisyActor: vi.fn(),
}));

import {
  assertStabilityPoolActionContext,
  captureStabilityPoolActionContext,
  readPendingStabilityPoolDeposit,
  stabilityPoolService,
} from './stabilityPoolService';

const ownerA = Principal.fromUint8Array(Uint8Array.of(1));
const ownerB = Principal.fromUint8Array(Uint8Array.of(2));
const ledgerA = Principal.fromUint8Array(Uint8Array.of(3));
const ledgerB = Principal.fromUint8Array(Uint8Array.of(4));

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(r => { resolve = r; });
  return { promise, resolve };
}

function connectedAs(principal: Principal) {
  mocks.state.isConnected = true;
  mocks.state.principal = principal;
  mocks.sessionGeneration += 1;
}

function emptyStatus(nextSeq = 1n) {
  return { high_watermark: nextSeq - 1n, next_seq: [nextSeq], intent: [], active_intent: [] };
}

function completed(seq: bigint, ledger: Principal, amount: bigint) {
  return { Completed: { intent_seq: seq, token_ledger: ledger, amount, block_index: 19n } };
}

function pending(seq: bigint, ledger: Principal, amount: bigint) {
  return { Pending: { intent_seq: seq, token_ledger: ledger, amount, phase: { Dispatching: null }, reason: [] } };
}

function setupActors(poolActor: any, ledgerActor: any = { icrc2_approve: vi.fn().mockResolvedValue({ Ok: 1n }) }) {
  mocks.getActor.mockImplementation(async (canisterId: string) =>
    canisterId === ledgerA.toText() || canisterId === ledgerB.toText() ? ledgerActor : poolActor,
  );
  return { poolActor, ledgerActor };
}

describe('Stability Pool wallet-bound deposit intents', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
    mocks.isOisyWallet.mockReturnValue(false);
    mocks.lockBusy = false;
    mocks.sessionGeneration = 0;
    mocks.state.loading = false;
    connectedAs(ownerA);
    vi.stubGlobal('setTimeout', (callback: () => void) => {
      queueMicrotask(callback);
      return 0;
    });
    Object.defineProperty(navigator, 'locks', {
      configurable: true,
      value: {
        request: async (_name: string, _options: unknown, callback: (lock: unknown | null) => Promise<unknown>) => {
          if (mocks.lockBusy) return callback(null);
          mocks.lockBusy = true;
          try {
            return await callback({});
          } finally {
            mocks.lockBusy = false;
          }
        },
      },
    });
  });

  it('rejects a captured deposit after the active principal changes', async () => {
    const poolActor = { get_deposit_intent: vi.fn() };
    setupActors(poolActor);
    const context = captureStabilityPoolActionContext(ledgerA, 'deposit');
    connectedAs(ownerB);

    await expect(stabilityPoolService.deposit(ledgerA, 10n, context))
      .rejects.toThrow(/session or Stability Pool action changed/i);
    expect(mocks.getActor).not.toHaveBeenCalled();
  });

  it('does not dispatch after identity changes during intent status lookup', async () => {
    const statusRead = deferred<any>();
    const poolActor = {
      get_deposit_intent: vi.fn(() => statusRead.promise),
      deposit_with_intent: vi.fn(),
    };
    setupActors(poolActor);
    const context = captureStabilityPoolActionContext(ledgerA, 'deposit');
    const action = stabilityPoolService.deposit(ledgerA, 10n, context);
    await vi.waitFor(() => expect(poolActor.get_deposit_intent).toHaveBeenCalledOnce());
    connectedAs(ownerB);
    statusRead.resolve(emptyStatus());

    await expect(action).rejects.toThrow(/session or Stability Pool action changed/i);
    expect(localStorage.length).toBe(0);
    expect(poolActor.deposit_with_intent).not.toHaveBeenCalled();
  });

  it('aborts a delayed status read as soon as refreshWallet begins owner A to B reconnect', async () => {
    const statusRead = deferred<any>();
    const poolActor = {
      get_deposit_intent: vi.fn(() => statusRead.promise),
      deposit_with_intent: vi.fn(),
    };
    setupActors(poolActor);
    const context = captureStabilityPoolActionContext(ledgerA, 'deposit');
    const action = stabilityPoolService.deposit(ledgerA, 10n, context);
    await vi.waitFor(() => expect(poolActor.get_deposit_intent).toHaveBeenCalledOnce());

    // refreshWallet invalidates the captured generation and marks the wallet
    // loading before its first provider await, while the old principal may
    // still be visible in the store.
    mocks.state.loading = true;
    mocks.sessionGeneration += 1;
    statusRead.resolve(emptyStatus());

    await expect(action).rejects.toThrow(/wallet session is changing|session or Stability Pool action changed/i);
    expect(localStorage.length).toBe(0);
    expect(poolActor.deposit_with_intent).not.toHaveBeenCalled();
  });

  it('persists the exact sequence, payload, and wallet context before approval', async () => {
    const ledgerActor = {
      icrc2_approve: vi.fn(() => {
        expect(readPendingStabilityPoolDeposit(ownerA.toText())).toMatchObject({
          owner: ownerA.toText(), ledger: ledgerA.toText(), amount: '10', intentSeq: '1',
          walletIcon: '/wallets/plug.svg', oisy: false,
        });
        return Promise.resolve({ Ok: 1n });
      }),
    };
    const poolActor = {
      get_deposit_intent: vi.fn().mockResolvedValue(emptyStatus(1n)),
      deposit_with_intent: vi.fn().mockResolvedValue({ Ok: completed(1n, ledgerA, 10n) }),
      deposit: vi.fn(),
    };
    setupActors(poolActor, ledgerActor);
    const context = captureStabilityPoolActionContext(ledgerA, 'deposit');

    await expect(stabilityPoolService.deposit(ledgerA, 10n, context)).resolves.toBeUndefined();
    expect(poolActor.deposit_with_intent).toHaveBeenCalledWith(1n, ledgerA, 10n);
    expect(poolActor.deposit).not.toHaveBeenCalled();
  });

  it('fails closed when Web Locks are unavailable instead of using a tab-local fallback', async () => {
    const poolActor = { get_deposit_intent: vi.fn(), deposit_with_intent: vi.fn() };
    const ledgerActor = { icrc2_approve: vi.fn() };
    setupActors(poolActor, ledgerActor);
    Object.defineProperty(navigator, 'locks', { configurable: true, value: undefined });
    const context = captureStabilityPoolActionContext(ledgerA, 'deposit');

    await expect(stabilityPoolService.deposit(ledgerA, 10n, context)).rejects.toThrow(/cannot safely coordinate.*No deposit was submitted/i);
    expect(mocks.getActor).not.toHaveBeenCalled();
    expect(ledgerActor.icrc2_approve).not.toHaveBeenCalled();
    expect(poolActor.deposit_with_intent).not.toHaveBeenCalled();
  });

  it('keeps the original owner lock and never unlocks after the wallet changes during dispatch', async () => {
    const dispatch = deferred<any>();
    const poolActor = {
      get_deposit_intent: vi.fn().mockResolvedValue(emptyStatus(1n)),
      deposit_with_intent: vi.fn(() => dispatch.promise),
      deposit: vi.fn(),
    };
    setupActors(poolActor);
    const context = captureStabilityPoolActionContext(ledgerA, 'deposit');
    const action = stabilityPoolService.deposit(ledgerA, 10n, context);
    await vi.waitFor(() => expect(poolActor.deposit_with_intent).toHaveBeenCalledOnce());
    connectedAs(ownerB);
    dispatch.resolve({ Ok: completed(1n, ledgerA, 10n) });

    await expect(action).rejects.toThrow(/session or Stability Pool action changed/i);
    expect(readPendingStabilityPoolDeposit(ownerA.toText())).toMatchObject({ intentSeq: '1', ledger: ledgerA.toText() });
    expect(readPendingStabilityPoolDeposit(ownerB.toText())).toBeNull();
    expect(poolActor.deposit).not.toHaveBeenCalled();
  });

  it('does not reuse a stale actor when wallet provider identity changes during status lookup', async () => {
    const statusRead = deferred<any>();
    const poolActor = {
      get_deposit_intent: vi.fn(() => statusRead.promise),
      deposit_with_intent: vi.fn(),
    };
    setupActors(poolActor);
    const context = captureStabilityPoolActionContext(ledgerA, 'deposit');
    const action = stabilityPoolService.deposit(ledgerA, 10n, context);
    await vi.waitFor(() => expect(poolActor.get_deposit_intent).toHaveBeenCalledOnce());
    mocks.isOisyWallet.mockReturnValue(true);
    mocks.sessionGeneration += 1;
    statusRead.resolve(emptyStatus());

    await expect(action).rejects.toThrow(/session or Stability Pool action changed/i);
    expect(poolActor.deposit_with_intent).not.toHaveBeenCalled();
    expect(localStorage.length).toBe(0);
  });

  it('does not dispatch a withdrawal when identity changes during actor acquisition', async () => {
    const actorReady = deferred<any>();
    const poolActor = { withdraw: vi.fn(async () => ({ Ok: null })) };
    mocks.getActor.mockReturnValue(actorReady.promise);
    const context = captureStabilityPoolActionContext(ledgerA, 'withdraw');

    const action = stabilityPoolService.withdraw(ledgerA, 10n, context);
    await vi.waitFor(() => expect(mocks.getActor).toHaveBeenCalledOnce());
    connectedAs(ownerB);
    actorReady.resolve(poolActor);

    await expect(action).rejects.toThrow(/session or Stability Pool action changed/i);
    expect(poolActor.withdraw).not.toHaveBeenCalled();
  });

  it('does not report a stale withdrawal result after the wallet changes during dispatch', async () => {
    const withdrawal = deferred<any>();
    const poolActor = { withdraw: vi.fn(() => withdrawal.promise) };
    mocks.getActor.mockResolvedValue(poolActor);
    const context = captureStabilityPoolActionContext(ledgerA, 'withdraw');
    const action = stabilityPoolService.withdraw(ledgerA, 10n, context);
    await vi.waitFor(() => expect(poolActor.withdraw).toHaveBeenCalledOnce());
    connectedAs(ownerB);
    withdrawal.resolve({ Ok: null });

    await expect(action).rejects.toThrow(/session or Stability Pool action changed/i);
    expect(poolActor.withdraw).toHaveBeenCalledOnce();
  });

  it('reuses the same sequence and payload after a lost reply and reload-style retry', async () => {
    const poolActor = {
      get_deposit_intent: vi.fn()
        .mockResolvedValueOnce(emptyStatus(1n))
        .mockResolvedValueOnce({ ...emptyStatus(1n), intent: [pending(1n, ledgerA, 10n)], active_intent: [pending(1n, ledgerA, 10n)], next_seq: [] })
        .mockResolvedValueOnce({ ...emptyStatus(1n), intent: [pending(1n, ledgerA, 10n)], active_intent: [pending(1n, ledgerA, 10n)], next_seq: [] }),
      deposit_with_intent: vi.fn()
        .mockRejectedValueOnce(new Error('connection closed after dispatch'))
        .mockResolvedValueOnce({ Ok: completed(1n, ledgerA, 10n) }),
    };
    setupActors(poolActor);
    const context = captureStabilityPoolActionContext(ledgerA, 'deposit');

    await expect(stabilityPoolService.deposit(ledgerA, 10n, context)).rejects.toThrow(/intent 1.*no terminal update result/i);
    expect(readPendingStabilityPoolDeposit(ownerA.toText())).toMatchObject({
      owner: ownerA.toText(), ledger: ledgerA.toText(), action: 'deposit', amount: '10', intentSeq: '1', status: 'pending',
    });

    // Models another mount after the wallet session returns: same saved seq is reconciled first.
    await expect(stabilityPoolService.deposit(ledgerA, 10n, context)).resolves.toBeUndefined();
    expect(poolActor.deposit_with_intent).toHaveBeenCalledTimes(2);
    expect(poolActor.deposit_with_intent).toHaveBeenNthCalledWith(1, 1n, ledgerA, 10n);
    expect(poolActor.deposit_with_intent).toHaveBeenNthCalledWith(2, 1n, ledgerA, 10n);
    expect(readPendingStabilityPoolDeposit(ownerA.toText())).toBeNull();
  });

  it('adopts a pending canister intent discovered on another device and blocks cross-token submission', async () => {
    const poolActor = {
      get_deposit_intent: vi.fn().mockResolvedValue({
        ...emptyStatus(1n),
        next_seq: [],
        active_intent: [pending(7n, ledgerA, 10n)],
      }),
      deposit_with_intent: vi.fn(),
    };
    const ledgerActor = { icrc2_approve: vi.fn() };
    setupActors(poolActor, ledgerActor);
    const context = captureStabilityPoolActionContext(ledgerB, 'deposit');

    await expect(stabilityPoolService.deposit(ledgerB, 20n, context)).rejects.toThrow(/another device has pending deposit intent 7/i);
    expect(readPendingStabilityPoolDeposit(ownerA.toText())).toMatchObject({
      ledger: ledgerA.toText(), amount: '10', intentSeq: '7',
    });
    expect(ledgerActor.icrc2_approve).not.toHaveBeenCalled();
    expect(poolActor.deposit_with_intent).not.toHaveBeenCalled();
  });

  it('rebinds a stale local marker to the authenticated caller-wide active intent', async () => {
    localStorage.setItem(`rumi:stability-pool:pending-deposit:${ownerA.toText()}`, JSON.stringify({
      owner: ownerA.toText(), ledger: ledgerB.toText(), action: 'deposit', amount: '20', intentSeq: '2',
      walletIcon: '/wallets/plug.svg', oisy: false, createdAt: Date.now(), status: 'pending',
    }));
    const poolActor = {
      get_deposit_intent: vi.fn().mockResolvedValue({
        high_watermark: 0n,
        next_seq: [],
        intent: [],
        active_intent: [pending(1n, ledgerA, 10n)],
      }),
      deposit_with_intent: vi.fn(),
    };
    const ledgerActor = { icrc2_approve: vi.fn() };
    setupActors(poolActor, ledgerActor);
    const context = captureStabilityPoolActionContext(ledgerB, 'deposit');

    await expect(stabilityPoolService.deposit(ledgerB, 20n, context)).rejects.toThrow(/bound to that exact intent/i);
    expect(readPendingStabilityPoolDeposit(ownerA.toText())).toMatchObject({
      ledger: ledgerA.toText(), amount: '10', intentSeq: '1',
    });
    expect(ledgerActor.icrc2_approve).not.toHaveBeenCalled();
    expect(poolActor.deposit_with_intent).not.toHaveBeenCalled();
  });

  it('uses the Web Lock to serialize tabs before status allocation or approval', async () => {
    const poolActor = { get_deposit_intent: vi.fn(), deposit_with_intent: vi.fn() };
    const ledgerActor = { icrc2_approve: vi.fn() };
    setupActors(poolActor, ledgerActor);
    mocks.lockBusy = true;
    const context = captureStabilityPoolActionContext(ledgerA, 'deposit');

    await expect(stabilityPoolService.deposit(ledgerA, 10n, context)).rejects.toThrow(/active in another tab/i);
    expect(mocks.getActor).not.toHaveBeenCalled();
    expect(ledgerActor.icrc2_approve).not.toHaveBeenCalled();
  });

  it('rejects a concurrent cross-token deposit while another token intent owns the principal lock', async () => {
    const statusRead = deferred<any>();
    const poolActor = {
      get_deposit_intent: vi.fn(() => statusRead.promise),
      deposit_with_intent: vi.fn().mockResolvedValue({ Ok: completed(1n, ledgerA, 10n) }),
    };
    const ledgerActor = { icrc2_approve: vi.fn().mockResolvedValue({ Ok: 1n }) };
    mocks.getActor.mockImplementation(async (canisterId: string) =>
      canisterId === ledgerA.toText() || canisterId === ledgerB.toText() ? ledgerActor : poolActor,
    );
    const contextA = captureStabilityPoolActionContext(ledgerA, 'deposit');
    const contextB = captureStabilityPoolActionContext(ledgerB, 'deposit');

    const first = stabilityPoolService.deposit(ledgerA, 10n, contextA);
    await vi.waitFor(() => expect(poolActor.get_deposit_intent).toHaveBeenCalledOnce());
    await expect(stabilityPoolService.deposit(ledgerB, 20n, contextB)).rejects.toThrow(/active in another tab/i);
    expect(poolActor.get_deposit_intent).toHaveBeenCalledOnce();
    expect(ledgerActor.icrc2_approve).not.toHaveBeenCalled();

    statusRead.resolve(emptyStatus(1n));
    await expect(first).resolves.toBeUndefined();
    expect(poolActor.deposit_with_intent).toHaveBeenCalledOnce();
    expect(poolActor.deposit_with_intent).toHaveBeenCalledWith(1n, ledgerA, 10n);
  });

  it('clears only after an exact authenticated NoEffect result, then allocates the next sequence', async () => {
    const poolActor = {
      get_deposit_intent: vi.fn()
        .mockResolvedValueOnce(emptyStatus(1n))
        .mockResolvedValueOnce(emptyStatus(2n)),
      deposit_with_intent: vi.fn().mockResolvedValueOnce({
        Ok: { NoEffect: { intent_seq: 1n, token_ledger: ledgerA, amount: 10n, reason: 'Insufficient allowance' } },
      }).mockResolvedValueOnce({ Ok: completed(2n, ledgerA, 10n) }),
    };
    setupActors(poolActor);
    const context = captureStabilityPoolActionContext(ledgerA, 'deposit');

    await expect(stabilityPoolService.deposit(ledgerA, 10n, context)).rejects.toThrow(/Insufficient allowance/i);
    expect(readPendingStabilityPoolDeposit(ownerA.toText())).toBeNull();
    await expect(stabilityPoolService.deposit(ledgerA, 10n, context)).resolves.toBeUndefined();
    expect(poolActor.deposit_with_intent).toHaveBeenNthCalledWith(1, 1n, ledgerA, 10n);
    expect(poolActor.deposit_with_intent).toHaveBeenNthCalledWith(2, 2n, ledgerA, 10n);
  });

  it('clears a typed error only when authenticated status proves no sequence was reserved', async () => {
    const poolActor = {
      get_deposit_intent: vi.fn()
        .mockResolvedValueOnce(emptyStatus(1n))
        .mockResolvedValueOnce(emptyStatus(1n))
        .mockResolvedValueOnce(emptyStatus(1n)),
      deposit_with_intent: vi.fn()
        .mockResolvedValueOnce({ Err: { TokenNotActive: { ledger: ledgerA } } })
        .mockResolvedValueOnce({ Ok: completed(1n, ledgerA, 10n) }),
      deposit: vi.fn(),
    };
    setupActors(poolActor);
    const context = captureStabilityPoolActionContext(ledgerA, 'deposit');

    await expect(stabilityPoolService.deposit(ledgerA, 10n, context)).rejects.toThrow(/not currently active/i);
    expect(poolActor.get_deposit_intent).toHaveBeenCalledTimes(2);
    expect(readPendingStabilityPoolDeposit(ownerA.toText())).toBeNull();
    await expect(stabilityPoolService.deposit(ledgerA, 10n, context)).resolves.toBeUndefined();
    expect(poolActor.deposit_with_intent).toHaveBeenNthCalledWith(1, 1n, ledgerA, 10n);
    expect(poolActor.deposit_with_intent).toHaveBeenNthCalledWith(2, 1n, ledgerA, 10n);
    expect(poolActor.deposit).not.toHaveBeenCalled();
  });

  it('shows capacity as a non-retryable admission limit after status proves no reservation', async () => {
    const poolActor = {
      get_deposit_intent: vi.fn()
        .mockResolvedValueOnce(emptyStatus(1n))
        .mockResolvedValueOnce(emptyStatus(1n)),
      deposit_with_intent: vi.fn().mockResolvedValue({ Err: { DepositIntentCapacityReached: null } }),
    };
    setupActors(poolActor);
    const context = captureStabilityPoolActionContext(ledgerA, 'deposit');

    await expect(stabilityPoolService.deposit(ledgerA, 10n, context)).rejects.toThrow(
      /capacity is full.*No deposit was submitted.*cannot be retried.*withdrawals remain available/i,
    );
    expect(poolActor.deposit_with_intent).toHaveBeenCalledOnce();
    expect(readPendingStabilityPoolDeposit(ownerA.toText())).toBeNull();
  });

  it('keeps a Pending response locked and retries only its exact sequence', async () => {
    const poolActor = {
      get_deposit_intent: vi.fn()
        .mockResolvedValueOnce(emptyStatus(1n))
        .mockResolvedValueOnce({ ...emptyStatus(1n), next_seq: [], intent: [pending(1n, ledgerA, 10n)], active_intent: [pending(1n, ledgerA, 10n)] })
        .mockResolvedValueOnce({ ...emptyStatus(2n), intent: [completed(1n, ledgerA, 10n)] }),
      deposit_with_intent: vi.fn()
        .mockResolvedValueOnce({ Ok: pending(1n, ledgerA, 10n) })
        .mockResolvedValueOnce({ Ok: completed(1n, ledgerA, 10n) }),
    };
    setupActors(poolActor);
    const context = captureStabilityPoolActionContext(ledgerA, 'deposit');

    await expect(stabilityPoolService.deposit(ledgerA, 10n, context)).rejects.toThrow(/still pending/i);
    expect(readPendingStabilityPoolDeposit(ownerA.toText())).toMatchObject({ intentSeq: '1', amount: '10' });
    await expect(stabilityPoolService.deposit(ledgerA, 10n, context)).resolves.toBeUndefined();
    expect(poolActor.deposit_with_intent).toHaveBeenNthCalledWith(2, 1n, ledgerA, 10n);
  });

  it('rejects a duplicate withdrawal while the first actor acquisition is pending', async () => {
    const actorReady = deferred<any>();
    const poolActor = { withdraw: vi.fn(async () => ({ Ok: null })) };
    mocks.getActor.mockReturnValue(actorReady.promise);
    const context = captureStabilityPoolActionContext(ledgerA, 'withdraw');

    const first = stabilityPoolService.withdraw(ledgerA, 10n, context);
    await vi.waitFor(() => expect(mocks.getActor).toHaveBeenCalledOnce());
    await expect(stabilityPoolService.withdraw(ledgerA, 10n, context)).rejects.toThrow(/already in progress/i);
    actorReady.resolve(poolActor);
    await expect(first).resolves.toBeUndefined();
    expect(poolActor.withdraw).toHaveBeenCalledOnce();
  });

  it('binds the action to its token and verb as well as its principal', () => {
    const context = captureStabilityPoolActionContext(ledgerA, 'deposit');
    expect(() => assertStabilityPoolActionContext(context, ownerA.toText(), '/wallets/plug.svg', 1, false, ledgerB, 'deposit'))
      .toThrow(/session or Stability Pool action changed/i);
    expect(() => assertStabilityPoolActionContext(context, ownerA.toText(), '/wallets/plug.svg', 1, false, ledgerA, 'withdraw'))
      .toThrow(/session or Stability Pool action changed/i);
    expect(() => assertStabilityPoolActionContext(context, ownerA.toText(), '/wallets/ii.svg', 1, false, ledgerA, 'deposit'))
      .toThrow(/session or Stability Pool action changed/i);
    expect(() => assertStabilityPoolActionContext(context, ownerA.toText(), '/wallets/plug.svg', 2, false, ledgerA, 'deposit'))
      .toThrow(/session or Stability Pool action changed/i);
  });
});
