import type { Principal } from '@dfinity/principal';

export const SATOSHI_PER_BTC = 100_000_000;
export const BTC_DECIMALS = 8;
/** nat64 upper bound — the wire type for every satoshi amount field. */
export const NAT64_MAX_SATOSHI = 18446744073709551615n;

/** Client-side bound on the update_balance confirmation poll. Named per spec: 60s cadence, 120 attempts. */
export const POLL_INTERVAL_MS = 60_000;
export const POLL_MAX_ATTEMPTS = 120;

// Bitcoin mainnet address shapes: bech32, P2PKH (1), or P2SH (3). Checksum verification is left to the minter.
const BTC_ADDRESS_PATTERN = /^(bc1|[13])[a-zA-HJ-NP-Z0-9]{25,90}$/i;

export function isPlausibleBitcoinAddress(address: string): boolean {
  return BTC_ADDRESS_PATTERN.test(address.trim());
}

/** Positive integer satoshi only — no blanks, decimals, signs, or leading zeroes. */
export function parseSatoshiInput(raw: string): bigint | null {
  const trimmed = raw.trim();
  if (!/^[1-9][0-9]*$/.test(trimmed)) return null;
  return BigInt(trimmed);
}

export function satoshiToBtc(satoshi: bigint): number {
  return Number(satoshi) / SATOSHI_PER_BTC;
}

export function btcToSatoshi(amountBtc: number): bigint {
  return BigInt(Math.round(amountBtc * SATOSHI_PER_BTC));
}

/**
 * Parses a user-entered decimal BTC amount (e.g. "50", "1.25", "0.00000001")
 * into exact satoshi using only string/BigInt arithmetic — never Number/parseFloat,
 * since a float multiply against SATOSHI_PER_BTC can misround fractional input.
 * Rejects blank, zero, negative, scientific notation, malformed strings, more
 * than 8 fractional digits (satoshi is the smallest unit — no silent rounding),
 * and anything above the nat64 wire bound.
 */
export function parseBtcAmountInput(raw: string): bigint | null {
  const trimmed = raw.trim();
  const match = /^(0|[1-9][0-9]*)(?:\.([0-9]{1,8}))?$/.exec(trimmed);
  if (!match) return null;
  const [, wholePart, fracPart = ''] = match;
  const satoshi = BigInt(wholePart + fracPart.padEnd(BTC_DECIMALS, '0'));
  if (satoshi <= 0n || satoshi > NAT64_MAX_SATOSHI) return null;
  return satoshi;
}

export function formatSatoshiAsBtc(satoshi: bigint): string {
  const rounded = satoshiToBtc(satoshi).toFixed(8).replace(/\.?0+$/, '');
  return `${rounded === '' ? '0' : rounded} BTC`;
}

export function computeApprovalAmount(requestedSatoshi: bigint, ledgerFeeSatoshi: bigint): bigint {
  return requestedSatoshi + ledgerFeeSatoshi;
}

export function formatWithdrawalFeeSummary(bitcoinFeeSatoshi: bigint, minterFeeSatoshi: bigint, ledgerFeeSatoshi: bigint): string {
  return `Network fee ~${formatSatoshiAsBtc(bitcoinFeeSatoshi)} + minter fee ${formatSatoshiAsBtc(minterFeeSatoshi)} + ledger fee ${formatSatoshiAsBtc(ledgerFeeSatoshi)}`;
}

export interface WithdrawalFeeEstimate {
  bitcoinFeeSatoshi: bigint;
  minterFeeSatoshi: bigint;
}

/** ckBTC estimate_withdrawal_fee returns a bare record with bitcoin_fee and minter_fee. */
export function parseWithdrawalFeeEstimate(result: Record<string, any>): WithdrawalFeeEstimate | null {
  try {
    if (!result || typeof result !== 'object' || !('bitcoin_fee' in result) || !('minter_fee' in result)) return null;
    const bitcoinFeeSatoshi = BigInt(result.bitcoin_fee);
    const minterFeeSatoshi = BigInt(result.minter_fee);
    if (bitcoinFeeSatoshi < 0n || minterFeeSatoshi < 0n) return null;
    return { bitcoinFeeSatoshi, minterFeeSatoshi };
  } catch {
    return null;
  }
}

export function summarizeWithdrawalFeeError(err: unknown): string {
  if (err instanceof Error && err.message) return `Bitcoin minter fee estimate failed: ${err.message}`;
  if (typeof err === 'string' && err.trim()) return `Bitcoin minter fee estimate failed: ${err}`;
  return 'Bitcoin minter could not estimate the withdrawal fee.';
}

