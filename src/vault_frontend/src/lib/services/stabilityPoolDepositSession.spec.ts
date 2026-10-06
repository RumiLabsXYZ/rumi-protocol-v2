import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { Principal } from '@dfinity/principal';

const mocks = vi.hoisted(() => ({
  getActor: vi.fn(),
  isOisy: false,
  lockBusy: false,
  walletState: { isConnected: true, principal: null as any },
  ledgerActor: { icrc2_approve: vi.fn() },
  poolActor: { deposit: vi.fn() },
  getSignerAgent: vi.fn(),
  createOisyActor: vi.fn(),
}));

vi.mock('../stores/wallet', () => ({
  walletStore: {
    subscribe: (run: (value: any) => void) => { run(mocks.walletState); return () => {}; },
    getActor: mocks.getActor,
  },
}));

vi.mock('./protocol/walletOperations', () => ({
  isOisyWallet: () => mocks.isOisy,
  captureActionBoundContext: vi.fn(),
  assertActionBoundContextCurrent: (context: { expectedPrincipalText: string; assertCurrent: () => boolean }) => {
    if (!context.assertCurrent() || mocks.walletState.principal?.toText() !== context.expectedPrincipalText) {
      throw new Error('Wallet session changed during this action. Nothing further was submitted.');
    }
  },
}));

vi.mock('./pnp', () => ({ pnp: {}, canisterIDLs: { stability_pool: {}, icusd_ledger: {} } }));
vi.mock('./oisySigner', () => ({ getOisySignerAgent: mocks.getSignerAgent, createOisyActor: mocks.createOisyActor }));
vi.mock('./stabilityPoolNativeXrp', () => ({
  ackNativeXrpPayoutSettledWithActor: vi.fn(),
  getMyNativeXrpPayoutsWithActor: vi.fn(),
  optInNativeCollateralWithTagUsingActor: vi.fn(),
}));

import { CANISTER_IDS } from '../config';
import { stabilityPoolService } from './stabilityPoolService';
import { readPendingStabilityPoolDeposit } from '../utils/stabilityPoolDepositLock';

const OWNER = Principal.fromUint8Array(new Uint8Array([1, 2, 3, 4]));
const context = (assertCurrent: () => boolean = () => true) => ({
  expectedPrincipalText: OWNER.toText(),
  assertCurrent,
});

describe('Stability Pool deposit session and retry lock', () => {
  let previousLocks: PropertyDescriptor | undefined;

  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
    mocks.isOisy = false;
    mocks.lockBusy = false;
    mocks.walletState.isConnected = true;
    mocks.walletState.principal = OWNER;
    mocks.ledgerActor.icrc2_approve.mockResolvedValue({ Ok: 1n });
    mocks.poolActor.deposit.mockResolvedValue({ Ok: null });
    mocks.getActor.mockImplementation(async (canisterId: string) => {
      if (canisterId === CANISTER_IDS.ICUSD_LEDGER) return mocks.ledgerActor;
      if (canisterId === CANISTER_IDS.STABILITY_POOL) return mocks.poolActor;
      throw new Error(`unexpected actor ${canisterId}`);
    });
    previousLocks = Object.getOwnPropertyDescriptor(navigator, 'locks');
    Object.defineProperty(navigator, 'locks', {
      configurable: true,
      value: { request: async (_name: string, _options: unknown, callback: (lock: unknown) => Promise<unknown>) => callback(mocks.lockBusy ? null : {}) },
    });
  });

  afterEach(() => {
    if (previousLocks) Object.defineProperty(navigator, 'locks', previousLocks);
    else delete (navigator as unknown as { locks?: unknown }).locks;
  });

  it('does not deposit after an A to B to A switch during approval', async () => {
    let generation = 7;
    mocks.ledgerActor.icrc2_approve.mockImplementation(async () => {
      generation += 2;
      return { Ok: 1n };
    });

    await expect(stabilityPoolService.deposit(
      Principal.fromText(CANISTER_IDS.ICUSD_LEDGER), 25_000_000n, context(() => generation === 7),
    )).rejects.toThrow('Wallet session changed');
    expect(mocks.ledgerActor.icrc2_approve).toHaveBeenCalledOnce();
    expect(mocks.poolActor.deposit).not.toHaveBeenCalled();
  });

  it('does not approve or deposit while another tab owns the Web Lock', async () => {
    mocks.lockBusy = true;
    await expect(stabilityPoolService.deposit(
      Principal.fromText(CANISTER_IDS.ICUSD_LEDGER), 25_000_000n, context(),
    )).rejects.toThrow('another tab');
    expect(mocks.ledgerActor.icrc2_approve).not.toHaveBeenCalled();
    expect(mocks.poolActor.deposit).not.toHaveBeenCalled();
  });

  it('keeps an ambiguous accepted deposit locked across a retry', async () => {
    mocks.poolActor.deposit.mockRejectedValueOnce(new Error('reply lost after dispatch'));
    await expect(stabilityPoolService.deposit(
      Principal.fromText(CANISTER_IDS.ICUSD_LEDGER), 25_000_000n, context(),
    )).rejects.toThrow('no confirmed response');

    expect(readPendingStabilityPoolDeposit(OWNER.toText(), CANISTER_IDS.ICUSD_LEDGER)).toMatchObject({
      owner: OWNER.toText(), ledger: CANISTER_IDS.ICUSD_LEDGER, amount: '25000000',
    });
    await expect(stabilityPoolService.deposit(
      Principal.fromText(CANISTER_IDS.ICUSD_LEDGER), 25_000_000n, context(),
    )).rejects.toThrow('no confirmed result');
    expect(mocks.poolActor.deposit).toHaveBeenCalledOnce();
  });

  it('clears only an explicit no-transfer result so a later deliberate retry can proceed', async () => {
    mocks.poolActor.deposit
      .mockResolvedValueOnce({ Err: { LedgerTransferFailed: { reason: 'InsufficientAllowance' } } })
      .mockResolvedValueOnce({ Ok: null });
    await expect(stabilityPoolService.deposit(
      Principal.fromText(CANISTER_IDS.ICUSD_LEDGER), 25_000_000n, context(),
    )).rejects.toThrow();
    expect(readPendingStabilityPoolDeposit(OWNER.toText(), CANISTER_IDS.ICUSD_LEDGER)).toBeNull();
    await expect(stabilityPoolService.deposit(
      Principal.fromText(CANISTER_IDS.ICUSD_LEDGER), 25_000_000n, context(),
    )).resolves.toBeUndefined();
    expect(mocks.poolActor.deposit).toHaveBeenCalledTimes(2);
  });
});
