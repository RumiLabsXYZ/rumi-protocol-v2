/**
 * Status mapping, error wording and polling helpers for the Base bridge.
 *
 * The canister is the settlement authority. Everything that comes from a wallet
 * (a transaction hash, a receipt, a callback) is a hint and is labelled as such.
 * In particular, a Base-to-ICP transfer is never reported as released unless
 * the canister's own release record says `Released`.
 */

import type { Principal } from '@dfinity/principal';
import type {
  BridgeError,
  DepositView,
  ReleaseView,
  _SERVICE as BridgeService,
} from '$declarations/rumi_icusd_bridge/rumi_icusd_bridge.did';
import { formatE8s } from './amounts';
import type { EvmReceipt } from './evmWallet';

export type Tone = 'pending' | 'success' | 'warning' | 'error';

/** Wording for the expected settlement delay, shared by every surface. */
export const FINALITY_NOTE = 'about 20 minutes';

// ---------------------------------------------------------------------------
// ICP -> Base: deposit status
// ---------------------------------------------------------------------------

export type DepositStage =
  | 'pulling'
  | 'pull_unknown'
  | 'pull_failed'
  | 'escrowed'
  | 'minting'
  | 'included'
  | 'delivered'
  | 'held'
  | 'refunding'
  | 'refunded';

export interface DepositDisplay {
  stage: DepositStage;
  label: string;
  detail: string;
  tone: Tone;
  /** True when polling can stop (nothing further will change without user action). */
  terminal: boolean;
  /** Base block of the mint, when known. */
  blockNumber?: bigint;
  holdReason?: string;
  /** Owner of an unhappy `MintHeld` deposit: the refund button may be shown. */
  canRefund: boolean;
  /** True only once the canister has recorded finalized delivery. */
  finalized: boolean;
}

export interface DepositContext {
  /** Text form of the signed-in ICP principal. */
  principal?: string | null;
}

function variantKey(value: object): string {
  return Object.keys(value)[0] ?? '';
}

export function describeDeposit(view: DepositView, ctx: DepositContext = {}): DepositDisplay {
  const status = view.status as Record<string, unknown>;
  const key = variantKey(status);
  const stage = view.mint_stage[0] as Record<string, any> | undefined;
  const stageKey = stage ? variantKey(stage) : '';
  const isOwner = !!ctx.principal && view.owner.toText() === ctx.principal;
  const base = { canRefund: false, finalized: false, terminal: false } as const;

  switch (key) {
    case 'PullPending':
      return {
        ...base,
        stage: 'pulling',
        label: 'Pulling icUSD',
        detail: 'The bridge is moving your icUSD into escrow on the Internet Computer.',
        tone: 'pending',
      };
    case 'PullUnknown':
      return {
        ...base,
        stage: 'pull_unknown',
        label: 'Confirming escrow',
        detail:
          'The ledger reply has not been confirmed yet. The bridge retries the identical transfer, so you cannot be charged twice.',
        tone: 'pending',
      };
    case 'PullFailed': {
      const reason = (status.PullFailed as { reason: string }).reason;
      return {
        ...base,
        stage: 'pull_failed',
        label: 'Escrow failed (no funds moved)',
        detail: reason || 'The ledger did not accept the transfer. Your icUSD was not moved.',
        tone: 'error',
        terminal: true,
        holdReason: reason,
      };
    }
    case 'Escrowed':
      return {
        ...base,
        stage: 'escrowed',
        label: 'Escrowed',
        detail: 'Your icUSD is locked in escrow. The Base mint is queued.',
        tone: 'pending',
      };
    case 'MintInFlight': {
      if (stageKey === 'Included') {
        const blockNumber = BigInt(stage!.Included.block_number);
        return {
          ...base,
          stage: 'included',
          label: `Included on Base (block ${blockNumber})`,
          detail: `The mint is on Base but not final yet. The bridge counts it as delivered after Base L1 batch finality, ${FINALITY_NOTE}.`,
          tone: 'pending',
          blockNumber,
        };
      }
      if (stageKey === 'Finalized' && stage!.Finalized.success) {
        const blockNumber = BigInt(stage!.Finalized.block_number);
        return {
          ...base,
          stage: 'included',
          label: `Included on Base (block ${blockNumber})`,
          detail: 'The mint is final on Base. The bridge is confirming delivery.',
          tone: 'pending',
          blockNumber,
        };
      }
      const reverted = stageKey === 'Finalized';
      return {
        ...base,
        stage: 'minting',
        label: 'Minting on Base',
        detail: reverted
          ? 'A mint attempt did not succeed on Base. The bridge will retry or hold it; your escrowed icUSD is safe.'
          : 'The mint transaction is being sent to Base and waiting to be included.',
        tone: 'pending',
      };
    }
    case 'Delivered': {
      const delivered = view.delivered[0];
      return {
        ...base,
        stage: 'delivered',
        label: 'Delivered (finalized)',
        detail: `${formatE8s(view.mint_amount_e8s)} icUSD was minted on Base to ${view.base_recipient}.`,
        tone: 'success',
        terminal: true,
        finalized: true,
        blockNumber: delivered ? BigInt(delivered.block_number) : undefined,
      };
    }
    case 'MintHeld': {
      const reason = (status.MintHeld as { reason: string }).reason;
      const canRefund = isOwner && variantKey(view.kind) === 'UserDeposit';
      return {
        ...base,
        stage: 'held',
        label: 'Held',
        detail: `The Base mint is on hold: ${reason || 'no reason given'}. Your escrowed icUSD is safe. A hold caused by a pause or limit is retried automatically; a reverted mint waits for an operator retry${canRefund ? ' or your refund' : ''}.`,
        tone: 'warning',
        holdReason: reason,
        canRefund,
      };
    }
    case 'Refunding':
      return {
        ...base,
        stage: 'refunding',
        label: 'Refunding',
        detail: 'The bridge is returning your escrowed icUSD.',
        tone: 'pending',
      };
    case 'Refunded': {
      const amount = view.refund_amount_e8s[0];
      return {
        ...base,
        stage: 'refunded',
        label: 'Refunded',
        detail: `${amount !== undefined ? `${formatE8s(amount)} icUSD was` : 'Your icUSD was'} returned to your account.`,
        tone: 'success',
        terminal: true,
      };
    }
    default:
      return {
        ...base,
        stage: 'pulling',
        label: 'In progress',
        detail: 'The bridge reported a status this page does not recognise. Check back shortly.',
        tone: 'pending',
      };
  }
}

