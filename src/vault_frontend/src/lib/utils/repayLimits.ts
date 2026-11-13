/** Fixed ordinary repayment minimum and strict post-payment forgiveness boundary. */
export const MIN_REPAY_E8S = 10_000_000n;

/** Largest authorized icUSD payment after reserving the approval fee. */
export function computeSafeIcusdRepayMax(
  balanceE8s: bigint,
  debtE8s: bigint,
  approvalFeeE8s: bigint,
): bigint {
  if (balanceE8s <= approvalFeeE8s || debtE8s <= 0n) return 0n;
  const spendableE8s = balanceE8s - approvalFeeE8s;
  return spendableE8s < debtE8s ? spendableE8s : debtE8s;
}

/** Estimate settlement after a successful payment; accrued interest can change it. */
export function repaymentPreview(debtE8s: bigint, authorizedE8s: bigint) {
  const debt = debtE8s > 0n ? debtE8s : 0n;
  const authorized = authorizedE8s > 0n ? authorizedE8s : 0n;
  const paidE8s = authorized < debt ? authorized : debt;
  const residual = debt - paidE8s;
  const forgivenE8s = paidE8s > 0n && residual < MIN_REPAY_E8S ? residual : 0n;
  return { paidE8s, forgivenE8s, remainingE8s: residual - forgivenE8s };
}

/** Maximum signed amount for a full claim repayment, bounded by spendable balance. */
export function claimRepayMaximum(debt: number, spendable: number, safePartial: number): number {
  if (!Number.isFinite(debt) || !Number.isFinite(spendable) || debt <= 0 || spendable <= 0) return 0;
  if (spendable < debt) return Math.min(safePartial, spendable);
  return Math.min(spendable, debt + Math.max(0.01, debt * 0.001));
}
