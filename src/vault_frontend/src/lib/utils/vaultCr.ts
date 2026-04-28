/**
 * Compute a vault's collateral ratio as a percent (e.g., 243 for 243%).
 *
 * The backend doesn't store CR on the vault -- it's computed on demand from
 * collateral_amount x price / borrowed_icusd_amount whenever needed. The
 * frontend has the same inputs (vaults from get_all_vaults, prices from
 * twap or last_price), so computing here is straightforward.
 *
 * Returns null when the vault has zero debt (CR is undefined) or when we
 * don't have a price for the collateral (better to skip than show 0).
 */

function principalText(p: any): string {
  if (!p) return '';
  if (typeof p === 'string') return p;
  if (typeof p?.toText === 'function') return p.toText();
  return String(p);
}

export function computeVaultCrPct(
  vault: any,
  priceMap: Map<string, number>,
  decimalsMap: Map<string, number>,
): number | null {
  const debtE8s = Number(vault.borrowed_icusd_amount ?? 0n);
  if (debtE8s <= 0) return null;

  const collType = principalText(vault.collateral_type);
  const price = priceMap.get(collType);
  if (price == null || price <= 0) return null;

  const decimals = decimalsMap.get(collType) ?? 8;
  const collAmount = Number(vault.collateral_amount ?? 0n) / Math.pow(10, decimals);
  const collateralUsd = collAmount * price;
  const debtUsd = debtE8s / 1e8;
  return (collateralUsd / debtUsd) * 100;
}

/**
 * Median of a numeric array. Returns null for empty input.
 * Used for the per-collateral median CR column in CollateralTable.
 */
export function median(values: number[]): number | null {
  if (values.length === 0) return null;
  const sorted = [...values].sort((a, b) => a - b);
  const mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 0
    ? (sorted[mid - 1] + sorted[mid]) / 2
    : sorted[mid];
}
