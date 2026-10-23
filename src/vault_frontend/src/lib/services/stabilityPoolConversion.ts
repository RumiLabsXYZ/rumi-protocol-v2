import { Principal } from '@dfinity/principal';
import { get } from 'svelte/store';
import { walletStore } from '../stores/wallet';
import { walletSessionGeneration } from './auth';
import { isOisyWallet } from './protocol/walletOperations';
import { CANISTER_IDS } from '../config';
import {
  assertStabilityPoolActionContext,
  captureStabilityPoolActionContext,
  getStabilityPoolLedgerMutationActor,
  getStabilityPoolMutationActor,
  type StabilityPoolActionContext,
} from './stabilityPoolService';

export const CONVERSION_QUOTE_MAX_AGE_MS = 90_000;
export const CONVERSION_SLIPPAGE_BPS = 100n;
const BPS = 10_000n;

export interface StabilityPoolConversionQuote {
  token_ledger: Principal;
  lp_ledger: Principal;
  pool_canister: Principal;
  pool_coin_index: number;
  principal_raw: bigint;
  quoted_lp: bigint;
  live_transfer_fee_raw: bigint;
  minimum_fee_budget_raw: bigint;
  quotedAtMs: number;
}

export interface PendingStabilityPoolConversion {
  owner: string;
  ledger: string;
  principalRaw: string;
  maxFeeBudgetRaw: string;
  minLp: string;
  intentSeq: string;
  walletIcon: string;
  oisy: boolean;
  createdAt: number;
  stage: 'new' | 'approval-submitted' | 'start-submitted';
  approvalFeeRaw?: string;
  approvalAmountRaw?: string;
  approvalSpender?: string;
  approvalMemoHex?: string;
  approvalCreatedAtNs?: string;
  approvalExpiresAtNs?: string;
}

export type StabilityPoolConversionStatus =
  | { Pending: { intent_seq: bigint; phase: unknown; reason: [] | [string] } }
  | { Completed: { intent_seq: bigint; lp_minted: bigint; lp_credited: boolean; uncredited_lp: [] | [bigint]; retained_dust_raw: bigint } }
  | { Refunded: { intent_seq: bigint; refunded_raw: bigint; retained_dust_raw: bigint } }
  | { Rejected: { intent_seq: bigint; reason: string } };

export class PendingStabilityPoolConversionError extends Error {
  constructor(message = 'A prior 3USD conversion is unresolved. Resume that exact request before starting another conversion.') {
    super(message);
    this.name = 'PendingStabilityPoolConversionError';
  }
}

const pendingKey = (owner: string) => `rumi:stability-pool:pending-conversion:${owner}`;

function storageOrThrow(): Storage {
  if (typeof localStorage === 'undefined') {
    throw new PendingStabilityPoolConversionError('Persistent browser storage is unavailable. The conversion was not submitted.');
  }
  return localStorage;
}

export function readPendingStabilityPoolConversion(owner: string): PendingStabilityPoolConversion | null {
  const raw = storageOrThrow().getItem(pendingKey(owner));
  if (raw === null) return null;
  try {
    const r = JSON.parse(raw) as Partial<PendingStabilityPoolConversion>;
    const validLedger = typeof r.ledger === 'string' && Principal.fromText(r.ledger).toText() === r.ledger;
    if (r.owner === owner && validLedger && typeof r.principalRaw === 'string' && /^\d+$/.test(r.principalRaw) &&
        typeof r.maxFeeBudgetRaw === 'string' && /^\d+$/.test(r.maxFeeBudgetRaw) &&
        typeof r.minLp === 'string' && /^\d+$/.test(r.minLp) &&
        typeof r.intentSeq === 'string' && /^\d+$/.test(r.intentSeq) &&
        typeof r.walletIcon === 'string' && typeof r.oisy === 'boolean' &&
        typeof r.createdAt === 'number' &&
        (r.stage === 'new' || r.stage === 'approval-submitted' || r.stage === 'start-submitted')) {
      return r as PendingStabilityPoolConversion;
    }
  } catch {
    // Preserve an unreadable marker as a lock. A damaged record must not allow
    // the UI to allocate a new caller sequence.
  }
  throw new PendingStabilityPoolConversionError('A 3USD conversion recovery record is unreadable. Keep the request locked and contact support.');
}

function writePending(record: PendingStabilityPoolConversion, replace = false): void {
  const storage = storageOrThrow();
  if (!replace && readPendingStabilityPoolConversion(record.owner)) {
    throw new PendingStabilityPoolConversionError();
  }
  const serialized = JSON.stringify(record);
  storage.setItem(pendingKey(record.owner), serialized);
  if (storage.getItem(pendingKey(record.owner)) !== serialized) {
    throw new PendingStabilityPoolConversionError('Could not persist the conversion recovery lock. No conversion was submitted.');
  }
}

