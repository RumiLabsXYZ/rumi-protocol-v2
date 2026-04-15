import type { VaultDTO, CollateralInfo } from '../../services/types';
import { CANISTER_IDS } from '../../config';

export type HealthTier = 'safe' | 'caution' | 'danger' | 'no-debt' | 'unknown';

export interface AssetBreakdown {
  principal: string;       // Ledger canister principal text
  symbol: string;          // "ICP", "ckBTC", "ckXAUT"
  nativeAmount: number;    // Sum of collateral across vaults of this type
  usdValue: number;        // nativeAmount * priceUsd (0 if price unknown)
  hasPrice: boolean;       // False if we had to treat this as $0 for totals
}

export interface PositionSummary {
  totalCollateralUsd: number;
  totalBorrowed: number;        // In icUSD (human-readable)
  overallCr: number;            // Infinity when totalBorrowed === 0
  healthTier: HealthTier;
  perCollateral: AssetBreakdown[]; // Sorted by usdValue desc; zero-balance omitted
  hasAnyMissingPrice: boolean;  // True if any collateral lacked a price
}

/**
 * Resolve the collateral principal for a vault. Vaults created before the
 * multi-collateral upgrade have an empty collateralType and should fall back
 * to the ICP ledger.
 */
export function resolveCollateralPrincipal(vault: VaultDTO): string {
  return vault.collateralType || CANISTER_IDS.ICP_LEDGER;
}

/**
 * Return the vault's collateral amount in native units, preferring the
 * multi-collateral field and falling back to the legacy icpMargin field.
 */
export function getCollateralAmount(vault: VaultDTO): number {
  return vault.collateralAmount ?? vault.icpMargin ?? 0;
}

/**
 * Look up collateral info by principal. Returns undefined if unknown.
 */
export function findCollateralInfo(
  collaterals: CollateralInfo[],
  principal: string,
): CollateralInfo | undefined {
  return collaterals.find(c => c.principal === principal);
}

/**
 * Map a collateral ratio (as a ratio, e.g. 2.81 for 281%) to a health tier.
 *   >= 2.0  -> safe
 *   1.5..2  -> caution
 *   < 1.5   -> danger
 * Infinity (no debt) -> 'no-debt'. NaN -> 'unknown'.
 *
 * NOTE: This is an aggregate heuristic across all the user's vaults, not a
 * per-vault liquidation signal. Individual vault liquidation prices are
 * displayed on VaultCard.
 */
export function healthTierFor(cr: number): HealthTier {
  if (!Number.isFinite(cr)) return cr === Infinity ? 'no-debt' : 'unknown';
  if (cr >= 2.0) return 'safe';
  if (cr >= 1.5) return 'caution';
  return 'danger';
}

/**
 * Aggregate a user's vaults into a single PositionSummary.
 *
 * Algorithm:
 *  1. Group vaults by collateral principal.
 *  2. For each group, sum native amounts and compute USD using the group's
 *     price from CollateralInfo (or 0 if unknown). Track missing-price flag.
 *  3. Totals = sum across groups (collateral USD) and across vaults (debt).
 *  4. Overall CR = totalCollateralUsd / totalBorrowed (Infinity if no debt).
 *  5. Emit per-collateral breakdown sorted by USD value desc, skipping
 *     any group whose nativeAmount is 0.
 */
export function aggregatePosition(
  vaults: VaultDTO[],
  collaterals: CollateralInfo[],
): PositionSummary {
  // Group native amounts by principal.
  const byPrincipal = new Map<string, number>();
  let totalBorrowed = 0;

  for (const v of vaults) {
    const principal = resolveCollateralPrincipal(v);
    const amount = getCollateralAmount(v);
    byPrincipal.set(principal, (byPrincipal.get(principal) ?? 0) + amount);
    totalBorrowed += v.borrowedIcusd || 0;
  }

  let totalCollateralUsd = 0;
  let hasAnyMissingPrice = false;
  const perCollateral: AssetBreakdown[] = [];

  for (const [principal, nativeAmount] of byPrincipal) {
    if (nativeAmount === 0) continue;
    const info = findCollateralInfo(collaterals, principal);
    const price = info?.price ?? 0;
    const hasPrice = price > 0;
    const usdValue = hasPrice ? nativeAmount * price : 0;
    if (!hasPrice) hasAnyMissingPrice = true;
    totalCollateralUsd += usdValue;
    perCollateral.push({
      principal,
      symbol: info?.symbol ?? principal.slice(0, 5),
      nativeAmount,
      usdValue,
      hasPrice,
    });
  }

  perCollateral.sort((a, b) => b.usdValue - a.usdValue);

  const overallCr = totalBorrowed > 0
    ? totalCollateralUsd / totalBorrowed
    : Infinity;

  return {
    totalCollateralUsd,
    totalBorrowed,
    overallCr,
    healthTier: healthTierFor(overallCr),
    perCollateral,
    hasAnyMissingPrice,
  };
}