export type WithdrawalFeeEstimateOutcome =
  | { success: true; estimate: WithdrawalFeeEstimate; label: string }
  | { success: false; label: string };

/** Summarizes the bare record returned by ckBTC estimate_withdrawal_fee. */
export function summarizeWithdrawalFeeEstimate(result: Record<string, any>): WithdrawalFeeEstimateOutcome {
  const estimate = parseWithdrawalFeeEstimate(result);
  if (!estimate) {
    return { success: false, label: 'Bitcoin minter returned an invalid fee estimate.' };
  }
  return {
    success: true,
    estimate,
    label: `Network fee ~${formatSatoshiAsBtc(estimate.bitcoinFeeSatoshi)} + minter fee ${formatSatoshiAsBtc(estimate.minterFeeSatoshi)}`,
  };
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

export function buildApproveArgs(minterPrincipal: Principal, amountSatoshi: bigint): ApproveArgs {
  return {
    spender: { owner: minterPrincipal, subaccount: [] },
    amount: amountSatoshi,
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

export function buildRetrieveWithApprovalArgs(address: string, amountSatoshi: bigint): RetrieveWithApprovalArgs {
  return { address: address.trim(), amount: amountSatoshi, from_subaccount: [] };
}

export type UtxoStatusKind = 'Checked' | 'ValueTooSmall' | 'Tainted' | 'Minted' | 'Unknown';

export interface UtxoStatusSummary {
  kind: UtxoStatusKind;
  label: string;
  satoshiAmount?: bigint;
  blockIndex?: bigint;
}

export function classifyUtxoStatus(status: Record<string, any>): UtxoStatusSummary {
  if ('Minted' in status) {
    const satoshiAmount = BigInt(status.Minted.minted_amount);
    return {
      kind: 'Minted',
      label: `Minted ${formatSatoshiAsBtc(satoshiAmount)} into your wallet.`,
      satoshiAmount,
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
  satoshiAmount: bigint;
  confirmations: number;
  label: string;
}

export function summarizePendingUtxos(pending: Array<{ value: bigint | number; confirmations: number }>): PendingUtxoSummary[] {
  return pending.map((p) => {
    const satoshiAmount = BigInt(p.value);
    return {
      satoshiAmount,
      confirmations: p.confirmations,
      label: `${formatSatoshiAsBtc(satoshiAmount)} pending, ${p.confirmations} confirmations so far`,
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
      .reverse()
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

export function classifyRetrieveBtcStatus(status: Record<string, any>): RetrieveStatusSummary {
  if ('Confirmed' in status) {
    return { kind: 'Confirmed', label: 'BTC has landed on chain.', txid: formatTxid(status.Confirmed.txid) };
  }
  if ('Submitted' in status) {
    return { kind: 'Submitted', label: 'Submitted to the Bitcoin network.', txid: formatTxid(status.Submitted.txid) };
  }
  if ('Sending' in status) {
    return { kind: 'Sending', label: 'Minter is sending your BTC now.', txid: formatTxid(status.Sending.txid) };
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
    return { kind: 'WillReimburse', label: 'Withdrawal failed. Minter will reimburse your ckBTC balance soon.' };
  }
  if ('Reimbursed' in status) {
    return { kind: 'Reimbursed', label: 'Withdrawal failed and your BTC balance has been reimbursed.' };
  }
  return { kind: 'Unknown', label: 'Unrecognized status.' };
}

export function summarizeRetrieveError(err: Record<string, any>): string {
  if ('MalformedAddress' in err) return `Invalid address: ${err.MalformedAddress}`;
  if ('AmountTooLow' in err) return `Amount is below the minimum withdrawal: ${formatSatoshiAsBtc(BigInt(err.AmountTooLow))}`;
  if ('InsufficientFunds' in err) return `Not enough BTC in your balance. You have ${formatSatoshiAsBtc(BigInt(err.InsufficientFunds.balance))}`;
  if ('InsufficientAllowance' in err) return `Approval too small. Allowance is only ${formatSatoshiAsBtc(BigInt(err.InsufficientAllowance.allowance))}`;
  if ('TemporarilyUnavailable' in err) return `Minter is temporarily unavailable: ${err.TemporarilyUnavailable}`;
  if ('AlreadyProcessing' in err) return 'A withdrawal is already processing.';
  if ('GenericError' in err) return `Error: ${err.GenericError.error_message}`;
  return 'An unknown error occurred.';
}

/** ICRC-2 icrc2_approve Err variant — numeric/BigInt fields are coerced with String()/BigInt(), never JSON.stringify (which throws on BigInt). */
export function summarizeApproveError(err: Record<string, any>): string {
  if ('BadFee' in err) {
    return `Ledger requires an exact fee of ${formatSatoshiAsBtc(BigInt(err.BadFee.expected_fee))}.`;
  }
  if ('InsufficientFunds' in err) {
    return `Not enough ckBTC to cover that approval. You have ${formatSatoshiAsBtc(BigInt(err.InsufficientFunds.balance))}`;
  }
  if ('AllowanceChanged' in err) {
    return `Allowance changed. It is now ${formatSatoshiAsBtc(BigInt(err.AllowanceChanged.current_allowance))}. Try again.`;
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
  /** Same value as minConfirmationsValue, as a number — for the confirmation meter's denominator. */
  minConfirmationsCount: number;
}

/** get_minter_info's fields are all non-optional — no kyt_fee on this minter. */
export function summarizeMinterInfo(info: {
  min_confirmations: number;
  deposit_btc_min_amount: [] | [bigint | number];
  retrieve_btc_min_amount: bigint | number;
}): MinterInfoSummary {
  const minDepositRaw = info.deposit_btc_min_amount[0];
  const minDepositValue = minDepositRaw === undefined ? 'Unavailable' : formatSatoshiAsBtc(BigInt(minDepositRaw));
  const minConfirmationsValue = `${info.min_confirmations}`;
  return {
    minConfirmationsLabel: `min confirmations: ${info.min_confirmations}`,
    minDepositLabel: `min deposit: ${minDepositValue}`,
    minWithdrawalLabel: `min withdrawal: ${formatSatoshiAsBtc(BigInt(info.retrieve_btc_min_amount))}`,
    minDepositValue,
    minConfirmationsValue,
    minConfirmationsCount: info.min_confirmations,
  };
}

export function pollProgressLabel(attempt: number, maxAttempts: number = POLL_MAX_ATTEMPTS): string {
  return `Checking, attempt ${attempt} of ${maxAttempts}.`;
}

export function isPollingExhausted(attempt: number, maxAttempts: number = POLL_MAX_ATTEMPTS): boolean {
  return attempt >= maxAttempts;
}

export function disconnectedWalletCopy(): string {
  return 'Connect your wallet to get a personal ckBTC deposit address.';
}


/**
 * The compact mint tracker's active step. Deposit stays current until the user
 * clicks "I sent the BTC"; Confirmations stays current for the whole bounded
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

/**
 * The confirmation UI's phase. 'waiting' = not polling yet. 'confirming' = actively
 * polling. 'stopped' = the bounded poll (POLL_MAX_ATTEMPTS) exhausted while a deposit
 * was still pending, not an error — the user can manually recheck. 'error' = a
 * terminal failure (bad UTXO, wallet changed mid-check, unexpected exception,
 * or a non-retryable update_balance error). 'minted' = done.
 */
export type ConfirmationPhase = 'waiting' | 'confirming' | 'stopped' | 'error' | 'minted';

/**
 * A confirmation count pair sourced ONLY from real minter data (pending UTXOs, a
 * Checked status, or the NoNewUtxos error's current/required fields) — never
 * synthesized from pollAttempt/maxAttempts. `confirmations` is only ever set when
 * the minter has actually reported a measured value; callers must not render a
 * meter at 0% when this is entirely absent (unknown progress must not imply zero
 * progress).
 */
export interface ConfirmationMeter {
  confirmations: number;
  requiredConfirmations: number;
}

export interface ConfirmationDisplay {
  phase: ConfirmationPhase;
  statusLabel: string;
  /** null when the minter hasn't reported any real confirmation count yet. */
  meter: ConfirmationMeter | null;
  /** Sum of every pending UTXO's detected value, formatted as BTC. null when none detected. */
  amountDetectedLabel: string | null;
  /** e.g. "2 UTXOs detected" — only rendered when more than one UTXO is pending. */
  utxoCountLabel: string | null;
  /** Only set while actively polling — there is no scheduled next check once stopped/errored. */
  nextCheckLabel: string | null;
}

/** 0-100, clamped. Returns 0 only when the meter itself reports 0 confirmations — never as a stand-in for "unknown". */
export function confirmationMeterPercent(meter: ConfirmationMeter): number {
  if (meter.requiredConfirmations <= 0) return 100;
  return Math.max(0, Math.min(100, (meter.confirmations / meter.requiredConfirmations) * 100));
}

export function computeConfirmationDisplay(params: {
  isPolling: boolean;
  pollingStopped: boolean;
  pollAttempt: number;
  maxAttempts?: number;
  mintedSummary: UtxoStatusSummary | null;
  pollFatalMessage: string;
  lastUpdateBalanceError: UpdateBalanceErrorSummary | null;
  utxoStatuses: UtxoStatusSummary[];
  /** get_minter_info's min_confirmations — used only to fill the meter's denominator once a UTXO is Checked (already fully confirmed, waiting to mint), since that response carries no explicit confirmation counts. */
  minConfirmationsHint?: number;
}): ConfirmationDisplay {
  const maxAttempts = params.maxAttempts ?? POLL_MAX_ATTEMPTS;

  if (params.mintedSummary) {
    return {
      phase: 'minted',
      statusLabel: 'Minted',
      meter: null,
      amountDetectedLabel: params.mintedSummary.satoshiAmount !== undefined
        ? formatSatoshiAsBtc(params.mintedSummary.satoshiAmount)
        : null,
      utxoCountLabel: null,
      nextCheckLabel: null,
    };
  }

  if (params.pollFatalMessage) {
    return {
      phase: 'error',
      statusLabel: 'Stopped — needs attention',
      meter: null,
      amountDetectedLabel: null,
      utxoCountLabel: null,
      nextCheckLabel: null,
    };
  }

  const pending = params.lastUpdateBalanceError?.pendingUtxos ?? [];
  const checkedStatus = params.utxoStatuses.find((s) => s.kind === 'Checked');

  let meter: ConfirmationMeter | null = null;
  let amountDetectedLabel: string | null = null;
  let utxoCountLabel: string | null = null;

  if (checkedStatus && params.minConfirmationsHint !== undefined) {
    // Checked means the minter already saw enough confirmations to act on this
    // UTXO — update_balance's Ok/Checked response carries no confirmation count
    // of its own, so the meter reads full against the known requirement.
    meter = { confirmations: params.minConfirmationsHint, requiredConfirmations: params.minConfirmationsHint };
  } else if (pending.length > 0) {
    const minConfirmations = Math.min(...pending.map((p) => p.confirmations));
    meter = { confirmations: minConfirmations, requiredConfirmations: params.lastUpdateBalanceError!.requiredConfirmations! };
    const totalSatoshi = pending.reduce((sum, p) => sum + p.satoshiAmount, 0n);
    amountDetectedLabel = formatSatoshiAsBtc(totalSatoshi);
    utxoCountLabel = pending.length === 1 ? '1 UTXO detected' : `${pending.length} UTXOs detected`;
  } else if (
    params.lastUpdateBalanceError?.currentConfirmations !== undefined &&
    params.lastUpdateBalanceError?.requiredConfirmations !== undefined
  ) {
    meter = {
      confirmations: params.lastUpdateBalanceError.currentConfirmations,
      requiredConfirmations: params.lastUpdateBalanceError.requiredConfirmations,
    };
  }

  const phase: ConfirmationPhase = params.pollingStopped ? 'stopped' : params.isPolling ? 'confirming' : 'waiting';

  const statusLabel = phase === 'stopped'
    ? `Paused after attempt ${params.pollAttempt} of ${maxAttempts}`
    : checkedStatus
      ? 'Confirmed, waiting to mint'
      : meter
        ? `Checking, attempt ${params.pollAttempt} of ${maxAttempts}`
        : `Watching for your deposit, attempt ${params.pollAttempt} of ${maxAttempts}`;

  // The poll cadence is a fixed client-side setTimeout(POLL_INTERVAL_MS) — this is
  // the real scheduled interval, not a guess, so it's only shown while a next
  // check is actually scheduled (i.e. actively polling).
  const nextCheckLabel = phase === 'confirming' ? `Next check in ~${Math.round(POLL_INTERVAL_MS / 1000)}s` : null;

  return { phase, statusLabel, meter, amountDetectedLabel, utxoCountLabel, nextCheckLabel };
}

export const satoshiToBitcoin = satoshiToBtc;
export const formatSatoshiAsBitcoin = formatSatoshiAsBtc;
export const bitcoinToSatoshi = btcToSatoshi;
