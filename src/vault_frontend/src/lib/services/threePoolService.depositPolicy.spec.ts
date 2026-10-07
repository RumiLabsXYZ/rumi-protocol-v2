import { beforeEach, describe, expect, it, vi } from 'vitest';
const mocks = vi.hoisted(() => {
  const principalA = { toText: () => 'aaaaa-aa' };
  const principalB = { toText: () => '2vxsx-fae' };
  return {
    principalA,
    principalB,
    walletState: {
      isConnected: true,
      principal: principalA as any,
    },
    isOisy: false,
    queryResult: { Ok: 100n } as any,
    updateResult: { Ok: { status: { Completed: null }, result_lp: [100n] } } as any,
    queryCalls: 0,
    approvalCalls: [] as string[],
    poolCalls: 0,
    queryActor: {
      calc_add_liquidity_query: vi.fn(async () => {
        mocks.queryCalls += 1;
        return mocks.queryResult;
      }),
    },
    ledgerActor: {
      icrc2_approve: vi.fn(async () => {
        mocks.approvalCalls.push('approve');
        return { Ok: 1n };
      }),
    },
    poolActor: {
      get_next_intent_sequence_v1: vi.fn(async () => [1n]),
      add_liquidity_with_receipt_v1: vi.fn(async () => {
        mocks.poolCalls += 1;
        return mocks.updateResult;
      }),
      add_liquidity: vi.fn(async () => {
        mocks.poolCalls += 1;
        return mocks.updateResult;
      }),
    },
    getOisySignerAgent: vi.fn(async () => ({ kind: 'signer-agent' })),
    createOisyActor: vi.fn((canisterId: string) =>
      canisterId === 'fohh4-yyaaa-aaaap-qtkpa-cai' ? mocks.poolActor : mocks.ledgerActor,
    ),
    walletGetActor: vi.fn(async (canisterId: string) =>
      canisterId === 'fohh4-yyaaa-aaaap-qtkpa-cai' ? mocks.poolActor : mocks.ledgerActor,
    ),
  };
});

vi.mock('@dfinity/agent', () => ({
  Actor: { createActor: vi.fn(() => mocks.queryActor) },
  HttpAgent: vi.fn(class MockHttpAgent {
    fetchRootKey = vi.fn().mockResolvedValue(undefined);
  }),
  AnonymousIdentity: vi.fn(),
}));

vi.mock('../config', () => ({
  CONFIG: { host: 'https://icp0.io', isLocal: false },
  CANISTER_IDS: {
    THREEPOOL: 'fohh4-yyaaa-aaaap-qtkpa-cai',
    ICUSD_LEDGER: 'ryjl3-tyaaa-aaaaa-aaaba-cai',
    CKUSDT_LEDGER: 'rrkah-fqaaa-aaaaa-aaaaq-cai',
    CKUSDC_LEDGER: 'ryjl3-tyaaa-aaaaa-aaaba-cai',
  },
}));

vi.mock('./pnp', () => ({ canisterIDLs: { three_pool: {} } }));

vi.mock('../stores/wallet', () => ({
  walletStore: {
    subscribe: (set: (value: typeof mocks.walletState) => void) => {
      set(mocks.walletState);
      return () => {};
    },
    getActor: mocks.walletGetActor,
  },
}));

vi.mock('./ledgerFeeService', () => ({
  fetchLedgerFee: vi.fn().mockResolvedValue(10n),
  getCachedLedgerFee: vi.fn(() => 10n),
}));

vi.mock('./tokenService', () => ({
  TokenService: { getTokenBalance: vi.fn().mockResolvedValue(0n) },
}));

vi.mock('./protocol/walletOperations', () => ({
  isOisyWallet: () => mocks.isOisy,
}));

vi.mock('./oisySigner', () => ({
  getOisySignerAgent: mocks.getOisySignerAgent,
  createOisyActor: mocks.createOisyActor,
}));

vi.mock('./protocol/oisyResilience', () => ({
  callWithOisyFalseNegativeGuard: async (call: () => Promise<unknown>) => call(),
  OISY_LANDED: { __oisyLanded: true },
}));

import { threePoolService } from './threePoolService';

const AMOUNTS = [1n, 0n, 0n];
const OTHER_AMOUNTS = [2n, 0n, 0n];

