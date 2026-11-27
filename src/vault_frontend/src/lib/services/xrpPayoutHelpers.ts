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

// XRPL classic-address structural check. SYNC on purpose: it runs in the click
// handler before the signer call, and any async (e.g. crypto.subtle for a checksum)
// would burn the browser's user-gesture window and block the Oisy popup. Base58-
// decodes with the RIPPLE alphabet and requires a 25-byte payload with the classic-
// address version byte (0x00). The 4-byte checksum is NOT verified here — that needs
// SHA-256, and the backend already does a full base58+checksum decode before signing
// (chains/xrp/address.rs). This just catches typos / wrong-alphabet / X-addresses.
const RIPPLE_B58 = 'rpshnaf39wBUDNEGHJKLM4PQRST7VWXYZ2bcdeCg65jkm8oFqi1tuvAxyz';
export function isValidXrplClassicAddress(addr: string): boolean {
  if (!addr || addr[0] !== 'r' || addr.length < 25 || addr.length > 35) return false;
  let num = 0n;
  for (const ch of addr) {
    const idx = RIPPLE_B58.indexOf(ch);
    if (idx < 0) return false; // char outside the ripple base58 alphabet
    num = num * 58n + BigInt(idx);
  }
  const bytes: number[] = [];
  while (num > 0n) {
    bytes.unshift(Number(num & 0xffn));
    num >>= 8n;
  }
  // Leading 'r' (alphabet index 0) chars decode to leading zero bytes.
  for (const ch of addr) {
    if (ch === 'r') bytes.unshift(0);
    else break;
  }
  return bytes.length === 25 && bytes[0] === 0x00;
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
    // The icUSD debt the backend actually cleared (e8s). `[]` on paths that
    // don't populate it; callers should fall back to their own local
    // prediction in that case rather than treat it as zero.
    debt_liquidated_e8s?: CandidOpt<bigint | number>;
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

  const debtLiquidated = ok.debt_liquidated_e8s;
  if (debtLiquidated && debtLiquidated.length > 0) {
    result.debtLiquidatedIcusd = Number(debtLiquidated[0]) / E8S;
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

/**
 * Amount to show on the Stability Pool "Collateral Gains" line for a collateral.
 *
 * ICRC collateral accrues into the pool's `collateral_gains` map, so the gains
 * value is the whole story. Native XRP never does: an absorb pays depositors
 * through XRPL claims instead, so its `collateral_gains` entry is permanently
 * zero. Rendering that raw zero told a depositor who was owed real XRP that
 * they had received nothing, so for native XRP we show the pending payout
 * total (in drops) instead.
 *
 * Returns the amount plus whether it came from the claim rail, so callers can
 * label it rather than passing it off as an ordinary claimable balance.
 */
export function collateralGainDisplayAmount(
  collateralPrincipal: PrincipalLike,
  icrcGainAmount: bigint,
  pendingXrpDrops: bigint,
): { amount: bigint; viaXrpClaims: boolean } {
  if (isNativeXrpPrincipal(collateralPrincipal)) {
    return { amount: pendingXrpDrops, viaXrpClaims: true };
  }
  return { amount: icrcGainAmount, viaXrpClaims: false };
}

/** Total drops still owed across pending native-XRP payouts. */
export function sumPendingXrpDrops(
  payouts: ReadonlyArray<{ drops: bigint | number }> | null | undefined,
): bigint {
  if (!payouts?.length) return 0n;
  return payouts.reduce<bigint>((total, p) => total + BigInt(p.drops ?? 0), 0n);
}
