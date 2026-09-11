import type { Principal } from '@dfinity/principal';

export const KOINU_PER_DOGE = 100_000_000;

/** Client-side bound on the update_balance confirmation poll. Named per spec: 60s cadence, 120 attempts. */
export const POLL_INTERVAL_MS = 60_000;
export const POLL_MAX_ATTEMPTS = 120;

// Dogecoin mainnet P2PKH addresses start with 'D', P2SH with '9' or 'A', base58 (no 0/O/I/l).
const DOGE_ADDRESS_PATTERN = /^[D9A][a-km-zA-HJ-NP-Z1-9]{24,33}$/;

export function isPlausibleDogecoinAddress(address: string): boolean {
  return DOGE_ADDRESS_PATTERN.test(address.trim());
}

/** Positive integer koinu only — no blanks, decimals, signs, or leading zeroes. */
export function parseKoinuInput(raw: string): bigint | null {
  const trimmed = raw.trim();
  if (!/^[1-9][0-9]*$/.test(trimmed)) return null;
  return BigInt(trimmed);
}

export function koinuToDoge(koinu: bigint): number {
  return Number(koinu) / KOINU_PER_DOGE;
}

export function dogeToKoinu(amountDoge: number): bigint {
  return BigInt(Math.round(amountDoge * KOINU_PER_DOGE));
}

export function formatKoinuAsDoge(koinu: bigint): string {
  const rounded = koinuToDoge(koinu).toFixed(8).replace(/\.?0+$/, '');
  return `${rounded === '' ? '0' : rounded} DOGE`;
}

export function computeApprovalAmount(requestedKoinu: bigint, ledgerFeeKoinu: bigint): bigint {
  return requestedKoinu + ledgerFeeKoinu;
}

export function formatWithdrawalFeeSummary(dogecoinFeeKoinu: bigint, minterFeeKoinu: bigint, ledgerFeeKoinu: bigint): string {
  return `much fee math: ~${formatKoinuAsDoge(dogecoinFeeKoinu)} dogecoin network fee + ${formatKoinuAsDoge(minterFeeKoinu)} minter fee + ${formatKoinuAsDoge(ledgerFeeKoinu)} ledger fee`;
}

export interface WithdrawalFeeEstimate {
  dogecoinFeeKoinu: bigint;
  minterFeeKoinu: bigint;
}

/** estimate_withdrawal_fee returns variant {Ok; Err} — never a bare record, never a `doge_fee` field. */
export function parseWithdrawalFeeEstimate(result: Record<string, any>): WithdrawalFeeEstimate | null {
  if ('Ok' in result) {
    return {
      dogecoinFeeKoinu: BigInt(result.Ok.dogecoin_fee),
      minterFeeKoinu: BigInt(result.Ok.minter_fee),
    };
  }
  return null;
}

export function summarizeWithdrawalFeeError(err: Record<string, any>): string {
  if ('AmountTooLow' in err) {
    return `much smol — that amount is below the minimum this minter will estimate a fee for: ${formatKoinuAsDoge(BigInt(err.AmountTooLow.min_amount))}`;
  }
  if ('AmountTooHigh' in err) {
    return 'wow, such big — that amount is too high to estimate a withdrawal fee for';
  }
  return 'very unknown error estimating the withdrawal fee';
}

export type WithdrawalFeeEstimateOutcome =
  | { success: true; estimate: WithdrawalFeeEstimate; label: string }
  | { success: false; label: string };

/** estimate_withdrawal_fee variant {Ok:{dogecoin_fee, minter_fee}; Err:{AmountTooLow}|{AmountTooHigh}} — no made-up doge_fee, no ledger fee (that's a separate ICRC-2 concern). */
export function summarizeWithdrawalFeeEstimate(result: Record<string, any>): WithdrawalFeeEstimateOutcome {
  if ('Ok' in result) {
    const estimate = parseWithdrawalFeeEstimate(result) as WithdrawalFeeEstimate;
    return {
      success: true,
      estimate,
      label: `much fee math: ~${formatKoinuAsDoge(estimate.dogecoinFeeKoinu)} dogecoin network fee + ${formatKoinuAsDoge(estimate.minterFeeKoinu)} minter fee`,
    };
  }
  return { success: false, label: summarizeWithdrawalFeeError(result.Err ?? {}) };
}

