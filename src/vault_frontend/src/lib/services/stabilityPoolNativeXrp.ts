import type { Principal } from '@dfinity/principal';
import {
  XRP_NATIVE_PRINCIPAL_TEXT,
  xrpClaimIdToBigInt,
  type CandidOpt,
  type XrpClaimId,
} from './xrpPayoutHelpers';

export { XRP_NATIVE_PRINCIPAL_TEXT };

export interface NativeXrpPendingPayout {
  claim_id: bigint;
  collateral_type: Principal;
  vault_id: bigint;
  drops: bigint;
  payout_address: string;
  destination_tag: CandidOpt<number>;
  created_at_ns: bigint;
}

export interface StabilityPoolNativeXrpActor {
  opt_in_native_collateral?: (
    collateralType: Principal,
    payoutAddress: string
  ) => Promise<{ Ok: null } | { Err: unknown }>;
  opt_in_native_collateral_with_tag?: (
    collateralType: Principal,
    payoutAddress: string,
    destinationTag: CandidOpt<number>
  ) => Promise<{ Ok: null } | { Err: unknown }>;
  get_my_native_xrp_payouts?: () => Promise<NativeXrpPendingPayout[]>;
  ack_native_xrp_payout_settled?: (claimId: bigint) => Promise<{ Ok: null } | { Err: unknown }>;
}

type FormatError = (err: unknown) => string;

function assertOk(result: { Ok: null } | { Err: unknown }, formatError: FormatError): void {
  if ('Err' in result) {
    throw new Error(formatError(result.Err));
  }
}

function destinationTagOpt(destinationTag?: number): CandidOpt<number> {
  if (destinationTag === undefined) return [];
  if (!Number.isInteger(destinationTag) || destinationTag < 0 || destinationTag > 0xffffffff) {
    throw new Error('Destination tag must be a whole number from 0 to 4294967295.');
  }
  return [destinationTag];
}

export async function optInNativeCollateralWithTagUsingActor(
  actor: StabilityPoolNativeXrpActor,
  collateralType: Principal,
  payoutAddress: string,
  destinationTag: number | undefined,
  formatError: FormatError
): Promise<void> {
  const address = payoutAddress.trim();
  if (!address) throw new Error('Enter an XRP address');

  if (actor.opt_in_native_collateral_with_tag) {
    const result = await actor.opt_in_native_collateral_with_tag(
      collateralType,
      address,
      destinationTagOpt(destinationTag)
    );
    assertOk(result, formatError);
    return;
  }

  if (destinationTag === undefined && actor.opt_in_native_collateral) {
    const result = await actor.opt_in_native_collateral(collateralType, address);
    assertOk(result, formatError);
    return;
  }

  throw new Error('Destination tags are not available on this Stability Pool canister yet.');
}

export async function getMyNativeXrpPayoutsWithActor(
  actor: StabilityPoolNativeXrpActor
): Promise<NativeXrpPendingPayout[]> {
  if (!actor.get_my_native_xrp_payouts) return [];
  return actor.get_my_native_xrp_payouts();
}

export async function ackNativeXrpPayoutSettledWithActor(
  actor: StabilityPoolNativeXrpActor,
  claimId: XrpClaimId | number | bigint,
  formatError: FormatError
): Promise<void> {
  if (!actor.ack_native_xrp_payout_settled) {
    throw new Error('Pending XRP payout acknowledgement is not available on this Stability Pool canister yet.');
  }
  const result = await actor.ack_native_xrp_payout_settled(xrpClaimIdToBigInt(claimId));
  assertOk(result, formatError);
}
