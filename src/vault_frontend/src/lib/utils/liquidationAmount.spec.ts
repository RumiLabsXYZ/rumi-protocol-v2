import { describe, expect, it } from 'vitest';
import {
  computeLiquidationCap,
  getEffectiveLiquidationAmount,
  getMaxLiquidatableForBalance,
  type LiquidationCapParams,
} from './liquidationAmount';

// Shared per-collateral defaults (BOB-ish config): 15% liquidation bonus,
// 133% liquidation ratio, 150% borrow threshold, 155% recovery target.
const BASE: Omit<LiquidationCapParams, 'debtIcusd' | 'collateralAmount' | 'priceUsd'> = {
  liquidationBonus: 1.15,
  minimumCr: 1.5,
  liquidationCr: 1.33,
  recoveryTargetCr: 1.55,
  isRecoveryMode: false,
};

describe('computeLiquidationCap / getEffectiveLiquidationAmount', () => {
  it('vault #172: dust vault (debt 0.2076 <= 1 icUSD threshold) — max is the full debt', () => {
    const params: LiquidationCapParams = {
      ...BASE,
      debtIcusd: 0.20762188,
      collateralAmount: 5.51502002,
      priceUsd: 0.053922,
      dustThresholdIcusd: 1,
    };

    const result = computeLiquidationCap(params);
    expect(result.isDustVault).toBe(true);
    expect(result.maxLiquidatable).toBeCloseTo(0.20762188, 8);
  });

  it('vault #172: typing a small amount on a dust vault still resolves to the full debt', () => {
    const params: LiquidationCapParams = {
      ...BASE,
      debtIcusd: 0.20762188,
      collateralAmount: 5.51502002,
      priceUsd: 0.053922,
      dustThresholdIcusd: 1,
    };

    expect(getEffectiveLiquidationAmount(params, 0.01835132)).toBeCloseTo(0.20762188, 8);
  });

  it('large non-dust vault whose restore-cap formula is below the 0.1 icUSD minimum is floored to 0.1', () => {
    const params: LiquidationCapParams = {
      ...BASE,
      debtIcusd: 1000,
      collateralAmount: 1499.98, // CR just barely under the 150% borrow threshold
      priceUsd: 1,
      dustThresholdIcusd: 1,
    };

    const result = computeLiquidationCap(params);
    expect(result.isDustVault).toBe(false);
    // Raw restore-cap formula: (1.5*1000 - 1499.98) / (1.5 - 1.15) = 0.02 / 0.35 ≈ 0.0571
    expect(result.maxLiquidatable).toBeCloseTo(0.1, 8);
  });

  it('a partial amount that would leave a residual <= the dust threshold resolves to the full debt', () => {
    const params: LiquidationCapParams = {
      ...BASE,
      debtIcusd: 10,
      collateralAmount: 20,
      priceUsd: 1,
      dustThresholdIcusd: 1,
    };

    // 10 - 9.5 = 0.5 residual, <= the 1 icUSD dust threshold.
    expect(getEffectiveLiquidationAmount(params, 9.5)).toBe(10);
  });

  it('a residual exactly equal to the dust threshold does NOT force a full liquidation (review finding 5: strict boundary)', () => {
    const params: LiquidationCapParams = {
      ...BASE,
      debtIcusd: 10,
      collateralAmount: 20,
      priceUsd: 1,
      dustThresholdIcusd: 1,
    };

    // 10 - 9 = 1 residual, EXACTLY equal to the 1 icUSD dust threshold.
    // The backend's `round_up_partial_liq_dust` only rounds up for
    // `0 < residual < dust_floor` (strict), so residual == threshold must be
    // taken as typed, not bumped to the full debt.
    expect(getEffectiveLiquidationAmount(params, 9)).toBe(9);
  });

  it('a residual exactly equal to min_vault_debt does NOT force a full liquidation (strict boundary)', () => {
    const params: LiquidationCapParams = {
      ...BASE,
      debtIcusd: 10,
      collateralAmount: 20,
      priceUsd: 1,
      dustThresholdIcusd: 1,
      minVaultDebtIcusd: 3,
    };

    // 10 - 7 = 3 residual, EXACTLY equal to min_vault_debt (and above the
    // dust threshold) — must be taken as typed.
    expect(getEffectiveLiquidationAmount(params, 7)).toBe(7);
  });

  it('a partial amount leaving a residual above the dust threshold is taken as typed', () => {
    const params: LiquidationCapParams = {
      ...BASE,
      debtIcusd: 10,
      collateralAmount: 20,
      priceUsd: 1,
      dustThresholdIcusd: 1,
    };

    // 10 - 5 = 5 residual, above the 1 icUSD dust threshold, and above min_vault_debt (0).
    expect(getEffectiveLiquidationAmount(params, 5)).toBe(5);
  });

  it('a residual below the collateral min_vault_debt also forces a full liquidation, independent of the dust threshold', () => {
    const params: LiquidationCapParams = {
      ...BASE,
      debtIcusd: 10,
      collateralAmount: 20,
      priceUsd: 1,
      dustThresholdIcusd: 1,
      minVaultDebtIcusd: 3, // higher than the dust threshold
    };

    // 10 - 8 = 2 residual: above the 1 icUSD dust threshold, but below the 3 icUSD min_vault_debt.
    expect(getEffectiveLiquidationAmount(params, 8)).toBe(10);
  });

  it('threshold 0 disables dust handling entirely, even for a tiny debt', () => {
    const params: LiquidationCapParams = {
      ...BASE,
      debtIcusd: 0.5,
      collateralAmount: 0.7,
      priceUsd: 1,
      dustThresholdIcusd: 0,
    };

    const result = computeLiquidationCap(params);
    expect(result.isDustVault).toBe(false);
    // (1.5*0.5 - 0.7) / (1.5 - 1.15) = 0.05 / 0.35 ≈ 0.142857
    expect(result.maxLiquidatable).toBeCloseTo(0.05 / 0.35, 8);

    // With dust disabled, a small residual is not forced to full — only min_vault_debt would do that.
    expect(getEffectiveLiquidationAmount(params, 0.05)).toBeCloseTo(0.05, 8);
  });

  it('the Max button amount is clamped by the caller wallet balance', () => {
    const params: LiquidationCapParams = {
      ...BASE,
      debtIcusd: 0.20762188,
      collateralAmount: 5.51502002,
      priceUsd: 0.053922,
      dustThresholdIcusd: 1,
    };

    expect(getMaxLiquidatableForBalance(params, 0.05)).toBeCloseTo(0.05, 8);
    expect(getMaxLiquidatableForBalance(params, 100)).toBeCloseTo(0.20762188, 8);
    expect(getMaxLiquidatableForBalance(params, 0)).toBe(0);
  });

  it('in Recovery mode with the vault CR between the liquidation and borrow-threshold ratios, uses the collateral recovery_target_cr (not the borrow threshold)', () => {
    const params: LiquidationCapParams = {
      ...BASE,
      debtIcusd: 100,
      collateralAmount: 140,
      priceUsd: 1, // CR = 1.4, strictly between liquidationCr (1.33) and minimumCr (1.5)
      isRecoveryMode: true,
      dustThresholdIcusd: 1,
    };

    const result = computeLiquidationCap(params);
    // recovery target formula: (1.55*100 - 140) / (1.55 - 1.15) = 15 / 0.4 = 37.5
    expect(result.maxLiquidatable).toBeCloseTo(37.5, 8);
  });

  it('in Recovery mode with the vault CR outside the recovery-applicable range, falls back to the ordinary borrow-threshold cap', () => {
    const params: LiquidationCapParams = {
      ...BASE,
      debtIcusd: 100,
      collateralAmount: 155,
      priceUsd: 1, // CR = 1.55, at/above minimumCr (1.5) — recovery cap does not apply
      isRecoveryMode: true,
      dustThresholdIcusd: 1,
    };

    const result = computeLiquidationCap(params);
    // borrow-threshold formula: (1.5*100 - 155) = -5 <= 0 -> already at/above target -> floored to 0.1
    expect(result.maxLiquidatable).toBeCloseTo(0.1, 8);
  });

  it('a misconfigured/deeply underwater vault (denominator <= 0) falls back to the full debt', () => {
    const params: LiquidationCapParams = {
      ...BASE,
      liquidationBonus: 1.6, // bonus >= target CR
      debtIcusd: 50,
      collateralAmount: 10,
      priceUsd: 1,
      dustThresholdIcusd: 1,
    };

    const result = computeLiquidationCap(params);
    expect(result.maxLiquidatable).toBe(50);
  });
});
