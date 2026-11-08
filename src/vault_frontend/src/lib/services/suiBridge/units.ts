/**
 * Sui gas is SUI, which has 9 decimals (1 SUI = 1,000,000,000 MIST). The bridged coin is icUSD, which has 8
 * (1 icUSD = 100,000,000 e8s). They are different units of different assets: nothing here converts one into the other,
 * and amounts stay bigint end to end.
 */

export const MIST_PER_SUI = 1_000_000_000n;

/** SUI for display, from MIST, with up to nine decimals and no trailing zeros. */
export function formatMist(mist: bigint): string {
  const whole = mist / MIST_PER_SUI;
  const fraction = (mist % MIST_PER_SUI).toString().padStart(9, '0').replace(/0+$/, '');
  return fraction === '' ? whole.toString() : `${whole}.${fraction}`;
}
