import type { Principal } from '@dfinity/principal';

/** Route hint only; the canister authorizes configured viewers and signers. */
export function canViewSentinelTelemetry(principal: Principal | null | undefined): boolean {
	return principal !== null && principal !== undefined && !principal.isAnonymous();
}
