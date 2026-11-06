/**
 * Browser-side journal for the Solana bridge branch.
 *
 * It follows the Base journal's rules (`../baseBridge/intents.ts`): an attempt is written BEFORE the effect it
 * describes, the record is scoped to one signed-in principal and versioned, an unreadable record is copied aside
 * before anything is rewritten, and a record written by a newer app is left alone (the store goes read-only).
 * It lives under its own key, `rumi:solana-bridge:v1:<principal>`, so the Base journal is never touched.
 *
 * What is persisted is the intent's whole identity, so a reload, an account change or a network change cannot
 * change what an old attempt was: the network and genesis hash, the bridge instance, the mint and the token
 * program, the source wallet and token account, the ICP destination, the amount, the intent id and nonce, the
 * exact memo, and the PREPARED ATTEMPT (the unsigned message bytes with their recent blockhash). When a wallet
 * returns signed bytes they are saved, and the first signature derived from them, before anything is broadcast.
 *
 * The journal stores intents and hints only. A wallet callback, a signature or a transaction hash is never
 * settlement proof: the bridge canister and the chain are authoritative.
 */

import type { KeyValueStorage } from '../baseBridge/evmWallet';
import { fromHex } from './bytes';
import { isNetworkKey } from '../baseBridge/intents';

export const SOLANA_INTENT_VERSION = 1;
export const SOLANA_INTENT_PREFIX = 'rumi:solana-bridge:v1:';
export const MAX_SOLANA_INTENTS = 120;
export const MAX_RESOLVED_SOLANA_INTENTS = 40;

/** Where a Solana-to-ICP attempt stands. Only `done` is final; none of the others is settlement. */
export type SolReturnStatus =
  /** The intent and the exact unsigned message are saved. The wallet has not been asked. */
  | 'prepared'
  /** The wallet was handed the transaction. What it did with it is not recorded (a reload lands here). */
  | 'signing'
  /** Signed bytes and the first signature are saved. Nothing has been broadcast yet. */
  | 'signed'
  /** The signed transaction was broadcast (or the wallet reported sending it). A hint, not proof. */
  | 'sent'
  /** A sign-and-send wallet gave an ambiguous answer. The original intent is held until evidence settles it. */
  | 'held'
  /** The bridge canister finished the return (released to ICP). */
  | 'done';

export interface SolReturnIntent {
  v: typeof SOLANA_INTENT_VERSION;
  kind: 'solana_to_icp';
  /** 32 hex characters: 16 random bytes. The intent id. */
  id: string;
  /** The per-network destination key (`solana-mainnet`). */
  destination: string;
  genesisHash: string;
  instance: string;
  mint: string;
  tokenProgram: 'classic' | 'token2022';
  returnAccount: string;
  /** The holder wallet (fee payer and signer), base58. */
  wallet: string;
  /** The holder's token account the tokens leave from, base58. */
  sourceAccount: string;
  /** The ICP destination, exactly as written into the memo. */
  icpPrincipal: string;
  icpSubaccount: string | null;
  amountE8s: bigint;
  /** The u64 client nonce, as a decimal string. */
  nonce: string;
  /** The exact memo text the transaction carries. */
  memo: string;
  /** The ordinary recent blockhash the message uses, base58. */
  blockhash: string;
  /** The last block height at which that blockhash is valid, when the RPC reported it. */
  lastValidBlockHeight: string | null;
  /** The unsigned message bytes, hex. The attempt that is signed is exactly this. */
  messageHex: string;
  /** How the wallet will be asked: sign only (we broadcast) or sign and send (the wallet broadcasts). */
  signMode: 'sign' | 'sign_and_send';
  /** The signed transaction, hex, once a sign-only wallet returned it. */
  signedHex?: string;
  /** The first signature, base58, derived from the signed bytes (or reported by a sign-and-send wallet). */
  signature?: string;
  createdAt: number;
  status: SolReturnStatus;
  /** A short plain-language note (why it is held). */
  note?: string;
}

export type SolDepositStatus = 'prepared' | 'approved' | 'submitted' | 'done' | 'failed';