export interface CandidAccountArgs {
  owner: [Principal];
  subaccount: [];
}

/** Explicitly binds the connected non-anonymous principal with the default subaccount. */
export function buildAccountArgs(owner: Principal): CandidAccountArgs {
  return { owner: [owner], subaccount: [] };
}

export interface ApproveArgs {
  spender: { owner: Principal; subaccount: [] };
  amount: bigint;
  fee: [];
  memo: [];
  from_subaccount: [];
  created_at_time: [];
  expected_allowance: [];
  expires_at: [];
}

export function buildApproveArgs(minterPrincipal: Principal, amountKoinu: bigint): ApproveArgs {
  return {
    spender: { owner: minterPrincipal, subaccount: [] },
    amount: amountKoinu,
    fee: [],
    memo: [],
    from_subaccount: [],
    created_at_time: [],
    expected_allowance: [],
    expires_at: [],
  };
}

export interface RetrieveWithApprovalArgs {
  address: string;
  amount: bigint;
  from_subaccount: [];
}

export function buildRetrieveWithApprovalArgs(address: string, amountKoinu: bigint): RetrieveWithApprovalArgs {
  return { address: address.trim(), amount: amountKoinu, from_subaccount: [] };
}

export type UtxoStatusKind = 'Checked' | 'ValueTooSmall' | 'Tainted' | 'Minted' | 'Unknown';

export interface UtxoStatusSummary {
  kind: UtxoStatusKind;
  label: string;
  koinuAmount?: bigint;
  blockIndex?: bigint;
}

export function classifyUtxoStatus(status: Record<string, any>): UtxoStatusSummary {
  if ('Minted' in status) {
    const koinuAmount = BigInt(status.Minted.minted_amount);
    return {
      kind: 'Minted',
      label: `wow, minted! ${formatKoinuAsDoge(koinuAmount)} landed in your wallet`,
      koinuAmount,
      blockIndex: BigInt(status.Minted.block_index),
    };
  }
  if ('Checked' in status) {
    return { kind: 'Checked', label: 'much confirmations, very legit — checked and waiting to mint' };
  }
  if ('ValueTooSmall' in status) {
    return { kind: 'ValueTooSmall', label: 'wow, such smol — that deposit is too tiny to mint' };
  }
  if ('Tainted' in status) {
    return { kind: 'Tainted', label: 'much suspicion — this UTXO got flagged and will not mint' };
  }
  return { kind: 'Unknown', label: 'much mystery status, very unknown' };
}

/** ValueTooSmall, Tainted, and Minted are terminal — Checked keeps polling. */
export function isTerminalUtxoKind(kind: UtxoStatusKind): boolean {
  return kind === 'Minted' || kind === 'Tainted' || kind === 'ValueTooSmall';
}

export interface PendingUtxoSummary {
  koinuAmount: bigint;
  confirmations: number;
  label: string;
}

export function summarizePendingUtxos(pending: Array<{ value: bigint | number; confirmations: number }>): PendingUtxoSummary[] {
  return pending.map((p) => {
    const koinuAmount = BigInt(p.value);
    return {
      koinuAmount,
      confirmations: p.confirmations,
      label: `${formatKoinuAsDoge(koinuAmount)} waiting, ${p.confirmations} confirmations so far`,
    };
  });
}

export type UpdateBalanceErrorKind = 'NoNewUtxos' | 'AlreadyProcessing' | 'TemporarilyUnavailable' | 'GenericError' | 'Unknown';

export interface UpdateBalanceErrorSummary {
  kind: UpdateBalanceErrorKind;
  message: string;
  currentConfirmations?: number;
  requiredConfirmations?: number;
  pendingUtxos?: PendingUtxoSummary[];
}

