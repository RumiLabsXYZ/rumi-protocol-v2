/**
 * Browser-side journal for the Sui bridge branch.
 *
 * It follows the Base and Solana journals' rules (`../baseBridge/intents.ts`, `../solanaBridge/intents.ts`): an attempt
 * is written BEFORE the effect it describes, the record is scoped to one signed-in principal and versioned, an
 * unreadable record is copied aside before anything is rewritten, and a record written by a newer app is left alone
 * (the store goes read-only). It lives under its own key, `rumi:sui-bridge:v1:<principal>`, so no other journal is
 * touched.
 *
 * What is persisted is the intent's whole identity, so a reload, an account change or a network change cannot change
 * what an old attempt was: the network (full chain identifier), the bridge instance, the package, the Gate and the coin
 * type, the sender, the ICP destination, the amount, the client nonce, the predicted transfer id, the coins the
 * amount is taken from, and (once a wallet returned it) the exact signed bytes with their signature and typed digest.
 *
 * The journal stores intents and hints only. A wallet callback, a transaction digest or an event is never settlement
 * proof: the bridge canister's release record is authoritative.
 */

import type { KeyValueStorage } from '../baseBridge/evmWallet';
import { isNetworkKey } from '../baseBridge/intents';

export const SUI_INTENT_VERSION = 1;
export const SUI_INTENT_PREFIX = 'rumi:sui-bridge:v1:';
export const MAX_SUI_INTENTS = 120;
export const MAX_RESOLVED_SUI_INTENTS = 40;

export interface SuiCoinRef {
  objectId: string;
  version: string;
  digest: string;
  /** Balance in e8s. */
  balance: bigint;
}

/** Where a Sui-to-ICP attempt stands. Only `done` is final; none of the others is settlement. */
export type SuiReturnStatus =
  /** The intent (with its coins, nonce and predicted transfer id) is saved. The wallet has not been asked. */
  | 'prepared'
  /** The wallet was handed the transaction. What it did with it is not recorded (a reload lands here). */
  | 'signing'
  /** Signed bytes, signature and typed digest are saved. Nothing has been broadcast yet. */
  | 'signed'
  /** The signed transaction was broadcast (or the wallet reported executing it). A hint, not proof. */
  | 'sent'
  /** A sign-and-execute wallet gave an ambiguous answer. The original intent is held until evidence settles it. */
  | 'held'
  /** The bridge canister finished the return (released to ICP). */
  | 'done';

export interface SuiReturnIntent {
  v: typeof SUI_INTENT_VERSION;
  kind: 'sui_to_icp';
  /** 32 hex characters: 16 random bytes. The intent id. */
  id: string;
  /** The per-network destination key (`sui-mainnet`). */
  destination: string;
  /** The full 32-byte chain identifier, lowercase hex (64 digits). */
  chainIdentifier: string;
  instance: string;
  packageId: string;
  gateId: string;
  coinType: string;
  /** The holder wallet (sender and signer): `0x` and 64 hex digits. */
  sender: string;
  /**
   * The icUSD coin objects the amount is taken from (merged and split by the transaction), in call order, with the exact
   * version and digest the transaction references. Their balances added up exceed `amountE8s` exactly when the
   * transaction splits the amount off.
   */
  coins: SuiCoinRef[];
  /** The ICP principal text, exactly as encoded into the call. */
  icpPrincipal: string;
  /** 64 lowercase hex digits, or null for the default (all zero) subaccount. */
  icpSubaccount: string | null;
  amountE8s: bigint;
  /** The u64 client nonce, as a decimal string. */
  nonce: string;
  /** The Gate's `transfer_id` this attempt will have, 64 lowercase hex digits (never a transaction digest). */
  transferId: string;
  /** How the wallet will be asked: sign only (we broadcast) or sign and execute (the wallet broadcasts). */
  signMode: 'sign' | 'sign_and_execute';
  /** The signed transaction bytes, base64, once a wallet returned them (validated before they are saved). */
  signedBytes?: string;
  /** The wallet's signature over those bytes, base64. */
  signature?: string;
  /** The typed transaction digest (base58) derived from the signed bytes. The attempt's identity on chain. */
  digest?: string;
  createdAt: number;
  status: SuiReturnStatus;
  /** A short plain-language note (why it is held). */
  note?: string;
}

