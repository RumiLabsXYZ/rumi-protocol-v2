import { describe, expect, it } from 'vitest';
import { Principal } from '@dfinity/principal';
import {
  KOINU_PER_DOGE,
  POLL_MAX_ATTEMPTS,
  buildAccountArgs,
  buildApproveArgs,
  buildRetrieveWithApprovalArgs,
  classifyRetrieveDogeStatus,
  classifyUtxoStatus,
  computeApprovalAmount,
  dogeToKoinu,
  formatKoinuAsDoge,
  formatWithdrawalFeeSummary,
  isPlausibleDogecoinAddress,
  isPollingExhausted,
  isRetryableUpdateBalanceError,
  isTerminalUtxoKind,
  koinuToDoge,
  parseKoinuInput,
  parseWithdrawalFeeEstimate,
  pollProgressLabel,
  summarizeMinterInfo,
  summarizePendingUtxos,
  summarizeApproveError,
  summarizeRetrieveError,
  summarizeUpdateBalanceError,
  summarizeWithdrawalFeeError,
  summarizeWithdrawalFeeEstimate,
} from './dogeBorrowFlow';

const OWNER = Principal.fromText('zegjz-jpi6k-qkand-c2bgf-qw6za-xk4si-nz3gx-qzzia-fk6fg-snepb-tae');
const MINTER = Principal.fromText('eqltq-xqaaa-aaaar-qb3vq-cai');

describe('koinu/DOGE conversion', () => {
  it('round-trips whole DOGE amounts through koinu', () => {
    expect(dogeToKoinu(1)).toBe(BigInt(KOINU_PER_DOGE));
    expect(koinuToDoge(BigInt(KOINU_PER_DOGE))).toBe(1);
  });

  it('formats koinu as a trimmed DOGE label', () => {
    expect(formatKoinuAsDoge(100_000_000n)).toBe('1 DOGE');
    expect(formatKoinuAsDoge(150_000_000n)).toBe('1.5 DOGE');
    expect(formatKoinuAsDoge(1n)).toBe('0.00000001 DOGE');
    expect(formatKoinuAsDoge(0n)).toBe('0 DOGE');
  });

  it('computes the runtime approval amount as requested + ledger fee', () => {
    expect(computeApprovalAmount(500_000_000n, 100_000n)).toBe(500_100_000n);
  });

  it('builds a fee summary mentioning the dogecoin, minter, and ledger fees separately', () => {
    const summary = formatWithdrawalFeeSummary(25_000n, 50_000n, 100_000n);
    expect(summary).toContain('0.00025 DOGE');
    expect(summary).toContain('0.0005 DOGE');
    expect(summary).toContain('0.001 DOGE');
  });
});

describe('koinu input validation', () => {
  it('accepts positive integers only', () => {
    expect(parseKoinuInput('500000000')).toBe(500_000_000n);
    expect(parseKoinuInput(' 42 ')).toBe(42n);
  });

  it('rejects blank, zero, negative, decimal, and non-numeric input', () => {
    expect(parseKoinuInput('')).toBeNull();
    expect(parseKoinuInput('   ')).toBeNull();
    expect(parseKoinuInput('0')).toBeNull();
    expect(parseKoinuInput('-5')).toBeNull();
    expect(parseKoinuInput('1.5')).toBeNull();
    expect(parseKoinuInput('abc')).toBeNull();
    expect(parseKoinuInput('01')).toBeNull();
  });
});

describe('Dogecoin address validation', () => {
  it('accepts plausible mainnet P2PKH/P2SH-shaped addresses', () => {
    expect(isPlausibleDogecoinAddress('DBXu2kgc3xtvCUWFcxFE3r9hEYgmuaaCyD')).toBe(true);
    expect(isPlausibleDogecoinAddress('9xQzKz9Xg7HqvV3nq9Yy9wq3zL8bF7mN2p')).toBe(true);
  });

  it('rejects blank and clearly impossible strings', () => {
    expect(isPlausibleDogecoinAddress('')).toBe(false);
    expect(isPlausibleDogecoinAddress('   ')).toBe(false);
    expect(isPlausibleDogecoinAddress('not-an-address')).toBe(false);
    expect(isPlausibleDogecoinAddress('1BvBMSEYstWetqTFn5Au4m4GFg7xJaNVN2')).toBe(false); // BTC prefix
    expect(isPlausibleDogecoinAddress('D')).toBe(false); // too short
  });
});

