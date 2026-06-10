/**
 * Raw vault-snapshot cache backing the Oisy `_arr` false-negative verifiers.
 *
 * Warmed wherever get_vaults is fetched (page-load / poll / refresh-after)
 * so the verifiers can read pre-op ("before") state synchronously instead of
 * awaiting get_vaults inside the click gesture window.
 *
 * FE-002: every warm and read is keyed to the wallet principal it was
 * captured under. Reads under a different (or no) principal are misses, and
 * warming under a new principal drops the previous wallet's entries, so a
 * verifier never compares against a snapshot captured for another wallet.
 * `clearRawSnapshots()` is also called from ApiClient.clearVaultCache(),
 * which runs on wallet connect and disconnect.
 */

export interface RawVaultSnapshot {
  collateralAmount: bigint;
  borrowedIcusd: bigint;
  icpMargin: bigint;
}

let snapshots = new Map<number, RawVaultSnapshot>();
let vaultIds = new Set<number>();
let warmedFor: string | null = null;

/** Drop anything captured under a previous principal before warming. */
function rekey(principalText: string): void {
  if (warmedFor !== principalText) {
    snapshots = new Map();
    vaultIds = new Set();
    warmedFor = principalText;
  }
}

/** Replace both the snapshot map and the vault-id set (full get_vaults fetch). */
export function warmRawSnapshots(
  principalText: string,
  entries: Array<[number, RawVaultSnapshot]>,
): void {
  rekey(principalText);
  snapshots = new Map(entries);
  vaultIds = new Set(entries.map(([id]) => id));
}

/** Upsert a single vault snapshot (single-vault refresh). */
export function warmRawSnapshot(
  principalText: string,
  vaultId: number,
  snapshot: RawVaultSnapshot,
): void {
  rekey(principalText);
  snapshots.set(vaultId, snapshot);
}

/** Replace the vault-id set only (open-vault verifier snapshot). */
export function warmRawVaultIds(principalText: string, ids: Iterable<number>): void {
  rekey(principalText);
  vaultIds = new Set(ids);
}

export function getRawSnapshot(
  principalText: string | null,
  vaultId: number,
): RawVaultSnapshot | null {
  if (principalText === null || warmedFor !== principalText) return null;
  return snapshots.get(vaultId) ?? null;
}

export function getRawVaultIds(principalText: string | null): Set<number> | null {
  if (principalText === null || warmedFor !== principalText) return null;
  return vaultIds.size > 0 ? new Set(vaultIds) : null;
}

export function clearRawSnapshots(): void {
  snapshots = new Map();
  vaultIds = new Set();
  warmedFor = null;
}
