/**
 * Browser-side intent journal for the Base bridge.
 *
 * An intent is written BEFORE the first wallet prompt of an action, so a
 * reload, a crashed tab or a lost reply can always be reconciled with the same
 * client request id (ICP to Base) or the same client nonce and transfer id
 * (Base to ICP). The canister and the contract stay authoritative: this record
 * only stores intents and hints (transaction hashes), never settlement state.
 *
 * Storage key: `rumi:base-bridge:v1:<icp principal>`. The record is scoped to
 * one principal, versioned, and never silently destroyed: an unreadable record
 * is copied aside before anything is rewritten, and a record written by a
 * newer version of the app is left untouched (the store becomes read-only).
 *
 * Versions. Version 1 records (the original Base-only app) carry no destination.
 * Version 2 adds a destination key to every NEW intent: the same 0x address is
 * valid on every EVM chain, so a saved row must say which destination it was
 * for. The key is PER NETWORK, the profile's network id (`base-mainnet`,
 * `base-sepolia`, `ethereum-mainnet`), never a family label such as `base`: a
 * mainnet and a testnet instance never share one key. A version 1 record is
 * still read in full. An intent without the key belongs to the Base network of
 * the page's configured Base bridge (`intentBelongsTo`), never to any other
 * network. The storage key keeps its `v1` name on purpose, so nothing already on
 * a device is orphaned. An older build that finds a version 2 record sees a
 * newer version and goes read-only.
 */

import { bytesToHex, isAddress, isBytes32, isHex } from './abi';
import type { KeyValueStorage } from './evmWallet';

export const INTENT_VERSION = 2;
/** The first record version: no destination key on its intents. Still read, never written for new intents. */
export const LEGACY_INTENT_VERSION = 1;
/** Every intent written before destinations existed was a Base intent: it belongs to the Base profile. */
export const LEGACY_PROFILE_ID = 'base';
export type IntentVersion = typeof LEGACY_INTENT_VERSION | typeof INTENT_VERSION;
export const INTENT_STORAGE_PREFIX = 'rumi:base-bridge:v1:';
/** Resolved intents beyond this many are pruned oldest first. Unresolved ones are never pruned. */
export const MAX_RESOLVED_INTENTS = 40;
export const MAX_INTENTS = 120;

export type IcpToBaseStatus = 'prepared' | 'approved' | 'submitted' | 'done' | 'failed';
export type BaseToIcpStatus = 'prepared' | 'sent' | 'done';

export interface IcpToBaseIntent {
  /** 1: written before destinations existed. 2: carries `destination`. */
  v: IntentVersion;
  /** Per-network destination key (for example `base-mainnet`). Absent only on version 1 intents, which are Base intents. */
  destination?: string;
  kind: 'icp_to_base';
  /** 0x + 32 hex chars: 16 random bytes. */
  clientRequestId: string;
  amountE8s: bigint;
  recipient: string;
  expectedDepositFee: bigint;
  createdAt: number;
  status: IcpToBaseStatus;
  /** Set once the canister returned a deposit for this request. */
  depositId?: bigint;
  /** Short reason when the canister rejected the request before creating anything. */
  failure?: string;
}

export interface BaseToIcpIntent {
  /** 1: written before destinations existed. 2: carries `destination`. */
  v: IntentVersion;
  /** Per-network destination key (for example `base-mainnet`). Absent only on version 1 intents, which are Base intents. */
  destination?: string;
  kind: 'base_to_icp';
  /** 0x + 64 hex: 32 random bytes. */
  clientNonce: string;
  /** Contract-computed transfer id for (from, clientNonce). */
  transferId: string;
  from: string;
  amountE8s: bigint;
  destinationPrincipal: string;
  chainId: number;
  contract: string;
  createdAt: number;
  status: BaseToIcpStatus;
  /** Wallet-reported transaction hash. A hint, never proof. */
  txHash?: string;
  /** True once the user chose to tell the bridge about the hash. */
  notified?: boolean;
}

export type BridgeIntent = IcpToBaseIntent | BaseToIcpIntent;

/** The part of a destination that decides whether an intent belongs to it. */
export interface IntentOwner {
  /** Per-network key (the profile's network id). */
  id: string;
  /** The profile this network belongs to (a family label). */
  profileId: string;
}

