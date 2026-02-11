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
 */

/** Minimum Collateral Ratio to open a vault / borrow more (ratio, e.g. 1.5 = 150%).
 *  Backend: RECOVERY_COLLATERAL_RATIO */
export const MINIMUM_CR = 1.5;

/** Liquidation threshold — vaults at or below this ratio can be liquidated (ratio, e.g. 1.33 ≈ 133%).
 *  Backend: MINIMUM_COLLATERAL_RATIO */
export const LIQUIDATION_CR = 1.33;

/** E8S conversion factor for ICP (10^8). */
export const E8S = 100_000_000;