export interface SolDepositIntent {
  v: typeof SOLANA_INTENT_VERSION;
  kind: 'icp_to_solana';
  /** The client request id, 32 hex characters: 16 random bytes. */
  id: string;
  destination: string;
  genesisHash: string;
  instance: string;
  mint: string;
  tokenProgram: 'classic' | 'token2022';
  /** The Solana wallet that receives the icUSD, base58. */
  recipient: string;
  amountE8s: bigint;
  expectedDepositFee: bigint;
  createdAt: number;
  status: SolDepositStatus;
  depositId?: bigint;
  failure?: string;
}

export type SolIntent = SolReturnIntent | SolDepositIntent;

export function isSolResolved(intent: SolIntent): boolean {
  return intent.status === 'done' || (intent.kind === 'icp_to_solana' && intent.status === 'failed');
}

export function solIntentStorageKey(principalText: string): string {
  return `${SOLANA_INTENT_PREFIX}${principalText}`;
}

// ---------------------------------------------------------------------------
// Persisted shape (bigints are decimal strings)
// ---------------------------------------------------------------------------

type Json = Record<string, unknown>;
const DECIMAL = /^\d{1,30}$/;
const HEX32 = /^[0-9a-f]{32}$/;
const str = (v: unknown): v is string => typeof v === 'string';
const optStr = (v: unknown): v is string | undefined => v === undefined || typeof v === 'string';

function serialize(intent: SolIntent): Json {
  const base: Json = { ...intent, amountE8s: intent.amountE8s.toString() };
  if (intent.kind === 'icp_to_solana') {
    base.expectedDepositFee = intent.expectedDepositFee.toString();
    if (intent.depositId !== undefined) base.depositId = intent.depositId.toString();
  }
  return base;
}

const RETURN_STATUSES: readonly SolReturnStatus[] = ['prepared', 'signing', 'signed', 'sent', 'held', 'done'];
const DEPOSIT_STATUSES: readonly SolDepositStatus[] = ['prepared', 'approved', 'submitted', 'done', 'failed'];

function common(r: Json): boolean {
  return (
    r.v === SOLANA_INTENT_VERSION &&
    str(r.id) && HEX32.test(r.id) &&
    str(r.destination) && isNetworkKey(r.destination) &&
    str(r.genesisHash) && str(r.instance) && str(r.mint) &&
    (r.tokenProgram === 'classic' || r.tokenProgram === 'token2022') &&
    str(r.amountE8s) && DECIMAL.test(r.amountE8s) &&
    typeof r.createdAt === 'number' && Number.isFinite(r.createdAt)
  );
}

