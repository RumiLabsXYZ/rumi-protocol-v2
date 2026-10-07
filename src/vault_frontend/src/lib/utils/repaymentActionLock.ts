/**
 * The repayment V2 request ID sequence is owner-global, so ordinary repay and
 * repay-and-close must share one browser lock across all vaults for an owner.
 * Stable repayments use the same lock locally while they remain on the legacy
 * backend route, preventing same-origin overlap with journaled icUSD repayment.
 */
export function repaymentActionLockName(principalText: string, networkScope: string): string {
  return `rumi_repayment_sequence_lock_${encodeURIComponent(networkScope)}_${encodeURIComponent(principalText)}`;
}
