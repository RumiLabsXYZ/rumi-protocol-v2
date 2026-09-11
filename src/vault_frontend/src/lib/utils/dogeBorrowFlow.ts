import type { Principal } from '@dfinity/principal';

export const KOINU_PER_DOGE = 100_000_000;
export const KOINU_DECIMALS = 8;
/** nat64 upper bound — the wire type for every koinu amount field. */
export const NAT64_MAX_KOINU = 18446744073709551615n;

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

/**
 * Parses a user-entered decimal DOGE amount (e.g. "50", "1.25", "0.00000001")
 * into exact koinu using only string/BigInt arithmetic — never Number/parseFloat,
 * since a float multiply against KOINU_PER_DOGE can misround fractional input.
 * Rejects blank, zero, negative, scientific notation, malformed strings, more
 * than 8 fractional digits (koinu is the smallest unit — no silent rounding),
 * and anything above the nat64 wire bound.
 */
export function parseDogeAmountInput(raw: string): bigint | null {
  const trimmed = raw.trim();
  const match = /^(0|[1-9][0-9]*)(?:\.([0-9]{1,8}))?$/.exec(trimmed);
  if (!match) return null;
  const [, wholePart, fracPart = ''] = match;
  const koinu = BigInt(wholePart + fracPart.padEnd(KOINU_DECIMALS, '0'));
  if (koinu <= 0n || koinu > NAT64_MAX_KOINU) return null;
  return koinu;
}

export function formatKoinuAsDoge(koinu: bigint): string {
  const rounded = koinuToDoge(koinu).toFixed(8).replace(/\.?0+$/, '');
  return `${rounded === '' ? '0' : rounded} DOGE`;
}

export function computeApprovalAmount(requestedKoinu: bigint, ledgerFeeKoinu: bigint): bigint {
  return requestedKoinu + ledgerFeeKoinu;
}

export function formatWithdrawalFeeSummary(dogecoinFeeKoinu: bigint, minterFeeKoinu: bigint, ledgerFeeKoinu: bigint): string {
  return `Network fee ~${formatKoinuAsDoge(dogecoinFeeKoinu)} + minter fee ${formatKoinuAsDoge(minterFeeKoinu)} + ledger fee ${formatKoinuAsDoge(ledgerFeeKoinu)}`;
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
    return `Below the minimum amount this minter will estimate a fee for: ${formatKoinuAsDoge(BigInt(err.AmountTooLow.min_amount))}`;
  }
  if ('AmountTooHigh' in err) {
    return 'That amount is too high to estimate a withdrawal fee for.';
  }
  return 'An unknown error occurred estimating the withdrawal fee.';
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
      label: `Network fee ~${formatKoinuAsDoge(estimate.dogecoinFeeKoinu)} + minter fee ${formatKoinuAsDoge(estimate.minterFeeKoinu)}`,
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
      label: `Minted ${formatKoinuAsDoge(koinuAmount)} into your wallet.`,
      koinuAmount,
      blockIndex: BigInt(status.Minted.block_index),
    };
  }
  if ('Checked' in status) {
    return { kind: 'Checked', label: 'Confirmed and waiting to mint.' };
  }
  if ('ValueTooSmall' in status) {
    return { kind: 'ValueTooSmall', label: 'That deposit is too small to mint.' };
  }
  if ('Tainted' in status) {
    return { kind: 'Tainted', label: 'This deposit was flagged and will not mint.' };
  }
  return { kind: 'Unknown', label: 'Unrecognized deposit status.' };
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
      label: `${formatKoinuAsDoge(koinuAmount)} pending, ${p.confirmations} confirmations so far`,
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
        ? `${current}/${required} confirmations so far. Not minted yet.`
        : `No deposit detected yet, needs ${required} confirmations.`,
      currentConfirmations: current,
      requiredConfirmations: required,
      pendingUtxos: pending,
    };
  }
  if ('AlreadyProcessing' in err) {
    return { kind: 'AlreadyProcessing', message: 'Already checking your balance. Please wait.' };
  }
  if ('TemporarilyUnavailable' in err) {
    return { kind: 'TemporarilyUnavailable', message: `Minter is temporarily unavailable: ${err.TemporarilyUnavailable}` };
  }
  if ('GenericError' in err) {
    return { kind: 'GenericError', message: `Error: ${err.GenericError.error_message}` };
  }
  return { kind: 'Unknown', message: 'An unknown error occurred.' };
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
    return { kind: 'Confirmed', label: 'DOGE has landed on chain.', txid: formatTxid(status.Confirmed.txid) };
  }
  if ('Submitted' in status) {
    return { kind: 'Submitted', label: 'Submitted to the Dogecoin network.', txid: formatTxid(status.Submitted.txid) };
  }
  if ('Sending' in status) {
    return { kind: 'Sending', label: 'Minter is sending your DOGE now.', txid: formatTxid(status.Sending.txid) };
  }
  if ('Signing' in status) {
    return { kind: 'Signing', label: 'Minter is signing your withdrawal transaction.' };
  }
  if ('Pending' in status) {
    return { kind: 'Pending', label: 'Minter has received your request.' };
  }
  if ('AmountTooLow' in status) {
    return { kind: 'AmountTooLow', label: 'Amount is below the minimum. Try a larger amount.' };
  }
  if ('WillReimburse' in status) {
    return { kind: 'WillReimburse', label: 'Withdrawal failed. Minter will reimburse your ckDOGE balance soon.' };
  }
  if ('Reimbursed' in status) {
    return { kind: 'Reimbursed', label: 'Withdrawal failed and your DOGE balance has been reimbursed.' };
  }
  return { kind: 'Unknown', label: 'Unrecognized status.' };
}

