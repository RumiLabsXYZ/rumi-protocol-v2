/**
 * Pure liquidation-amount math for manual liquidations.
 *
 * Mirrors the backend rule (dust_liquidation_threshold, default 1 icUSD) in
 * src/rumi_protocol_backend/src/state.rs:
 *
 *   1. If debt <= threshold: the backend takes the FULL debt, regardless of
 *      the amount argument the caller sends.
 *   2. Otherwise the cap is:
 *        - `compute_recovery_repay_cap` when the protocol is in Recovery
 *          mode AND the vault's CR sits strictly between the collateral's
 *          liquidation_ratio and its borrow_threshold_ratio (uses the
 *          collateral's recovery_target_cr as the restore target), else
 *        - `compute_partial_liquidation_cap` (uses the collateral's
 *          borrow_threshold_ratio, i.e. its minimum/borrow CR, as the
 *          restore target).
 *      Both use the same shape:
 *        repay = (targetCr * debt - collateralValue) / (targetCr - liquidationBonus)
 *      clamped to the debt, and falling back to the full debt when the
 *      denominator is non-positive (deeply underwater / misconfigured) or to
 *      the full partial-liquidation cap logic when the recovery cap does not
 *      apply (already at/above target, so nothing to seize under that rule).
 *   3. The cap is floored at min(0.1 icUSD, debt) — the protocol-wide
 *      minimum liquidation amount (`min_icusd_amount`, MIN_ICUSD_AMOUNT).
 *   4. If a candidate repay amount would leave a residual that is > 0 and
 *      <= threshold, or below the collateral's min_vault_debt, the backend
 *      takes the full debt instead (no un-liquidatable dust vault left
 *      behind).
 *
 * This module is UI-agnostic: it works entirely in human icUSD units (not
 * e8s) and knows nothing about wallets, balances, or Svelte stores. The
 * caller is responsible for clamping the Max button by the user's wallet
 * balance (see `getMaxLiquidatableForBalance`).
 */

/** Protocol-wide default when `dust_liquidation_threshold_e8s` is absent (older backend) or unset. */
export const DEFAULT_DUST_LIQUIDATION_THRESHOLD_ICUSD = 1;

/** Protocol-wide minimum liquidation amount (`min_icusd_amount` / MIN_ICUSD_AMOUNT), matches 10_000_000 e8s. */
export const DEFAULT_MIN_PARTIAL_LIQUIDATION_ICUSD = 0.1;

export interface LiquidationCapParams {
  /** Vault debt in human icUSD. */
  debtIcusd: number;
  /** Vault collateral amount in human units (already divided by decimals). */
  collateralAmount: number;
  /** Current USD price of the collateral. */
  priceUsd: number;
  /** Per-collateral liquidation bonus, as a multiplier (e.g. 1.15 for a 15% bonus). */
  liquidationBonus: number;
  /** Per-collateral borrow_threshold_ratio (a.k.a. minimum/borrow CR), as a ratio (e.g. 1.5). */
  minimumCr: number;
  /** Per-collateral liquidation_ratio, as a ratio (e.g. 1.33). */
  liquidationCr: number;
  /** Per-collateral recovery_target_cr, as a ratio (e.g. 1.55). */
  recoveryTargetCr: number;
  /** Whether the protocol is currently in Recovery mode. */
  isRecoveryMode: boolean;
  /** Global dust_liquidation_threshold_e8s, in human icUSD. Defaults to 1. Pass 0 to disable dust handling entirely. */
  dustThresholdIcusd?: number;
  /** Per-collateral min_vault_debt, in human icUSD. Defaults to 0 (no extra floor beyond the dust threshold). */
  minVaultDebtIcusd?: number;
  /** Protocol-wide minimum liquidation amount, in human icUSD. Defaults to 0.1. */
  minPartialLiquidationIcusd?: number;
}

export interface LiquidationCapResult {
  /** True when the vault's whole debt is at or below the dust threshold — it can only ever be liquidated in full. */
  isDustVault: boolean;
  /** The largest amount the backend will actually accept as a partial repay, capped at the debt. */
  maxLiquidatable: number;
}

function collateralValueUsd(params: LiquidationCapParams): number {
  return params.collateralAmount * params.priceUsd;
}

/**
 * True when the protocol is in Recovery mode and the vault's CR is strictly
 * between the collateral's liquidation_ratio and its borrow_threshold_ratio —
 * the range in which `compute_recovery_repay_cap` (state.rs ~4415) applies
 * instead of the ordinary partial-liquidation cap.
 */
function recoveryCapApplies(params: LiquidationCapParams): boolean {
  if (!params.isRecoveryMode) return false;
  if (params.debtIcusd <= 0) return false;
  const cr = collateralValueUsd(params) / params.debtIcusd;
  return cr > params.liquidationCr && cr < params.minimumCr;
}

