/**
 * EIP-55 mixed-case address checksums.
 *
 * The rule, shared with the bridge canister (`ids::parse_address`):
 *  - 0x followed by 40 hex digits that are all lowercase or all uppercase (digits
 *    carry no case) is an UNCHECKSUMMED address and is accepted as typed.
 *  - Hex that mixes upper and lower case claims to be checksummed, so it must
 *    match the EIP-55 checksum exactly. One flipped letter is a typo, and a typo
 *    in a recipient would mint to somebody else.
 *
 * The hash comes from `keccak.ts`. Every function takes an optional `hash`
 * argument so specs can inject one; the default is the app's configured one.
 * With no hash available a mixed-case address is `unverifiable`, never `valid`.
 */

import { keccak256 as configuredKeccak256, type Keccak256 } from './keccak';

export type { Keccak256 } from './keccak';

/**
 * - `unchecksummed`: all lowercase or all uppercase (or digits only); accepted.
 * - `valid`: mixed case and the checksum matches.
 * - `invalid`: mixed case and the checksum does not match.
 * - `unverifiable`: mixed case but no hash is available to check it.
 * - `malformed`: not 0x plus 40 hex digits.
 */
export type ChecksumStatus = 'unchecksummed' | 'valid' | 'invalid' | 'unverifiable' | 'malformed';

/** Shown inline when a mixed-case address fails its checksum. */
export const CHECKSUM_ERROR_MESSAGE =
  'This address has a checksum error. Check for a typo, or paste it in all lowercase.';

/** Shown when a mixed-case address cannot be checked because no hash is configured. */
export const CHECKSUM_UNVERIFIABLE_MESSAGE =
  'This address uses mixed case and its checksum cannot be checked here. Paste it in all lowercase.';

const ADDRESS = /^0x[0-9a-fA-F]{40}$/;

function asciiBytes(text: string): Uint8Array {
  const out = new Uint8Array(text.length);
  for (let i = 0; i < text.length; i++) out[i] = text.charCodeAt(i) & 0x7f;
  return out;
}

/** The EIP-55 form of a well-formed address, or null if it is malformed or no hash is available. */
export function toChecksumAddress(address: string, hash: Keccak256 | null = configuredKeccak256): string | null {
  if (!hash || !ADDRESS.test(address)) return null;
  const lower = address.slice(2).toLowerCase();
  const digest = hash(asciiBytes(lower));
  if (!digest || digest.length !== 32) return null;
  let out = '0x';
  for (let i = 0; i < lower.length; i++) {
    const nibble = i % 2 === 0 ? digest[i >> 1] >> 4 : digest[i >> 1] & 0x0f;
    const c = lower[i];
    out += c >= 'a' && c <= 'f' && nibble >= 8 ? c.toUpperCase() : c;
  }
  return out;
}

export function checksumStatus(address: string, hash: Keccak256 | null = configuredKeccak256): ChecksumStatus {
  if (typeof address !== 'string' || !ADDRESS.test(address)) return 'malformed';
  const body = address.slice(2);
  const hasLower = /[a-f]/.test(body);
  const hasUpper = /[A-F]/.test(body);
  if (!(hasLower && hasUpper)) return 'unchecksummed';
  const expected = toChecksumAddress(address, hash);
  if (expected === null) return 'unverifiable';
  return expected === address ? 'valid' : 'invalid';
}

/** True when the address may be used: unchecksummed, or mixed case with a matching checksum. */
export function passesChecksumRule(address: string, hash: Keccak256 | null = configuredKeccak256): boolean {
  const status = checksumStatus(address, hash);
  return status === 'unchecksummed' || status === 'valid';
}

/**
 * How to show an address to a person: EIP-55 when a hash is available, otherwise
 * lowercase. Anything that is not a well-formed address is returned unchanged.
 * Purely presentational: never feed the result to calldata, storage or comparisons.
 */
export function displayAddress(address: string, hash: Keccak256 | null = configuredKeccak256): string {
  if (typeof address !== 'string' || !ADDRESS.test(address)) return address;
  return toChecksumAddress(address, hash) ?? address.toLowerCase();
}
