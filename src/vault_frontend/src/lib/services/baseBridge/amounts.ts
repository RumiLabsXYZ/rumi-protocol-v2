/**
 * Exact icUSD amount handling for the Base bridge.
 *
 * Both sides of the bridge account in e8s (8 decimals): the ICP icUSD ledger,
 * the canister, and the Base contract (`decimals() == 8`, so one token base
 * unit equals one e8s). Amounts are parsed from decimal strings straight into
 * bigint and formatted straight from bigint. No floating point is involved, so
 * the value the user typed is the value that is signed.
 */

export const E8S_DECIMALS = 8;
export const E8S_PER_UNIT = 100_000_000n;
/** The canister and the contract both cap amounts at a u64. */
export const U64_MAX = 18_446_744_073_709_551_615n;

export type AmountParseErrorCode = 'empty' | 'invalid' | 'too_many_decimals' | 'too_large' | 'zero';

export class AmountParseError extends Error {
  readonly code: AmountParseErrorCode;
  constructor(code: AmountParseErrorCode, message: string) {
    super(message);
    this.name = 'AmountParseError';
    this.code = code;
  }
}

export type AmountParseResult =
  | { ok: true; value: bigint }
  | { ok: false; code: AmountParseErrorCode; message: string };

const DECIMAL_PATTERN = /^(\d*)(?:\.(\d*))?$/;

/**
 * Parse a decimal string (for example "12.5") into e8s.
 *
 * Accepts digits with an optional single decimal point and at most 8 digits
 * after it. Rejects signs, exponents, thousands separators, and anything that
 * would not fit in a u64. Zero is rejected unless `allowZero` is set.
 */
export function parseE8s(input: string, opts: { allowZero?: boolean } = {}): bigint {
  const text = (input ?? '').trim();
  if (text === '') throw new AmountParseError('empty', 'Enter an amount.');
  const match = DECIMAL_PATTERN.exec(text);
  if (!match) throw new AmountParseError('invalid', 'Enter a plain number such as 12.5.');
  const whole = match[1] ?? '';
  const fraction = match[2] ?? '';
  if (whole.length + fraction.length === 0) {
    throw new AmountParseError('invalid', 'Enter a plain number such as 12.5.');
  }
  if (fraction.length > E8S_DECIMALS) {
    throw new AmountParseError('too_many_decimals', 'icUSD supports at most 8 decimal places.');
  }
  const padded = fraction.padEnd(E8S_DECIMALS, '0');
  const value = BigInt(whole === '' ? '0' : whole) * E8S_PER_UNIT + BigInt(padded === '' ? '0' : padded);
  if (value > U64_MAX) throw new AmountParseError('too_large', 'That amount is too large.');
  if (value === 0n && !opts.allowZero) throw new AmountParseError('zero', 'Amount must be greater than zero.');
  return value;
}

/** Non-throwing variant of {@link parseE8s} for form validation. */
export function tryParseE8s(input: string, opts: { allowZero?: boolean } = {}): AmountParseResult {
  try {
    return { ok: true, value: parseE8s(input, opts) };
  } catch (cause) {
    if (cause instanceof AmountParseError) return { ok: false, code: cause.code, message: cause.message };
    throw cause;
  }
}

export interface FormatE8sOptions {
  /** Minimum number of fraction digits to show (0 to 8). Default 0 (trailing zeros trimmed). */
  minFraction?: number;
  /** Insert a comma between thousands in the whole part. */
  group?: boolean;
}

/** Format e8s as an exact decimal string. Never rounds. */
export function formatE8s(value: bigint, opts: FormatE8sOptions = {}): string {
  const negative = value < 0n;
  const abs = negative ? -value : value;
  const whole = abs / E8S_PER_UNIT;
  const rawFraction = (abs % E8S_PER_UNIT).toString().padStart(E8S_DECIMALS, '0');
  const minFraction = Math.min(Math.max(opts.minFraction ?? 0, 0), E8S_DECIMALS);
  let fraction = rawFraction.replace(/0+$/, '');
  if (fraction.length < minFraction) fraction = fraction.padEnd(minFraction, '0');
  let wholeText = whole.toString();
  if (opts.group) wholeText = wholeText.replace(/\B(?=(\d{3})+(?!\d))/g, ',');
  return `${negative ? '-' : ''}${wholeText}${fraction ? `.${fraction}` : ''}`;
}

/** Convenience: "12.5 icUSD". */
export function formatIcusd(value: bigint, opts: FormatE8sOptions = {}): string {
  return `${formatE8s(value, opts)} icUSD`;
}

/** Format a wei amount as ETH, truncated (never rounded up) to `maxFraction` digits. */
export function formatWeiAsEth(wei: bigint, maxFraction = 6): string {
  const unit = 10n ** 18n;
  const whole = wei / unit;
  const fraction = (wei % unit).toString().padStart(18, '0').slice(0, Math.max(0, Math.min(maxFraction, 18))).replace(/0+$/, '');
  return `${whole}${fraction ? `.${fraction}` : ''}`;
}
