/**
 * Format a number as a string with the given number of decimals
 */
export function formatNumber(
  value: number | string | undefined | null, 
  decimals: number = 2, 
  options: Intl.NumberFormatOptions = {}
): string {
  if (value === undefined || value === null) return '0';
  
  const numValue = typeof value === 'string' ? parseFloat(value) : value;
  
  if (isNaN(numValue)) return '0';
  
  const formatter = new Intl.NumberFormat('en-US', {
    minimumFractionDigits: 0,
    maximumFractionDigits: decimals,
    ...options
  });
  
  return formatter.format(numValue);
}

/**
 * Format a number with safety checks for Infinity and NaN
 * @param value Number to format
 * @param maxLength Maximum length (to prevent extremely large numbers)
 */
export function safeFormatNumber(value: number, maxLength: number = 10): string {
  if (!isFinite(value)) return value > 0 ? '∞' : '-∞';
  if (isNaN(value)) return '0';
  
  const formatted = formatNumber(value);
  
  // Cap extremely large numbers
  if (formatted.length > maxLength) {
    return `>${formatted.charAt(0)}e${formatted.length - 1}`;
  }
  
  return formatted;
}

/**
 * Format a number as a currency string
 */
export function formatCurrency(
  value: number | string | undefined | null, 
  currency: string = 'USD', 
  decimals: number = 2
): string {
  return formatNumber(value, decimals, {
    style: 'currency',
    currency
  });
}

/**
 * Format a percentage
 */
export function formatPercent(
  value: number | string | undefined | null,
  decimals: number = 2
): string {
  return formatNumber(value, decimals, {
    style: 'percent',
    maximumFractionDigits: decimals,
    minimumFractionDigits: decimals
  });
}

/**
 * Format an address for display by truncating the middle
 */
export function formatAddress(
  address: string | null | undefined,
  startChars: number = 6,
  endChars: number = 4
): string {
  if (!address) return '';
  if (address.length <= startChars + endChars) return address;
  
  return `${address.slice(0, startChars)}...${address.slice(-endChars)}`;
}

/**
 * Smart token balance formatting — 2 significant decimal digits, no trailing zeros.
 *
 * Rules:
 * - Show at most 2 decimal places when the integer part is non-zero (97.12)
 * - For values < 1, extend decimals until 2 non-zero digits are visible:
 *     0.012 (3 places), 0.0012 (4 places), 0.00012 (5 places), etc.
 * - Never show trailing zeros: 0.0010 → 0.001, 254.20 → 254.2
 * - "0" for zero/invalid values
 */
export function formatTokenBalance(value: number | string | undefined | null): string {
  if (value === undefined || value === null) return '0';
  const num = typeof value === 'string' ? parseFloat(value) : value;
  if (isNaN(num) || num === 0) return '0';

  const abs = Math.abs(num);

  // Determine how many decimal places we need for 2 significant digits
  let decimals: number;
  if (abs >= 1) {
    // Integer part is non-zero — 2 decimal places max
    decimals = 2;
  } else {
    // Count leading zeros after decimal: e.g. 0.0012 has 2 leading zeros
    // magnitude of 0.0012 is -3, so leading zeros = abs(magnitude) - 1 = 2
    const magnitude = Math.floor(Math.log10(abs));
    // We want (leading zeros) + 2 significant digits
    decimals = Math.abs(magnitude) + 1;
  }

  const fixed = num.toFixed(decimals);

  // Strip trailing zeros after the decimal point
  if (fixed.includes('.')) {
    let trimmed = fixed.replace(/0+$/, '');
    if (trimmed.endsWith('.')) trimmed = trimmed.slice(0, -1);
    return trimmed;
  }
  return fixed;
}

// ── Stablecoin formatters ──────────────────────────────────────────────
// All stablecoin amounts (icUSD, ckUSDT, ckUSDC) use these functions.
// They always floor (never round up) — critical for financial UX.

function floorToDecimals(value: number, decimals: number): number {
  const factor = Math.pow(10, decimals);
  return Math.floor(value * factor) / factor;
}

/**
 * Format a stablecoin amount for display contexts (balances, stats, dashboards).
 * Always shows exactly 4 decimal places with trailing zeros, always floors.
 * Example: 1.198 → "1.1980", 0.002 → "0.0020"
 */
export function formatStableDisplay(
  value: number | string | undefined | null,
): string {
  if (value === undefined || value === null) return '0.0000';
  const num = typeof value === 'string' ? parseFloat(value) : value;
  if (isNaN(num)) return '0.0000';
  const floored = floorToDecimals(num, 4);
  return floored.toLocaleString('en-US', {
    minimumFractionDigits: 4,
    maximumFractionDigits: 4,
    useGrouping: true,
  });
}

/**
 * Format a stablecoin amount for transactional contexts (fees, deposits,
 * withdrawals, borrows, repays — anywhere real money moves).
 * Shows up to maxDecimals (8 for icUSD, 6 for ckUSDT/ckUSDC).
 * Always floors. Keeps at least 4 decimal places, trims trailing zeros beyond that.
 *
 * Examples (maxDecimals=8):
 *   0.998      → "0.9980"
 *   1.00234567 → "1.00234567"
 *   0.002      → "0.0020"
 */
export function formatStableTx(
  value: number | string | undefined | null,
  maxDecimals: number = 8,
): string {
  if (value === undefined || value === null) return '0';
  const num = typeof value === 'string' ? parseFloat(value) : value;
  if (isNaN(num) || num === 0) return '0';

  const floored = floorToDecimals(num, maxDecimals);
  const fixed = floored.toFixed(maxDecimals);

  const [intPart, fracPart] = fixed.split('.');
  if (!fracPart) return intPart;

  // Keep at least 4 decimal digits, trim zeros beyond that
  const minKeep = Math.min(4, fracPart.length);
  let trimmed = fracPart;
  while (trimmed.length > minKeep && trimmed.endsWith('0')) {
    trimmed = trimmed.slice(0, -1);
  }

  const formattedInt = parseInt(intPart).toLocaleString('en-US');
  return `${formattedInt}.${trimmed}`;
}

/**
 * Format a BigInt token amount for display context.
 * Converts from raw units (e.g. e8s) to human-readable, then floors to 4 decimals.
 */
export function formatStableTokenDisplay(amount: bigint, decimals: number): string {
  const value = Number(amount) / Math.pow(10, decimals);
  return formatStableDisplay(value);
}

/**
 * Format a BigInt token amount for transactional context.
 * Converts from raw units, floors to native precision, keeps min 4 decimals.
 */
export function formatStableTokenTx(amount: bigint, decimals: number): string {
  const value = Number(amount) / Math.pow(10, decimals);
  return formatStableTx(value, decimals);
}

/**
 * Format a date
 */
export function formatDate(
  timestamp: number | Date,
  options: Intl.DateTimeFormatOptions = {
    year: 'numeric',
    month: 'short',
    day: 'numeric'
  }
): string {
  const date = timestamp instanceof Date ? timestamp : new Date(timestamp);
  return new Intl.DateTimeFormat('en-US', options).format(date);
}