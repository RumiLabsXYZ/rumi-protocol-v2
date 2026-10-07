import { beforeEach, describe, expect, it, vi } from 'vitest';

const mocks = vi.hoisted(() => ({
  actors: {} as Record<string, any>,
  updateCalls: [] as Array<{ principal: any; args: any }>,
  walletGetActor: vi.fn(async (id: string) => mocks.actors[id]),
  isOisyWallet: vi.fn(() => false),
  getOisySignerAgent: vi.fn(async () => ({ signer: 'one signer' })),
  createOisyActor: vi.fn((id: string, _idl: any, agent: any) => ({ ...mocks.actors[id], signer: agent })),
}));

vi.mock('@dfinity/agent', async () => {
  const actual = await vi.importActual<typeof import('@dfinity/agent')>('@dfinity/agent');
  return {
    ...actual,
    HttpAgent: vi.fn(class MockHttpAgent {
      identity: any;
      fetchRootKey = vi.fn();
      constructor(options: any) { this.identity = options.identity; }
    }),
    Actor: {
      ...actual.Actor,
      createActor: vi.fn((_idl: any, { agent, canisterId }: any) => ({
        update_balance: vi.fn(async (args: any) => {
          const principal = agent.identity.getPrincipal();
          if (principal.isAnonymous()) throw new Error('anonymous caller not allowed');
          mocks.updateCalls.push({ principal, args });
          return { Ok: [] };
        }),
        canisterId,
      })),
    },
  };
});
vi.mock('../config', () => ({ CONFIG: { host: 'https://icp-api.io', isLocal: false }, CANISTER_IDS: { CKBTC_MINTER: 'mqygn-kiaaa-aaaar-qaadq-cai', CKBTC_LEDGER: 'mxzaz-hqaaa-aaaar-qaada-cai' } }));
vi.mock('../stores/wallet', () => ({ walletStore: { getActor: mocks.walletGetActor } }));
vi.mock('../idls/ckbtc_minter.idl.js', () => ({ idlFactory: { name: 'ckbtc' } }));
vi.mock('../idls/ledger.idl.js', () => ({ ICRC1_IDL: { name: 'ledger' } }));
vi.mock('./protocol/walletOperations', () => ({ isOisyWallet: mocks.isOisyWallet }));
vi.mock('./oisySigner', () => ({ getOisySignerAgent: mocks.getOisySignerAgent, createOisyActor: mocks.createOisyActor }));

import { Principal } from '@dfinity/principal';
import {
  _resetCkbtcMinterAgents,
  submitCkbtcWithdrawal,
  updateBtcBalanceForOwner,
} from './ckbtcMinterActors';

const owner = Principal.fromText('rrkah-fqaaa-aaaaa-aaaaq-cai');
const minterId = 'mqygn-kiaaa-aaaar-qaadq-cai';
const ledgerId = 'mxzaz-hqaaa-aaaar-qaada-cai';
const approveArgs = { amount: 123n, created_at_time: [1n] };
const retrieveArgs = { amount: 100n, address: '1BoatSLRHtKNngkdXEeobR76b53LETtpyT' };

