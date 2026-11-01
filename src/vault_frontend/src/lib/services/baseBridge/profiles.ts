/**
 * A pure reader for the destination profiles in `src/rumi_bridge_destinations/profiles/*.json`.
 *
 * The profiles are groundwork data. They describe what each network needs before it can be used; they
 * never turn anything on. The Rust crate `rumi_bridge_destinations` is the strict validator (it runs the
 * full rule set). This reader only takes what a page or a test needs, and it is strict about the two
 * things a JavaScript consumer can get wrong:
 *
 *  - AMOUNTS AND BIG INTEGERS STAY STRINGS. An amount (`min_e8s`, `max_e8s`, `max_outstanding_e8s`) must
 *    be a canonical decimal string (digits only, no sign, no leading zero, at most u64::MAX), and it is
 *    returned as that same string. It is never passed through `Number`, which would round a value such as
 *    18446744073709551615. A JSON number where a string is required is an error, not a conversion. A chain
 *    identity (an EVM chain id, an XRPL network id) is a string for the same reason.
 *  - `status` IS ADVISORY. A profile can say `staged` or `enabled` in the vocabulary, but groundwork data
 *    is all `disabled` with `instance_ref` null, and nothing here may enable a destination. Only the bridge
 *    canister's own answer does that (see `destinations.ts`).
 *
 * The functions take parsed JSON, so the caller decides how the files are loaded (the specs read them from
 * disk; the page does not import them).
 */

export type ProfileFamily = 'evm' | 'solana' | 'sui' | 'xrpl';
export type NetworkRole = 'mainnet' | 'testnet' | 'devnet';
export type NetworkStatus = 'disabled' | 'staged' | 'enabled';
export type BlockerKind = 'config' | 'code' | 'external' | 'decision';
export type IdentityKind = 'evm_chain_id' | 'solana_genesis_hash' | 'sui_chain_identifier' | 'xrpl_network_id';
export type FinalityRule = 'evm_tag' | 'monad_verified' | 'solana_commitment' | 'sui_checkpoint' | 'xrpl_validated';
export type AmountEncoding = 'u64' | 'int63' | 'xrpl_decimal_15';

export const PROFILE_SCHEMA_VERSION = 1;
/** The largest u64, as the canonical decimal string every amount is compared against. */
export const U64_MAX_DECIMAL = '18446744073709551615';

export class ProfileError extends Error {
  readonly path: string;
  constructor(path: string, message: string) {
    super(`${path}: ${message}`);
    this.name = 'ProfileError';
    this.path = path;
  }
}

export interface ProfileAmountBounds {
  /** Canonical decimal strings, in e8s. */
  minE8s: string;
  maxE8s: string;
  maxOutstandingE8s: string | null;
  encoding: AmountEncoding;
  basis: string | null;
}

export interface ProfileBlocker {
  id: string;
  kind: BlockerKind;
  text: string;
  source: string;
}

export interface ProfileFinality {
  rule: FinalityRule;
  /** The user-facing wait, as the profile words it. */
  typicalWait: string;
  tag: string | null;
  commitment: string | null;
  checkpoint: string | null;
  ledgerIndex: string | null;
  requiredResult: string | null;
  /** Monad only: blocks between Finalized and Verified. A small count, not an amount. */
  executionDelayBlocks: number | null;
}

export interface ProfileNetwork {
  /** Per-network id (`base-mainnet`, `ethereum-sepolia`). The destination key of saved records. */
  networkId: string;
  role: NetworkRole;
  /** The chain identity as a STRING (an EVM chain id is `"8453"`, never a number). */
  identity: { kind: IdentityKind; value: string };
  /** Advisory. Groundwork data is `disabled`. */
  status: NetworkStatus;
  /** Null in groundwork; later the instance canister principal. */
  instanceRef: string | null;
  blockers: ProfileBlocker[];
  /** The ids of `blockers`, in order. */
  blockerIds: string[];
  finality: ProfileFinality;
}

export interface DestinationProfile {
  schemaVersion: number;
  /** The family label (`base`, `ethereum`). */
  id: string;
  displayName: string;
  family: ProfileFamily;
  decimals: number;
  amountBounds: ProfileAmountBounds;
  networks: ProfileNetwork[];
}

// ---------------------------------------------------------------------------
// Validation helpers
// ---------------------------------------------------------------------------

type Json = Record<string, unknown>;

const FAMILIES: readonly ProfileFamily[] = ['evm', 'solana', 'sui', 'xrpl'];
const ROLES: readonly NetworkRole[] = ['mainnet', 'testnet', 'devnet'];
const STATUSES: readonly NetworkStatus[] = ['disabled', 'staged', 'enabled'];
const BLOCKER_KINDS: readonly BlockerKind[] = ['config', 'code', 'external', 'decision'];
const IDENTITY_KINDS: readonly IdentityKind[] = ['evm_chain_id', 'solana_genesis_hash', 'sui_chain_identifier', 'xrpl_network_id'];
const FINALITY_RULES: readonly FinalityRule[] = ['evm_tag', 'monad_verified', 'solana_commitment', 'sui_checkpoint', 'xrpl_validated'];
const ENCODINGS: readonly AmountEncoding[] = ['u64', 'int63', 'xrpl_decimal_15'];