describe('argument builders', () => {
  it('explicitly binds the connected principal with the default subaccount', () => {
    expect(buildAccountArgs(OWNER)).toEqual({ owner: [OWNER], subaccount: [] });
  });

  it('builds ICRC-2 approve args targeting the minter as spender', () => {
    const args = buildApproveArgs(MINTER, 500_100_000n);
    expect(args.spender).toEqual({ owner: MINTER, subaccount: [] });
    expect(args.amount).toBe(500_100_000n);
    expect(args.fee).toEqual([]);
  });

  it('builds retrieve_doge_with_approval args with the requested amount only (fee excluded)', () => {
    const args = buildRetrieveWithApprovalArgs(' DBXu2kgc3xtvCUWFcxFE3r9hEYgmuaaCyD ', 500_000_000n);
    expect(args).toEqual({
      address: 'DBXu2kgc3xtvCUWFcxFE3r9hEYgmuaaCyD',
      amount: 500_000_000n,
      from_subaccount: [],
    });
  });
});

describe('UTXO status classification', () => {
  it('classifies Minted with amount and block index', () => {
    const summary = classifyUtxoStatus({
      Minted: { block_index: 42n, minted_amount: 500_000_000n, utxo: {} },
    });
    expect(summary.kind).toBe('Minted');
    expect(summary.koinuAmount).toBe(500_000_000n);
    expect(summary.blockIndex).toBe(42n);
    expect(summary.label).toContain('5 DOGE');
  });

  it('classifies Checked, ValueTooSmall, and Tainted', () => {
    expect(classifyUtxoStatus({ Checked: {} }).kind).toBe('Checked');
    expect(classifyUtxoStatus({ ValueTooSmall: {} }).kind).toBe('ValueTooSmall');
    expect(classifyUtxoStatus({ Tainted: {} }).kind).toBe('Tainted');
  });

  it('falls back to Unknown for an unrecognized variant tag', () => {
    expect(classifyUtxoStatus({ SomethingNew: {} }).kind).toBe('Unknown');
  });

  it('treats Minted, Tainted, and ValueTooSmall as terminal, Checked as retryable', () => {
    expect(isTerminalUtxoKind('Minted')).toBe(true);
    expect(isTerminalUtxoKind('Tainted')).toBe(true);
    expect(isTerminalUtxoKind('ValueTooSmall')).toBe(true);
    expect(isTerminalUtxoKind('Checked')).toBe(false);
  });

  it('summarizes pending UTXOs with amount and confirmations', () => {
    const pending = summarizePendingUtxos([{ value: 100_000_000n, confirmations: 2 }]);
    expect(pending).toHaveLength(1);
    expect(pending[0].koinuAmount).toBe(100_000_000n);
    expect(pending[0].confirmations).toBe(2);
    expect(pending[0].label).toContain('1 DOGE');
    expect(pending[0].label).toContain('2 confirmations');
  });
});

describe('update_balance error summarization', () => {
  it('extracts current/required confirmations and pending UTXOs from NoNewUtxos', () => {
    const summary = summarizeUpdateBalanceError({
      NoNewUtxos: {
        current_confirmations: [3],
        required_confirmations: 6,
        pending_utxos: [[{ value: 100_000_000n, confirmations: 3 }]],
      },
    });
    expect(summary.kind).toBe('NoNewUtxos');
    expect(summary.currentConfirmations).toBe(3);
    expect(summary.requiredConfirmations).toBe(6);
    expect(summary.pendingUtxos).toHaveLength(1);
    expect(summary.message).toContain('3/6 confirmations');
  });

  it('handles NoNewUtxos with no confirmations yet observed', () => {
    const summary = summarizeUpdateBalanceError({
      NoNewUtxos: { current_confirmations: [], required_confirmations: 6, pending_utxos: [] },
    });
    expect(summary.currentConfirmations).toBeUndefined();
    expect(summary.message).toContain('needs 6 confirmations');
  });

  it('classifies AlreadyProcessing, TemporarilyUnavailable, GenericError, and unknown variants', () => {
    expect(summarizeUpdateBalanceError({ AlreadyProcessing: null }).kind).toBe('AlreadyProcessing');
    expect(summarizeUpdateBalanceError({ TemporarilyUnavailable: 'down' }).message).toContain('down');
    expect(
      summarizeUpdateBalanceError({ GenericError: { error_code: 1n, error_message: 'boom' } }).message
    ).toContain('boom');
    expect(summarizeUpdateBalanceError({ SomethingElse: null }).kind).toBe('Unknown');
  });

  it('flags NoNewUtxos, AlreadyProcessing, and TemporarilyUnavailable as retryable, Generic/unknown as terminal', () => {
    expect(isRetryableUpdateBalanceError('NoNewUtxos')).toBe(true);
    expect(isRetryableUpdateBalanceError('AlreadyProcessing')).toBe(true);
    expect(isRetryableUpdateBalanceError('TemporarilyUnavailable')).toBe(true);
    expect(isRetryableUpdateBalanceError('GenericError')).toBe(false);
    expect(isRetryableUpdateBalanceError('Unknown')).toBe(false);
  });
});