export function summarizeRetrieveError(err: Record<string, any>): string {
  if ('MalformedAddress' in err) return `Invalid address: ${err.MalformedAddress}`;
  if ('AmountTooLow' in err) return `Amount is below the minimum withdrawal: ${formatKoinuAsDoge(BigInt(err.AmountTooLow))}`;
  if ('InsufficientFunds' in err) return `Not enough DOGE in your balance. You have ${formatKoinuAsDoge(BigInt(err.InsufficientFunds.balance))}`;
  if ('InsufficientAllowance' in err) return `Approval too small. Allowance is only ${formatKoinuAsDoge(BigInt(err.InsufficientAllowance.allowance))}`;
  if ('TemporarilyUnavailable' in err) return `Minter is temporarily unavailable: ${err.TemporarilyUnavailable}`;
  if ('AlreadyProcessing' in err) return 'A withdrawal is already processing.';
  if ('GenericError' in err) return `Error: ${err.GenericError.error_message}`;
  return 'An unknown error occurred.';
}

/** ICRC-2 icrc2_approve Err variant — numeric/BigInt fields are coerced with String()/BigInt(), never JSON.stringify (which throws on BigInt). */
export function summarizeApproveError(err: Record<string, any>): string {
  if ('BadFee' in err) {
    return `Ledger requires an exact fee of ${formatKoinuAsDoge(BigInt(err.BadFee.expected_fee))}.`;
  }
  if ('InsufficientFunds' in err) {
    return `Not enough ckDOGE to cover that approval. You have ${formatKoinuAsDoge(BigInt(err.InsufficientFunds.balance))}`;
  }
  if ('AllowanceChanged' in err) {
    return `Allowance changed. It is now ${formatKoinuAsDoge(BigInt(err.AllowanceChanged.current_allowance))}. Try again.`;
  }
  if ('Expired' in err) {
    return `Approval expired (ledger time ${String(err.Expired.ledger_time)}).`;
  }
  if ('TooOld' in err) {
    return 'This approval request is too old. Try again.';
  }
  if ('CreatedInFuture' in err) {
    return `Created-at time is ahead of ledger time ${String(err.CreatedInFuture.ledger_time)}.`;
  }
  if ('Duplicate' in err) {
    return `Already submitted as block ${String(err.Duplicate.duplicate_of)}.`;
  }
  if ('TemporarilyUnavailable' in err) {
    return 'Ledger is temporarily unavailable. Try again soon.';
  }
  if ('GenericError' in err) {
    return `Error: ${String(err.GenericError.error_message)}`;
  }
  return 'An unknown approval error occurred.';
}

export interface MinterInfoSummary {
  minConfirmationsLabel: string;
  minDepositLabel: string;
  minWithdrawalLabel: string;
  /** Plain numeric value for two-column stat layouts, no leading "min ..." label text. */
  minDepositValue: string;
  /** Plain numeric value for two-column stat layouts, no leading "min ..." label text. */
  minConfirmationsValue: string;
}

/** get_minter_info's fields are all non-optional — no kyt_fee on this minter. */
export function summarizeMinterInfo(info: {
  min_confirmations: number;
  deposit_doge_min_amount: bigint | number;
  retrieve_doge_min_amount: bigint | number;
}): MinterInfoSummary {
  const minDepositValue = formatKoinuAsDoge(BigInt(info.deposit_doge_min_amount));
  const minConfirmationsValue = `${info.min_confirmations}`;
  return {
    minConfirmationsLabel: `min confirmations: ${info.min_confirmations}`,
    minDepositLabel: `min deposit: ${minDepositValue}`,
    minWithdrawalLabel: `min withdrawal: ${formatKoinuAsDoge(BigInt(info.retrieve_doge_min_amount))}`,
    minDepositValue,
    minConfirmationsValue,
  };
}

export function pollProgressLabel(attempt: number, maxAttempts: number = POLL_MAX_ATTEMPTS): string {
  return `Checking, attempt ${attempt} of ${maxAttempts}.`;
}

export function isPollingExhausted(attempt: number, maxAttempts: number = POLL_MAX_ATTEMPTS): boolean {
  return attempt >= maxAttempts;
}

export function disconnectedWalletCopy(): string {
  return 'Connect your wallet to get a personal ckDOGE deposit address.';
}

export function betaRiskNotice(): string {
  return 'ckDOGE is in beta. Bridging can have bugs, so do not send more DOGE than you can afford to have stuck. There are no security or timing guarantees, but this rail is actively monitored.';
}

/**
 * The compact mint tracker's active step. Deposit stays current until the user
 * clicks "I sent the DOGE"; Confirmations stays current for the whole bounded
 * poll, including after it pauses while still waiting (not just while isPolling
 * is literally true); Minted only lights up once a Minted UTXO is actually observed.
 */
export type MintStepIndex = 1 | 2 | 3;

export function computeMintStepIndex(params: {
  isPolling: boolean;
  pollingStopped: boolean;
  hasMinted: boolean;
}): MintStepIndex {
  if (params.hasMinted) return 3;
  if (params.isPolling || params.pollingStopped) return 2;
  return 1;
}