describe('threePoolService deposit policy preflight', () => {
  beforeEach(() => {
    vi.useRealTimers();
    mocks.walletState.principal = mocks.principalA;
    mocks.isOisy = false;
    mocks.queryResult = { Ok: 100n };
    mocks.updateResult = { Ok: { status: { Completed: null }, result_lp: [100n] } };
    mocks.queryCalls = 0;
    mocks.approvalCalls.length = 0;
    mocks.poolCalls = 0;
    mocks.poolActor.get_next_intent_sequence_v1.mockClear();
    mocks.poolActor.add_liquidity_with_receipt_v1.mockClear();
    mocks.queryActor.calc_add_liquidity_query.mockClear();
    mocks.ledgerActor.icrc2_approve.mockClear();
    mocks.poolActor.add_liquidity.mockClear();
    mocks.getOisySignerAgent.mockClear();
    mocks.createOisyActor.mockClear();
    mocks.walletGetActor.mockClear();
  });

  it('rejects a query cap error before any non-Oisy approval', async () => {
    mocks.queryResult = { Err: { DepositConcentrationLimitExceeded: null } };

    await expect(threePoolService.addLiquidity(AMOUNTS, 1n)).rejects.toThrow(
      '66.6% icUSD concentration cap',
    );

    expect(mocks.approvalCalls).toHaveLength(0);
    expect(mocks.walletGetActor).not.toHaveBeenCalled();
  });

  it.each([
    ['missing preflight', AMOUNTS, mocks.principalA],
    ['mismatched amount', OTHER_AMOUNTS, mocks.principalA],
  ])('rejects Oisy %s before signer or approval calls', async (_label, amounts, principal) => {
    mocks.isOisy = true;
    mocks.walletState.principal = principal;

    await expect(threePoolService.addLiquidity(amounts, 1n)).rejects.toThrow('quote is stale');

    expect(mocks.getOisySignerAgent).not.toHaveBeenCalled();
    expect(mocks.createOisyActor).not.toHaveBeenCalled();
    expect(mocks.approvalCalls).toHaveLength(0);
  });

  it('rejects Oisy when the cached quote belongs to another wallet', async () => {
    await threePoolService.preflightAddLiquidity(AMOUNTS);
    mocks.isOisy = true;
    mocks.walletState.principal = mocks.principalB;

    await expect(threePoolService.addLiquidity(AMOUNTS, 1n)).rejects.toThrow('quote is stale');

    expect(mocks.getOisySignerAgent).not.toHaveBeenCalled();
    expect(mocks.approvalCalls).toHaveLength(0);
  });

  it('rejects an expired Oisy quote before opening the signer', async () => {
    await threePoolService.preflightAddLiquidity(AMOUNTS);
    mocks.isOisy = true;
    vi.setSystemTime(new Date(Date.now() + 30_001));

    await expect(threePoolService.addLiquidity(AMOUNTS, 1n)).rejects.toThrow('quote is stale');

    expect(mocks.getOisySignerAgent).not.toHaveBeenCalled();
    expect(mocks.approvalCalls).toHaveLength(0);
  });

  it('uses a matching fresh Oisy cache without another network preflight before approval', async () => {
    await threePoolService.preflightAddLiquidity(AMOUNTS);
    mocks.isOisy = true;
    const queryCallsAfterQuote = mocks.queryCalls;

    await threePoolService.addLiquidity(AMOUNTS, 1n);

    expect(mocks.queryCalls).toBe(queryCallsAfterQuote);
    expect(mocks.getOisySignerAgent).toHaveBeenCalledOnce();
    expect(mocks.approvalCalls).toEqual(['approve']);
    expect(mocks.poolCalls).toBe(1);
  });

  it('clears a successful Oisy cache when the next quote fails', async () => {
    await threePoolService.preflightAddLiquidity(AMOUNTS);
    mocks.queryResult = { Err: { DepositConcentrationLimitExceeded: null } };
    await expect(threePoolService.preflightAddLiquidity(AMOUNTS)).rejects.toThrow(
      '66.6% icUSD concentration cap',
    );

    mocks.isOisy = true;
    await expect(threePoolService.addLiquidity(AMOUNTS, 1n)).rejects.toThrow('quote is stale');
    expect(mocks.getOisySignerAgent).not.toHaveBeenCalled();
    expect(mocks.approvalCalls).toHaveLength(0);
  });

  it('formats a cap error returned by the update after approvals', async () => {
    await threePoolService.preflightAddLiquidity(AMOUNTS);
    mocks.updateResult = { Err: { DepositConcentrationLimitExceeded: null } };

    await expect(threePoolService.addLiquidity(AMOUNTS, 1n)).rejects.toThrow(
      '66.6% icUSD concentration cap',
    );

    expect(mocks.approvalCalls).toEqual(['approve']);
    expect(mocks.poolCalls).toBe(1);
  });
});
