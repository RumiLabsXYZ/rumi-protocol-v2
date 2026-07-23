import type { VaultDTO, CollateralInfo } from '../../services/types';
import { CANISTER_IDS } from '../../config';

// ICP's own borrow threshold (mirrors $lib/protocol's MINIMUM_CR). Restated
// locally rather than imported so this module stays a pure, dependency-light
// aggregation utility (protocol.ts pulls in the wallet/canister-agent stack
// via collateralStore -> tokenService -> pnp).
const ICP_MINIMUM_CR = 1.5;

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
 * Multiplier above an asset's borrow threshold at which a position reads as
 * comfortably healthy. Matches the per-vault convention already used by
 * VaultCard.svelte and ManualLiquidations.svelte (`comfortCR = minCR * 1.234`)
 * so this aggregate badge and the individual vault cards never disagree about
 * the same position.
 */
const COMFORT_MULTIPLIER = 1.234;

/**
 * Map a collateral ratio to a health tier against caution/safe cutoffs.
 *   >= safeCr         -> safe
 *   cautionCr..safeCr -> caution
 *   < cautionCr       -> danger
 * Infinity (no debt) -> 'no-debt'. NaN -> 'unknown'.
 *
 * cautionCr is the borrow threshold: below it the position is in liquidation
 * territory. safeCr is COMFORT_MULTIPLIER above that. Both default to ICP's
 * own numbers for callers that don't have a collateral mix on hand (e.g.
 * isolated unit tests).
 *
 * NOTE: This is an aggregate heuristic across all the user's vaults, not a
 * per-vault liquidation signal. Individual vault liquidation prices are
 * displayed on VaultCard.
 */
export function healthTierFor(
  cr: number,
  cautionCr: number = ICP_MINIMUM_CR,
  safeCr: number = ICP_MINIMUM_CR * COMFORT_MULTIPLIER,
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
 *  6. Blend each group's borrow threshold (minimumCr) by its USD share to get
 *     caution/safe cutoffs specific to this user's collateral mix (see
 *     healthTierFor).
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

  // Blend each held asset's own borrow threshold (minimumCr), weighted by its
  // share of the user's total collateral USD, so a mixed (or non-ICP) position
  // is judged against its own risk profile rather than ICP's. Each asset
  // publishes this threshold directly; it must NOT be inferred from
  // liquidationCr, because the borrow-threshold-to-liquidation spread differs
  // per asset (ICP 1.5/1.33 = 1.128, ckXAUT 1.18/1.12 = 1.054).
  const weightedMinimumCr = totalCollateralUsd > 0
    ? perCollateral.reduce((sum, asset) => {
        const info = findCollateralInfo(collaterals, asset.principal);
        const minimumCr = info?.minimumCr ?? ICP_MINIMUM_CR;
        return sum + (asset.usdValue / totalCollateralUsd) * minimumCr;
      }, 0)
    : ICP_MINIMUM_CR;

  const cautionCr = weightedMinimumCr;
  const safeCr = weightedMinimumCr * COMFORT_MULTIPLIER;

  return {
    totalCollateralUsd,
    totalBorrowed,
    overallCr,
    healthTier: healthTierFor(overallCr, cautionCr, safeCr),
    perCollateral,
    hasAnyMissingPrice,
  };
}