// ---------------------------------------------------------------------------
// Base -> ICP: release status
// ---------------------------------------------------------------------------

export type ReleaseStage = 'release_pending' | 'confirming' | 'released' | 'held' | 'reminting' | 'reminted';

export interface ReleaseDisplay {
  stage: ReleaseStage;
  label: string;
  detail: string;
  tone: Tone;
  terminal: boolean;
  /** ICP ledger block of the release, once the canister has one. */
  releaseBlockIndex?: bigint;
  holdReason?: string;
}

export function describeRelease(view: ReleaseView): ReleaseDisplay {
  const status = view.status as Record<string, any>;
  const key = variantKey(status);
  switch (key) {
    case 'Pending':
      return {
        stage: 'release_pending',
        label: 'Release pending',
        detail: 'The bridge verified your finalized Base burn and is releasing icUSD on the Internet Computer.',
        tone: 'pending',
        terminal: false,
      };
    case 'TransferUnknown':
      return {
        stage: 'confirming',
        label: 'Confirming',
        detail:
          'The release was sent but the ledger reply is not confirmed yet. The bridge retries the identical transfer, so it cannot pay twice.',
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
      const reason = status.Held.reason as string;
      return {
        stage: 'held',
        label: 'Held',
        detail: `The bridge could not release to the ICP account: ${reason || 'no reason given'}. The amount is re-minted to your Base address.`,
        tone: 'warning',
        terminal: false,
        holdReason: reason,
      };
    }
    case 'Reminting':
      return {
        stage: 'reminting',
        label: 'Re-minting to your Base address',
        detail: 'The bridge is minting the held amount back to the Base address that sent it.',
        tone: 'warning',
        terminal: false,
      };
    case 'Reminted':
      return {
        stage: 'reminted',
        label: 'Re-minted to your Base address',
        detail: 'The held amount was minted back to the Base address that sent it.',
        tone: 'success',
        terminal: true,
      };
    default:
      return {
        stage: 'release_pending',
        label: 'In progress',
        detail: 'The bridge reported a status this page does not recognise. Check back shortly.',
        tone: 'pending',
        terminal: false,
      };
  }
}

export type BaseOutStage =
  | 'submitted'
  | 'reverted'
  | 'awaiting_finality'
  | ReleaseStage;