function clearPending(owner: string, intentSeq: string): void {
  const storage = storageOrThrow();
  const current = readPendingStabilityPoolConversion(owner);
  if (!current || current.intentSeq !== intentSeq) return;
  storage.removeItem(pendingKey(owner));
  if (storage.getItem(pendingKey(owner)) !== null) throw new PendingStabilityPoolConversionError('Could not clear the completed conversion recovery lock.');
}

type ApprovalTuple = {
  amountRaw: bigint;
  feeRaw: bigint;
  spender: Principal;
  memo: Uint8Array;
  createdAtNs: bigint;
  expiresAtNs: bigint;
};

function bytesToHex(bytes: Uint8Array): string {
  return Array.from(bytes, byte => byte.toString(16).padStart(2, '0')).join('');
}

function hexToBytes(hex: string): Uint8Array {
  if (!/^[0-9a-f]{64}$/i.test(hex)) throw new PendingStabilityPoolConversionError('The retained wallet approval memo is invalid. Keep the request locked.');
  const bytes = new Uint8Array(32);
  for (let i = 0; i < bytes.length; i += 1) bytes[i] = Number.parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  return bytes;
}

function approvalTuple(record: PendingStabilityPoolConversion): ApprovalTuple | null {
  if (!record.approvalFeeRaw || !record.approvalAmountRaw || !record.approvalSpender ||
      !record.approvalMemoHex || !record.approvalCreatedAtNs || !record.approvalExpiresAtNs) return null;
  try {
    return {
      amountRaw: BigInt(record.approvalAmountRaw),
      feeRaw: BigInt(record.approvalFeeRaw),
      spender: Principal.fromText(record.approvalSpender),
      memo: hexToBytes(record.approvalMemoHex),
      createdAtNs: BigInt(record.approvalCreatedAtNs),
      expiresAtNs: BigInt(record.approvalExpiresAtNs),
    };
  } catch {
    throw new PendingStabilityPoolConversionError('The retained wallet approval tuple is invalid. Keep the request locked.');
  }
}

function newApprovalTuple(amountRaw: bigint, feeRaw: bigint): ApprovalTuple {
  if (typeof crypto === 'undefined' || typeof crypto.getRandomValues !== 'function') {
    throw new PendingStabilityPoolConversionError('Secure approval memo generation is unavailable. No conversion was submitted.');
  }
  const memo = crypto.getRandomValues(new Uint8Array(32));
  const createdAtNs = BigInt(Date.now()) * 1_000_000n;
  return {
    amountRaw,
    feeRaw,
    spender: Principal.fromText(CANISTER_IDS.STABILITY_POOL),
    memo,
    createdAtNs,
    expiresAtNs: createdAtNs + 10n * 60n * 1_000_000_000n,
  };
}

function approvalFields(tuple: ApprovalTuple): Pick<PendingStabilityPoolConversion, 'approvalFeeRaw' | 'approvalAmountRaw' | 'approvalSpender' | 'approvalMemoHex' | 'approvalCreatedAtNs' | 'approvalExpiresAtNs'> {
  return {
    approvalFeeRaw: tuple.feeRaw.toString(),
    approvalAmountRaw: tuple.amountRaw.toString(),
    approvalSpender: tuple.spender.toText(),
    approvalMemoHex: bytesToHex(tuple.memo),
    approvalCreatedAtNs: tuple.createdAtNs.toString(),
    approvalExpiresAtNs: tuple.expiresAtNs.toString(),
  };
}

function approvalArgs(tuple: ApprovalTuple): Record<string, unknown> {
  return {
    amount: tuple.amountRaw,
    spender: { owner: tuple.spender, subaccount: [] },
    expires_at: [tuple.expiresAtNs],
    expected_allowance: [],
    memo: [tuple.memo],
    fee: [tuple.feeRaw],
    from_subaccount: [],
    created_at_time: [tuple.createdAtNs],
  };
}

function exactContext(context: StabilityPoolActionContext, ledger: Principal): void {
  const wallet = get(walletStore);
  assertStabilityPoolActionContext(
    context,
    wallet.isConnected ? wallet.principal?.toText() ?? null : null,
    wallet.icon,
    get(walletSessionGeneration),
    isOisyWallet(),
    ledger,
    'convert',
  );
}

