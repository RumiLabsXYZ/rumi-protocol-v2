import { describe, expect, it } from 'vitest';
import { Principal } from '@dfinity/principal';
import { IDL } from '@dfinity/candid';
import { idlFactory } from '../idls/ckerc20_minter.idl.js';
import { advanceCkErc20MintReceipt, compareAndSetCkErc20Journal, ckErc20ReceiptOutcome, type CkErc20MintReceipt } from './ckerc20Reconciliation';
const owner = Principal.fromUint8Array(Uint8Array.of(1));
const receipt: CkErc20MintReceipt = { hash: `0x${'ab'.repeat(32)}`, principal: owner.toText(), evmAccount: `0x${'aa'.repeat(20)}`, tokenLedgerId: 'xevnm-gaaaa-aaaar-qafnq-cai', erc20Address: `0x${'11'.repeat(20)}`, tokenSymbol: 'ckUSDC', atomicAmount: '2000000', eventCursor: '100' };
const accepted = { transaction_hash: receipt.hash, principal: owner, subaccount: [], from_address: receipt.evmAccount, value: 2000000n, erc20_contract_address: receipt.erc20Address, log_index: 7n, block_number: 1n };
const minted = { event_source: { transaction_hash: receipt.hash, log_index: 7n }, erc20_contract_address: receipt.erc20Address, mint_block_index: 55n, ckerc20_token_symbol: 'ckUSDC' };
const event = (payload: any) => ({ timestamp: 1n, payload });

describe('ckERC20 exact reconciliation', () => {
  it('joins the accepted deposit and mint across bounded pages and preserves the ledger receipt', () => {
    const first = advanceCkErc20MintReceipt(receipt, [event({ AcceptedErc20Deposit: accepted })]);
    expect(first.eventCursor).toBe('101'); expect(first.eventLogIndex).toBe('7'); expect(first.mintBlock).toBeUndefined();
    const next = advanceCkErc20MintReceipt(first, [event({ MintedCkErc20: minted })]);
    expect(next.eventCursor).toBe('102'); expect(next.mintBlock).toBe('55');
  });
  it.each([
    { principal: Principal.fromUint8Array(Uint8Array.of(2)) }, { value: 1n }, { erc20_contract_address: `0x${'22'.repeat(20)}` },
    { from_address: `0x${'bb'.repeat(20)}` }, { transaction_hash: `0x${'cd'.repeat(32)}` }, { subaccount: [Uint8Array.of(1)] },
  ])('rejects an accepted deposit with a mismatched frozen field %#', (change) => {
    expect(advanceCkErc20MintReceipt(receipt, [event({ AcceptedErc20Deposit: { ...accepted, ...change } }), event({ MintedCkErc20: minted })]).mintBlock).toBeUndefined();
  });
  it.each([{ event_source: { ...minted.event_source, log_index: 8n } }, { ckerc20_token_symbol: 'ckLINK' }, { erc20_contract_address: `0x${'22'.repeat(20)}` }])('rejects a different mint source/token %#', (change) => {
    expect(advanceCkErc20MintReceipt(receipt, [event({ AcceptedErc20Deposit: accepted }), event({ MintedCkErc20: { ...minted, ...change } })]).mintBlock).toBeUndefined();
  });
  it('never advances beyond the returned event count and never overwrites a newer cursor', () => {
    const old = JSON.stringify(receipt), next = advanceCkErc20MintReceipt(receipt, []);
    expect(next.eventCursor).toBe('100');
    const values = new Map([['key', old]]), storage = { getItem: (key: string) => values.get(key) ?? null, setItem: (key: string, value: string) => values.set(key, value), removeItem: (key: string) => values.delete(key) };
    const newer = JSON.stringify({ ...receipt, eventCursor: '120', eventLogIndex: '7' }); storage.setItem('key', newer);
    expect(compareAndSetCkErc20Journal(storage, 'key', old, JSON.stringify(next))).toBe(false); expect(storage.getItem('key')).toBe(newer);
  });
  it('does not resolve malformed or mismatched Ethereum receipts', () => {
    expect(ckErc20ReceiptOutcome(receipt.hash, { status: '0x1' })).toBe('pending');
    expect(ckErc20ReceiptOutcome(receipt.hash, { transactionHash: 'other', status: '0x1', blockHash: 'x', blockNumber: '0x1' })).toBe('pending');
  });
});

describe('deployed ckETH Candid projection (metadata verified 2026-10-31)', () => {
  const service: any = idlFactory({ IDL });
  const method = (name: string): any => service._fields.find(([key]: any) => key === name)[1];
  it('encodes the exact nat64 withdrawal search and decodes terminal reimbursement', () => {
    const func = method('withdrawal_status');
    expect(func.annotations).toEqual(['query']);
    expect(() => IDL.encode(func.argTypes, [{ ByWithdrawalId: 3n }])).not.toThrow();
    expect(() => IDL.encode(func.argTypes, [{ ByWithdrawalId: 1n << 64n }])).toThrow();
    const value = [{ token_symbol: 'ckUSDC', withdrawal_amount: 1n, max_transaction_fee: [], withdrawal_id: 3n, from: owner, from_subaccount: [], recipient_address: receipt.evmAccount, status: { TxFinalized: { Reimbursed: { transaction_hash: receipt.hash, reimbursed_amount: 1n, reimbursed_in_block: 4n } } } }];
    expect(IDL.decode(func.retTypes, IDL.encode(func.retTypes, [value]))[0]).toEqual(value);
  });
  it('decodes exact interested events and safely discards unrelated full payloads', () => {
    const func = method('get_events');
    expect(func.annotations).toEqual(['query']);
    expect(() => IDL.encode(func.argTypes, [{ start: 0n, length: 1000n }])).not.toThrow();
    expect(() => IDL.encode(func.argTypes, [{ start: 1n << 64n, length: 1n }])).toThrow();
    const value = { events: [event({ AcceptedErc20Deposit: accepted }), event({ MintedCkErc20: minted })], total_event_count: 102n };
    expect(IDL.decode(func.retTypes, IDL.encode(func.retTypes, [value]))[0]).toEqual(value);
    const full = IDL.Record({ events: IDL.Vec(IDL.Record({ timestamp: IDL.Nat64, payload: IDL.Variant({ SyncedToBlock: IDL.Record({ block_number: IDL.Nat }) }) })), total_event_count: IDL.Nat64 });
    const decoded: any = IDL.decode(func.retTypes, IDL.encode([full], [{ events: [event({ SyncedToBlock: { block_number: 99n } })], total_event_count: 1n }]))[0];
    expect(decoded.events[0].payload).toHaveProperty('SyncedToBlock');
  });
});

it('retires withdrawal intent only for explicit pre-burn rejection variants', async () => {
  const { ckErc20WithdrawalHasDefinitiveNoBurn: noBurn } = await import('./ckerc20Reconciliation');
  expect(noBurn({ Err: { RecipientAddressBlocked: { address: receipt.evmAccount } } })).toBe(true);
  expect(noBurn({ Err: { CkEthLedgerError: { error: { InsufficientFunds: { balance: 0n } } } } })).toBe(true);
  expect(noBurn({ Err: { CkEthLedgerError: { error: { TemporarilyUnavailable: 'reply lost' } } } })).toBe(false);
  expect(noBurn({ Err: { TemporarilyUnavailable: 'busy' } })).toBe(false);
  expect(noBurn({ Err: { GenericError: {} } })).toBe(false);
  expect(noBurn({ Err: { CkErc20LedgerError: { cketh_block_index: 3n, error: { InsufficientFunds: {} } } } })).toBe(false);
});
