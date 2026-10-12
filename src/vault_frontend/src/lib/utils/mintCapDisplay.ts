const E8S = 100_000_000n;
const UNLIMITED_CAP = (1n << 64n) - 1n;

/** Keep small caps visible and recognize the backend's unlimited value exactly. */
export function formatGlobalMintCap(capE8s: bigint | null): string {
  if (capE8s === null) return 'Unavailable';
  if (capE8s === UNLIMITED_CAP) return 'Unlimited';

  const whole = (capE8s / E8S).toLocaleString('en-US');
  const fraction = (capE8s % E8S).toString().padStart(8, '0').replace(/0+$/, '');
  return `${whole}${fraction ? `.${fraction}` : ''} icUSD`;
}
