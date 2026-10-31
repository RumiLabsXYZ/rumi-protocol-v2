/**
 * The one place that decides whether a flow may ask a wallet to sign.
 *
 * `assertCanSign` is called by every flow immediately before its first wallet or
 * signer call (see `icpToBase.ts` and `baseToIcp.ts`). It makes no call of its
 * own: it reads only what it is given, so it is synchronous and can sit in front
 * of a popup-signer's first call without costing the click its user gesture.
 *
 * Two states are kept apart on purpose:
 *  - DISABLED: the destination is not offered (no bridge exists for it, or the
 *    canister's configuration for it cannot be used). Nothing may be signed in
 *    either direction. This is what this guard enforces.
 *  - PAUSED: an enabled destination whose deposits or releases are paused or
 *    whose bridge is halted. That is existing Base behavior and is NOT decided
 *    here: Base to ICP shows a warning that delivery will be held and still lets
 *    the user send, and ICP to Base is gated by `bridgeBlocked` in the form. The
 *    status handed to this guard therefore carries no pause or halt information.
 */

import type { DestinationView } from './destinations';
import { intentDestination, type BridgeIntent } from './intents';

export type SigningBlockCode = 'destination_disabled' | 'destination_unknown' | 'wrong_network';

export class SigningBlockedError extends Error {
  readonly code: SigningBlockCode;
  constructor(code: SigningBlockCode, message: string) {
    super(message);
    this.name = 'SigningBlockedError';
    this.code = code;
  }
}

/** What the caller knows about the wallet it is about to ask. */
export interface SigningWalletState {
  /** `icp_signer` for the Internet Computer wallet, `evm` for the browser wallet on the destination chain. */
  wallet: 'icp_signer' | 'evm';
  /**
   * The network the EVM wallet is on, when the caller has already read it. It is never read here:
   * this function makes no wallet call. Ignored for the ICP signer.
   */
  chainId?: number | string | null;
}

/**
 * The newest known status of the destination, taken from the page's most recent read of the
 * canister (not from the snapshot the form was opened with). `null` means the newest read no
 * longer reports the destination at all.
 */
export type FreshDestinationStatus = Pick<DestinationView, 'status' | 'reason'> | null;

/** What a flow carries to be able to ask. */
export interface SigningContext {
  /** The destination this action is for, as it was when the action began. */
  destination: DestinationView;
  /** Reads the newest known status. Called at the moment of use, never cached. */
  freshStatus(): FreshDestinationStatus;
}

export const NOT_SIGNED_NOTE = 'Your wallet was not asked to sign anything.';

export function unavailableMessage(destination: Pick<DestinationView, 'name'>, reason: string | null): string {
  return `${reason ?? `${destination.name} is not available right now.`} ${NOT_SIGNED_NOTE}`;
}

/**
 * Throws a `SigningBlockedError` unless this destination may be signed for right now.
 * Returns normally for an enabled destination, whether or not its releases or deposits are paused.
 */
export function assertCanSign(
  destination: DestinationView,
  walletState: SigningWalletState,
  freshStatus: FreshDestinationStatus,
): void {
  if (destination.status !== 'enabled') {
    throw new SigningBlockedError('destination_disabled', unavailableMessage(destination, destination.reason));
  }
  if (freshStatus === null) {
    throw new SigningBlockedError('destination_unknown', unavailableMessage(destination, `${destination.name} is no longer reported by the bridge.`));
  }
  if (freshStatus.status !== 'enabled') {
    throw new SigningBlockedError('destination_disabled', unavailableMessage(destination, freshStatus.reason));
  }
  if (
    walletState.wallet === 'evm' &&
    destination.family === 'evm' &&
    walletState.chainId !== undefined &&
    walletState.chainId !== null &&
    String(walletState.chainId) !== destination.chainId
  ) {
    throw new SigningBlockedError(
      'wrong_network',
      `Your wallet is on a different network than ${destination.name} (chain ${destination.chainId}). ${NOT_SIGNED_NOTE}`,
    );
  }
}

/** `assertCanSign` for a flow: the block reason as plain text, or null when signing may go ahead. */
export function signingBlockMessage(signing: SigningContext, walletState: SigningWalletState): string | null {
  try {
    assertCanSign(signing.destination, walletState, signing.freshStatus());
    return null;
  } catch (error) {
    if (error instanceof SigningBlockedError) return error.message;
    throw error;
  }
}

/**
 * A saved request is bound to the destination it was made for. Handing it to the flow of another
 * destination must send nothing: the text says why, or null when the request belongs here. A keyless
 * (legacy) request belongs to Base. Synchronous, and the request itself is left exactly as it was.
 */
export function intentBindingMessage(signing: SigningContext, intent: Pick<BridgeIntent, 'destination'>): string | null {
  const owner = intentDestination(intent);
  if (owner === signing.destination.id) return null;
  return `This saved request is for another destination (${owner}), so it cannot be sent to ${signing.destination.name}. ${NOT_SIGNED_NOTE}`;
}

/** Attach the signing context to the dependencies a builder produced. */
export function withSigning<T extends object>(deps: T, signing: SigningContext): T & { signing: SigningContext } {
  return { ...deps, signing };
}
