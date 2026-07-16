import type { VaultDTO, CollateralInfo } from '../../services/types';
import { CANISTER_IDS } from '../../config';

// ICP's own defaults (mirrors $lib/protocol's MINIMUM_CR/LIQUIDATION_CR).
// Restated locally rather than imported so this module stays a pure,
// dependency-light aggregation utility (protocol.ts pulls in the wallet/
// canister-agent stack via collateralStore -> tokenService -> pnp).
const ICP_MINIMUM_CR = 1.5;
const ICP_LIQUIDATION_CR = 1.33;

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
 * Ratios implied by ICP's own thresholds (borrow threshold 1.5, liquidation
 * 1.33, legacy flat "safe" cutoff 2.0). Applied to a blended liquidationCr so a
 * non-ICP (or mixed-collateral) position gets a proportionally equivalent
 * buffer instead of being judged against ICP's flat numbers.
 */
const CAUTION_RATIO = ICP_MINIMUM_CR / ICP_LIQUIDATION_CR; // ≈1.128
const SAFE_RATIO = 2.0 / ICP_LIQUIDATION_CR;                // ≈1.504

/**
 * Map a collateral ratio to a health tier against caution/safe cutoffs.
 *   >= safeCr        -> safe
 *   cautionCr..safeCr -> caution
 *   < cautionCr      -> danger
 * Infinity (no debt) -> 'no-debt'. NaN -> 'unknown'.
 *
 * cautionCr/safeCr default to ICP's own thresholds so callers that don't yet
 * have a collateral mix (e.g. isolated unit tests) keep the old behavior.
 *
 * NOTE: This is an aggregate heuristic across all the user's vaults, not a
 * per-vault liquidation signal. Individual vault liquidation prices are
 * displayed on VaultCard.
 */
export function healthTierFor(
  cr: number,
  cautionCr: number = ICP_MINIMUM_CR,
  safeCr: number = 2.0,
): HealthTier {
  if (!Number.isFinite(cr)) return cr === Infinity ? 'no-debt' : 'unknown';
  if (cr >= safeCr) return 'safe';
  if (cr >= cautionCr) return 'caution';
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
 *  6. Blend each group's liquidationCr by its USD share to get a caution/safe
 *     cutoff specific to this user's actual collateral mix (see healthTierFor).
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

  // Blend each held asset's own liquidationCr, weighted by its share of the
  // user's total collateral USD, so a mixed (or non-ICP) position is judged
  // against its own risk profile rather than ICP's.
  const weightedLiquidationCr = totalCollateralUsd > 0
    ? perCollateral.reduce((sum, asset) => {
        const info = findCollateralInfo(collaterals, asset.principal);
        const liquidationCr = info?.liquidationCr ?? ICP_LIQUIDATION_CR;
        return sum + (asset.usdValue / totalCollateralUsd) * liquidationCr;
      }, 0)
    : ICP_LIQUIDATION_CR;

  const cautionCr = weightedLiquidationCr * CAUTION_RATIO;
  const safeCr = weightedLiquidationCr * SAFE_RATIO;

  return {
    totalCollateralUsd,
    totalBorrowed,
    overallCr,
    healthTier: healthTierFor(overallCr, cautionCr, safeCr),
    perCollateral,
    hasAnyMissingPrice,
  };
}