describe('ckbtcMinterActors', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    _resetCkbtcMinterAgents();
    mocks.updateCalls.length = 0;
    mocks.actors = {};
    mocks.isOisyWallet.mockReturnValue(false);
  });

  it('uses a transient nonanonymous request identity and an explicit owner for update_balance', async () => {
    await updateBtcBalanceForOwner(owner);
    expect(mocks.updateCalls).toHaveLength(1);
    expect(mocks.updateCalls[0].principal.isAnonymous()).toBe(false);
    expect(mocks.updateCalls[0].args).toEqual({ owner: [owner], subaccount: [] });
    expect(mocks.walletGetActor).not.toHaveBeenCalled();
  });

  it('rejects anonymous owners before creating a request or dispatching', async () => {
    await expect(updateBtcBalanceForOwner(Principal.anonymous())).rejects.toThrow('connected wallet principal');
    expect(mocks.updateCalls).toHaveLength(0);
  });

  it('checks the session after agent setup and does not dispatch a stale update_balance call', async () => {
    await expect(updateBtcBalanceForOwner(owner, () => false)).rejects.toThrow('no longer live');
    expect(mocks.updateCalls).toHaveLength(0);
  });

  it('uses one Oisy signer for approval and retrieval, forwarding the exact prepared arguments', async () => {
    mocks.isOisyWallet.mockReturnValue(true);
    const callOrder: string[] = [];
    mocks.actors[ledgerId] = { icrc2_approve: vi.fn(async (args) => { callOrder.push('approve'); expect(args).toBe(approveArgs); return { Ok: 8n }; }) };
    mocks.actors[minterId] = { retrieve_btc_with_approval: vi.fn(async (args) => { callOrder.push('retrieve'); expect(args).toBe(retrieveArgs); return { Ok: { block_index: 9n } }; }) };
    const result = await submitCkbtcWithdrawal({
      owner, ledgerCanisterId: ledgerId, minterCanisterId: minterId, ledgerIdl: { name: 'ledger' },
      approveArgs, retrieveArgs, isLive: () => true,
    });
    expect(result).toEqual({ kind: 'success', approveBlockIndex: 8n, withdrawalBlockIndex: 9n });
    expect(callOrder).toEqual(['approve', 'retrieve']);
    expect(mocks.getOisySignerAgent).toHaveBeenCalledTimes(1);
    expect(mocks.createOisyActor).toHaveBeenCalledTimes(2);
    expect(mocks.createOisyActor.mock.calls[0][2]).toBe(mocks.createOisyActor.mock.calls[1][2]);
  });

  it('halts before retrieval when the ledger returns an approval error', async () => {
    mocks.isOisyWallet.mockReturnValue(true);
    const retrieve = vi.fn();
    mocks.actors[ledgerId] = { icrc2_approve: vi.fn(async () => ({ Err: { BadFee: { expected_fee: 10n } } })) };
    mocks.actors[minterId] = { retrieve_btc_with_approval: retrieve };
    const result = await submitCkbtcWithdrawal({
      owner, ledgerCanisterId: ledgerId, minterCanisterId: minterId, ledgerIdl: { name: 'ledger' },
      approveArgs, retrieveArgs, isLive: () => true,
    });
    expect(result.kind).toBe('approve-error');
    expect(retrieve).not.toHaveBeenCalled();
  });

  it('does not send retrieval if the captured wallet session becomes stale after approval', async () => {
    mocks.isOisyWallet.mockReturnValue(true);
    const retrieve = vi.fn();
    mocks.actors[ledgerId] = { icrc2_approve: vi.fn(async () => ({ Ok: 8n })) };
    mocks.actors[minterId] = { retrieve_btc_with_approval: retrieve };
    let checks = 0;
    const result = await submitCkbtcWithdrawal({
      owner, ledgerCanisterId: ledgerId, minterCanisterId: minterId, ledgerIdl: { name: 'ledger' },
      approveArgs, retrieveArgs, isLive: () => ++checks < 3,
    });
    expect(result).toEqual({ kind: 'approval-only', approveBlockIndex: 8n });
    expect(retrieve).not.toHaveBeenCalled();
  });

  it('returns a definite retrieval success even when the component becomes stale during the call', async () => {
    mocks.isOisyWallet.mockReturnValue(true);
    let live = true;
    mocks.actors[ledgerId] = { icrc2_approve: vi.fn(async () => ({ Ok: 8n })) };
    mocks.actors[minterId] = { retrieve_btc_with_approval: vi.fn(async () => { live = false; return { Ok: { block_index: 9n } }; }) };
    const result = await submitCkbtcWithdrawal({
      owner, ledgerCanisterId: ledgerId, minterCanisterId: minterId, ledgerIdl: { name: 'ledger' },
      approveArgs, retrieveArgs, isLive: () => live,
    });
    expect(result).toEqual({ kind: 'success', approveBlockIndex: 8n, withdrawalBlockIndex: 9n });
  });

  it('reports transport failures as uncertain and never retries the debit call', async () => {
    mocks.isOisyWallet.mockReturnValue(true);
    const retrieve = vi.fn(async () => { throw new Error('connection closed'); });
    mocks.actors[ledgerId] = { icrc2_approve: vi.fn(async () => ({ Ok: 8n })) };
    mocks.actors[minterId] = { retrieve_btc_with_approval: retrieve };
    const result = await submitCkbtcWithdrawal({
      owner, ledgerCanisterId: ledgerId, minterCanisterId: minterId, ledgerIdl: { name: 'ledger' },
      approveArgs, retrieveArgs, isLive: () => true,
    });
    expect(result).toEqual({ kind: 'retrieve-uncertain', approveBlockIndex: 8n, message: 'connection closed' });
    expect(retrieve).toHaveBeenCalledTimes(1);
  });

  it.each([
    ['MalformedAddress', { MalformedAddress: 'invalid address' }, 'safe'],
    ['AmountTooLow', { AmountTooLow: 1n }, 'safe'],
    ['InsufficientFunds', { InsufficientFunds: { balance: 1n } }, 'safe'],
    ['InsufficientAllowance', { InsufficientAllowance: { allowance: 1n } }, 'safe'],
    ['TemporarilyUnavailable', { TemporarilyUnavailable: 'retry later' }, 'unknown'],
    ['GenericError', { GenericError: { error_code: 1n, error_message: 'transfer result unclear' } }, 'unknown'],
    ['AlreadyProcessing', { AlreadyProcessing: null }, 'unknown'],
  ])('classifies %s retrieval errors by retry safety', async (_name, retrievalError, retrySafety) => {
    mocks.isOisyWallet.mockReturnValue(true);
    const retrieve = vi.fn(async () => ({ Err: retrievalError }));
    mocks.actors[ledgerId] = { icrc2_approve: vi.fn(async () => ({ Ok: 8n })) };
    mocks.actors[minterId] = { retrieve_btc_with_approval: retrieve };
    const result = await submitCkbtcWithdrawal({
      owner, ledgerCanisterId: ledgerId, minterCanisterId: minterId, ledgerIdl: { name: 'ledger' },
      approveArgs, retrieveArgs, isLive: () => true,
    });
    expect(result.kind).toBe('retrieve-error');
    if (result.kind === 'retrieve-error') expect(result.retrySafety).toBe(retrySafety);
    expect(retrieve).toHaveBeenCalledTimes(1);
  });
});
