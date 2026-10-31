/**
 * Status mapping and error wording for the Sui bridge branch.
 *
 * The bridge canister is the settlement authority. Everything that comes from a wallet or a Sui node (a digest, a
 * callback, an event, a transaction status) is a hint and is labelled as such: a Sui-to-ICP return is reported as
 * released only when the canister's own release record says `Released`.
 */

import type { CandidateView, DepositView, ReleaseView, SuiBridgeError } from '$declarations/rumi_icusd_sui_bridge/rumi_icusd_sui_bridge.did';
import { formatE8s } from '../baseBridge/amounts';
import type { Tone } from '../baseBridge/status';

export type { Tone };

function variantKey(value: object): string {
  return Object.keys(value)[0] ?? '';
}

// ---------------------------------------------------------------------------
// ICP -> Sui: deposit status
// ---------------------------------------------------------------------------

export type SuiDepositStage = 'pulling' | 'pull_unknown' | 'pull_failed' | 'escrowed' | 'minting' | 'delivered' | 'held' | 'refunding' | 'refunded';

export interface SuiDepositDisplay {
  stage: SuiDepositStage;
  label: string;
  detail: string;
  tone: Tone;
  terminal: boolean;
  /** Owner of a held deposit with no mint in flight: the refund button may be shown. */
  canRefund: boolean;
  /** Sui transaction digest of the finalized mint, once the canister has recorded delivery. */
  digest?: string;
  holdReason?: string;
}

export function describeSuiDeposit(view: DepositView, ctx: { principal?: string | null } = {}): SuiDepositDisplay {
  const status = view.status as Record<string, any>;
  const isOwner = !!ctx.principal && view.owner.toText() === ctx.principal;
  const base = { canRefund: false, terminal: false } as const;
  switch (variantKey(status)) {
    case 'PullPending':
      return { ...base, stage: 'pulling', label: 'Pulling icUSD', detail: 'The bridge is moving your icUSD into escrow on the Internet Computer.', tone: 'pending' };
    case 'PullUnknown':
      return {
        ...base,
        stage: 'pull_unknown',
        label: 'Confirming escrow',
        detail: 'The ledger reply has not been confirmed yet. The bridge retries the identical transfer, so you cannot be charged twice.',
        tone: 'pending',
      };
    case 'PullFailed':
      return {
        ...base,
        stage: 'pull_failed',
        label: 'Escrow failed (no funds moved)',
        detail: (status.PullFailed?.reason as string) || 'The ledger did not accept the transfer. Your icUSD was not moved.',
        tone: 'error',
        terminal: true,
      };
    case 'Escrowed':
      return { ...base, stage: 'escrowed', label: 'In escrow', detail: 'Your icUSD is locked in escrow. The bridge is preparing the mint on Sui.', tone: 'pending' };
    case 'MintInFlight':
      return {
        ...base,
        stage: 'minting',
        label: 'Minting on Sui',
        detail: 'The bridge signed the mint. It is delivered only once the transaction is final on Sui and recorded by the bridge.',
        tone: 'pending',
      };
    case 'Delivered': {
      const inclusion = view.delivered[0];
      return {
        ...base,
        stage: 'delivered',
        label: 'Delivered on Sui',
        detail: 'The mint is final on Sui and recorded by the bridge.',
        tone: 'success',
        terminal: true,
        ...(inclusion?.tx_digest[0] ? { digest: inclusion.tx_digest[0] } : {}),
      };
    }
    case 'MintHeld': {
      const reason = (status.MintHeld?.reason as string) ?? '';
      return {
        ...base,
        stage: 'held',
        label: 'Held',
        detail: `The mint was not sent: ${reason || 'no reason given'}. Your icUSD is safe in escrow${isOwner ? ' and you can request a refund' : ''}.`,
        tone: 'warning',
        canRefund: isOwner && view.kind && 'UserDeposit' in view.kind && view.mint_op_id[0] === undefined,
        holdReason: reason,
      };
    }
    case 'Refunding':
      return { ...base, stage: 'refunding', label: 'Refunding', detail: 'The bridge is returning your icUSD on the Internet Computer.', tone: 'warning' };
    case 'Refunded':
      return { ...base, stage: 'refunded', label: 'Refunded', detail: 'Your icUSD was returned on the Internet Computer.', tone: 'success', terminal: true };
    default:
      return { ...base, stage: 'escrowed', label: 'In progress', detail: 'The bridge reported a status this page does not recognise. Check back shortly.', tone: 'pending' };
  }
}

