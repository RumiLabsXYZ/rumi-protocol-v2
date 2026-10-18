import { describe, expect, it } from 'vitest';
import { parseSol, formatSol, validateSolanaAddress, depositView, withdrawalView } from './cksolMinterFlow';

describe('ckSOL exact lamports', () => {
  it('round trips amounts above Number precision', () => {
    expect(parseSol('18446744073.709551615')).toBe(18446744073709551615n);
    expect(formatSol(18446744073709551615n)).toBe('18446744073.709551615');
    expect(parseSol('.000000001')).toBe(1n);
  });
  it.each(['1e3', '-1', '0', '1.0000000001', '18446744073.709551616', 'NaN'])('rejects invalid amount %s', (s) => expect(() => parseSol(s)).toThrow());
  it('validates the decoded public key size', () => {
    expect(validateSolanaAddress('GXVewvv6HehcLFYCmwpqh5CrMjqN9zJ8rqzGq3HEt9Ax')).toBe(true);
    expect(validateSolanaAddress('11111111111111111111111111111111')).toBe(true);
    expect(validateSolanaAddress('1111111111111111111111111111111')).toBe(false);
    expect(validateSolanaAddress('0XVewvv6HehcLFYCmwpqh5CrMjqN9zJ8rqzGq3HEt9Ax')).toBe(false);
  });
});
describe('ckSOL terminal truth', () => {
  it('does not call finalized sweep a mint', () => {
    expect(depositView({ Finalized: { signature: 's' } }).success).toBe(false);
    expect(depositView({ Minted: { block_index: 1n, minted_amount: 2n } }).success).toBe(true);
    expect(depositView({ Quarantined: { signature: 's' } })).toMatchObject({ terminal: true, success: false });
  });
  it('only reports SOL arrival for finalized success', () => {
    expect(withdrawalView({ TxFinalized: { Failure: { transaction_id: 's' } } })).toMatchObject({ terminal: true, success: false });
    expect(withdrawalView({ TxSent: { transaction_id: 's' } }).success).toBe(false);
    expect(withdrawalView({ TxFinalized: { Success: { transaction_id: 's' } } }).success).toBe(true);
    expect(withdrawalView({ FutureStatus: null })).toMatchObject({ success: false, terminal: true });
  });
});
