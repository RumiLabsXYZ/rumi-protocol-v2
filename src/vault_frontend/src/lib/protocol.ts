/**
 * Rumi Protocol — canonical frontend parameters.
 *
 * Single source of truth for protocol-level numbers that the frontend
 * needs to reason about risk, borrowing limits, and liquidation thresholds.
 *
 * These MUST stay in sync with the Rust backend constants in
 *   src/rumi_protocol_backend/src/lib.rs
 *
 * Backend names → Frontend names:
 *   RECOVERY_COLLATERAL_RATIO  →  MINIMUM_CR        (150%)
 *   MINIMUM_COLLATERAL_RATIO   →  LIQUIDATION_CR    (133%)
 *
 * If the protocol parameters are ever changed via governance, update
 * ONLY this file and every UI surface picks up the new values.
 *
 * Multi-collateral note: These globals are now ICP defaults / fallbacks.
 * Per-collateral values are read from collateralStore at runtime.
 * Use getMinimumCR(), getLiquidationCR(), etc. for per-collateral lookups.
 */

import { collateralStore } from './stores/collateralStore';
import { CANISTER_IDS } from './config';

/** Default Minimum Collateral Ratio (borrow threshold) — ICP default. */
export const MINIMUM_CR = 1.5;

/** Default Liquidation threshold — ICP default. */
export const LIQUIDATION_CR = 1.33;

/** E8S conversion factor for ICP (10^8). */
export const E8S = 100_000_000;

/** ICP ledger principal for quick comparisons. */
export const ICP_LEDGER_PRINCIPAL = CANISTER_IDS.ICP_LEDGER;

// ── Per-collateral helpers ──────────────────────────────────────────

/**
 * Get the minimum collateral ratio (borrow threshold) for a given collateral type.
 * Falls back to global MINIMUM_CR if collateral type is unknown or not loaded.
 */
export function getMinimumCR(collateralPrincipal?: string): number {
  if (!collateralPrincipal) return MINIMUM_CR;
  const info = collateralStore.getCollateralInfo(collateralPrincipal);
  return info?.minimumCr ?? MINIMUM_CR;
}

/**
 * Get the liquidation ratio for a given collateral type.
 * Falls back to global LIQUIDATION_CR if unknown.
 */
export function getLiquidationCR(collateralPrincipal?: string): number {
  if (!collateralPrincipal) return LIQUIDATION_CR;
  const info = collateralStore.getCollateralInfo(collateralPrincipal);
  return info?.liquidationCr ?? LIQUIDATION_CR;
}

/**
 * Get the decimals factor (10^decimals) for a given collateral type.
 * Falls back to E8S (10^8, ICP default) if unknown.
 */
export function getDecimalsFactor(collateralPrincipal?: string): number {
  if (!collateralPrincipal) return E8S;
  const info = collateralStore.getCollateralInfo(collateralPrincipal);
  const decimals = info?.decimals ?? 8;
  return Math.pow(10, decimals);
}

/**
 * Get the borrowing fee rate for a given collateral type.
 * Falls back to 0.005 (0.5%) if unknown.
 */
export function getBorrowingFee(collateralPrincipal?: string): number {
  if (!collateralPrincipal) return 0.005;
  const info = collateralStore.getCollateralInfo(collateralPrincipal);
  return info?.borrowingFee ?? 0.005;
}

/**
 * Get the USD price for a given collateral type.
 * Falls back to 0 if unknown.
 */
export function getCollateralPrice(collateralPrincipal?: string): number {
  if (!collateralPrincipal) return 0;
  return collateralStore.getCollateralPrice(collateralPrincipal);
}

/**
 * Get the recovery target CR for a given collateral type.
 * Falls back to 1.55 if unknown.
 */
export function getRecoveryTargetCR(collateralPrincipal?: string): number {
  if (!collateralPrincipal) return 1.55;
  const info = collateralStore.getCollateralInfo(collateralPrincipal);
  return info?.recoveryTargetCr ?? 1.55;
}