const CANONICAL_DECIMAL = /^(0|[1-9][0-9]*)$/;

/**
 * Whether text is a canonical decimal u64: ASCII digits only, no sign, no whitespace, no leading zero
 * (except "0") and not above u64::MAX. Decided on the text alone, so no number is ever formed.
 */
export function isCanonicalU64(text: unknown): text is string {
  if (typeof text !== 'string' || !CANONICAL_DECIMAL.test(text)) return false;
  if (text.length !== U64_MAX_DECIMAL.length) return text.length < U64_MAX_DECIMAL.length;
  return text <= U64_MAX_DECIMAL; // same length: digit-by-digit comparison is numeric
}

function object(value: unknown, path: string): Json {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) throw new ProfileError(path, 'expected an object');
  return value as Json;
}

function array(value: unknown, path: string): unknown[] {
  if (!Array.isArray(value)) throw new ProfileError(path, 'expected an array');
  return value;
}

function text(value: unknown, path: string): string {
  if (typeof value !== 'string' || value.trim() === '') throw new ProfileError(path, 'expected a non-empty string');
  return value;
}

function oneOf<T extends string>(value: unknown, allowed: readonly T[], path: string): T {
  if (typeof value !== 'string' || !(allowed as readonly string[]).includes(value)) {
    throw new ProfileError(path, `expected one of ${allowed.join(', ')}`);
  }
  return value as T;
}

function amount(value: unknown, path: string): string {
  if (typeof value === 'number') {
    throw new ProfileError(path, 'an amount must be a decimal string, not a JSON number (a number can round)');
  }
  if (!isCanonicalU64(value)) throw new ProfileError(path, 'expected a canonical decimal string (digits only, no leading zero, at most u64::MAX)');
  return value;
}

function nullable<T>(value: unknown, read: (v: unknown) => T): T | null {
  return value === null || value === undefined ? null : read(value);
}

/** A fact wrapper (`{ value, source_url, verified, source_kind }`): the value inside. */
function fact(value: unknown, path: string): unknown {
  const wrapper = object(value, path);
  if (!('value' in wrapper)) throw new ProfileError(path, 'expected a sourced fact with a value');
  return wrapper.value;
}

function identity(value: unknown, path: string): { kind: IdentityKind; value: string } {
  const raw = object(fact(value, path), `${path}.value`);
  const kind = oneOf(raw.kind, IDENTITY_KINDS, `${path}.value.kind`);
  const id = raw.value;
  if (typeof id !== 'string') throw new ProfileError(`${path}.value.value`, 'a chain identity must be a string, not a JSON number');
  const numeric = kind === 'evm_chain_id' || kind === 'xrpl_network_id';
  if (numeric ? !CANONICAL_DECIMAL.test(id) : id.trim() === '' || /\s/.test(id)) {
    throw new ProfileError(`${path}.value.value`, numeric ? 'expected a canonical decimal string' : 'expected a non-empty identifier without spaces');
  }
  return { kind, value: id };
}

function blocker(value: unknown, path: string): ProfileBlocker {
  const raw = object(value, path);
  return {
    id: text(raw.id, `${path}.id`),
    kind: oneOf(raw.kind, BLOCKER_KINDS, `${path}.kind`),
    text: text(raw.text, `${path}.text`),
    source: text(raw.source, `${path}.source`),
  };
}

function optionalText(value: unknown, path: string): string | null {
  return nullable(value, (v) => text(v, path));
}

function finality(value: unknown, path: string): ProfileFinality {
  const raw = object(value, path);
  const params = object(fact(raw.parameters, `${path}.parameters`), `${path}.parameters.value`);
  const delay = params.execution_delay_blocks;
  if (delay !== null && delay !== undefined && !(typeof delay === 'number' && Number.isSafeInteger(delay) && delay >= 0)) {
    throw new ProfileError(`${path}.parameters.value.execution_delay_blocks`, 'expected a small non-negative integer or null');
  }
  return {
    rule: oneOf(raw.rule, FINALITY_RULES, `${path}.rule`),
    typicalWait: text(params.typical_wait, `${path}.parameters.value.typical_wait`),
    tag: optionalText(params.tag, `${path}.parameters.value.tag`),
    commitment: optionalText(params.commitment, `${path}.parameters.value.commitment`),
    checkpoint: optionalText(params.checkpoint, `${path}.parameters.value.checkpoint`),
    ledgerIndex: optionalText(params.ledger_index, `${path}.parameters.value.ledger_index`),
    requiredResult: optionalText(params.required_result, `${path}.parameters.value.required_result`),
    executionDelayBlocks: (delay ?? null) as number | null,
  };
}