/**
 * Restore-to-target repay formula shared by both backend cap functions:
 *   repay = (targetCr * debt - collateralValue) / (targetCr - bonus)
 * Returns the full debt when the denominator is non-positive (misconfigured
 * or deeply underwater) and 0 when the vault is already at/above target.
 */
function restoreCapAmount(params: LiquidationCapParams, targetCr: number): number {
  const debt = params.debtIcusd;
  const value = collateralValueUsd(params);
  const numerator = targetCr * debt - value;
  const denominator = targetCr - params.liquidationBonus;

  if (denominator <= 0) {
    // Deeply underwater / misconfigured — full liquidation, mirrors
    // compute_partial_liquidation_cap's denominator <= 0 branch.
    return debt;
  }
  if (numerator <= 0) {
    // Already at or above target under this formula.
    return 0;
  }
  return Math.min(numerator / denominator, debt);
}

/**
 * Rule 2+3: compute the backend's partial-liquidation cap (before the dust
 * override in rule 1). Picks the recovery-mode target CR when applicable,
 * otherwise the ordinary borrow-threshold target CR, then floors the result
 * at min(minPartialLiquidationIcusd, debt).
 */
function computeCapBeforeDust(params: LiquidationCapParams): number {
  const debt = params.debtIcusd;
  if (debt <= 0 || params.priceUsd <= 0) return 0;

  const targetCr = recoveryCapApplies(params) ? params.recoveryTargetCr : params.minimumCr;
  let cap = restoreCapAmount(params, targetCr);

  const floor = Math.min(params.minPartialLiquidationIcusd ?? DEFAULT_MIN_PARTIAL_LIQUIDATION_ICUSD, debt);
  cap = Math.max(cap, floor);
  cap = Math.min(cap, debt);
  return cap;
}

/**
 * Compute whether a vault is dust and its max liquidatable amount (rules 1-3).
 * Does NOT clamp by the caller's wallet balance — use
 * `getMaxLiquidatableForBalance` for that.
 */
export function computeLiquidationCap(params: LiquidationCapParams): LiquidationCapResult {
  const debt = params.debtIcusd;
  const dustThreshold = params.dustThresholdIcusd ?? DEFAULT_DUST_LIQUIDATION_THRESHOLD_ICUSD;
  const isDustVault = debt > 0 && dustThreshold > 0 && debt <= dustThreshold;

  if (isDustVault) {
    return { isDustVault: true, maxLiquidatable: debt };
  }

  return { isDustVault: false, maxLiquidatable: computeCapBeforeDust(params) };
}

/**
 * Rule 1 + rule 4: the amount the backend will actually take for a given
 * typed input. For a dust vault this is always the full debt (any positive
 * amount triggers a full liquidation). Otherwise, if repaying `typedAmount`
 * would leave a residual that is positive and either <= the dust threshold
 * or below the collateral's min_vault_debt, the backend takes the full debt
 * instead of leaving an un-liquidatable dust remainder.
 */
export function getEffectiveLiquidationAmount(params: LiquidationCapParams, typedAmount: number): number {
  const debt = params.debtIcusd;
  if (typedAmount <= 0 || debt <= 0) return 0;

  const { isDustVault } = computeLiquidationCap(params);
  if (isDustVault) return debt;

  const clamped = Math.min(typedAmount, debt);
  const dustThreshold = params.dustThresholdIcusd ?? DEFAULT_DUST_LIQUIDATION_THRESHOLD_ICUSD;
  const minVaultDebt = params.minVaultDebtIcusd ?? 0;
  const residual = debt - clamped;

  // Mirrors the backend exactly: `round_up_partial_liq_dust` (vault.rs) rounds
  // up when `0 < residual < dust_floor`, where `dust_floor =
  // dust_liquidation_threshold.max(min_vault_debt)` (state.rs
  // `effective_liquidation_amount`, rule 4). Both bounds must be STRICT `<` —
  // a residual exactly equal to the dust threshold does NOT force a full
  // liquidation.
  const dustFloor = Math.max(dustThreshold, minVaultDebt);
  if (residual > 0 && residual < dustFloor) {
    return debt;
  }
  return clamped;
}

/**
 * The Max button amount: the backend cap (rules 1-3), further clamped by the
 * debt and the caller's available balance in the token they're paying with.
 */
export function getMaxLiquidatableForBalance(params: LiquidationCapParams, balance: number): number {
  const { maxLiquidatable } = computeLiquidationCap(params);
  return Math.max(0, Math.min(maxLiquidatable, balance, params.debtIcusd));
}
