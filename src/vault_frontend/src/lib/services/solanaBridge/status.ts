/**
 * Status mapping and error wording for the Solana bridge branch.
 *
 * The bridge canister is the settlement authority. Everything that comes from a wallet or a Solana node (a
 * signature, a callback, a transaction status) is a hint and is labelled as such: a Solana-to-ICP return is
 * reported as released only when the canister's own release record says `Released`.
 */

import type {
  BridgeError,
  DepositView,
  ReleaseView,
  ReturnView,
} from '$declarations/rumi_icusd_sol_bridge/rumi_icusd_sol_bridge.did';
import { formatE8s } from '../baseBridge/amounts';
import { classifyBridgeError, formatBridgeError, type Tone } from '../baseBridge/status';

export type { Tone };

function variantKey(value: object): string {
  return Object.keys(value)[0] ?? '';
}

// ---------------------------------------------------------------------------
// ICP -> Solana: deposit status
// ---------------------------------------------------------------------------

export type SolDepositStage = 'pulling' | 'pull_unknown' | 'pull_failed' | 'escrowed' | 'minting' | 'delivered' | 'held' | 'refunding' | 'refunded';

export interface SolDepositDisplay {
  stage: SolDepositStage;
  label: string;
  detail: string;
  tone: Tone;
  terminal: boolean;
  /** Owner of a held deposit with no mint in flight: the refund button may be shown. */
  canRefund: boolean;
  /** Solana signature of the finalized mint, once the canister has recorded delivery. */
  signature?: string;
  holdReason?: string;
}

export function describeSolDeposit(view: DepositView, ctx: { principal?: string | null } = {}): SolDepositDisplay {
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
      return { ...base, stage: 'escrowed', label: 'In escrow', detail: 'Your icUSD is locked in escrow. The bridge is preparing the mint on Solana.', tone: 'pending' };
    case 'MintInFlight':
      return {
        ...base,
        stage: 'minting',
        label: 'Minting on Solana',
        detail: 'The bridge signed the mint. It is delivered only once the transaction is finalized on Solana.',
        tone: 'pending',
      };
    case 'Delivered': {
      const inclusion = view.delivered[0];
      return {
        ...base,
        stage: 'delivered',
        label: 'Delivered on Solana',
        detail: 'The mint is finalized on Solana and recorded by the bridge.',
        tone: 'success',
        terminal: true,
        ...(inclusion ? { signature: inclusion.signature } : {}),
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
// Solana -> ICP: return and release status
// ---------------------------------------------------------------------------

export type SolReturnStage =
  | 'unseen'
  | 'credited'
  | 'burning'
  | 'release_pending'
  | 'confirming'
  | 'released'
  | 'held'
  | 'reminting'
  | 'reminted';

export interface SolReturnDisplay {
  stage: SolReturnStage;
  label: string;
  detail: string;
  tone: Tone;
  terminal: boolean;
  releaseBlockIndex?: bigint;
  holdReason?: string;
}

export function describeSolRelease(view: ReleaseView): SolReturnDisplay {
  const status = view.status as Record<string, any>;
  switch (variantKey(status)) {
    case 'Pending':
      return {
        stage: 'release_pending',
        label: 'Release pending',
        detail: 'The burn of your returned icUSD is final on Solana and the bridge is releasing icUSD on the Internet Computer.',
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
        detail: `The bridge could not release to the ICP account: ${reason || 'no reason given'}. The amount is re-minted to your Solana wallet.`,
        tone: 'warning',
        terminal: false,
        holdReason: reason,
      };
    }
    case 'Reminting':
      return { stage: 'reminting', label: 'Re-minting to your Solana wallet', detail: 'The bridge is minting the held amount back to the Solana wallet that sent it.', tone: 'warning', terminal: false };
    case 'Reminted':
      return { stage: 'reminted', label: 'Re-minted to your Solana wallet', detail: 'The held amount was minted back to the Solana wallet that sent it.', tone: 'success', terminal: true };
    default:
      return { stage: 'release_pending', label: 'In progress', detail: 'The bridge reported a status this page does not recognise. Check back shortly.', tone: 'pending', terminal: false };
  }
}

/** The bridge's view of a return: nothing yet, credited, burning, or (once burned) the release. */
export function describeSolReturn(ret: ReturnView | null, release: ReleaseView | null): SolReturnDisplay {
  if (release) return describeSolRelease(release);
  if (!ret) {
    return {
      stage: 'unseen',
      label: 'Waiting for the bridge to see your transfer',
      detail: 'The bridge credits a return only after it reads the transfer as finalized on Solana. This is a wait, not a failure.',
      tone: 'pending',
      terminal: false,
    };
  }
  const status = variantKey(ret.status as object);
  if (status === 'Burning' || status === 'Burned') {
    return {
      stage: 'burning',
      label: status === 'Burned' ? 'Burn final, release starting' : 'Burning your returned icUSD',
      detail: 'The bridge destroys the icUSD you returned. It pays on the Internet Computer only after that burn is final on Solana.',
      tone: 'pending',
      terminal: false,
    };
  }
  return {
    stage: 'credited',
    label: 'Credited by the bridge',
    detail: 'The bridge verified your finalized transfer and holds the icUSD in custody. It burns it next, then releases on the Internet Computer.',
    tone: 'pending',
    terminal: false,
  };
}

// ---------------------------------------------------------------------------
// Bridge errors
// ---------------------------------------------------------------------------

export { classifyBridgeError };

/** Plain text for a Solana bridge error. The variants are the Base bridge's; two sentences name Solana. */
export function formatSolBridgeError(err: BridgeError): string {
  if ('Rpc' in err) return `The bridge could not reach Solana: ${err.Rpc}`;
  if ('InvalidRecipient' in err) return `That Solana recipient is not valid: ${err.InvalidRecipient}`;
  return formatBridgeError(err as never);
}
