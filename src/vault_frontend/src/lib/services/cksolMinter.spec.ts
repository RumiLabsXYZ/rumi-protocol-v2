import { beforeEach, describe, expect, it, vi } from 'vitest';
import { Principal } from '@dfinity/principal';
const fx = vi.hoisted(() => ({
  actors: {} as Record<string, any>, publicActors: {} as Record<string, any>,
  walletState: { isConnected: true, principal: null as any, loading: false }, authState: { isConnected: true, account: null as any, walletType: 'internet-identity' }, generation: 0, signerPrincipal: null as any,
  getActor: vi.fn(async (id: string) => fx.actors[id]), oisy: false,
}));
vi.mock('@dfinity/agent', () => ({ HttpAgent: vi.fn(), Actor: { agentOf: () => ({ getPrincipal: async () => fx.signerPrincipal }), createActor: vi.fn((_idl: any, { canisterId }: any) => fx.publicActors[canisterId]) } }));
vi.mock('../config', () => ({ CONFIG: { isLocal: false, host: 'https://icp-api.io', currentCanisterId: 'protocol' }, CANISTER_IDS: { CKSOL_MINTER: 'lh22c-kyaaa-aaaar-qb5nq-cai', CKSOL_LEDGER: 'ls5lp-lqaaa-aaaar-qb5oa-cai' } }));
vi.mock('../stores/wallet', () => ({ walletStore: { getActor: fx.getActor, subscribe: (fn: any) => { fn(fx.walletState); return () => {}; } } }));
vi.mock('./auth', () => ({ auth: { subscribe: (fn: any) => { fn(fx.authState); return () => {}; } }, walletSessionGeneration: { subscribe: (fn: any) => { fn(fx.generation); return () => {}; } } }));
import { quoteSolWithdrawal, submitSolWithdrawal, loadSolWithdrawal, clearSafeSolIntent, acknowledgeSolWithdrawal, solWithdrawalKey, CKSOL_LEDGER, CKSOL_MINTER } from './cksolMinter';
const owner = Principal.fromText('rrkah-fqaaa-aaaaa-aaaaq-cai');
const address = 'GXVewvv6HehcLFYCmwpqh5CrMjqN9zJ8rqzGq3HEt9Ax';
const store = () => { const data = new Map<string, string>(); return { getItem: (k: string) => data.get(k) ?? null, setItem: (k: string, v: string) => { data.set(k, v); }, removeItem: (k: string) => { data.delete(k); } }; };
const info = { withdrawal_fee: 1_000_000n, minimum_withdrawal_amount: 2_000_000n };
let approve: ReturnType<typeof vi.fn>, withdraw: ReturnType<typeof vi.fn>;
beforeEach(() => {
  vi.clearAllMocks(); fx.oisy = false; fx.walletState = { isConnected: true, principal: owner, loading: false }; fx.authState = { isConnected: true, account: {owner}, walletType: 'internet-identity' }; fx.generation = 0; fx.signerPrincipal = owner;
  // Cached public actors keep the same object across tests.
  fx.publicActors[CKSOL_MINTER] ??= { get_minter_info: vi.fn() };
  fx.publicActors[CKSOL_LEDGER] ??= { icrc1_fee: vi.fn(), icrc1_balance_of: vi.fn(), icrc2_allowance: vi.fn() };
  fx.publicActors[CKSOL_MINTER].get_minter_info.mockResolvedValue(info);
  fx.publicActors[CKSOL_LEDGER].icrc1_fee.mockResolvedValue(500n);
  fx.publicActors[CKSOL_LEDGER].icrc1_balance_of.mockResolvedValue(1_000_000_000n);
  fx.publicActors[CKSOL_LEDGER].icrc2_allowance.mockResolvedValue({ allowance: 0n });
  approve = vi.fn(async () => ({ Ok: 4n })); withdraw = vi.fn(async () => ({ Ok: { block_index: 8n } }));
  fx.actors[CKSOL_LEDGER] = { icrc2_approve: approve }; fx.actors[CKSOL_MINTER] = { withdraw_sol: withdraw };
});
const quote = () => quoteSolWithdrawal(owner, 100_000_000n, address);
describe('ckSOL debit boundary', () => {
  it('persists the intent before approval and burn; uses lamports and exact spender', async () => {
    const storage = store();
    approve.mockImplementation(async (args: any) => {
      expect(loadSolWithdrawal(storage, owner.toText())?.phase).toBe('Approving');
      expect(args.amount).toBe(100_000_500n); expect(args.fee).toEqual([500n]);
      expect(args.expected_allowance).toEqual([0n]); expect(args.spender.owner.toText()).toBe(CKSOL_MINTER);
      return { Ok: 4n };
    });
    withdraw.mockImplementation(async (args: any) => {
      expect(loadSolWithdrawal(storage, owner.toText())?.phase).toBe('Withdrawing');
      expect(args).toEqual({ address, amount: 100_000_000n, from_subaccount: [] });
      return { Ok: { block_index: 8n } };
    });
    const result = await submitSolWithdrawal(await quote(), owner, storage, () => true);
    expect(result).toMatchObject({ phase: 'Submitted', burnBlock: '8', approveBlock: '4' });
  });
  it('halts before burn after approval error', async () => {
    approve.mockResolvedValue({ Err: { InsufficientFunds: { balance: 0n } } });
    expect((await submitSolWithdrawal(await quote(), owner, store(), () => true)).phase).toBe('Rejected');
    expect(withdraw).not.toHaveBeenCalled();
  });
  it('retains an uncertain burn and blocks retry', async () => {
    withdraw.mockRejectedValue(new Error('reply lost')); const storage = store(); const q = await quote();
    expect((await submitSolWithdrawal(q, owner, storage, () => true)).phase).toBe('Uncertain');
    await expect(submitSolWithdrawal(q, owner, storage, () => true)).rejects.toThrow('saved withdrawal');
    expect(() => clearSafeSolIntent(storage, owner.toText())).toThrow('may have burned');
    expect(withdraw).toHaveBeenCalledTimes(1);
  });
  it('stops a stale session after approval', async () => {
    let live = true; approve.mockImplementation(async () => { live = false; return { Ok: 4n }; });
    expect((await submitSolWithdrawal(await quote(), owner, store(), () => live)).phase).toBe('Approved');
    expect(withdraw).not.toHaveBeenCalled();
  });
  it('persists a receipt under the original owner after a session switch during burn', async () => {
    let live = true; const storage = store(); withdraw.mockImplementation(async () => { live = false; return { Ok: { block_index: 77n } }; });
    await submitSolWithdrawal(await quote(), owner, storage, () => live);
    expect(loadSolWithdrawal(storage, owner.toText())?.burnBlock).toBe('77');
  });
  it('fails before approving when persistent storage is unavailable', async () => {
    const storage = store(); storage.setItem = () => { throw new Error('storage blocked'); };
    await expect(submitSolWithdrawal(await quote(), owner, storage, () => true)).rejects.toThrow('storage blocked');
    expect(approve).not.toHaveBeenCalled(); expect(withdraw).not.toHaveBeenCalled();
  });
  it('keeps the known burn block in memory when receipt persistence fails', async () => {
    const storage = store(), original = storage.setItem;
    storage.setItem = (k, v) => { if (JSON.parse(v).phase === 'Submitted') throw new Error('quota'); original(k,v); };
    const result = await submitSolWithdrawal(await quote(), owner, storage, () => true);
    expect(result).toMatchObject({ phase: 'Submitted', burnBlock: '8' }); expect(result.message).toContain('burn block 8');
    expect(loadSolWithdrawal(storage, owner.toText())?.phase).toBe('Withdrawing');
  });
  it('blocks OISY before approval', async () => {
    fx.authState.walletType = 'oisy';
    await expect(submitSolWithdrawal(await quote(), owner, store(), () => true)).rejects.toThrow('OISY consent');
    expect(approve).not.toHaveBeenCalled();
  });
  it('rejects changed live fee terms before approval', async () => {
    const q = await quote(); fx.publicActors[CKSOL_LEDGER].icrc1_fee.mockResolvedValue(600n);
    await expect(submitSolWithdrawal(q, owner, store(), () => true)).rejects.toThrow('terms changed');
    expect(approve).not.toHaveBeenCalled();
  });
  it('does not silently discard corrupt recovery data', () => {
    const storage = store(); storage.setItem(solWithdrawalKey(owner.toText()), '{broken');
    expect(() => loadSolWithdrawal(storage, owner.toText())).toThrow();
  });
});

