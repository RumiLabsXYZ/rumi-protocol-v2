import type { Principal } from '@dfinity/principal';
import type { VaultOperationResult } from './types';

const E8S = 100_000_000;
const MAX_XRP_DESTINATION_TAG = 0xffffffff;

export const XRP_NATIVE_PRINCIPAL_TEXT = '5zjma-7dsov-wwsll-yojyc-23tbo-ruxmz-i';

export type XrpClaimId = string;
export type CandidOpt<T> = [] | [T];

export interface XrpPayoutValidation {
  ok: boolean;
  address?: string;
  destinationTag?: number;
  error?: string;
}

type PrincipalLike = string | Principal | { toText?: () => string } | null | undefined;

function principalText(value: PrincipalLike): string | null {
  if (!value) return null;
  if (typeof value === 'string') return value;
  try {
    return value.toText?.() ?? null;
  } catch {
    return null;
  }
}

export function isNativeXrpPrincipal(value: PrincipalLike): boolean {
  return principalText(value) === XRP_NATIVE_PRINCIPAL_TEXT;
}

export function isIcrcClaimableCollateral(value: PrincipalLike): boolean {
  return !isNativeXrpPrincipal(value);
}

export function validateXrpPayoutInput(addressInput: string, destinationTagInput?: string): XrpPayoutValidation {
  const address = addressInput.trim();
  if (!address) {
    return { ok: false, error: 'Enter an XRP payout address.' };
  }

  const rawTag = destinationTagInput?.trim() ?? '';
  if (rawTag === '') {
    return { ok: true, address, destinationTag: undefined };
  }

  if (!/^\d+$/.test(rawTag)) {
    return {
      ok: false,
      error: 'Destination tag must be a whole number from 0 to 4294967295.',
    };
  }

  const destinationTag = Number(rawTag);
  if (!Number.isInteger(destinationTag) || destinationTag < 0 || destinationTag > MAX_XRP_DESTINATION_TAG) {
    return {
      ok: false,
      error: 'Destination tag must be a whole number from 0 to 4294967295.',
    };
  }

  return { ok: true, address, destinationTag };
}

export function mapOptionalXrpClaimId(opt: CandidOpt<bigint | number | string> | undefined): XrpClaimId | undefined {
  if (!opt || opt.length === 0) return undefined;
  const raw = opt[0];
  if (typeof raw === 'bigint') {
    if (raw < 0n) throw new Error('XRP claim id cannot be negative');
    return raw.toString();
  }
  if (typeof raw === 'number') {
    if (!Number.isSafeInteger(raw) || raw < 0) {
      throw new Error('XRP claim id is outside the safe integer range');
    }
    return String(raw);
  }
  if (!/^\d+$/.test(raw)) {
    throw new Error('XRP claim id must be an unsigned integer string');
  }
  return raw;
}

export function xrpClaimIdToBigInt(claimId: XrpClaimId | number | bigint): bigint {
  if (typeof claimId === 'bigint') {
    if (claimId < 0n) throw new Error('XRP claim id cannot be negative');
    return claimId;
  }
  if (typeof claimId === 'number') {
    if (!Number.isSafeInteger(claimId) || claimId < 0) {
      throw new Error('XRP claim id is outside the safe integer range');
    }
    return BigInt(claimId);
  }
  if (!/^\d+$/.test(claimId)) {
    throw new Error('XRP claim id must be an unsigned integer string');
  }
  return BigInt(claimId);
}

export function mapLiquidationSuccessWithFee(
  vaultId: number,
  ok: {
    block_index: bigint | number;
    fee_amount_paid: bigint | number;
    xrp_claim_id?: CandidOpt<bigint | number | string>;
  }
): VaultOperationResult {
  const result: VaultOperationResult = {
    success: true,
    vaultId,
    blockIndex: Number(ok.block_index),
    feePaid: Number(ok.fee_amount_paid) / E8S,
  };

  const xrpClaimId = mapOptionalXrpClaimId(ok.xrp_claim_id);
  if (xrpClaimId !== undefined) {
    result.xrpClaimId = xrpClaimId;
  }
  return result;
}

export function unwrapNativePayoutAddresses(position: {
  native_payout_addresses?: CandidOpt<Array<[PrincipalLike, string]>>;
} | null | undefined): Map<string, string> {
  const entries = position?.native_payout_addresses?.[0] ?? [];
  return new Map(
    entries.flatMap(([principal, address]) => {
      const key = principalText(principal);
      return key ? [[key, address] as const] : [];
    })
  );
}

export function unwrapNativePayoutDestinationTags(position: {
  native_payout_destination_tags?: CandidOpt<Array<[PrincipalLike, bigint | number]>>;
} | null | undefined): Map<string, number> {
  const entries = position?.native_payout_destination_tags?.[0] ?? [];
  return new Map(
    entries.flatMap(([principal, tag]) => {
      const key = principalText(principal);
      const value = Number(tag);
      return key && Number.isInteger(value) && value >= 0 && value <= MAX_XRP_DESTINATION_TAG
        ? [[key, value] as const]
        : [];
    })
  );
}

export function buildManualXrpSettlementFailureCopy(claimId: XrpClaimId): string {
  return `Liquidation accepted and XRP claim #${claimId} created, but settlement did not complete. The claim #${claimId} remains outstanding and can be retried from this screen.`;
}

export function buildManualXrpSettlementSuccessCopy(claimId: XrpClaimId, txHash?: string): string {
  const suffix = txHash ? ` Tx hash: ${txHash}.` : '';
  return `Liquidation accepted and XRP claim #${claimId} created. XRP settlement submitted.${suffix}`;
}