export function summarizeUpdateBalanceError(err: Record<string, any>): UpdateBalanceErrorSummary {
  if ('NoNewUtxos' in err) {
    const info = err.NoNewUtxos;
    const current: number | undefined = info.current_confirmations?.[0];
    const required: number = info.required_confirmations;
    const pending = summarizePendingUtxos(info.pending_utxos?.[0] ?? []);
    return {
      kind: 'NoNewUtxos',
      message: current !== undefined
        ? `such patience, wow — ${current}/${required} confirmations so far, not minted yet`
        : `no new DOGE spotted yet — needs ${required} confirmations, very waiting`,
      currentConfirmations: current,
      requiredConfirmations: required,
      pendingUtxos: pending,
    };
  }
  if ('AlreadyProcessing' in err) {
    return { kind: 'AlreadyProcessing', message: 'much busy, already checking your balance — hold the leash' };
  }
  if ('TemporarilyUnavailable' in err) {
    return { kind: 'TemporarilyUnavailable', message: `minter says "wow, such downtime": ${err.TemporarilyUnavailable}` };
  }
  if ('GenericError' in err) {
    return { kind: 'GenericError', message: `much error: ${err.GenericError.error_message}` };
  }
  return { kind: 'Unknown', message: 'very unknown error, so confusing' };
}

/** NoNewUtxos, AlreadyProcessing, and TemporarilyUnavailable are expected while waiting — Generic/unknown errors are terminal. */
export function isRetryableUpdateBalanceError(kind: UpdateBalanceErrorKind): boolean {
  return kind === 'NoNewUtxos' || kind === 'AlreadyProcessing' || kind === 'TemporarilyUnavailable';
}

function formatTxid(txid: unknown): string | undefined {
  if (typeof txid === 'string') return txid;
  if (Array.isArray(txid) || txid instanceof Uint8Array) {
    return Array.from(txid as Iterable<number>)
      .map((b) => b.toString(16).padStart(2, '0'))
      .join('');
  }
  return undefined;
}

export type RetrieveStatusKind =
  | 'Unknown'
  | 'Pending'
  | 'Signing'
  | 'Sending'
  | 'Submitted'
  | 'AmountTooLow'
  | 'Confirmed'
  | 'WillReimburse'
  | 'Reimbursed';

export interface RetrieveStatusSummary {
  kind: RetrieveStatusKind;
  label: string;
  txid?: string;
}

export function classifyRetrieveDogeStatus(status: Record<string, any>): RetrieveStatusSummary {
  if ('Confirmed' in status) {
    return { kind: 'Confirmed', label: 'wow, such confirmed — DOGE has landed on chain', txid: formatTxid(status.Confirmed.txid) };
  }
  if ('Submitted' in status) {
    return { kind: 'Submitted', label: 'much broadcast — transaction submitted to the Dogecoin network', txid: formatTxid(status.Submitted.txid) };
  }
  if ('Sending' in status) {
    return { kind: 'Sending', label: 'very in-flight — minter is sending your DOGE now', txid: formatTxid(status.Sending.txid) };
  }
  if ('Signing' in status) {
    return { kind: 'Signing', label: 'much cryptography — minter is signing your withdrawal transaction' };
  }
  if ('Pending' in status) {
    return { kind: 'Pending', label: 'much queue, very pending — minter has your request' };
  }
  if ('AmountTooLow' in status) {
    return { kind: 'AmountTooLow', label: 'wow, such smol amount — below the minimum, try more DOGE' };
  }
  if ('WillReimburse' in status) {
    return { kind: 'WillReimburse', label: 'much oof — withdrawal failed, minter will reimburse your ckDOGE balance soon' };
  }
  if ('Reimbursed' in status) {
    return { kind: 'Reimbursed', label: 'so refund, very sorry — withdrawal failed and your DOGE balance has been reimbursed' };
  }
  return { kind: 'Unknown', label: 'much mystery, very unknown status' };
}