/**
 * Whether an intent belongs to a destination. A keyed intent belongs to exactly the network it names.
 * A keyless (legacy) intent belongs only to a Base destination, which is the Base network of the page's
 * configured Base bridge, found by the chain id that bridge reports. It never belongs to a non-Base network.
 */
export function intentBelongsTo(intent: Pick<BridgeIntent, 'destination'>, owner: IntentOwner): boolean {
  return intent.destination === undefined ? owner.profileId === LEGACY_PROFILE_ID : intent.destination === owner.id;
}

/**
 * A per-network key: a profile label and a role joined by a hyphen (`base-mainnet`, `ethereum-sepolia`).
 * A bare family label (`base`) is not one, so a version 2 intent keyed that way is unreadable by design.
 */
const NETWORK_KEY = /^[a-z0-9]+(?:-[a-z0-9]+)+$/;
const NETWORK_KEY_MAX = 48;
export function isNetworkKey(value: unknown): value is string {
  return typeof value === 'string' && value.length <= NETWORK_KEY_MAX && NETWORK_KEY.test(value);
}

export function intentId(intent: BridgeIntent): string {
  return intent.kind === 'icp_to_base' ? intent.clientRequestId : intent.transferId;
}

export function isResolved(intent: BridgeIntent): boolean {
  return intent.status === 'done' || (intent.kind === 'icp_to_base' && intent.status === 'failed');
}

export function intentStorageKey(principalText: string): string {
  return `${INTENT_STORAGE_PREFIX}${principalText}`;
}

export type RandomBytes = (length: number) => Uint8Array;

export const defaultRandomBytes: RandomBytes = (length) => {
  const out = new Uint8Array(length);
  crypto.getRandomValues(out);
  return out;
};

export function newClientRequestId(random: RandomBytes = defaultRandomBytes): string {
  return bytesToHex(random(16));
}

export function newClientNonce(random: RandomBytes = defaultRandomBytes): string {
  return bytesToHex(random(32));
}

// ---------------------------------------------------------------------------
// Persisted shape (bigint values are decimal strings)
// ---------------------------------------------------------------------------

type Json = Record<string, unknown>;

function serializeIntent(intent: BridgeIntent): Json {
  const base: Json = { ...intent, amountE8s: intent.amountE8s.toString() };
  if (intent.kind === 'icp_to_base') {
    base.expectedDepositFee = intent.expectedDepositFee.toString();
    if (intent.depositId !== undefined) base.depositId = intent.depositId.toString();
  }
  return base;
}

const DECIMAL = /^\d{1,30}$/;

function str(value: unknown): value is string {
  return typeof value === 'string';
}