export type SuiDepositStatus = 'prepared' | 'approved' | 'submitted' | 'done' | 'failed';

export interface SuiDepositIntent {
  v: typeof SUI_INTENT_VERSION;
  kind: 'icp_to_sui';
  /** The client request id, 32 hex characters: 16 random bytes. */
  id: string;
  destination: string;
  chainIdentifier: string;
  instance: string;
  packageId: string;
  gateId: string;
  coinType: string;
  /** The Sui address that receives the icUSD: `0x` and 64 lowercase hex digits. */
  recipient: string;
  amountE8s: bigint;
  expectedDepositFee: bigint;
  createdAt: number;
  status: SuiDepositStatus;
  depositId?: bigint;
  failure?: string;
}

export type SuiIntent = SuiReturnIntent | SuiDepositIntent;

export function isSuiResolved(intent: SuiIntent): boolean {
  return intent.status === 'done' || (intent.kind === 'icp_to_sui' && intent.status === 'failed');
}

export function suiIntentStorageKey(principalText: string): string {
  return `${SUI_INTENT_PREFIX}${principalText}`;
}

// ---------------------------------------------------------------------------
// Persisted shape (bigints are decimal strings)
// ---------------------------------------------------------------------------

type Json = Record<string, unknown>;
const DECIMAL = /^\d{1,30}$/;
const HEX32 = /^[0-9a-f]{32}$/;
const HEX64 = /^[0-9a-f]{64}$/;
const ADDRESS = /^0x[0-9a-f]{64}$/;
const str = (v: unknown): v is string => typeof v === 'string';
const optStr = (v: unknown): v is string | undefined => v === undefined || typeof v === 'string';

function serialize(intent: SuiIntent): Json {
  const base: Json = { ...intent, amountE8s: intent.amountE8s.toString() };
  if (intent.kind === 'sui_to_icp') base.coins = intent.coins.map((c) => ({ ...c, balance: c.balance.toString() }));
  if (intent.kind === 'icp_to_sui') {
    base.expectedDepositFee = intent.expectedDepositFee.toString();
    if (intent.depositId !== undefined) base.depositId = intent.depositId.toString();
  }
  return base;
}

const RETURN_STATUSES: readonly SuiReturnStatus[] = ['prepared', 'signing', 'signed', 'sent', 'held', 'done'];
const DEPOSIT_STATUSES: readonly SuiDepositStatus[] = ['prepared', 'approved', 'submitted', 'done', 'failed'];

function common(r: Json): boolean {
  return (
    r.v === SUI_INTENT_VERSION &&
    str(r.id) && HEX32.test(r.id) &&
    str(r.destination) && isNetworkKey(r.destination) &&
    str(r.chainIdentifier) && HEX64.test(r.chainIdentifier) &&
    str(r.instance) && str(r.packageId) && ADDRESS.test(r.packageId) &&
    str(r.gateId) && ADDRESS.test(r.gateId) && str(r.coinType) &&
    str(r.amountE8s) && DECIMAL.test(r.amountE8s) &&
    typeof r.createdAt === 'number' && Number.isFinite(r.createdAt)
  );
}

function parseCoins(value: unknown): SuiCoinRef[] | null {
  if (!Array.isArray(value) || value.length === 0) return null;
  const out: SuiCoinRef[] = [];
  for (const entry of value) {
    if (!entry || typeof entry !== 'object') return null;
    const c = entry as Json;
    if (!str(c.objectId) || !ADDRESS.test(c.objectId) || !str(c.version) || !DECIMAL.test(c.version) || !str(c.digest) || c.digest === '' || !str(c.balance) || !DECIMAL.test(c.balance)) return null;
    out.push({ objectId: c.objectId, version: c.version, digest: c.digest, balance: BigInt(c.balance) });
  }
  return out;
}

