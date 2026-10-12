/** Rejects status reads started for a prior wallet session or owner. */
export function isCurrentVaultPullStatusRead(
  requestGeneration: number,
  currentGeneration: number,
  requestOwner: string,
  currentOwner: string
): boolean {
  return requestGeneration === currentGeneration && requestOwner !== '' && requestOwner === currentOwner;
}