function parseIntent(value: unknown): BridgeIntent | null {
  if (!value || typeof value !== 'object') return null;
  const r = value as Json;
  if (r.v !== LEGACY_INTENT_VERSION && r.v !== INTENT_VERSION) return null;
  const version: IntentVersion = r.v;
  // Version 2 intents must name their destination. Version 1 intents have none (legacy, bound to Base on read).
  if (r.destination !== undefined && !isNetworkKey(r.destination)) return null;
  if (version === INTENT_VERSION && r.destination === undefined) return null;
  const destination = r.destination === undefined ? {} : { destination: r.destination as string };
  if (!str(r.amountE8s) || !DECIMAL.test(r.amountE8s)) return null;
  if (typeof r.createdAt !== 'number' || !Number.isFinite(r.createdAt)) return null;
  if (r.kind === 'icp_to_base') {
    if (!str(r.clientRequestId) || !isHex(r.clientRequestId, 16)) return null;
    if (!str(r.recipient) || !isAddress(r.recipient)) return null;
    if (!str(r.expectedDepositFee) || !DECIMAL.test(r.expectedDepositFee)) return null;
    if (!['prepared', 'approved', 'submitted', 'done', 'failed'].includes(r.status as string)) return null;
    if (r.depositId !== undefined && (!str(r.depositId) || !DECIMAL.test(r.depositId))) return null;
    return {
      v: version,
      ...destination,
      kind: 'icp_to_base',
      clientRequestId: r.clientRequestId.toLowerCase(),
      amountE8s: BigInt(r.amountE8s),
      recipient: r.recipient.toLowerCase(),
      expectedDepositFee: BigInt(r.expectedDepositFee),
      createdAt: r.createdAt,
      status: r.status as IcpToBaseStatus,
      ...(r.depositId !== undefined ? { depositId: BigInt(r.depositId as string) } : {}),
      ...(str(r.failure) ? { failure: r.failure } : {}),
    };
  }
  if (r.kind === 'base_to_icp') {
    if (!str(r.clientNonce) || !isBytes32(r.clientNonce)) return null;
    if (!str(r.transferId) || !isBytes32(r.transferId)) return null;
    if (!str(r.from) || !isAddress(r.from)) return null;
    if (!str(r.destinationPrincipal) || r.destinationPrincipal.length === 0) return null;
    if (typeof r.chainId !== 'number' || !Number.isSafeInteger(r.chainId)) return null;
    if (!str(r.contract) || !isAddress(r.contract)) return null;
    if (!['prepared', 'sent', 'done'].includes(r.status as string)) return null;
    if (r.txHash !== undefined && (!str(r.txHash) || !isBytes32(r.txHash))) return null;
    return {
      v: version,
      ...destination,
      kind: 'base_to_icp',
      clientNonce: r.clientNonce.toLowerCase(),
      transferId: r.transferId.toLowerCase(),
      from: r.from.toLowerCase(),
      amountE8s: BigInt(r.amountE8s),
      destinationPrincipal: r.destinationPrincipal,
      chainId: r.chainId,
      contract: r.contract.toLowerCase(),
      createdAt: r.createdAt,
      status: r.status as BaseToIcpStatus,
      ...(str(r.txHash) ? { txHash: r.txHash.toLowerCase() } : {}),
      ...(r.notified === true ? { notified: true } : {}),
    };
  }
  return null;
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

export type IntentStoreErrorCode = 'persist_failed' | 'read_only' | 'too_many';

export class IntentStoreError extends Error {
  readonly code: IntentStoreErrorCode;
  constructor(code: IntentStoreErrorCode, message: string) {
    super(message);
    this.name = 'IntentStoreError';
    this.code = code;
  }
}

export type IntentLoadStatus =
  /** No record, or a clean one. */
  | 'ok'
  /** The record (or part of it) was unreadable; the original was copied to `quarantineKey`. */
  | 'recovered'
  /** Written by a newer version of the app. Left untouched; the store is read-only. */
  | 'newer_version'
  /** Browser storage is unavailable. Nothing can be persisted. */
  | 'unavailable';

interface RawRecord {
  status: IntentLoadStatus;
  intents: BridgeIntent[];
  /** Raw text that needs to be preserved before a rewrite. */
  corruptRaw?: string;
}

export class IntentStore {
  readonly key: string;
  private readonly storage: KeyValueStorage | null;
  private readonly principal: string;
  private readonly now: () => number;
  private intents: BridgeIntent[] = [];
  private status: IntentLoadStatus = 'ok';
  private quarantine: string | null = null;

  constructor(storage: KeyValueStorage | null, principalText: string, opts: { now?: () => number } = {}) {
    this.storage = storage;
    this.principal = principalText;
    this.key = intentStorageKey(principalText);
    this.now = opts.now ?? Date.now;
    this.reload();
  }

  get loadStatus(): IntentLoadStatus {
    return this.status;
  }

  /** The key holding a copy of an unreadable record, when one was preserved. */
  get quarantineKey(): string | null {
    return this.quarantine;
  }

  get readOnly(): boolean {
    return this.status === 'newer_version' || this.status === 'unavailable';
  }

  /** Newest first. */
  list(): BridgeIntent[] {
    return [...this.intents].sort((a, b) => b.createdAt - a.createdAt);
  }

  /** Newest first, only the intents of one destination (legacy intents belong to the Base destination). */
  listFor(owner: IntentOwner): BridgeIntent[] {
    return this.list().filter((intent) => intentBelongsTo(intent, owner));
  }

  get(id: string): BridgeIntent | undefined {
    const wanted = id.toLowerCase();
    return this.intents.find((intent) => intentId(intent).toLowerCase() === wanted);
  }

  /** Re-read storage (another tab may have written). */
  reload(): IntentLoadStatus {
    const record = this.readRecord();
    this.status = record.status;
    this.intents = record.intents;
    if (record.corruptRaw !== undefined) this.preserveCorrupt(record.corruptRaw);
    return this.status;
  }

  put(intent: BridgeIntent): void {
    this.mutate((list) => {
      const id = intentId(intent).toLowerCase();
      const at = list.findIndex((existing) => intentId(existing).toLowerCase() === id);
      if (at >= 0) list[at] = intent;
      else list.push(intent);
      return list;
    });
  }

  patch<T extends BridgeIntent>(id: string, patch: Partial<T>): T {
    let updated: BridgeIntent | undefined;
    this.mutate((list) => {
      const wanted = id.toLowerCase();
      const at = list.findIndex((existing) => intentId(existing).toLowerCase() === wanted);
      if (at < 0) throw new IntentStoreError('persist_failed', 'The bridge recovery record for this request no longer exists.');
      updated = { ...list[at], ...patch } as BridgeIntent;
      list[at] = updated;
      return list;
    });
    return updated as T;
  }

  remove(id: string): void {
    const wanted = id.toLowerCase();
    this.mutate((list) => list.filter((existing) => intentId(existing).toLowerCase() !== wanted));
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
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
      return { status: 'recovered', intents: [], corruptRaw: raw };
    }
    const record = parsed as Json;
    if (typeof record.v === 'number' && record.v > INTENT_VERSION) {
      return { status: 'newer_version', intents: [] };
    }
    if (
      (record.v !== INTENT_VERSION && record.v !== LEGACY_INTENT_VERSION) ||
      record.principal !== this.principal ||
      !Array.isArray(record.intents)
    ) {
      return { status: 'recovered', intents: [], corruptRaw: raw };
    }
    const intents: BridgeIntent[] = [];
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
      // The copy is safe, so the main record can now be rewritten with what is readable.
      this.writeRecord(this.intents);
    } catch {
      // Could not preserve the original: do not touch it.
      this.status = 'unavailable';
    }
  }

  private writeRecord(list: BridgeIntent[]): void {
    const payload = JSON.stringify({ v: INTENT_VERSION, principal: this.principal, intents: list.map(serializeIntent) });
    this.storage!.setItem(this.key, payload);
  }

  private mutate(change: (list: BridgeIntent[]) => BridgeIntent[]): void {
    if (!this.storage) {
      throw new IntentStoreError('persist_failed', 'This browser cannot store the bridge recovery record, so nothing was sent.');
    }
    // Re-read first so a write from another tab is merged rather than overwritten.
    const record = this.readRecord();
    if (record.status === 'newer_version') {
      this.status = 'newer_version';
      throw new IntentStoreError('read_only', 'The bridge recovery record was written by a newer version of this app. Reload the page.');
    }
    if (record.status === 'unavailable') {
      this.status = 'unavailable';
      throw new IntentStoreError('persist_failed', 'This browser cannot store the bridge recovery record, so nothing was sent.');
    }
    if (record.corruptRaw !== undefined) {
      this.intents = record.intents;
      this.preserveCorrupt(record.corruptRaw);
      if (this.status === 'unavailable') {
        throw new IntentStoreError('persist_failed', 'The bridge recovery record is unreadable and could not be preserved, so nothing was sent.');
      }
    }
    const next = this.prune(change([...record.intents]));
    if (next.length > MAX_INTENTS) {
      throw new IntentStoreError('too_many', 'Too many unresolved bridge requests are stored on this device. Resolve some first.');
    }
    try {
      this.writeRecord(next);
    } catch {
      throw new IntentStoreError('persist_failed', 'Could not save the bridge recovery record, so nothing was sent.');
    }
    this.intents = next;
    // A recovery notice stays visible until the next reload.
    if (this.status !== 'recovered') this.status = 'ok';
  }

  private prune(list: BridgeIntent[]): BridgeIntent[] {
    const resolved = list.filter(isResolved).sort((a, b) => b.createdAt - a.createdAt);
    if (resolved.length <= MAX_RESOLVED_INTENTS) return list;
    const drop = new Set(resolved.slice(MAX_RESOLVED_INTENTS).map((intent) => intentId(intent)));
    return list.filter((intent) => !drop.has(intentId(intent)));
  }
}

/** A sessionStorage/localStorage handle that may be unavailable (private windows, blocked storage). */
export function browserLocalStorage(): KeyValueStorage | null {
  try {
    return typeof localStorage !== 'undefined' ? localStorage : null;
  } catch {
    return null;
  }
}