function conversionLock<T>(context: StabilityPoolActionContext, run: () => Promise<T>): Promise<T> {
  storageOrThrow();
  type AsyncLockManager = {
    request<U>(name: string, options: LockOptions, callback: (lock: Lock | null) => Promise<U>): Promise<U>;
  };
  const locks: AsyncLockManager | undefined = typeof navigator === 'undefined'
    ? undefined
    : navigator.locks as unknown as AsyncLockManager;
  if (!locks) {
    throw new PendingStabilityPoolConversionError('This browser cannot safely coordinate conversions across tabs. No conversion was submitted.');
  }
  return locks.request<T>(
    `rumi:stability-pool:conversion:${context.principalText}`,
    { mode: 'exclusive', ifAvailable: true },
    async lock => {
      if (!lock) throw new PendingStabilityPoolConversionError('A 3USD conversion is active in another tab. Resume its exact request first.');
      return run();
    },
  );
}

/** Parse decimal input without converting raw units through a JS Number. */
export function parseExactTokenAmount(input: string, decimals: number): bigint {
  const text = input.trim();
  if (!/^\d+(?:\.\d+)?$/.test(text)) throw new Error('Enter a valid amount');
  const [whole, fraction = ''] = text.split('.');
  if (fraction.length > decimals) throw new Error(`Use at most ${decimals} decimal places`);
  return BigInt(whole) * 10n ** BigInt(decimals) + BigInt((fraction + '0'.repeat(decimals)).slice(0, decimals) || '0');
}

export function formatExactTokenAmount(raw: bigint, decimals: number, maxFractionDigits = decimals): string {
  const scale = 10n ** BigInt(decimals);
  const whole = raw / scale;
  const fraction = (raw % scale).toString().padStart(decimals, '0').slice(0, maxFractionDigits).replace(/0+$/, '');
  return fraction ? `${whole}.${fraction}` : whole.toString();
}

export function conversionMinimumLp(quotedLp: bigint, slippageBps = CONVERSION_SLIPPAGE_BPS): bigint {
  if (slippageBps < 0n || slippageBps >= BPS) throw new Error('Invalid conversion slippage');
  return quotedLp * (BPS - slippageBps) / BPS;
}

export function conversionWalletRequirement(principalRaw: bigint, feeRaw: bigint, feeBudgetRaw: bigint): bigint {
  // Allowance covers principal + escrow budget + the pull fee. The approval
  // transaction consumes one additional ledger fee from the wallet.
  return principalRaw + feeBudgetRaw + feeRaw + feeRaw;
}

function resultOrThrow<T>(result: { Ok: T } | { Err: unknown } | T): T {
  if (result && typeof result === 'object' && 'Err' in result) {
    throw new Error(`3USD conversion request rejected: ${JSON.stringify(result.Err)}`);
  }
  return (result && typeof result === 'object' && 'Ok' in result ? result.Ok : result) as T;
}

function terminal(status: StabilityPoolConversionStatus): boolean {
  return 'Completed' in status || 'Refunded' in status || 'Rejected' in status;
}

function requestMatches(record: PendingStabilityPoolConversion, owner: string, ledger: Principal, principalRaw: bigint, budget: bigint, minLp: bigint): boolean {
  return record.owner === owner && record.ledger === ledger.toText() && record.principalRaw === principalRaw.toString() &&
    record.maxFeeBudgetRaw === budget.toString() && record.minLp === minLp.toString();
}

function recordsMatch(left: PendingStabilityPoolConversion, right: PendingStabilityPoolConversion): boolean {
  return left.owner === right.owner && left.ledger === right.ledger &&
    left.principalRaw === right.principalRaw && left.maxFeeBudgetRaw === right.maxFeeBudgetRaw &&
    left.minLp === right.minLp && left.intentSeq === right.intentSeq;
}

export async function quoteStabilityPoolConversion(
  tokenLedger: Principal,
  principalRaw: bigint,
  minLp = 0n,
  context = captureStabilityPoolActionContext(tokenLedger, 'convert'),
): Promise<StabilityPoolConversionQuote> {
  exactContext(context, tokenLedger);
  const actor = await getStabilityPoolMutationActor() as any;
  exactContext(context, tokenLedger);
  if (typeof actor.quote_3usd_conversion !== 'function') throw new Error('3USD conversion quote is not available on this Stability Pool.');
  exactContext(context, tokenLedger);
  const result = await actor.quote_3usd_conversion(tokenLedger, principalRaw, minLp);
  exactContext(context, tokenLedger);
  const quote = resultOrThrow<any>(result);
  return {
    ...quote,
    principal_raw: BigInt(quote.principal_raw),
    quoted_lp: BigInt(quote.quoted_lp),
    live_transfer_fee_raw: BigInt(quote.live_transfer_fee_raw),
    minimum_fee_budget_raw: BigInt(quote.minimum_fee_budget_raw),
    quotedAtMs: Date.now(),
  };
}