describe('ckSOL cross-tab journal acknowledgement', () => {
  it('cannot erase a newer unresolved burn from a tab displaying an older success', async () => {
    const storage = store();
    const old = await submitSolWithdrawal(await quote(), owner, storage, () => true);
    acknowledgeSolWithdrawal(storage, old, {intentId: old.id, burnBlock: old.burnBlock!});
    withdraw.mockRejectedValueOnce(new Error('reply lost'));
    const newer = await submitSolWithdrawal(await quote(), owner, storage, () => true);
    expect(newer.phase).toBe('Uncertain');
    expect(() => acknowledgeSolWithdrawal(storage, old, {intentId: old.id, burnBlock: old.burnBlock!})).toThrow('changed in another session');
    expect(loadSolWithdrawal(storage, owner.toText())?.id).toBe(newer.id);
  });
  it('keeps ambiguous typed minter errors locked against duplicate burn attempts', async () => {
    withdraw.mockResolvedValue({ Err: { TemporarilyUnavailable: 'ledger reply unknown' } });
    const storage = store(), result = await submitSolWithdrawal(await quote(), owner, storage, () => true);
    expect(result.phase).toBe('Uncertain');
    expect(() => clearSafeSolIntent(storage, owner.toText())).toThrow('may have burned');
  });
});