function parseIntent(value: unknown): SolIntent | null {
  if (!value || typeof value !== 'object') return null;
  const r = value as Json;
  if (!common(r)) return null;
  if (r.kind === 'solana_to_icp') {
    if (!RETURN_STATUSES.includes(r.status as SolReturnStatus)) return null;
    if (!str(r.returnAccount) || !str(r.wallet) || !str(r.sourceAccount) || !str(r.icpPrincipal) || !str(r.memo) || !str(r.blockhash)) return null;
    if (!(r.icpSubaccount === null || str(r.icpSubaccount)) || !str(r.nonce) || !DECIMAL.test(r.nonce)) return null;
    if (!(r.lastValidBlockHeight === null || (str(r.lastValidBlockHeight) && DECIMAL.test(r.lastValidBlockHeight)))) return null;
    if (!str(r.messageHex) || fromHex(r.messageHex) === null) return null;
    if (r.signMode !== 'sign' && r.signMode !== 'sign_and_send') return null;
    if (!optStr(r.signedHex) || (r.signedHex !== undefined && fromHex(r.signedHex) === null)) return null;
    if (!optStr(r.signature) || !optStr(r.note)) return null;
    return {
      v: SOLANA_INTENT_VERSION,
      kind: 'solana_to_icp',
      id: r.id as string,
      destination: r.destination as string,
      genesisHash: r.genesisHash as string,
      instance: r.instance as string,
      mint: r.mint as string,
      tokenProgram: r.tokenProgram as 'classic' | 'token2022',
      returnAccount: r.returnAccount,
      wallet: r.wallet,
      sourceAccount: r.sourceAccount,
      icpPrincipal: r.icpPrincipal,
      icpSubaccount: r.icpSubaccount as string | null,
      amountE8s: BigInt(r.amountE8s as string),
      nonce: r.nonce,
      memo: r.memo,
      blockhash: r.blockhash,
      lastValidBlockHeight: r.lastValidBlockHeight as string | null,
      messageHex: r.messageHex,
      signMode: r.signMode,
      ...(r.signedHex !== undefined ? { signedHex: r.signedHex } : {}),
      ...(r.signature !== undefined ? { signature: r.signature } : {}),
      createdAt: r.createdAt as number,
      status: r.status as SolReturnStatus,
      ...(r.note !== undefined ? { note: r.note } : {}),
    };
  }
  if (r.kind === 'icp_to_solana') {
    if (!DEPOSIT_STATUSES.includes(r.status as SolDepositStatus)) return null;
    if (!str(r.recipient) || !str(r.expectedDepositFee) || !DECIMAL.test(r.expectedDepositFee)) return null;
    if (r.depositId !== undefined && (!str(r.depositId) || !DECIMAL.test(r.depositId))) return null;
    return {
      v: SOLANA_INTENT_VERSION,
      kind: 'icp_to_solana',
      id: r.id as string,
      destination: r.destination as string,
      genesisHash: r.genesisHash as string,
      instance: r.instance as string,
      mint: r.mint as string,
      tokenProgram: r.tokenProgram as 'classic' | 'token2022',
      recipient: r.recipient,
      amountE8s: BigInt(r.amountE8s as string),
      expectedDepositFee: BigInt(r.expectedDepositFee),
      createdAt: r.createdAt as number,
      status: r.status as SolDepositStatus,
      ...(r.depositId !== undefined ? { depositId: BigInt(r.depositId as string) } : {}),
      ...(str(r.failure) ? { failure: r.failure } : {}),
    };
  }
  return null;
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

export type SolIntentStoreErrorCode = 'persist_failed' | 'read_only' | 'too_many';

export class SolIntentStoreError extends Error {
  readonly code: SolIntentStoreErrorCode;
  constructor(code: SolIntentStoreErrorCode, message: string) {
    super(message);
    this.name = 'SolIntentStoreError';
    this.code = code;
  }
}

export type SolLoadStatus = 'ok' | 'recovered' | 'newer_version' | 'unavailable';

interface RawRecord {
  status: SolLoadStatus;
  intents: SolIntent[];
  corruptRaw?: string;
}

export class SolanaIntentStore {
  readonly key: string;
  private readonly storage: KeyValueStorage | null;
  private readonly principal: string;
  private readonly now: () => number;
  private intents: SolIntent[] = [];
  private status: SolLoadStatus = 'ok';
  private quarantine: string | null = null;

  constructor(storage: KeyValueStorage | null, principalText: string, opts: { now?: () => number } = {}) {
    this.storage = storage;
    this.principal = principalText;
    this.key = solIntentStorageKey(principalText);
    this.now = opts.now ?? Date.now;
    this.reload();
  }

  get loadStatus(): SolLoadStatus {
    return this.status;
  }

  get quarantineKey(): string | null {
    return this.quarantine;
  }

  get readOnly(): boolean {
    return this.status === 'newer_version' || this.status === 'unavailable';
  }

  /** Newest first. */
  list(): SolIntent[] {
    return [...this.intents].sort((a, b) => b.createdAt - a.createdAt);
  }

  /** Newest first, one network's intents only. */
  listFor(destination: string): SolIntent[] {
    return this.list().filter((intent) => intent.destination === destination);
  }

  get(id: string): SolIntent | undefined {
    const wanted = id.toLowerCase();
    return this.intents.find((intent) => intent.id.toLowerCase() === wanted);
  }

  reload(): SolLoadStatus {
    const record = this.readRecord();
    this.status = record.status;
    this.intents = record.intents;
    if (record.corruptRaw !== undefined) this.preserveCorrupt(record.corruptRaw);
    return this.status;
  }

  put(intent: SolIntent): void {
    this.mutate((list) => {
      const at = list.findIndex((existing) => existing.id === intent.id);
      if (at >= 0) list[at] = intent;
      else list.push(intent);
      return list;
    });
  }

  patch<T extends SolIntent>(id: string, patch: Partial<T>): T {
    let updated: SolIntent | undefined;
    this.mutate((list) => {
      const at = list.findIndex((existing) => existing.id === id.toLowerCase());
      if (at < 0) throw new SolIntentStoreError('persist_failed', 'The bridge recovery record for this request no longer exists.');
      updated = { ...list[at], ...patch } as SolIntent;
      list[at] = updated;
      return list;
    });
    return updated as T;
  }

  remove(id: string): void {
    const wanted = id.toLowerCase();
    this.mutate((list) => list.filter((existing) => existing.id !== wanted));
  }

  // -- internals -----------------------------------------------------------

  private readRecord(): RawRecord {
    if (!this.storage) return { status: 'unavailable', intents: [] };
    let raw: string | null;
    try {
      raw = this.storage.getItem(this.key);
    } catch {
      return { status: 'unavailable', intents: [] };
    }
    if (raw === null || raw === '') return { status: 'ok', intents: [] };
    let parsed: unknown;
    try {
      parsed = JSON.parse(raw);
    } catch {
      return { status: 'recovered', intents: [], corruptRaw: raw };
    }
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return { status: 'recovered', intents: [], corruptRaw: raw };
    const record = parsed as Json;
    if (typeof record.v === 'number' && record.v > SOLANA_INTENT_VERSION) return { status: 'newer_version', intents: [] };
    if (record.v !== SOLANA_INTENT_VERSION || record.principal !== this.principal || !Array.isArray(record.intents)) {
      return { status: 'recovered', intents: [], corruptRaw: raw };
    }
    const intents: SolIntent[] = [];
    let dropped = false;
    for (const entry of record.intents) {
      const intent = parseIntent(entry);
      if (intent) intents.push(intent);
      else dropped = true;
    }
    return dropped ? { status: 'recovered', intents, corruptRaw: raw } : { status: 'ok', intents };
  }

  private preserveCorrupt(raw: string): void {
    const key = `${this.key}:unreadable:${this.now()}`;
    try {
      this.storage!.setItem(key, raw);
      this.quarantine = key;
      this.writeRecord(this.intents);
    } catch {
      this.status = 'unavailable';
    }
  }

  private writeRecord(list: SolIntent[]): void {
    this.storage!.setItem(this.key, JSON.stringify({ v: SOLANA_INTENT_VERSION, principal: this.principal, intents: list.map(serialize) }));
  }

  private mutate(change: (list: SolIntent[]) => SolIntent[]): void {
    if (!this.storage) throw new SolIntentStoreError('persist_failed', 'This browser cannot store the bridge recovery record, so nothing was sent.');
    const record = this.readRecord();
    if (record.status === 'newer_version') {
      this.status = 'newer_version';
      throw new SolIntentStoreError('read_only', 'The bridge recovery record was written by a newer version of this app. Reload the page.');
    }
    if (record.status === 'unavailable') {
      this.status = 'unavailable';
      throw new SolIntentStoreError('persist_failed', 'This browser cannot store the bridge recovery record, so nothing was sent.');
    }
    if (record.corruptRaw !== undefined) {
      this.intents = record.intents;
      this.preserveCorrupt(record.corruptRaw);
      if (this.status === 'unavailable') {
        throw new SolIntentStoreError('persist_failed', 'The bridge recovery record is unreadable and could not be preserved, so nothing was sent.');
      }
    }
    const next = this.prune(change([...record.intents]));
    if (next.length > MAX_SOLANA_INTENTS) {
      throw new SolIntentStoreError('too_many', 'Too many unresolved bridge requests are stored on this device. Resolve some first.');
    }
    try {
      this.writeRecord(next);
    } catch {
      throw new SolIntentStoreError('persist_failed', 'Could not save the bridge recovery record, so nothing was sent.');
    }
    this.intents = next;
    if (this.status !== 'recovered') this.status = 'ok';
  }

  private prune(list: SolIntent[]): SolIntent[] {
    const resolved = list.filter(isSolResolved).sort((a, b) => b.createdAt - a.createdAt);
    if (resolved.length <= MAX_RESOLVED_SOLANA_INTENTS) return list;
    const drop = new Set(resolved.slice(MAX_RESOLVED_SOLANA_INTENTS).map((intent) => intent.id));
    return list.filter((intent) => !drop.has(intent.id));
  }
}
