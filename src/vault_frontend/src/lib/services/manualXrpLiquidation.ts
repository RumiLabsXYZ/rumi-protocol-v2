import {
  buildManualXrpSettlementFailureCopy,
  buildManualXrpSettlementSuccessCopy,
  type XrpClaimId,
} from './xrpPayoutHelpers';

export interface ManualXrpPendingClaim {
  claimId: XrpClaimId;
  vaultId?: number;
  payoutAddress: string;
  destinationTag?: number;
  drops?: bigint;
}

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
