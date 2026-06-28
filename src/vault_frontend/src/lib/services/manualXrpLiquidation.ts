import {
  buildManualXrpSettlementFailureCopy,
  buildManualXrpSettlementSuccessCopy,
  xrpClaimIdToBigInt,
  type XrpClaimId,
} from './xrpPayoutHelpers';

export interface ManualXrpPendingClaim {
  claimId: XrpClaimId;
  vaultId?: number;
  payoutAddress: string;
  destinationTag?: number;
  drops?: bigint;
}

/**
 * Outstanding manual-settlement claims, keyed by claim id (not vault id) so a
 * vault that produced more than one claim (multiple partial liquidations, or an
 * ambiguous-recovery sweep that returns several) keeps every claim's settle row
 * instead of the latest one clobbering the rest.
 */
export type ManualXrpPendingClaimMap = Record<XrpClaimId, ManualXrpPendingClaim>;

/** localStorage shape: `drops` is serialized as a decimal string (bigint is not JSON-safe). */
export type StoredManualXrpPendingClaim = Omit<ManualXrpPendingClaim, 'drops'> & { drops?: string };

export interface RecoverableXrpClaim {
  claimId: XrpClaimId | number | bigint;
  custodyNonce?: number;
  vaultId?: number;
  drops?: bigint;
}

export type SettleXrpClaim = (
  claimId: XrpClaimId,
  payoutAddress: string,
  destinationTag?: number
) => Promise<{ success: boolean; data?: { txHash?: string }; error?: string }>;

export type HasOutstandingClaim = (claimId: XrpClaimId) => Promise<boolean>;

export type ManualXrpSettlementResult =
  | { status: 'settled'; message: string; txHash?: string }
  | { status: 'retryable'; message: string; pendingClaim: ManualXrpPendingClaim; error?: string };

export async function settleManualXrpClaim(
  pendingClaim: ManualXrpPendingClaim,
  settleXrpClaim: SettleXrpClaim,
  hasOutstandingClaim: HasOutstandingClaim
): Promise<ManualXrpSettlementResult> {
  const result = await settleXrpClaim(
    pendingClaim.claimId,
    pendingClaim.payoutAddress,
    pendingClaim.destinationTag
  );

  if (result.success) {
    const txHash = result.data?.txHash;
    try {
      const claimOutstanding = await hasOutstandingClaim(pendingClaim.claimId);
      if (claimOutstanding) {
        return {
          status: 'retryable',
          pendingClaim,
          message: buildManualXrpSettlementSuccessCopy(pendingClaim.claimId, txHash),
        };
      }
    } catch (err: unknown) {
      return {
        status: 'retryable',
        pendingClaim,
        error: err instanceof Error ? err.message : String(err),
        message: buildManualXrpSettlementSuccessCopy(pendingClaim.claimId, txHash),
      };
    }

    return {
      status: 'settled',
      txHash,
      message: buildManualXrpSettlementSuccessCopy(pendingClaim.claimId, txHash),
    };
  }

  return {
    status: 'retryable',
    pendingClaim,
    error: result.error,
    message: buildManualXrpSettlementFailureCopy(pendingClaim.claimId),
  };
}

export async function recoverManualXrpClaimsForVault(
  vaultId: number,
  getMyClaims: () => Promise<RecoverableXrpClaim[]>
): Promise<RecoverableXrpClaim[]> {
  const claims = await getMyClaims();
  return claims.filter((claim) => claim.custodyNonce === vaultId || claim.vaultId === vaultId);
}

function compareXrpClaimId(a: XrpClaimId, b: XrpClaimId): number {
  try {
    const ba = xrpClaimIdToBigInt(a);
    const bb = xrpClaimIdToBigInt(b);
    return ba < bb ? -1 : ba > bb ? 1 : 0;
  } catch {
    return a < b ? -1 : a > b ? 1 : 0;
  }
}

/** Add or replace a single pending claim, keyed by its claim id. */
export function upsertManualXrpPendingClaim(
  map: ManualXrpPendingClaimMap,
  claim: ManualXrpPendingClaim
): ManualXrpPendingClaimMap {
  return { ...map, [claim.claimId]: claim };
}

/** Add or replace several pending claims in one pass (e.g. every recovered claim for a vault). */
export function upsertManualXrpPendingClaims(
  map: ManualXrpPendingClaimMap,
  claims: ManualXrpPendingClaim[]
): ManualXrpPendingClaimMap {
  return claims.reduce(upsertManualXrpPendingClaim, map);
}

/** Drop a single claim by id, leaving any sibling claims on the same vault intact. */
export function removeManualXrpPendingClaim(
  map: ManualXrpPendingClaimMap,
  claimId: XrpClaimId
): ManualXrpPendingClaimMap {
  const next = { ...map };
  delete next[claimId];
  return next;
}

/** Group claims by vault id for per-vault rendering; claims within a vault are sorted by claim id. */
export function groupManualXrpClaimsByVault(
  map: ManualXrpPendingClaimMap
): Record<number, ManualXrpPendingClaim[]> {
  const grouped: Record<number, ManualXrpPendingClaim[]> = {};
  for (const claim of Object.values(map)) {
    if (claim.vaultId === undefined) continue;
    (grouped[claim.vaultId] ??= []).push(claim);
  }
  for (const claims of Object.values(grouped)) {
    claims.sort((a, b) => compareXrpClaimId(a.claimId, b.claimId));
  }
  return grouped;
}

/** Serialize the claim map for localStorage, keyed by claim id with `drops` as a string. */
export function serializeManualXrpClaims(
  map: ManualXrpPendingClaimMap
): Record<string, StoredManualXrpPendingClaim> {
  return Object.fromEntries(
    Object.values(map).map((claim) => [
      claim.claimId,
      {
        ...claim,
        drops: claim.drops !== undefined ? claim.drops.toString() : undefined,
      } satisfies StoredManualXrpPendingClaim,
    ])
  );
}

/**
 * Parse a persisted claim map. Re-keys by each entry's own claim id, so it
 * transparently migrates an older vault-id-keyed store, and skips entries
 * missing a claim id or payout address. Returns {} on null/invalid JSON.
 */
export function deserializeManualXrpClaims(raw: string | null | undefined): ManualXrpPendingClaimMap {
  if (!raw) return {};
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return {};
  }
  if (!parsed || typeof parsed !== 'object') return {};

  const result: ManualXrpPendingClaimMap = {};
  for (const entry of Object.values(parsed as Record<string, StoredManualXrpPendingClaim>)) {
    if (!entry?.claimId || !entry.payoutAddress) continue;
    const claimId = String(entry.claimId);
    const vaultId = entry.vaultId !== undefined ? Number(entry.vaultId) : undefined;
    const normalizedVaultId =
      vaultId !== undefined && Number.isSafeInteger(vaultId) && vaultId >= 0 ? vaultId : undefined;
    let drops: bigint | undefined;
    if (entry.drops !== undefined) {
      try {
        drops = BigInt(entry.drops);
      } catch {
        drops = undefined;
      }
    }
    result[claimId] = {
      ...entry,
      claimId,
      vaultId: normalizedVaultId,
      drops,
    } satisfies ManualXrpPendingClaim;
  }
  return result;
}