// ---------------------------------------------------------------------------
// Sui -> ICP: release status (the canister's records only)
// ---------------------------------------------------------------------------

export type SuiReturnStage = 'unseen' | 'observed' | 'release_pending' | 'confirming' | 'released' | 'held' | 'reminting' | 'reminted';

export interface SuiReturnDisplay {
  stage: SuiReturnStage;
  label: string;
  detail: string;
  tone: Tone;
  terminal: boolean;
  releaseBlockIndex?: bigint;
  holdReason?: string;
}

export function describeSuiRelease(view: ReleaseView): SuiReturnDisplay {
  const status = view.status as Record<string, any>;
  switch (variantKey(status)) {
    case 'Pending':
      return {
        stage: 'release_pending',
        label: 'Burn observed, release pending',
        detail: 'The bridge verified the burn of your returned icUSD on Sui and is releasing icUSD on the Internet Computer.',
        tone: 'pending',
        terminal: false,
      };
    case 'TransferUnknown':
      return {
        stage: 'confirming',
        label: 'Confirming',
        detail: 'The release was sent but the ledger reply is not confirmed yet. The bridge retries the identical transfer, so it cannot pay twice.',
        tone: 'pending',
        terminal: false,
      };
    case 'Released': {
      const index = view.release_block_index[0];
      const received = view.amount_to_receive_e8s[0];
      return {
        stage: 'released',
        label: index !== undefined ? `Released (ledger block ${index})` : 'Released',
        detail: `${received !== undefined ? `${formatE8s(received)} icUSD was` : 'icUSD was'} released on the Internet Computer.`,
        tone: 'success',
        terminal: true,
        releaseBlockIndex: index,
      };
    }
    case 'Held': {
      const reason = (status.Held?.reason as string) ?? '';
      return {
        stage: 'held',
        label: 'Held',
        detail: `The bridge could not release to the ICP account: ${reason || 'no reason given'}. The amount is re-minted to the Sui address that sent it.`,
        tone: 'warning',
        terminal: false,
        holdReason: reason,
      };
    }
    case 'Reminting':
      return { stage: 'reminting', label: 'Re-minting to your Sui address', detail: 'The bridge is minting the held amount back to the Sui address that sent it.', tone: 'warning', terminal: false };
    case 'Reminted':
      return { stage: 'reminted', label: 'Re-minted to your Sui address', detail: 'The held amount was minted back to the Sui address that sent it.', tone: 'success', terminal: true };
    default:
      return { stage: 'release_pending', label: 'In progress', detail: 'The bridge reported a status this page does not recognise. Check back shortly.', tone: 'pending', terminal: false };
  }
}

/**
 * The bridge's view of a return: the release record when it has one, else (only as a hint about what the observer is
 * doing) a candidate it has heard of, else nothing yet. Nothing here comes from a wallet or a node.
 */
export function describeSuiReturn(release: ReleaseView | null, candidate: CandidateView | null): SuiReturnDisplay {
  if (release) return describeSuiRelease(release);
  if (candidate) {
    const waiting = candidate.waiting[0];
    return {
      stage: 'observed',
      label: candidate.verified ? 'Verified, release starting' : 'Seen by the bridge, verifying',
      detail: candidate.verified
        ? 'The bridge verified your burn on Sui and is opening the release on the Internet Computer.'
        : `The bridge has heard of your transaction and is verifying it against the Gate${waiting ? ` (${waiting})` : ''}. This is a wait, not a failure.`,
      tone: 'pending',
      terminal: false,
    };
  }
  return {
    stage: 'unseen',
    label: 'Waiting for the bridge to see your transfer',
    detail: 'The bridge opens a release only from the Gate\'s own record of your burn, after it is final on Sui. This is a wait, not a failure.',
    tone: 'pending',
    terminal: false,
  };
}

// ---------------------------------------------------------------------------
// Bridge errors
// ---------------------------------------------------------------------------

function nsToLocalTime(ns: bigint): string {
  return new Date(Number(ns / 1_000_000n)).toLocaleString();
}

/**
 * `no_effect`: the bridge refused before it created anything. `uncertain`: it may have happened (or may still), so the
 * outcome is resolved by looking the request up and retrying under the SAME request id. Unknown variants are uncertain.
 */
