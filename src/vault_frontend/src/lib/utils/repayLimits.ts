/**
 * Return the largest icUSD amount that can be pulled without the backend's
 * near-full repayment rule upgrading it to the entire debt.
 *
 * `repay_to_vault` and `repay_and_close_vault` snap a repayment to the full
 * debt when the remaining debt is at most max(1% of debt, 0.01 icUSD). A
 * wallet that cannot cover full debt plus the ledger fee must therefore leave
 * at least the configured minimum vault debt, and strictly more than that
 * snap threshold when the threshold is larger.
 */
export function computeSafeIcusdRepayMax(
  balanceE8s: bigint,
  debtE8s: bigint,
  ledgerFeeE8s: bigint,
  minVaultDebtE8s: bigint,
): bigint {
  if (balanceE8s <= ledgerFeeE8s || debtE8s <= 0n) return 0n;

  const spendableE8s = balanceE8s - ledgerFeeE8s;
  if (spendableE8s >= debtE8s) return debtE8s;

  const snapThresholdE8s = debtE8s / 100n > 1_000_000n
    ? debtE8s / 100n
    : 1_000_000n;
  // A partial repayment must also leave at least the collateral's configured
  // minimum vault debt. The strict +1 only applies to the backend's <= dust
  // snap rule; equality with min_vault_debt is accepted.
  const requiredResidualE8s = snapThresholdE8s + 1n > minVaultDebtE8s
    ? snapThresholdE8s + 1n
    : minVaultDebtE8s;
  if (debtE8s <= requiredResidualE8s) return 0n;

  const largestSafePartialE8s = debtE8s - requiredResidualE8s;
  return spendableE8s < largestSafePartialE8s ? spendableE8s : largestSafePartialE8s;
}