describe('retrieve_doge_status classification', () => {
  it('classifies every known status and decodes a byte-array txid to hex', () => {
    expect(classifyRetrieveDogeStatus({ Pending: null }).kind).toBe('Pending');
    expect(classifyRetrieveDogeStatus({ AmountTooLow: null }).kind).toBe('AmountTooLow');
    expect(classifyRetrieveDogeStatus({ Unknown: null }).kind).toBe('Unknown');

    const sending = classifyRetrieveDogeStatus({ Sending: { txid: [0xde, 0xad, 0xbe, 0xef] } });
    expect(sending.kind).toBe('Sending');
    expect(sending.txid).toBe('deadbeef');

    const confirmed = classifyRetrieveDogeStatus({ Confirmed: { txid: 'already-hex-string' } });
    expect(confirmed.kind).toBe('Confirmed');
    expect(confirmed.txid).toBe('already-hex-string');
  });

  it('falls back to Unknown for an unrecognized variant tag', () => {
    expect(classifyRetrieveDogeStatus({ SomethingNew: null }).kind).toBe('Unknown');
  });

  it('classifies Signing as in-flight, not success', () => {
    expect(classifyRetrieveDogeStatus({ Signing: null }).kind).toBe('Signing');
  });

  it('classifies WillReimburse and Reimbursed as non-success recovery states', () => {
    const willReimburse = classifyRetrieveDogeStatus({ WillReimburse: null });
    expect(willReimburse.kind).toBe('WillReimburse');
    expect(willReimburse.label.toLowerCase()).not.toContain('confirmed');

    const reimbursed = classifyRetrieveDogeStatus({ Reimbursed: null });
    expect(reimbursed.kind).toBe('Reimbursed');
    expect(reimbursed.label.toLowerCase()).not.toContain('confirmed');
  });
});

describe('retrieve error summarization', () => {
  it('renders every known error variant with the relevant number', () => {
    expect(summarizeRetrieveError({ MalformedAddress: 'nope' })).toContain('nope');
    expect(summarizeRetrieveError({ AmountTooLow: 100_000_000n })).toContain('1 DOGE');
    expect(summarizeRetrieveError({ InsufficientFunds: { balance: 50_000_000n } })).toContain('0.5 DOGE');
    expect(summarizeRetrieveError({ InsufficientAllowance: { allowance: 0n } })).toContain('0 DOGE');
    expect(summarizeRetrieveError({ TemporarilyUnavailable: 'busy' })).toContain('busy');
    expect(summarizeRetrieveError({ AlreadyProcessing: null })).toContain('already processing');
    expect(summarizeRetrieveError({ GenericError: { error_code: 1n, error_message: 'oops' } })).toContain('oops');
    expect(summarizeRetrieveError({ SomethingElse: null })).toContain('unknown');
  });
});