export function classifySuiBridgeError(err: SuiBridgeError): 'no_effect' | 'uncertain' {
  const key = Object.keys(err)[0] ?? '';
  switch (key) {
    case 'Anonymous':
    case 'Paused':
    case 'AmountBelowMinimum':
    case 'AmountAboveMaximum':
    case 'CapacityExceeded':
    case 'TooManyActiveOperations':
    case 'Unauthorized':
    case 'InvalidRecipient':
    case 'RateLimited':
    case 'NotConfigured':
    case 'InvalidArgument':
    case 'Halted':
    case 'RequestIdConflict':
    case 'FeeChanged':
    case 'InsufficientAllowance':
    case 'InsufficientFunds':
    case 'DeploymentMismatch':
      return 'no_effect';
    default:
      return 'uncertain';
  }
}

/** Plain text for a Sui bridge error. */
export function formatSuiBridgeError(err: SuiBridgeError): string {
  if ('Internal' in err) return `The bridge hit an internal error: ${err.Internal}`;
  if ('Anonymous' in err) return 'Sign in with your ICP wallet to use the bridge.';
  if ('Paused' in err) return `The bridge is paused: ${err.Paused}`;
  if ('InsufficientAllowance' in err) {
    return `Your allowance for the bridge is ${formatE8s(err.InsufficientAllowance.allowance_e8s)} icUSD, but ${formatE8s(err.InsufficientAllowance.required_e8s)} icUSD is needed. Approve again, then bridge.`;
  }
  if ('AmountBelowMinimum' in err) return `The minimum amount is ${formatE8s(err.AmountBelowMinimum.min_e8s)} icUSD.`;
  if ('AmountAboveMaximum' in err) return `The maximum amount per transfer is ${formatE8s(err.AmountAboveMaximum.max_e8s)} icUSD.`;
  if ('CapacityExceeded' in err) return `The bridge can take only ${formatE8s(err.CapacityExceeded.available_e8s)} icUSD more right now.`;
  if ('DeploymentMismatch' in err) return `The Sui deployment the bridge observed does not match its configuration: ${err.DeploymentMismatch}`;
  if ('SignerFailed' in err) return `The bridge could not sign on Sui: ${err.SignerFailed}`;
  if ('NotFound' in err) return 'The bridge has no record of that request.';
  if ('TooManyActiveOperations' in err) return 'Too many bridge transfers are in progress. Wait for one to finish, then try again.';
  if ('Unauthorized' in err) return 'You are not allowed to do that.';
  if ('NotOwner' in err) return 'Only the account that made this deposit can do that.';
  if ('InvalidRecipient' in err) return `That Sui recipient is not valid: ${err.InvalidRecipient}`;
  if ('RateLimited' in err) {
    return `The bridge's rolling limit is reached (${formatE8s(err.RateLimited.available_e8s)} icUSD available now). It resets around ${nsToLocalTime(err.RateLimited.window_resets_at_ns)}.`;
  }
  if ('NotConfigured' in err) return `The bridge is not fully configured yet: ${err.NotConfigured}`;
  if ('InvalidArgument' in err) return `The bridge rejected the request: ${err.InvalidArgument}`;
  if ('Ledger' in err) return `The icUSD ledger reported a problem: ${err.Ledger}`;
  if ('Unavailable' in err) return `The bridge could not reach Sui: ${err.Unavailable}`;
  if ('Halted' in err) return `The bridge is halted for safety: ${err.Halted}`;
  if ('RequestIdConflict' in err) return 'A different bridge request already uses this request id. Start a new transfer.';
  if ('InvalidState' in err) return `That action is not available in the current state: ${err.InvalidState}`;
  if ('FeeChanged' in err) return `The bridge fee changed to ${formatE8s(err.FeeChanged.current_fee_e8s)} icUSD. Review and confirm again.`;
  if ('InsufficientFunds' in err) {
    return `Your icUSD balance is ${formatE8s(err.InsufficientFunds.balance_e8s)} icUSD, but ${formatE8s(err.InsufficientFunds.required_e8s)} icUSD is needed (the amount plus the ledger fee).`;
  }
  if ('Conflict' in err) return `The bridge reported a conflict: ${err.Conflict}`;
  return exhaustive(err);
}

/** Compile-time check that every variant is handled above; a variant from a newer canister gets a generic message. */
function exhaustive(_unhandled: never): string {
  return 'The bridge returned an error this page does not recognise.';
}
