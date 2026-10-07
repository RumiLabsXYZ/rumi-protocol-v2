/**
 * One browser mutex for every frontend action allocating from the backend's
 * owner-and-ledger collateral request sequence. Keep the key independent of
 * route names so root, BTC, DOGE, and existing-vault collateral flows cannot
 * race the same next request ID in separate tabs.
 */
export function collateralSequenceLockName(
  principalText: string,
  networkScope: string,
  ledgerText: string,
): string {
  return `rumi_collateral_sequence_lock_${encodeURIComponent(networkScope)}_${encodeURIComponent(principalText)}_${encodeURIComponent(ledgerText)}`;
}
