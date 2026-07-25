import {
  buildManualSolSettlementFailureCopy,
  buildManualSolSettlementSuccessCopy,
  solClaimIdToBigInt,
  type SolClaimId,
} from './solPayoutHelpers';

export interface ManualSolPendingClaim {
  claimId: SolClaimId;
  vaultId?: number;
  payoutAddress: string;
  lamports?: bigint;
}

/**
 * Outstanding manual-settlement claims, keyed by claim id (not vault id) so a
 * vault that produced more than one claim (multiple partial liquidations, or an
 * ambiguous-recovery sweep that returns several) keeps every claim's settle row
 * instead of the latest one clobbering the rest.
 */
export type ManualSolPendingClaimMap = Record<SolClaimId, ManualSolPendingClaim>;

/** localStorage shape: `lamports` is serialized as a decimal string (bigint is not JSON-safe). */
export type StoredManualSolPendingClaim = Omit<ManualSolPendingClaim, 'lamports'> & { lamports?: string };

export interface RecoverableSolClaim {
  claimId: SolClaimId | number | bigint;
  custodyNonce?: number;
  vaultId?: number;
  lamports?: bigint;
}

export type SettleSolClaim = (
  claimId: SolClaimId,
  payoutAddress: string
) => Promise<{ success: boolean; data?: { signature?: string }; error?: string }>;

export type HasOutstandingClaim = (claimId: SolClaimId) => Promise<boolean>;

export type ManualSolSettlementResult =
  | { status: 'settled'; message: string; signature?: string }
  | { status: 'retryable'; message: string; pendingClaim: ManualSolPendingClaim; error?: string };

export async function settleManualSolClaim(
  pendingClaim: ManualSolPendingClaim,
  settleSolClaim: SettleSolClaim,
  hasOutstandingClaim: HasOutstandingClaim
): Promise<ManualSolSettlementResult> {
  const result = await settleSolClaim(pendingClaim.claimId, pendingClaim.payoutAddress);

  if (result.success) {
    const signature = result.data?.signature;
    try {
      const claimOutstanding = await hasOutstandingClaim(pendingClaim.claimId);
      if (claimOutstanding) {
        return {
          status: 'retryable',
          pendingClaim,
          message: buildManualSolSettlementSuccessCopy(pendingClaim.claimId, signature),
        };
      }
    } catch (err: unknown) {
      return {
        status: 'retryable',
        pendingClaim,
        error: err instanceof Error ? err.message : String(err),
        message: buildManualSolSettlementSuccessCopy(pendingClaim.claimId, signature),
      };
    }

    return {
      status: 'settled',
      signature,
      message: buildManualSolSettlementSuccessCopy(pendingClaim.claimId, signature),
    };
  }

  return {
    status: 'retryable',
    pendingClaim,
    error: result.error,
    message: buildManualSolSettlementFailureCopy(pendingClaim.claimId),
  };
}

// Fallback recovery path, mirroring XRP: the normal case reads the claim id
// straight off `SuccessWithFee.xrp_claim_id` (the shared native-custody
// claim-id field, see solPayoutHelpers.ts) and settles it directly. This scan
// only runs for the ambiguous case where that id was not returned, e.g. the
// call errored after the claim was created.
export async function recoverManualSolClaimsForVault(
  vaultId: number,
  getMyClaims: () => Promise<RecoverableSolClaim[]>
): Promise<RecoverableSolClaim[]> {
  const claims = await getMyClaims();
  return claims.filter((claim) => claim.custodyNonce === vaultId || claim.vaultId === vaultId);
}

function compareSolClaimId(a: SolClaimId, b: SolClaimId): number {
  try {
    const ba = solClaimIdToBigInt(a);
    const bb = solClaimIdToBigInt(b);
    return ba < bb ? -1 : ba > bb ? 1 : 0;
  } catch {
    return a < b ? -1 : a > b ? 1 : 0;
  }
}

/** Add or replace a single pending claim, keyed by its claim id. */
export function upsertManualSolPendingClaim(
  map: ManualSolPendingClaimMap,
  claim: ManualSolPendingClaim
): ManualSolPendingClaimMap {
  return { ...map, [claim.claimId]: claim };
}

/** Add or replace several pending claims in one pass (e.g. every recovered claim for a vault). */
export function upsertManualSolPendingClaims(
  map: ManualSolPendingClaimMap,
  claims: ManualSolPendingClaim[]
): ManualSolPendingClaimMap {
  return claims.reduce(upsertManualSolPendingClaim, map);
}

/** Drop a single claim by id, leaving any sibling claims on the same vault intact. */
export function removeManualSolPendingClaim(
  map: ManualSolPendingClaimMap,
  claimId: SolClaimId
): ManualSolPendingClaimMap {
  const next = { ...map };
  delete next[claimId];
  return next;
}

/** Group claims by vault id for per-vault rendering; claims within a vault are sorted by claim id. */
export function groupManualSolClaimsByVault(
  map: ManualSolPendingClaimMap
): Record<number, ManualSolPendingClaim[]> {
  const grouped: Record<number, ManualSolPendingClaim[]> = {};
  for (const claim of Object.values(map)) {
    if (claim.vaultId === undefined) continue;
    (grouped[claim.vaultId] ??= []).push(claim);
  }
  for (const claims of Object.values(grouped)) {
    claims.sort((a, b) => compareSolClaimId(a.claimId, b.claimId));
  }
  return grouped;
}

/** Serialize the claim map for localStorage, keyed by claim id with `lamports` as a string. */
export function serializeManualSolClaims(
  map: ManualSolPendingClaimMap
): Record<string, StoredManualSolPendingClaim> {
  return Object.fromEntries(
    Object.values(map).map((claim) => [
      claim.claimId,
      {
        ...claim,
        lamports: claim.lamports !== undefined ? claim.lamports.toString() : undefined,
      } satisfies StoredManualSolPendingClaim,
    ])
  );
}

/**
 * Parse a persisted claim map. Re-keys by each entry's own claim id, and skips
 * entries missing a claim id or payout address. Returns {} on null/invalid JSON.
 */
export function deserializeManualSolClaims(raw: string | null | undefined): ManualSolPendingClaimMap {
  if (!raw) return {};
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return {};
  }
  if (!parsed || typeof parsed !== 'object') return {};

  const result: ManualSolPendingClaimMap = {};
  for (const entry of Object.values(parsed as Record<string, StoredManualSolPendingClaim>)) {
    if (!entry?.claimId || !entry.payoutAddress) continue;
    const claimId = String(entry.claimId);
    const vaultId = entry.vaultId !== undefined ? Number(entry.vaultId) : undefined;
    const normalizedVaultId =
      vaultId !== undefined && Number.isSafeInteger(vaultId) && vaultId >= 0 ? vaultId : undefined;
    let lamports: bigint | undefined;
    if (entry.lamports !== undefined) {
      try {
        lamports = BigInt(entry.lamports);
      } catch {
        lamports = undefined;
      }
    }
    result[claimId] = {
      ...entry,
      claimId,
      vaultId: normalizedVaultId,
      lamports,
    } satisfies ManualSolPendingClaim;
  }
  return result;
}