async function readActive(actor: any, context: StabilityPoolActionContext, ledger: Principal): Promise<any | null> {
  if (typeof actor.get_active_3usd_conversion_v1 !== 'function') return null;
  exactContext(context, ledger);
  const active = await actor.get_active_3usd_conversion_v1();
  exactContext(context, ledger);
  return Array.isArray(active) ? active[0] ?? null : active ?? null;
}

/** Query the authenticated caller's active request so another device can
 * rehydrate the exact request before the user presses a financial button. */
export async function discoverActiveStabilityPoolConversion(
  context: StabilityPoolActionContext,
): Promise<PendingStabilityPoolConversion | null> {
  const ledger = Principal.fromText(context.ledgerText);
  exactContext(context, ledger);
  const actor = await getStabilityPoolMutationActor() as any;
  exactContext(context, ledger);
  const view = await readActive(actor, context, ledger);
  if (!view) return null;
  const marker = recordFromView(view, context);
  const existing = readPendingStabilityPoolConversion(context.principalText);
  if (existing && !recordsMatch(existing, marker)) {
    throw new PendingStabilityPoolConversionError(
      `The canister reports conversion intent ${marker.intentSeq}, but the local recovery record is for intent ${existing.intentSeq}. Keep both requests locked until the exact request is reconciled.`,
    );
  }
  if (!existing) writePending(marker);
  return existing ?? marker;
}

function recordFromView(view: any, context: StabilityPoolActionContext): PendingStabilityPoolConversion {
  const request = view.request;
  return {
    owner: context.principalText,
    ledger: request.token_ledger.toText(),
    principalRaw: BigInt(request.principal_raw).toString(),
    maxFeeBudgetRaw: BigInt(request.max_fee_budget_raw).toString(),
    minLp: BigInt(request.min_lp).toString(),
    intentSeq: BigInt(request.intent_seq).toString(),
    walletIcon: context.walletIcon,
    oisy: context.oisy,
    createdAt: Date.now(),
    stage: 'start-submitted',
  };
}

async function statusFor(actor: any, seq: bigint, context: StabilityPoolActionContext, ledger: Principal): Promise<StabilityPoolConversionStatus> {
  exactContext(context, ledger);
  const result = await actor.get_3usd_conversion_status(seq);
  exactContext(context, ledger);
  return resultOrThrow<StabilityPoolConversionStatus>(result);
}