function parseIntent(value: unknown): SuiIntent | null {
  if (!value || typeof value !== 'object') return null;
  const r = value as Json;
  if (!common(r)) return null;
  if (r.kind === 'sui_to_icp') {
    if (!RETURN_STATUSES.includes(r.status as SuiReturnStatus)) return null;
    if (!str(r.sender) || !ADDRESS.test(r.sender) || !str(r.icpPrincipal) || !str(r.transferId) || !HEX64.test(r.transferId)) return null;
    const coins = parseCoins(r.coins);
    if (coins === null) return null;
    if (!(r.icpSubaccount === null || (str(r.icpSubaccount) && HEX64.test(r.icpSubaccount))) || !str(r.nonce) || !DECIMAL.test(r.nonce)) return null;
    if (r.signMode !== 'sign' && r.signMode !== 'sign_and_execute') return null;
    if (!optStr(r.signedBytes) || !optStr(r.signature) || !optStr(r.digest) || !optStr(r.note)) return null;
    return {
      v: SUI_INTENT_VERSION,
      kind: 'sui_to_icp',
      id: r.id as string,
      destination: r.destination as string,
      chainIdentifier: r.chainIdentifier as string,
      instance: r.instance as string,
      packageId: r.packageId as string,
      gateId: r.gateId as string,
      coinType: r.coinType as string,
      sender: r.sender,
      coins,
      icpPrincipal: r.icpPrincipal,
      icpSubaccount: r.icpSubaccount as string | null,
      amountE8s: BigInt(r.amountE8s as string),
      nonce: r.nonce,
      transferId: r.transferId,
      signMode: r.signMode,
      ...(r.signedBytes !== undefined ? { signedBytes: r.signedBytes } : {}),
      ...(r.signature !== undefined ? { signature: r.signature } : {}),
      ...(r.digest !== undefined ? { digest: r.digest } : {}),
      createdAt: r.createdAt as number,
      status: r.status as SuiReturnStatus,
      ...(r.note !== undefined ? { note: r.note } : {}),
    };
  }
  if (r.kind === 'icp_to_sui') {
    if (!DEPOSIT_STATUSES.includes(r.status as SuiDepositStatus)) return null;
    if (!str(r.recipient) || !ADDRESS.test(r.recipient) || !str(r.expectedDepositFee) || !DECIMAL.test(r.expectedDepositFee)) return null;
    if (r.depositId !== undefined && (!str(r.depositId) || !DECIMAL.test(r.depositId))) return null;
    return {
      v: SUI_INTENT_VERSION,
      kind: 'icp_to_sui',
      id: r.id as string,
      destination: r.destination as string,
      chainIdentifier: r.chainIdentifier as string,
      instance: r.instance as string,
      packageId: r.packageId as string,
      gateId: r.gateId as string,
      coinType: r.coinType as string,
      recipient: r.recipient,
      amountE8s: BigInt(r.amountE8s as string),
      expectedDepositFee: BigInt(r.expectedDepositFee),
      createdAt: r.createdAt as number,
      status: r.status as SuiDepositStatus,
      ...(r.depositId !== undefined ? { depositId: BigInt(r.depositId as string) } : {}),
      ...(str(r.failure) ? { failure: r.failure } : {}),
    };
  }
  return null;
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

export type SuiIntentStoreErrorCode = 'persist_failed' | 'read_only' | 'too_many';

export class SuiIntentStoreError extends Error {
  readonly code: SuiIntentStoreErrorCode;
  constructor(code: SuiIntentStoreErrorCode, message: string) {
    super(message);
    this.name = 'SuiIntentStoreError';
    this.code = code;
  }
}

export type SuiLoadStatus = 'ok' | 'recovered' | 'newer_version' | 'unavailable';

interface RawRecord {
  status: SuiLoadStatus;
  intents: SuiIntent[];
  corruptRaw?: string;
}

export class SuiIntentStore {
  readonly key: string;
  private readonly storage: KeyValueStorage | null;
  private readonly principal: string;
  private readonly now: () => number;
  private intents: SuiIntent[] = [];
  private status: SuiLoadStatus = 'ok';
  private quarantine: string | null = null;

  constructor(storage: KeyValueStorage | null, principalText: string, opts: { now?: () => number } = {}) {
    this.storage = storage;
    this.principal = principalText;
    this.key = suiIntentStorageKey(principalText);
    this.now = opts.now ?? Date.now;
    this.reload();
  }

  get loadStatus(): SuiLoadStatus {
    return this.status;
  }

  get quarantineKey(): string | null {
    return this.quarantine;
  }

  get readOnly(): boolean {
    return this.status === 'newer_version' || this.status === 'unavailable';
  }

  /** Newest first. */
  list(): SuiIntent[] {
    return [...this.intents].sort((a, b) => b.createdAt - a.createdAt);
  }

  /** Newest first, one network's intents only. */
  listFor(destination: string): SuiIntent[] {
    return this.list().filter((intent) => intent.destination === destination);
  }

  get(id: string): SuiIntent | undefined {
    const wanted = id.toLowerCase();
    return this.intents.find((intent) => intent.id.toLowerCase() === wanted);
  }

  reload(): SuiLoadStatus {
    const record = this.readRecord();
    this.status = record.status;
    this.intents = record.intents;
    if (record.corruptRaw !== undefined) this.preserveCorrupt(record.corruptRaw);
    return this.status;
  }

  put(intent: SuiIntent): void {
    this.mutate((list) => {
      const at = list.findIndex((existing) => existing.id === intent.id);
      if (at >= 0) list[at] = intent;
      else list.push(intent);
      return list;
    });
  }

  patch<T extends SuiIntent>(id: string, patch: Partial<T>): T {
    let updated: SuiIntent | undefined;
    this.mutate((list) => {
      const at = list.findIndex((existing) => existing.id === id.toLowerCase());
      if (at < 0) throw new SuiIntentStoreError('persist_failed', 'The bridge recovery record for this request no longer exists.');
      updated = { ...list[at], ...patch } as SuiIntent;
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
    if (typeof record.v === 'number' && record.v > SUI_INTENT_VERSION) return { status: 'newer_version', intents: [] };
    if (record.v !== SUI_INTENT_VERSION || record.principal !== this.principal || !Array.isArray(record.intents)) {
      return { status: 'recovered', intents: [], corruptRaw: raw };
    }
    const intents: SuiIntent[] = [];
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

  private writeRecord(list: SuiIntent[]): void {
    this.storage!.setItem(this.key, JSON.stringify({ v: SUI_INTENT_VERSION, principal: this.principal, intents: list.map(serialize) }));
  }

  private mutate(change: (list: SuiIntent[]) => SuiIntent[]): void {
    if (!this.storage) throw new SuiIntentStoreError('persist_failed', 'This browser cannot store the bridge recovery record, so nothing was sent.');
    const record = this.readRecord();
    if (record.status === 'newer_version') {
      this.status = 'newer_version';
      throw new SuiIntentStoreError('read_only', 'The bridge recovery record was written by a newer version of this app. Reload the page.');
    }
    if (record.status === 'unavailable') {
      this.status = 'unavailable';
      throw new SuiIntentStoreError('persist_failed', 'This browser cannot store the bridge recovery record, so nothing was sent.');
    }
    if (record.corruptRaw !== undefined) {
      this.intents = record.intents;
      this.preserveCorrupt(record.corruptRaw);
      if (this.status === 'unavailable') {
        throw new SuiIntentStoreError('persist_failed', 'The bridge recovery record is unreadable and could not be preserved, so nothing was sent.');
      }
    }
    const next = this.prune(change([...record.intents]));
    if (next.length > MAX_SUI_INTENTS) {
      throw new SuiIntentStoreError('too_many', 'Too many unresolved bridge requests are stored on this device. Resolve some first.');
    }
    try {
      this.writeRecord(next);
    } catch {
      throw new SuiIntentStoreError('persist_failed', 'Could not save the bridge recovery record, so nothing was sent.');
    }
    this.intents = next;
    if (this.status !== 'recovered') this.status = 'ok';
  }

  private prune(list: SuiIntent[]): SuiIntent[] {
    const resolved = list.filter(isSuiResolved).sort((a, b) => b.createdAt - a.createdAt);
    if (resolved.length <= MAX_RESOLVED_SUI_INTENTS) return list;
    const drop = new Set(resolved.slice(MAX_RESOLVED_SUI_INTENTS).map((intent) => intent.id));
    return list.filter((intent) => !drop.has(intent.id));
  }
}
