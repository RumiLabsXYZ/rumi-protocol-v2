import type { NicpRedemptionClaimDTO } from '../services/types';

export function decodeNicpClaim(option: any): NicpRedemptionClaimDTO | undefined {
  const claim = option?.[0];
  if (!claim) return undefined;
  return {
    stage: Object.keys(claim.stage ?? {})[0] ?? 'Unknown',
    icpAmount: Number(claim.received_icp_e8s?.[0] ?? claim.icp_due_e8s) / 1e8,
    withdrawalId: claim.withdrawal_id?.[0]?.toString(),
    ...(claim.release?.[0] ? { releasePending: !claim.release[0].block?.length, releaseBlock: claim.release[0].block?.[0]?.toString() } : {}),
  };
}

export function nicpClaimValueUsd(claim: NicpRedemptionClaimDTO, icpPrice: number, haircut: number | undefined): number {
  if (claim.releasePending || claim.releaseBlock) return 0;
  if (!Number.isFinite(icpPrice) || icpPrice <= 0 || haircut === undefined
    || !Number.isFinite(haircut) || haircut < 0 || haircut > 1
    || !Number.isFinite(claim.icpAmount) || claim.icpAmount < 0) return 0;
  return claim.icpAmount * icpPrice * (1 - haircut);
}