export async function convertStabilityPoolStableTo3Usd(
  quote: StabilityPoolConversionQuote,
  minLp: bigint,
  context = captureStabilityPoolActionContext(quote.token_ledger, 'convert'),
): Promise<StabilityPoolConversionStatus> {
  const ledger = quote.token_ledger;
  exactContext(context, ledger);
  if (Date.now() - quote.quotedAtMs > CONVERSION_QUOTE_MAX_AGE_MS) throw new Error('The conversion quote expired. Refresh the quote before authorizing the wallet.');
  if (quote.minimum_fee_budget_raw <= 0n) throw new Error('The Stability Pool did not provide a bounded fee budget.');
  return conversionLock(context, async () => {
    exactContext(context, ledger);
    const actor = await getStabilityPoolMutationActor() as any;
    exactContext(context, ledger);
    let marker = readPendingStabilityPoolConversion(context.principalText);
    const active = await readActive(actor, context, ledger);
    if (active) {
      const activeMarker = recordFromView(active, context);
      if (marker && !recordsMatch(marker, activeMarker)) {
        throw new PendingStabilityPoolConversionError(
          `The canister reports conversion intent ${activeMarker.intentSeq}, but the local recovery record is for intent ${marker.intentSeq}. Resume the exact retained request first.`,
        );
      }
      if (!marker) {
        marker = activeMarker;
        writePending(marker);
      }
    }
    if (marker && !requestMatches(marker, context.principalText, ledger, quote.principal_raw, quote.minimum_fee_budget_raw, minLp)) {
      throw new PendingStabilityPoolConversionError(`Conversion intent ${marker.intentSeq} is locked for a different request. Resume that exact conversion first.`);
    }
    if (marker && marker.stage !== 'new') {
      const status = await resumeConversionWithContext(BigInt(marker.intentSeq), marker, actor, context);
      if (terminal(status)) clearPending(marker.owner, marker.intentSeq);
      return status;
    }

    if (!marker) {
      exactContext(context, ledger);
      const next = await actor.get_3usd_conversion_next_seq();
      exactContext(context, ledger);
      const seq = BigInt(resultOrThrow<any>(next));
      const fee = quote.live_transfer_fee_raw;
      const allowance = quote.principal_raw + quote.minimum_fee_budget_raw + fee;
      const tuple = newApprovalTuple(allowance, fee);
      marker = {
        owner: context.principalText,
        ledger: ledger.toText(),
        principalRaw: quote.principal_raw.toString(),
        maxFeeBudgetRaw: quote.minimum_fee_budget_raw.toString(),
        minLp: minLp.toString(),
        intentSeq: seq.toString(),
        walletIcon: context.walletIcon,
        oisy: context.oisy,
        createdAt: Date.now(),
        stage: 'new',
        ...approvalFields(tuple),
      };
      // This write is before the first potentially mutating wallet call.
      writePending(marker);
    }

    const fee = quote.live_transfer_fee_raw;
    const allowance = quote.principal_raw + quote.minimum_fee_budget_raw + fee;
    const tuple = approvalTuple(marker);
    if (marker.stage === 'new' && !tuple) {
      throw new PendingStabilityPoolConversionError('This conversion has no retained wallet approval tuple. Keep it locked and recover the exact request before approving again.');
    }
    if (marker.stage === 'new' && tuple &&
        (tuple.amountRaw !== allowance || tuple.feeRaw !== fee || tuple.spender.toText() !== Principal.fromText(CANISTER_IDS.STABILITY_POOL).toText())) {
      throw new PendingStabilityPoolConversionError('The retained wallet approval tuple differs from the current quote. Keep the exact request locked and recover it before approving again.');
    }
    const ledgerActor = await getStabilityPoolLedgerMutationActor(ledger);
    exactContext(context, ledger);
    if (marker.stage === 'new') {
      exactContext(context, ledger);
      const approval = await ledgerActor.icrc2_approve(approvalArgs(tuple!));
      exactContext(context, ledger);
      resultOrThrow(approval);
      marker = { ...marker, stage: 'approval-submitted' };
      writePending(marker, true);
    }

    marker = { ...marker, stage: 'start-submitted' };
    writePending(marker, true);
    const request = {
      intent_seq: BigInt(marker.intentSeq),
      token_ledger: ledger,
      principal_raw: BigInt(marker.principalRaw),
      max_fee_budget_raw: BigInt(marker.maxFeeBudgetRaw),
      min_lp: BigInt(marker.minLp),
    };
    try {
      exactContext(context, ledger);
      const result = await actor.start_3usd_conversion(request);
      exactContext(context, ledger);
      const status = resultOrThrow<StabilityPoolConversionStatus>(result);
      if (terminal(status)) clearPending(marker.owner, marker.intentSeq);
      return status;
    } catch {
      try {
        const status = await statusFor(actor, BigInt(marker.intentSeq), context, ledger);
        if (terminal(status)) {
          clearPending(marker.owner, marker.intentSeq);
          return status;
        }
      } catch {
        // Preserve the exact marker through a lost reply or unavailable status.
      }
      throw new PendingStabilityPoolConversionError(`Conversion intent ${marker.intentSeq} is unresolved. Resume this exact request; do not start a new one.`);
    }
  });
}

async function resumeConversionWithContext(seq: bigint, marker: PendingStabilityPoolConversion, actor: any, context: StabilityPoolActionContext): Promise<StabilityPoolConversionStatus> {
  const ledger = Principal.fromText(marker.ledger);
  try {
    exactContext(context, ledger);
    const result = await actor.resume_3usd_conversion(seq);
    exactContext(context, ledger);
    return resultOrThrow<StabilityPoolConversionStatus>(result);
  } catch {
    const status = await statusFor(actor, seq, context, ledger);
    if (terminal(status)) return status;
    throw new PendingStabilityPoolConversionError(`Conversion intent ${seq} is unresolved. Resume this exact request; do not start a new one.`);
  }
}

export async function resumePendingStabilityPoolConversion(
  context: StabilityPoolActionContext,
): Promise<StabilityPoolConversionStatus> {
  const marker = readPendingStabilityPoolConversion(context.principalText);
  if (!marker) throw new Error('No pending 3USD conversion is available.');
  const ledger = Principal.fromText(marker.ledger);
  exactContext(context, ledger);
  return conversionLock(context, async () => {
    const actor = await getStabilityPoolMutationActor() as any;
    exactContext(context, ledger);
    const status = await resumeConversionWithContext(BigInt(marker.intentSeq), marker, actor, context);
    if (terminal(status)) clearPending(marker.owner, marker.intentSeq);
    return status;
  });
}

export function isConversionTerminal(status: StabilityPoolConversionStatus): boolean {
  return terminal(status);
}