export interface BaseOutTracking {
  stage: BaseOutStage;
  label: string;
  detail: string;
  tone: Tone;
  terminal: boolean;
  /** Base block from the wallet receipt. A hint. */
  includedBlock?: bigint;
  release?: ReleaseDisplay;
}

/**
 * Combine the wallet's view (receipt) with the canister's (release record) and
 * the contract's (`bridgeOutUsed`). Only a canister `Released` status can
 * produce `released`.
 */
export function deriveBaseOutTracking(input: {
  receipt: EvmReceipt | null;
  release: ReleaseView | null;
  /** Result of `bridgeOutUsed(transferId)`, or null when it could not be read. */
  transferUsed: boolean | null;
}): BaseOutTracking {
  const { receipt, release, transferUsed } = input;
  if (release) {
    const display = describeRelease(release);
    return {
      stage: display.stage,
      label: display.label,
      detail: display.detail,
      tone: display.tone,
      terminal: display.terminal,
      includedBlock: receipt?.success ? receipt.blockNumber : undefined,
      release: display,
    };
  }
  if (receipt && !receipt.success && transferUsed !== true) {
    return {
      stage: 'reverted',
      label: 'Failed on Base',
      detail: 'Your wallet reports that the transaction failed on Base. Nothing was burned and nothing will be released.',
      tone: 'error',
      terminal: true,
    };
  }
  if ((receipt && receipt.success) || transferUsed === true) {
    return {
      stage: 'awaiting_finality',
      label: 'Awaiting finality',
      detail: `${receipt?.success ? `Included on Base (block ${receipt.blockNumber}). ` : 'The burn is recorded on Base. '}The bridge releases on the Internet Computer only after Base L1 batch finality, ${FINALITY_NOTE}.`,
      tone: 'pending',
      terminal: false,
      includedBlock: receipt?.success ? receipt.blockNumber : undefined,
    };
  }
  return {
    stage: 'submitted',
    label: 'Submitted to Base',
    detail: 'Waiting for Base to include the transaction. This status comes from your wallet and is only a hint.',
    tone: 'pending',
    terminal: false,
  };
}

// ---------------------------------------------------------------------------
// Bridge errors
// ---------------------------------------------------------------------------

export function bridgeErrorKind(err: BridgeError): string {
  return variantKey(err);
}

/**
 * `no_effect`: the canister rejected the request before creating anything.
 * `uncertain`: the failure may have happened after an operation was created, so
 * the outcome has to be resolved by looking the request up.
 */
export function classifyBridgeError(err: BridgeError): 'no_effect' | 'uncertain' {
  switch (bridgeErrorKind(err)) {
    case 'Internal':
    case 'Rpc':
    case 'Ledger':
    case 'InvalidState':
    case 'NotFound':
    case 'NotOwner':
      return 'uncertain';
    default:
      return 'no_effect';
  }
}

function nsToLocalTime(ns: bigint): string {
  const ms = Number(ns / 1_000_000n);
  return new Date(ms).toLocaleString();
}