function network(value: unknown, path: string): ProfileNetwork {
  const raw = object(value, path);
  const blockers = array(raw.activation_blockers, `${path}.activation_blockers`).map((b, i) => blocker(b, `${path}.activation_blockers[${i}]`));
  const ids = blockers.map((b) => b.id);
  if (new Set(ids).size !== ids.length) throw new ProfileError(`${path}.activation_blockers`, 'duplicate blocker id');
  return {
    networkId: text(raw.network_id, `${path}.network_id`),
    role: oneOf(raw.role, ROLES, `${path}.role`),
    identity: identity(raw.identity, `${path}.identity`),
    status: oneOf(raw.status, STATUSES, `${path}.status`),
    instanceRef: optionalText(raw.instance_ref, `${path}.instance_ref`),
    blockers,
    blockerIds: ids,
    finality: finality(raw.finality, `${path}.finality`),
  };
}

function amountBounds(value: unknown, path: string): ProfileAmountBounds {
  const raw = object(fact(value, path), `${path}.value`);
  return {
    minE8s: amount(raw.min_e8s, `${path}.value.min_e8s`),
    maxE8s: amount(raw.max_e8s, `${path}.value.max_e8s`),
    maxOutstandingE8s: nullable(raw.max_outstanding_e8s, (v) => amount(v, `${path}.value.max_outstanding_e8s`)),
    encoding: oneOf(raw.encoding, ENCODINGS, `${path}.value.encoding`),
    basis: optionalText(raw.basis, `${path}.value.basis`),
  };
}

// ---------------------------------------------------------------------------
// Public readers
// ---------------------------------------------------------------------------

/** Read one parsed profile. Throws `ProfileError` (with the JSON path) on the first thing it cannot accept. */
export function parseProfile(raw: unknown): DestinationProfile {
  const root = object(raw, 'profile');
  const id = text(root.id, 'profile.id');
  const where = `profile ${id}`;
  if (root.schema_version !== PROFILE_SCHEMA_VERSION) {
    throw new ProfileError(`${where}.schema_version`, `expected ${PROFILE_SCHEMA_VERSION}`);
  }
  const representation = object(root.representation, `${where}.representation`);
  const decimals = representation.decimals;
  if (!(typeof decimals === 'number' && Number.isInteger(decimals) && decimals >= 0 && decimals <= 255)) {
    throw new ProfileError(`${where}.representation.decimals`, 'expected a small integer');
  }
  const networks = array(root.networks, `${where}.networks`).map((n, i) => network(n, `${where}.networks[${i}]`));
  if (networks.length === 0) throw new ProfileError(`${where}.networks`, 'expected at least one network');
  const networkIds = networks.map((n) => n.networkId);
  if (new Set(networkIds).size !== networkIds.length) throw new ProfileError(`${where}.networks`, 'duplicate network id');
  return {
    schemaVersion: PROFILE_SCHEMA_VERSION,
    id,
    displayName: text(root.display_name, `${where}.display_name`),
    family: oneOf(root.family, FAMILIES, `${where}.family`),
    decimals,
    amountBounds: amountBounds(representation.amount_bounds, `${where}.representation.amount_bounds`),
    networks,
  };
}

/**
 * Read a set of parsed profiles. A profile id and a network id each appear once across the set: a per-network
 * id that two profiles share would let two networks share one destination key.
 */
export function parseProfiles(raws: readonly unknown[]): DestinationProfile[] {
  const profiles = raws.map(parseProfile);
  const profileIds = new Set<string>();
  const networkIds = new Set<string>();
  for (const profile of profiles) {
    if (profileIds.has(profile.id)) throw new ProfileError(`profile ${profile.id}`, 'duplicate profile id');
    profileIds.add(profile.id);
    for (const net of profile.networks) {
      if (networkIds.has(net.networkId)) throw new ProfileError(`profile ${profile.id}`, `network id ${net.networkId} is used twice`);
      networkIds.add(net.networkId);
    }
  }
  return profiles;
}

export function findNetwork(
  profiles: readonly DestinationProfile[],
  networkId: string,
): { profile: DestinationProfile; network: ProfileNetwork } | null {
  for (const profile of profiles) {
    const found = profile.networks.find((n) => n.networkId === networkId);
    if (found) return { profile, network: found };
  }
  return null;
}

/** The EVM network with this chain id (given as a string, as the profile keeps it). */
export function findEvmNetwork(
  profiles: readonly DestinationProfile[],
  chainId: string,
): { profile: DestinationProfile; network: ProfileNetwork } | null {
  for (const profile of profiles) {
    const found = profile.networks.find((n) => n.identity.kind === 'evm_chain_id' && n.identity.value === chainId);
    if (found) return { profile, network: found };
  }
  return null;
}
