import type { Principal } from '@dfinity/principal';
import { SOL_NATIVE_PRINCIPAL_TEXT, solClaimIdToBigInt, type SolClaimId } from './solPayoutHelpers';

export { SOL_NATIVE_PRINCIPAL_TEXT };

export interface NativeSolPendingPayout {
  claim_id: bigint;
  collateral_type: Principal;
  vault_id: bigint;
  lamports: bigint;
  payout_address: string;
  created_at_ns: bigint;
}

/**
 * SOL opt-in is tagless only: there is no `opt_in_native_collateral_with_tag`
 * counterpart, unlike XRP. `opt_in_native_collateral(collateralType, payoutAddress)`
 * works unchanged for SOL (design spec §6).
 */
export interface StabilityPoolNativeSolActor {
  opt_in_native_collateral?: (
    collateralType: Principal,
    payoutAddress: string
  ) => Promise<{ Ok: null } | { Err: unknown }>;
  get_my_native_sol_payouts?: () => Promise<NativeSolPendingPayout[]>;
  ack_native_sol_payout_settled?: (claimId: bigint) => Promise<{ Ok: null } | { Err: unknown }>;
}

type FormatError = (err: unknown) => string;

function assertOk(result: { Ok: null } | { Err: unknown }, formatError: FormatError): void {
  if ('Err' in result) {
    throw new Error(formatError(result.Err));
  }
}

export async function optInNativeSolCollateralUsingActor(
  actor: StabilityPoolNativeSolActor,
  collateralType: Principal,
  payoutAddress: string,
  formatError: FormatError
): Promise<void> {
  const address = payoutAddress.trim();
  if (!address) throw new Error('Enter a SOL address');

  if (!actor.opt_in_native_collateral) {
    throw new Error('Native SOL opt-in is not available on this Stability Pool canister yet.');
  }
  const result = await actor.opt_in_native_collateral(collateralType, address);
  assertOk(result, formatError);
}

export async function getMyNativeSolPayoutsWithActor(
  actor: StabilityPoolNativeSolActor
): Promise<NativeSolPendingPayout[]> {
  if (!actor.get_my_native_sol_payouts) return [];
  return actor.get_my_native_sol_payouts();
}

export async function ackNativeSolPayoutSettledWithActor(
  actor: StabilityPoolNativeSolActor,
  claimId: SolClaimId | number | bigint,
  formatError: FormatError
): Promise<void> {
  if (!actor.ack_native_sol_payout_settled) {
    throw new Error('Pending SOL payout acknowledgement is not available on this Stability Pool canister yet.');
  }
  const result = await actor.ack_native_sol_payout_settled(solClaimIdToBigInt(claimId));
  assertOk(result, formatError);
}
