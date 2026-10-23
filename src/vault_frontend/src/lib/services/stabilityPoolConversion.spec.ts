import { beforeEach, describe, expect, it, vi } from 'vitest';
import { Principal } from '@dfinity/principal';

const mocks = vi.hoisted(() => ({
  ownerText: '2vxsx-fae',
  otherOwnerText: 'rrkah-fqaaa-aaaaa-aaaaq-cai',
  ledgerText: 'ryjl3-tyaaa-aaaaa-aaaba-cai',
  wallet: { isConnected: true, principal: null as Principal | null, icon: 'plug', loading: false },
  generation: 1,
  poolActor: {} as any,
  ledgerActor: {} as any,
}));

vi.mock('../stores/wallet', () => ({ walletStore: { subscribe(run: (value: typeof mocks.wallet) => void) { run(mocks.wallet); return () => {}; } } }));
vi.mock('./auth', () => ({ walletSessionGeneration: { subscribe(run: (value: number) => void) { run(mocks.generation); return () => {}; } } }));
vi.mock('./protocol/walletOperations', () => ({ isOisyWallet: () => false }));
vi.mock('./stabilityPoolService', () => ({
  captureStabilityPoolActionContext: (ledger: Principal, action: string) => ({ principalText: mocks.ownerText, walletIcon: 'plug', sessionGeneration: mocks.generation, ledgerText: ledger.toText(), action, oisy: false }),
  assertStabilityPoolActionContext: (context: any, livePrincipal: string | null) => { if (context.principalText !== livePrincipal) throw new Error('Wallet session changed'); },
  getStabilityPoolMutationActor: vi.fn(async () => mocks.poolActor),
  getStabilityPoolLedgerMutationActor: vi.fn(async () => mocks.ledgerActor),
}));
vi.mock('../config', () => ({ CANISTER_IDS: { STABILITY_POOL: 'aaaaa-aa' } }));
import {
  conversionMinimumLp,
  conversionWalletRequirement,
  formatExactTokenAmount,
  parseExactTokenAmount,
  readPendingStabilityPoolConversion,
  convertStabilityPoolStableTo3Usd,
  quoteStabilityPoolConversion,
  type StabilityPoolConversionStatus,
  type PendingStabilityPoolConversion,
} from './stabilityPoolConversion';

function context() {
  return { principalText: mocks.ownerText, walletIcon: 'plug', sessionGeneration: mocks.generation, ledgerText: mocks.ledgerText, action: 'convert' as const, oisy: false };
}

function quote() {
  const ledger = Principal.fromText(mocks.ledgerText);
  return {
    token_ledger: ledger,
    lp_ledger: ledger,
    pool_canister: ledger,
    pool_coin_index: 0,
    principal_raw: 1_000_000n,
    quoted_lp: 990_000n,
    live_transfer_fee_raw: 10n,
    minimum_fee_budget_raw: 50n,
    quotedAtMs: Date.now(),
  };
}

function pendingStatus(seq = 1n) { return { Pending: { intent_seq: seq, phase: { Pulling: null }, reason: [] } }; }

beforeEach(() => {
  localStorage.clear();
  mocks.wallet.principal = Principal.fromText(mocks.ownerText);
  mocks.poolActor = { get_active_3usd_conversion_v1: vi.fn().mockResolvedValue([]), get_3usd_conversion_next_seq: vi.fn().mockResolvedValue(1n), start_3usd_conversion: vi.fn(), resume_3usd_conversion: vi.fn(), get_3usd_conversion_status: vi.fn() };
  mocks.ledgerActor = { icrc2_approve: vi.fn().mockResolvedValue({ Ok: 1n }) };
  Object.defineProperty(navigator, 'locks', { configurable: true, value: { request: async (_name: string, _opts: unknown, cb: (lock: unknown) => Promise<unknown>) => cb({}) } });
});

describe('stability pool conversion arithmetic', () => {
  it('parses 6 and 8 decimal inputs without Number rounding', () => {
    expect(parseExactTokenAmount('1.234567', 6)).toBe(1_234_567n);
    expect(parseExactTokenAmount('1.23456789', 8)).toBe(123_456_789n);
    expect(formatExactTokenAmount(1_234_567n, 6)).toBe('1.234567');
    expect(formatExactTokenAmount(123_456_789n, 8)).toBe('1.23456789');
  });

  it('keeps minimum LP and all wallet fees bounded with BigInt math', () => {
    expect(conversionMinimumLp(1_000n, 100n)).toBe(990n);
    // principal + fee budget + pull fee + the wallet approval fee
    expect(conversionWalletRequirement(1_000_000n, 10n, 50n)).toBe(1_000_070n);
  });

  it('keeps a malformed or missing browser marker locked before mutation', () => {
    const owner = 'aaaaa-aa';
    localStorage.removeItem(`rumi:stability-pool:pending-conversion:${owner}`);
    expect(readPendingStabilityPoolConversion(owner)).toBeNull();
    const record: PendingStabilityPoolConversion = {
      owner,
      ledger: owner,
      principalRaw: '1000000',
      maxFeeBudgetRaw: '50',
      minLp: '990000',
      intentSeq: '7',
      walletIcon: 'plug',
      oisy: false,
      createdAt: Date.now(),
      stage: 'start-submitted',
    };
    localStorage.setItem(`rumi:stability-pool:pending-conversion:${owner}`, JSON.stringify(record));
    expect(readPendingStabilityPoolConversion(owner)).toMatchObject({ intentSeq: '7', stage: 'start-submitted' });
    localStorage.setItem(`rumi:stability-pool:pending-conversion:${owner}`, '{bad');
    expect(() => readPendingStabilityPoolConversion(owner)).toThrow(/recovery record is unreadable/i);
  });
});

