import { describe, expect, it, beforeEach, vi } from 'vitest';
import { dogeWithdrawalKey, readDogeWithdrawal, replaceDogeWithdrawal, isDogeWithdrawalTerminal, withDogeWithdrawalLock, type DogeWithdrawal } from './dogeMinterRecovery';
const record: DogeWithdrawal = { version: 1, id: 'first', owner: 'A', minter: 'minter', address: 'destination', amount: '100', createdAt: 1, phase: 'retrieving', approveBlock: '20' };
describe('DOGE withdrawal persistence', () => {
  beforeEach(() => localStorage.clear());
  it('restores an uncertain dispatch without treating it as permission to replay', () => {
    replaceDogeWithdrawal(localStorage, null, record, 'A', 'minter');
    expect(readDogeWithdrawal(localStorage, 'A', 'minter')).toEqual(record);
    expect(isDogeWithdrawalTerminal(readDogeWithdrawal(localStorage, 'A', 'minter')?.status)).toBe(false);
  });
  it('keeps A separate through an A -> B -> A wallet switch', () => {
    replaceDogeWithdrawal(localStorage, null, record, 'A', 'minter');
    expect(readDogeWithdrawal(localStorage, 'B', 'minter')).toBeNull();
    expect(readDogeWithdrawal(localStorage, 'A', 'minter')).toEqual(record);
  });
  it('compares the exact saved lineage before releasing it', () => {
    replaceDogeWithdrawal(localStorage, null, record, 'A', 'minter');
    const newer = { ...record, id: 'newer' };
    replaceDogeWithdrawal(localStorage, record, newer, 'A', 'minter');
    expect(() => replaceDogeWithdrawal(localStorage, record, null, 'A', 'minter')).toThrow('another tab');
    expect(readDogeWithdrawal(localStorage, 'A', 'minter')).toEqual(newer);
  });
  it.each(['Unknown', 'Pending', 'Submitted', 'WillReimburse', 'AmountTooLow', undefined])('does not release %s', status => {
    expect(isDogeWithdrawalTerminal(status)).toBe(false);
  });
  it.each(['Confirmed', 'Reimbursed'])('retains a terminal %s receipt', status => {
    const terminal: DogeWithdrawal = { ...record, phase: 'terminal', burnBlock: '30', status };
    replaceDogeWithdrawal(localStorage, null, terminal, 'A', 'minter');
    expect(readDogeWithdrawal(localStorage, 'A', 'minter')).toEqual(terminal);
    expect(isDogeWithdrawalTerminal(status)).toBe(true);
  });
  it('fails closed and preserves unreadable saved evidence', () => {
    localStorage.setItem(dogeWithdrawalKey('A', 'minter'), '{broken');
    expect(() => readDogeWithdrawal(localStorage, 'A', 'minter')).toThrow();
    expect(localStorage.getItem(dogeWithdrawalKey('A', 'minter'))).toBe('{broken');
  });
  it('requires a cross-tab lock before spending', async () => {
    vi.stubGlobal('navigator', {});
    const run = vi.fn();
    await expect(withDogeWithdrawalLock('A', 'minter', run)).rejects.toThrow('Web Locks');
    expect(run).not.toHaveBeenCalled();
    vi.unstubAllGlobals();
  });
});