export function summarizeRetrieveError(err: Record<string, any>): string {
  if ('MalformedAddress' in err) return `wow, such bad address: ${err.MalformedAddress}`;
  if ('AmountTooLow' in err) return `much smol — minimum withdrawal is ${formatKoinuAsDoge(BigInt(err.AmountTooLow))}`;
  if ('InsufficientFunds' in err) return `not enough DOGE in your balance — you have ${formatKoinuAsDoge(BigInt(err.InsufficientFunds.balance))}`;
  if ('InsufficientAllowance' in err) return `approval too smol — allowance is only ${formatKoinuAsDoge(BigInt(err.InsufficientAllowance.allowance))}`;
  if ('TemporarilyUnavailable' in err) return `minter napping right now: ${err.TemporarilyUnavailable}`;
  if ('AlreadyProcessing' in err) return 'much patience — a withdrawal is already processing';
  if ('GenericError' in err) return `much error: ${err.GenericError.error_message}`;
  return 'very unknown error, so confusing';
}

/** ICRC-2 icrc2_approve Err variant — numeric/BigInt fields are coerced with String()/BigInt(), never JSON.stringify (which throws on BigInt). */
export function summarizeApproveError(err: Record<string, any>): string {
  if ('BadFee' in err) {
    return `wow, bad fee — ledger wants exactly ${formatKoinuAsDoge(BigInt(err.BadFee.expected_fee))}`;
  }
  if ('InsufficientFunds' in err) {
    return `not enough ckDOGE to cover that approval — you have ${formatKoinuAsDoge(BigInt(err.InsufficientFunds.balance))}`;
  }
  if ('AllowanceChanged' in err) {
    return `much change — allowance is now ${formatKoinuAsDoge(BigInt(err.AllowanceChanged.current_allowance))}, try again`;
  }
  if ('Expired' in err) {
    return `wow, too late — approval expired (ledger time ${String(err.Expired.ledger_time)})`;
  }
  if ('TooOld' in err) {
    return 'much old — this approval request is too old, try again';
  }
  if ('CreatedInFuture' in err) {
    return `wow, time traveler — created_at_time is ahead of ledger time ${String(err.CreatedInFuture.ledger_time)}`;
  }
  if ('Duplicate' in err) {
    return `much duplicate — already submitted as block ${String(err.Duplicate.duplicate_of)}`;
  }
  if ('TemporarilyUnavailable' in err) {
    return 'ledger napping right now, much unavailable, try again soon';
  }
  if ('GenericError' in err) {
    return `much error: ${String(err.GenericError.error_message)}`;
  }
  return 'very unknown approval error, so confusing';
}

export interface MinterInfoSummary {
  minConfirmationsLabel: string;
  minDepositLabel: string;
  minWithdrawalLabel: string;
}

/** get_minter_info's fields are all non-optional — no kyt_fee on this minter. */
export function summarizeMinterInfo(info: {
  min_confirmations: number;
  deposit_doge_min_amount: bigint | number;
  retrieve_doge_min_amount: bigint | number;
}): MinterInfoSummary {
  return {
    minConfirmationsLabel: `much confirmations needed: ${info.min_confirmations}, very patience`,
    minDepositLabel: `min deposit: ${formatKoinuAsDoge(BigInt(info.deposit_doge_min_amount))}`,
    minWithdrawalLabel: `min withdrawal: ${formatKoinuAsDoge(BigInt(info.retrieve_doge_min_amount))}`,
  };
}

export function pollProgressLabel(attempt: number, maxAttempts: number = POLL_MAX_ATTEMPTS): string {
  return `such patience — check ${attempt} of ${maxAttempts}, very watching the blockchain`;
}

export function isPollingExhausted(attempt: number, maxAttempts: number = POLL_MAX_ATTEMPTS): boolean {
  return attempt >= maxAttempts;
}

export function disconnectedWalletCopy(): string {
  return 'wow, such empty wallet. connect your ICP wallet first, much handshake needed before any DOGE magic happens.';
}

export function betaRiskNotice(): string {
  return 'much beta, very new rail. ckDOGE bridging can have bugs — never send more DOGE than you can afford to have stuck. no security promises, no timing promises, just vibes and monitoring.';
}