describe('stability pool conversion recovery boundaries', () => {
  it('retains the exact request when the start reply and status lookup are lost', async () => {
    mocks.poolActor.start_3usd_conversion.mockRejectedValueOnce(new Error('transport timeout'));
    mocks.poolActor.get_3usd_conversion_status.mockRejectedValueOnce(new Error('status unavailable'));
    await expect(convertStabilityPoolStableTo3Usd(quote(), 980_100n, context())).rejects.toThrow(/exact request/i);
    const marker = readPendingStabilityPoolConversion(mocks.ownerText);
    expect(marker).toMatchObject({ intentSeq: '1', principalRaw: '1000000', maxFeeBudgetRaw: '50', minLp: '980100', stage: 'start-submitted' });
    expect(mocks.poolActor.start_3usd_conversion).toHaveBeenCalledOnce();
  });

  it('replays the same cached start request after authenticated NotFound recovery', async () => {
    mocks.poolActor.start_3usd_conversion
      .mockRejectedValueOnce(new Error('transport before acceptance'))
      .mockResolvedValueOnce(pendingStatus(1n));
    mocks.poolActor.get_3usd_conversion_status.mockResolvedValueOnce({ Err: { NotFound: null } });
    const status = await convertStabilityPoolStableTo3Usd(quote(), 980100n, context());
    expect('Pending' in (status as StabilityPoolConversionStatus)).toBe(true);
    expect(mocks.poolActor.start_3usd_conversion).toHaveBeenCalledTimes(2);
    expect(mocks.poolActor.start_3usd_conversion.mock.calls[1][0]).toEqual(mocks.poolActor.start_3usd_conversion.mock.calls[0][0]);
    expect(mocks.poolActor.start_3usd_conversion.mock.calls[1][0].intent_seq).toBe(1n);
    expect(readPendingStabilityPoolConversion(mocks.ownerText)).toMatchObject({ intentSeq: '1', stage: 'start-submitted' });
  });

  it('does not duplicate an accepted start when status proves it is still pending', async () => {
    mocks.poolActor.start_3usd_conversion.mockRejectedValueOnce(new Error('accepted reply lost'));
    mocks.poolActor.get_3usd_conversion_status.mockResolvedValueOnce(pendingStatus(1n));
    const status = await convertStabilityPoolStableTo3Usd(quote(), 980100n, context());
    expect('Pending' in (status as StabilityPoolConversionStatus)).toBe(true);
    expect(mocks.poolActor.start_3usd_conversion).toHaveBeenCalledOnce();
  });

  it('retains the exact wallet approval tuple when its reply is lost', async () => {
    mocks.ledgerActor.icrc2_approve.mockRejectedValueOnce(new Error('approval transport timeout'));
    await expect(convertStabilityPoolStableTo3Usd(quote(), 980100n, context())).rejects.toThrow(/approval transport timeout/i);
    const marker = readPendingStabilityPoolConversion(mocks.ownerText);
    expect(marker).toMatchObject({
      intentSeq: '1',
      stage: 'new',
      approvalFeeRaw: '10',
      approvalAmountRaw: '1000060',
      approvalSpender: 'aaaaa-aa',
    });
    const args = mocks.ledgerActor.icrc2_approve.mock.calls[0][0];
    expect(args.fee).toEqual([10n]);
    expect(args.created_at_time).toHaveLength(1);
    expect(args.expires_at).toHaveLength(1);
    expect(args.memo).toHaveLength(1);
    expect(args.memo[0]).toBeInstanceOf(Uint8Array);
    expect(args.memo[0]).toHaveLength(32);
  });

  it('acknowledges only an authenticated duplicate for the exact approval tuple', async () => {
    mocks.ledgerActor.icrc2_approve.mockResolvedValueOnce({ Err: { Duplicate: { duplicate_of: 12_345n } } });
    mocks.poolActor.start_3usd_conversion.mockResolvedValueOnce(pendingStatus(1n));
    const status = await convertStabilityPoolStableTo3Usd(quote(), 980100n, context());
    expect('Pending' in (status as StabilityPoolConversionStatus)).toBe(true);
    expect(mocks.ledgerActor.icrc2_approve).toHaveBeenCalledOnce();
    expect(mocks.poolActor.start_3usd_conversion).toHaveBeenCalledOnce();
    const args = mocks.ledgerActor.icrc2_approve.mock.calls[0][0];
    expect(args.fee).toEqual([10n]);
    expect(args.amount).toBe(1_000_060n);
    expect(args.memo[0]).toBeInstanceOf(Uint8Array);
    expect(readPendingStabilityPoolConversion(mocks.ownerText)).toMatchObject({
      stage: 'start-submitted',
      approvalFeeRaw: '10',
      approvalAmountRaw: '1000060',
    });
  });

  it('resumes a retained request without allocating a new sequence or approving again', async () => {
    localStorage.setItem(`rumi:stability-pool:pending-conversion:${mocks.ownerText}`, JSON.stringify({
      owner: mocks.ownerText, ledger: mocks.ledgerText, principalRaw: '1000000', maxFeeBudgetRaw: '50', minLp: '980100', intentSeq: '7', walletIcon: 'plug', oisy: false, createdAt: Date.now(), stage: 'start-submitted',
    }));
    mocks.poolActor.resume_3usd_conversion.mockResolvedValueOnce({ Completed: { intent_seq: 7n, lp_minted: 980100n, lp_credited: true, uncredited_lp: [], retained_dust_raw: 0n } });
    const status = await convertStabilityPoolStableTo3Usd({ ...quote(), minimum_fee_budget_raw: 50n }, 980100n, context());
    expect('Completed' in (status as StabilityPoolConversionStatus)).toBe(true);
    expect(mocks.poolActor.resume_3usd_conversion).toHaveBeenCalledWith(7n);
    expect(mocks.poolActor.get_3usd_conversion_next_seq).not.toHaveBeenCalled();
    expect(mocks.ledgerActor.icrc2_approve).not.toHaveBeenCalled();
    expect(readPendingStabilityPoolConversion(mocks.ownerText)).toBeNull();
  });

  it('stops before a financial call when the captured wallet changes', async () => {
    const captured = context();
    mocks.wallet.principal = Principal.fromText(mocks.otherOwnerText);
    await expect(convertStabilityPoolStableTo3Usd(quote(), 980100n, captured)).rejects.toThrow(/wallet session changed/i);
    expect(mocks.poolActor.start_3usd_conversion).not.toHaveBeenCalled();
    expect(mocks.ledgerActor.icrc2_approve).not.toHaveBeenCalled();
  });

  it('does not replay after the wallet changes during authenticated recovery lookup', async () => {
    mocks.poolActor.start_3usd_conversion.mockRejectedValueOnce(new Error('transport before acceptance'));
    mocks.poolActor.get_3usd_conversion_status.mockImplementationOnce(async () => {
      mocks.wallet.principal = Principal.fromText(mocks.otherOwnerText);
      return { Err: { NotFound: null } };
    });
    await expect(convertStabilityPoolStableTo3Usd(quote(), 980100n, context())).rejects.toThrow(/exact request/i);
    expect(mocks.poolActor.start_3usd_conversion).toHaveBeenCalledOnce();
    expect(mocks.ledgerActor.icrc2_approve).toHaveBeenCalledOnce();
  });

  it('binds an active request discovered on another device and resumes it', async () => {
    mocks.poolActor.get_active_3usd_conversion_v1.mockResolvedValueOnce([{
      request: { intent_seq: 9n, token_ledger: Principal.fromText(mocks.ledgerText), principal_raw: 1_000_000n, max_fee_budget_raw: 50n, min_lp: 980100n },
      status: pendingStatus(9n),
    }]);
    mocks.poolActor.resume_3usd_conversion.mockResolvedValueOnce(pendingStatus(9n));
    const status = await convertStabilityPoolStableTo3Usd(quote(), 980100n, context());
    expect('Pending' in (status as StabilityPoolConversionStatus)).toBe(true);
    expect(mocks.poolActor.get_3usd_conversion_next_seq).not.toHaveBeenCalled();
    expect(mocks.ledgerActor.icrc2_approve).not.toHaveBeenCalled();
    expect(readPendingStabilityPoolConversion(mocks.ownerText)).toMatchObject({ intentSeq: '9', minLp: '980100' });
  });

  it('does not approve when the quote is paused or rejected', async () => {
    mocks.poolActor.quote_3usd_conversion = vi.fn().mockResolvedValue({ Err: { EmergencyPaused: null } });
    await expect(quoteStabilityPoolConversion(Principal.fromText(mocks.ledgerText), 1_000_000n, 0n, context())).rejects.toThrow(/rejected/i);
    expect(mocks.ledgerActor.icrc2_approve).not.toHaveBeenCalled();
  });
});