describe('ICRC-2 approve error summarization', () => {
  it('formats a BigInt InsufficientFunds payload without throwing', () => {
    expect(() => summarizeApproveError({ InsufficientFunds: { balance: 50_000_000n } })).not.toThrow();
    expect(summarizeApproveError({ InsufficientFunds: { balance: 50_000_000n } })).toContain('0.5 DOGE');
  });

  it('formats a BigInt BadFee payload without throwing', () => {
    expect(() => summarizeApproveError({ BadFee: { expected_fee: 100_000n } })).not.toThrow();
    expect(summarizeApproveError({ BadFee: { expected_fee: 100_000n } })).toContain('0.001 DOGE');
  });

  it('formats every other standard variant with numeric/BigInt fields, never using JSON.stringify', () => {
    expect(summarizeApproveError({ AllowanceChanged: { current_allowance: 0n } })).toContain('0 DOGE');
    expect(summarizeApproveError({ Expired: { ledger_time: 1_700_000_000_000_000_000n } })).toContain('1700000000000000000');
    expect(summarizeApproveError({ TooOld: null })).toContain('too old');
    expect(summarizeApproveError({ CreatedInFuture: { ledger_time: 42n } })).toContain('42');
    expect(summarizeApproveError({ Duplicate: { duplicate_of: 7n } })).toContain('7');
    expect(summarizeApproveError({ TemporarilyUnavailable: null })).toContain('unavailable');
    expect(summarizeApproveError({ GenericError: { error_code: 1n, error_message: 'boom' } })).toContain('boom');
    expect(summarizeApproveError({ SomethingElse: null })).toContain('unknown');
  });
});

describe('minter info summarization', () => {
  it('renders min confirmations, min deposit, and min withdrawal from non-optional fields', () => {
    const summary = summarizeMinterInfo({
      min_confirmations: 6,
      deposit_doge_min_amount: 200_000_000n,
      retrieve_doge_min_amount: 100_000_000n,
    });
    expect(summary.minConfirmationsLabel).toContain('6');
    expect(summary.minDepositLabel).toContain('2 DOGE');
    expect(summary.minWithdrawalLabel).toContain('1 DOGE');
  });
});

describe('withdrawal fee estimate parsing', () => {
  it('extracts dogecoin_fee and minter_fee from the Ok variant', () => {
    const estimate = parseWithdrawalFeeEstimate({ Ok: { dogecoin_fee: 25_000n, minter_fee: 50_000n } });
    expect(estimate).toEqual({ dogecoinFeeKoinu: 25_000n, minterFeeKoinu: 50_000n });
  });

  it('returns null for the Err variant instead of fabricating a fee', () => {
    expect(parseWithdrawalFeeEstimate({ Err: { AmountTooHigh: null } })).toBeNull();
  });

  it('summarizes AmountTooLow with the minimum amount and AmountTooHigh plainly', () => {
    expect(summarizeWithdrawalFeeError({ AmountTooLow: { min_amount: 100_000_000n } })).toContain('1 DOGE');
    expect(summarizeWithdrawalFeeError({ AmountTooHigh: null })).toContain('too high');
    expect(summarizeWithdrawalFeeError({ SomethingElse: null })).toContain('unknown');
  });
});

describe('withdrawal fee estimate summarization (actual Candid variant)', () => {
  it('produces a success summary from Ok without assuming a doge_fee or ledger fee', () => {
    const outcome = summarizeWithdrawalFeeEstimate({ Ok: { dogecoin_fee: 25_000n, minter_fee: 50_000n } });
    expect(outcome.success).toBe(true);
    if (outcome.success) {
      expect(outcome.estimate).toEqual({ dogecoinFeeKoinu: 25_000n, minterFeeKoinu: 50_000n });
      expect(outcome.label).toContain('0.00025 DOGE');
      expect(outcome.label).toContain('0.0005 DOGE');
      expect(outcome.label).not.toContain('ledger fee');
    }
  });

  it('produces an error summary from Err.AmountTooLow and Err.AmountTooHigh', () => {
    const tooLow = summarizeWithdrawalFeeEstimate({ Err: { AmountTooLow: { min_amount: 100_000_000n } } });
    expect(tooLow.success).toBe(false);
    expect(tooLow.label).toContain('1 DOGE');

    const tooHigh = summarizeWithdrawalFeeEstimate({ Err: { AmountTooHigh: null } });
    expect(tooHigh.success).toBe(false);
    expect(tooHigh.label).toContain('too high');
  });
});

describe('polling bounds', () => {
  it('is not exhausted before the max attempt count', () => {
    expect(isPollingExhausted(POLL_MAX_ATTEMPTS - 1)).toBe(false);
  });

  it('is exhausted at and beyond the max attempt count', () => {
    expect(isPollingExhausted(POLL_MAX_ATTEMPTS)).toBe(true);
    expect(isPollingExhausted(POLL_MAX_ATTEMPTS + 1)).toBe(true);
  });

  it('labels progress with the current attempt and the bound', () => {
    expect(pollProgressLabel(5)).toContain(`5 of ${POLL_MAX_ATTEMPTS}`);
  });
});