describe('ckSOL active signer binding', () => {
  it('blocks a signer whose principal differs from the displayed account', async () => {
    fx.signerPrincipal = Principal.fromText('ryjl3-tyaaa-aaaaa-aaaba-cai');
    await expect(submitSolWithdrawal(await quote(), owner, store(), () => true)).rejects.toThrow('signer principal differs');
    expect(approve).not.toHaveBeenCalled();
  });
  it('blocks new actions while the wallet is loading', async () => {
    fx.walletState.loading = true;
    await expect(submitSolWithdrawal(await quote(), owner, store(), () => true)).rejects.toThrow('Wallet session changed');
    expect(approve).not.toHaveBeenCalled();
  });
  it('stops when generation changes during actor acquisition', async () => {
    fx.getActor.mockImplementationOnce(async (id:string) => { fx.generation++; return fx.actors[id]; });
    await expect(submitSolWithdrawal(await quote(), owner, store(), () => true)).rejects.toThrow('Wallet session changed');
    expect(approve).not.toHaveBeenCalled();
  });
});

describe('ckSOL finality belongs to the displayed burn', () => {
  it('rejects a stale success proof even after loading the newer intent', async () => {
    const storage = store();
    const old = await submitSolWithdrawal(await quote(), owner, storage, () => true);
    const proof = {intentId:old.id,burnBlock:old.burnBlock!};
    acknowledgeSolWithdrawal(storage, old, proof);
    const newer = await submitSolWithdrawal(await quote(), owner, storage, () => true);
    expect(() => acknowledgeSolWithdrawal(storage, newer, proof)).toThrow('may have burned');
    expect(loadSolWithdrawal(storage, owner.toText())?.id).toBe(newer.id);
  });
});

describe('ckSOL signer replacement after approval', () => {
  it('rechecks the actual minter signer before burn dispatch', async () => {
    approve.mockImplementation(async () => { fx.signerPrincipal=Principal.fromText('ryjl3-tyaaa-aaaaa-aaaba-cai'); return {Ok:4n}; });
    const storage=store();
    await expect(submitSolWithdrawal(await quote(),owner,storage,()=>true)).rejects.toThrow('signer principal differs');
    expect(loadSolWithdrawal(storage,owner.toText())?.phase).toBe('Approved');
    expect(withdraw).not.toHaveBeenCalled();
  });
});