/** Plain-language text for a bridge error. Narrows on the generated `BridgeError` union, so a new variant is a compile error here. */
export function formatBridgeError(err: BridgeError): string {
  if ('Rpc' in err) return `The bridge could not reach Base: ${err.Rpc}`;
  if ('Internal' in err) return `The bridge hit an internal error: ${err.Internal}`;
  if ('Anonymous' in err) return 'Sign in with your ICP wallet to use the bridge.';
  if ('Paused' in err) return `The bridge is paused: ${err.Paused}`;
  if ('AmountBelowMinimum' in err) return `The minimum amount is ${formatE8s(err.AmountBelowMinimum.min_e8s)} icUSD.`;
  if ('AmountAboveMaximum' in err) return `The maximum amount per transfer is ${formatE8s(err.AmountAboveMaximum.max_e8s)} icUSD.`;
  if ('CapacityExceeded' in err) return `The bridge can take only ${formatE8s(err.CapacityExceeded.available_e8s)} icUSD more right now.`;
  if ('NotFound' in err) return 'The bridge has no record of that request.';
  if ('TooManyActiveOperations' in err) return 'Too many bridge transfers are in progress. Wait for one to finish, then try again.';
  if ('Unauthorized' in err) return 'You are not allowed to do that.';
  if ('NotOwner' in err) return 'Only the account that made this deposit can do that.';
  if ('InvalidRecipient' in err) return `That Base recipient is not valid: ${err.InvalidRecipient}`;
  if ('RateLimited' in err) {
    return `The bridge's rolling limit is reached (${formatE8s(err.RateLimited.available_e8s)} icUSD available now). It resets around ${nsToLocalTime(err.RateLimited.window_resets_at_ns)}.`;
  }
  if ('NotConfigured' in err) return `The bridge is not fully configured yet: ${err.NotConfigured}`;
  if ('InvalidArgument' in err) return `The bridge rejected the request: ${err.InvalidArgument}`;
  if ('Ledger' in err) return `The icUSD ledger reported a problem: ${err.Ledger}`;
  if ('Halted' in err) return `The bridge is halted for safety: ${err.Halted}`;
  if ('RequestIdConflict' in err) return 'A different bridge request already uses this request id. Start a new transfer.';
  if ('InvalidState' in err) return `That action is not available in the current state: ${err.InvalidState}`;
  if ('FeeChanged' in err) return `The bridge fee changed to ${formatE8s(err.FeeChanged.current_fee_e8s)} icUSD. Review and confirm again.`;
  // Returned by bridge_to_base before any deposit exists, so nothing was created.
  if ('InsufficientAllowance' in err) {
    return `Your allowance for the bridge is ${formatE8s(err.InsufficientAllowance.allowance_e8s)} icUSD, but ${formatE8s(err.InsufficientAllowance.required_e8s)} icUSD is needed. Approve again, then bridge.`;
  }
  if ('InsufficientFunds' in err) {
    return `Your icUSD balance is ${formatE8s(err.InsufficientFunds.balance_e8s)} icUSD, but ${formatE8s(err.InsufficientFunds.required_e8s)} icUSD is needed (the amount plus the ledger fee).`;
  }
  return exhaustiveBridgeError(err);
}

/** Compile-time check that every `BridgeError` variant is handled above; at runtime a variant from a newer canister gets a generic message. */
function exhaustiveBridgeError(_unhandled: never): string {
  return 'The bridge returned an error this page does not recognise.';
}

// ---------------------------------------------------------------------------
// ICP ledger approve errors
// ---------------------------------------------------------------------------

export function formatApproveError(err: unknown): string {
  const e = err as Record<string, any>;
  const key = err && typeof err === 'object' ? variantKey(err) : '';
  switch (key) {
    case 'InsufficientFunds':
      return `Your icUSD balance (${formatE8s(BigInt(e.InsufficientFunds.balance))}) cannot cover the approval fee.`;
    case 'BadFee':
      return `The icUSD ledger fee is now ${formatE8s(BigInt(e.BadFee.expected_fee))} icUSD. Review the amounts and try again.`;
    case 'TemporarilyUnavailable':
      return 'The icUSD ledger is busy. Try again in a moment.';
    case 'GenericError':
      return `The icUSD ledger rejected the approval: ${e.GenericError.message}`;
    case 'AllowanceChanged':
      return 'Your allowance changed while approving. Try again.';
    case 'Expired':
    case 'TooOld':
    case 'CreatedInFuture':
      return 'The approval request was rejected as out of date. Check that your device clock is correct, then try again.';
    default:
      return 'The icUSD ledger did not accept the approval.';
  }
}

// ---------------------------------------------------------------------------
// Polling
// ---------------------------------------------------------------------------

export interface BackoffOptions {
  baseMs?: number;
  maxMs?: number;
  factor?: number;
}

/** Delay before poll number `attempt` (0-based): grows by `factor` and is capped at `maxMs`. */
export function pollDelayMs(attempt: number, opts: BackoffOptions = {}): number {
  const base = opts.baseMs ?? 2_000;
  const max = opts.maxMs ?? 30_000;
  const factor = opts.factor ?? 1.5;
  return Math.min(max, Math.round(base * Math.pow(factor, Math.max(0, attempt))));
}

export type Sleep = (ms: number, signal?: AbortSignal) => Promise<void>;

/** Sleep that wakes early when the signal aborts. */
export const abortableSleep: Sleep = (ms, signal) =>
  new Promise<void>((resolve) => {
    if (signal?.aborted) return resolve();
    const timer = setTimeout(done, ms);
    function done() {
      clearTimeout(timer);
      signal?.removeEventListener('abort', done);
      resolve();
    }
    signal?.addEventListener('abort', done, { once: true });
  });

export interface PollOptions<T> {
  fetch: () => Promise<T>;
  isDone: (value: T) => boolean;
  onUpdate: (value: T) => void;
  onError?: (error: unknown) => void;
  sleep?: Sleep;
  signal?: AbortSignal;
  backoff?: BackoffOptions;
  /** Stop after this many fetches even if not done. Default 600. */
  maxAttempts?: number;
}

