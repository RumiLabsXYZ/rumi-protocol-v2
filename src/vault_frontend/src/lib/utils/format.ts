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