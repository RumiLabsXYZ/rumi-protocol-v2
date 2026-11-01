import { beforeEach, describe, expect, it, vi } from 'vitest';
import { Principal } from '@dfinity/principal';
const mocks = vi.hoisted(() => {
  const readable = (initial: any) => { let value = initial; return { subscribe(fn: any) { fn(value); return () => {}; }, set(next: any) { value = next; } }; };
  return { auth: readable({ walletType: 'oisy' }), generation: readable(0), actor: vi.fn(), signer: vi.fn(), walletActor: vi.fn() };
});
vi.mock('./auth', () => ({ auth: mocks.auth, walletSessionGeneration: mocks.generation }));
vi.mock('./minterOisySigner', () => ({ getMinterOisySignerAgent: mocks.signer }));
vi.mock('../stores/wallet', () => ({ walletStore: { getActor: mocks.walletActor } }));
vi.mock('../config', () => ({ CONFIG: { host: 'https://icp-api.io', isLocal: false }, CANISTER_IDS: { CKETH_LEDGER: 'ss2fx-dyaaa-aaaar-qacoq-cai', CKERC20_MINTER: 'sv3dd-oaaaa-aaaar-qacoa-cai' } }));
vi.mock('@dfinity/agent', () => ({ Actor: { createActor: (...args: any[]) => mocks.actor(...args), agentOf: (actor: any) => actor.__agent }, AnonymousIdentity: class {}, HttpAgent: class {} }));
import { approveAndWithdrawCkErc20, getCkErc20WithdrawalQuote } from './ckerc20Minter';
const OWNER = Principal.fromUint8Array(Uint8Array.of(1));
const token = { symbol: 'ckUSDC', decimals: 6, ledgerId: 'xevnm-gaaaa-aaaar-qafnq-cai', erc20Address: `0x${'11'.repeat(20)}`, minimumDepositAmount: 1n };
let ckEth: any, ckToken: any, minter: any;
const agent = { getPrincipal: vi.fn().mockResolvedValue(OWNER) };
beforeEach(() => {
  mocks.auth.set({ walletType: 'oisy' }); mocks.generation.set(0);
  mocks.signer.mockReset().mockReturnValue(agent); agent.getPrincipal.mockReset().mockResolvedValue(OWNER);
  ckEth = { __agent: agent, icrc1_fee: vi.fn().mockResolvedValue(1n), icrc1_balance_of: vi.fn().mockResolvedValue(100n), icrc2_approve: vi.fn().mockResolvedValue({ Ok: 1n }) };
  ckToken = { ...ckEth, icrc2_approve: vi.fn().mockResolvedValue({ Ok: 2n }) };
  minter = { __agent: agent, eip_1559_transaction_price: vi.fn().mockResolvedValue({ max_transaction_fee: 3n, timestamp: [] }), withdraw_erc20: vi.fn().mockResolvedValue({ Ok: { cketh_block_index: 3n, ckerc20_block_index: 4n } }) };
  mocks.actor.mockReset().mockImplementation((_idl, options) => options.canisterId === token.ledgerId ? ckToken : options.canisterId === 'ss2fx-dyaaa-aaaar-qacoq-cai' ? ckEth : minter);
});
async function params(extra: any = {}) {
  return { token, amount: 10n, recipient: `0x${'aa'.repeat(20)}`, owner: OWNER, quote: await getCkErc20WithdrawalQuote(token, 10n, OWNER), isLive: () => true, ...extra };
}
describe('ckERC20 explicit wallet action', () => {
  it('uses one owner-bound Oisy channel for the three explicit consents and closes it', async () => {
    const action = { ready: Promise.resolve(), close: vi.fn().mockResolvedValue(undefined) }, submitted = vi.fn(), resolved = vi.fn();
    const result = await approveAndWithdrawCkErc20(await params({ oisyAction: action, onWithdrawalSubmitted: submitted, onWithdrawalResolved: resolved }));
    expect(result.ckTokenBurnBlock).toBe(4n); expect(mocks.signer).toHaveBeenCalledTimes(3);
    expect(ckEth.icrc2_approve).toHaveBeenCalledTimes(1); expect(ckToken.icrc2_approve).toHaveBeenCalledTimes(1); expect(minter.withdraw_erc20).toHaveBeenCalledTimes(1);
    expect(submitted).toHaveBeenCalledTimes(1); expect(resolved).toHaveBeenCalledWith({ Ok: { cketh_block_index: 3n, ckerc20_block_index: 4n } }); expect(action.close).toHaveBeenCalledTimes(1);
  });
  it('stops after the first approval when the same owner gets a new session', async () => {
    ckEth.icrc2_approve.mockImplementation(async () => { mocks.generation.set(1); return { Ok: 1n }; });
    await expect(approveAndWithdrawCkErc20(await params())).rejects.toThrow('wallet changed');
    expect(ckToken.icrc2_approve).not.toHaveBeenCalled(); expect(minter.withdraw_erc20).not.toHaveBeenCalled();
  });
  it('verifies the actor principal before any wallet call', async () => {
    agent.getPrincipal.mockResolvedValue(Principal.fromUint8Array(Uint8Array.of(2)));
    await expect(approveAndWithdrawCkErc20(await params())).rejects.toThrow('wallet signer changed');
    expect(ckEth.icrc2_approve).not.toHaveBeenCalled();
  });
  it('keeps uncertain withdrawal submission unresolved after a lost response', async () => {
    const submitted = vi.fn(), resolved = vi.fn(); minter.withdraw_erc20.mockRejectedValue(new Error('reply lost'));
    await expect(approveAndWithdrawCkErc20(await params({ onWithdrawalSubmitted: submitted, onWithdrawalResolved: resolved }))).rejects.toThrow('reply lost');
    expect(submitted).toHaveBeenCalledOnce(); expect(resolved).not.toHaveBeenCalled(); expect(minter.withdraw_erc20).toHaveBeenCalledOnce();
  });
  it('returns the partial ckETH burn evidence to durable storage before reporting token rejection', async () => {
    const resolved = vi.fn(), response = { Err: { CkErc20LedgerError: { cketh_block_index: 9n, error: { TemporarilyUnavailable: 'busy' } } } }; minter.withdraw_erc20.mockResolvedValue(response);
    await expect(approveAndWithdrawCkErc20(await params({ onWithdrawalResolved: resolved }))).rejects.toThrow('ckETH burn block: 9');
    expect(resolved).toHaveBeenCalledWith(response);
  });
  it('closes the wallet channel when validation fails before the first consent', async () => {
    const action = { ready: Promise.resolve(), close: vi.fn().mockResolvedValue(undefined) };
    await expect(approveAndWithdrawCkErc20(await params({ recipient: 'bad', oisyAction: action }))).rejects.toThrow('valid Ethereum address');
    expect(action.close).toHaveBeenCalledOnce(); expect(ckEth.icrc2_approve).not.toHaveBeenCalled();
  });
});

it('does not replace an accepted withdrawal result with a popup cleanup rejection', async () => {
  const close = vi.fn().mockRejectedValue(new Error('popup gone'));
  const result = await approveAndWithdrawCkErc20(await params({ oisyAction: { ready: Promise.resolve(), close } }));
  expect(result.ckEthBurnBlock).toBe(3n); expect(result.ckTokenBurnBlock).toBe(4n);
  expect(close).toHaveBeenCalledOnce(); expect(minter.withdraw_erc20).toHaveBeenCalledOnce();
});