/** Poll until done, aborted or out of attempts. Errors are reported and retried with the same backoff. */
export async function pollWithBackoff<T>(opts: PollOptions<T>): Promise<'done' | 'aborted' | 'exhausted'> {
  const sleep = opts.sleep ?? abortableSleep;
  const maxAttempts = opts.maxAttempts ?? 600;
  for (let attempt = 0; attempt < maxAttempts; attempt++) {
    if (opts.signal?.aborted) return 'aborted';
    try {
      const value = await opts.fetch();
      if (opts.signal?.aborted) return 'aborted';
      opts.onUpdate(value);
      if (opts.isDone(value)) return 'done';
    } catch (error) {
      if (opts.signal?.aborted) return 'aborted';
      opts.onError?.(error);
    }
    await sleep(pollDelayMs(attempt, opts.backoff), opts.signal);
  }
  return opts.signal?.aborted ? 'aborted' : 'exhausted';
}

// ---------------------------------------------------------------------------
// Small formatting helpers
// ---------------------------------------------------------------------------

export function shortHash(value: string, head = 10, tail = 6): string {
  return value.length <= head + tail + 1 ? value : `${value.slice(0, head)}...${value.slice(-tail)}`;
}

export function shortAddress(value: string): string {
  return shortHash(value, 6, 4);
}

export function formatTimestampNs(ns: bigint): string {
  return nsToLocalTime(ns);
}

// ---------------------------------------------------------------------------
// History (public queries only, so every wallet type gets the same history)
// ---------------------------------------------------------------------------

export const HISTORY_PAGE = 20;

export type HistoryReader = Pick<
  BridgeService,
  'list_deposits_for_owner' | 'list_releases_for_owner' | 'list_releases_for_base_sender'
>;

export interface BridgeHistory {
  /** Deposits to Base, newest first. */
  deposits: DepositView[];
  /** Releases to the Internet Computer, newest first, deduplicated by transfer id. */
  releases: ReleaseView[];
  /** True when the Base-sender listing failed, so held claims for this address may be missing. */
  senderListingFailed: boolean;
}

/** Merge release listings, keeping one row per transfer id, newest first. */
export function mergeReleases(...lists: ReleaseView[][]): ReleaseView[] {
  const byId = new Map<string, ReleaseView>();
  for (const list of lists) {
    for (const release of list) {
      const key = release.transfer_id.toLowerCase();
      const known = byId.get(key);
      // The most recently updated record wins if two listings disagree.
      if (!known || release.updated_at_ns > known.updated_at_ns) byId.set(key, release);
    }
  }
  return [...byId.values()].sort((a, b) => (a.created_at_ns === b.created_at_ns ? 0 : a.created_at_ns < b.created_at_ns ? 1 : -1));
}

/**
 * Load the history shown on the page with an anonymous reader:
 *  - deposits by `list_deposits_for_owner(owner)`,
 *  - releases by `list_releases_for_owner(owner)`,
 *  - and, when a Base wallet is connected, releases by
 *    `list_releases_for_base_sender(address)`, so held claims and re-mints show up
 *    on a new device. The two release listings are deduplicated by transfer id.
 */
export async function loadBridgeHistory(
  reader: HistoryReader,
  opts: { owner: Principal; baseSender: string | null; limit?: number },
): Promise<BridgeHistory> {
  const limit = opts.limit ?? HISTORY_PAGE;
  const [deposits, ownerReleases, senderResult] = await Promise.all([
    reader.list_deposits_for_owner({ owner: opts.owner, start_after: [], limit }),
    reader.list_releases_for_owner({ owner: opts.owner, start_after_sequence: [], limit }),
    opts.baseSender
      ? reader
          .list_releases_for_base_sender({ base_sender: opts.baseSender.toLowerCase(), start_after_sequence: [], limit })
          .then((rows) => ({ ok: true as const, rows }), () => ({ ok: false as const, rows: [] as ReleaseView[] }))
      : Promise.resolve({ ok: true as const, rows: [] as ReleaseView[] }),
  ]);
  return {
    deposits: [...deposits].sort((a, b) => (a.id === b.id ? 0 : a.id < b.id ? 1 : -1)),
    releases: mergeReleases(ownerReleases, senderResult.rows),
    senderListingFailed: !senderResult.ok,
  };
}
