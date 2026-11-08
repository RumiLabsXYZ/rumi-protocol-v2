/**
 * The user return memo: `rumi1|r|<principal>|<subaccount>|<nonce>`, written to match `memo.rs` exactly.
 *
 *  - ASCII, at most 160 bytes, five `|`-separated fields, nothing else;
 *  - the principal is the canonical ICP text of a principal that is not anonymous, not the management
 *    canister and not the bridge itself;
 *  - the subaccount is `-` for the default (or all-zero) subaccount, else 64 lowercase hex characters;
 *  - the nonce is a u64 in decimal without leading zeros (`0` alone is allowed).
 *
 * The backend parses this memo strictly. A memo it would refuse turns the holder's transfer into a stray
 * that is only refunded manually, so the page refuses to build one.
 */

import { Principal } from '@dfinity/principal';

export const MEMO_TAG = 'rumi1';
export const MAX_RETURN_MEMO_BYTES = 160;
export const U64_MAX = 18_446_744_073_709_551_615n;

export type MemoProblemCode =
  | 'empty_principal'
  | 'principal_syntax'
  | 'principal_not_canonical'
  | 'principal_anonymous'
  | 'principal_management'
  | 'principal_is_bridge'
  | 'subaccount_syntax'
  | 'nonce_syntax'
  | 'nonce_leading_zero'
  | 'nonce_overflow'
  | 'too_long';

export class MemoError extends Error {
  readonly code: MemoProblemCode;
  constructor(code: MemoProblemCode, message: string) {
    super(message);
    this.name = 'MemoError';
    this.code = code;
  }
}

export interface ReturnMemoInput {
  /** The ICP principal credited with the returned icUSD, as text. */
  principal: string;
  /** 64 lowercase hex characters, or null/`-`/all zeros for the default subaccount. */
  subaccount: string | null;
  /** A u64, as a decimal string or a bigint. */
  nonce: string | bigint;
}

const ZERO_SUBACCOUNT = '0'.repeat(64);

/** The destination principal, checked exactly as the backend does. `bridge` is the bridge canister's principal. */
export function checkDestinationPrincipal(text: string, bridge: string | null): Principal {
  if (text.trim() === '') throw new MemoError('empty_principal', 'Enter the Internet Computer principal that should receive the icUSD.');
  let principal: Principal;
  try {
    principal = Principal.fromText(text);
  } catch {
    throw new MemoError('principal_syntax', 'That is not a valid Internet Computer principal.');
  }
  if (principal.toText() !== text) {
    throw new MemoError('principal_not_canonical', `Write the principal exactly as ${principal.toText()} (lower case, with its dashes).`);
  }
  if (principal.isAnonymous()) throw new MemoError('principal_anonymous', 'The anonymous principal cannot receive icUSD.');
  if (principal.toText() === 'aaaaa-aa') throw new MemoError('principal_management', 'The management canister cannot receive icUSD.');
  if (bridge !== null && principal.toText() === bridge) {
    throw new MemoError('principal_is_bridge', 'The bridge canister itself cannot receive icUSD. Use your own principal.');
  }
  return principal;
}

/** The canonical subaccount field: `-` for the default, else the 64 lowercase hex characters. */
export function checkSubaccountField(value: string | null): string {
  if (value === null || value === '' || value === '-' || value === ZERO_SUBACCOUNT) return '-';
  if (!/^[0-9a-f]{64}$/.test(value)) {
    throw new MemoError('subaccount_syntax', 'A subaccount is 64 lowercase hexadecimal characters, or empty for the default one.');
  }
  return value;
}

export function checkNonce(value: string | bigint): string {
  const text = typeof value === 'bigint' ? value.toString() : value;
  if (!/^[0-9]+$/.test(text)) throw new MemoError('nonce_syntax', 'The request number must be digits only.');
  if (text.length > 1 && text.startsWith('0')) throw new MemoError('nonce_leading_zero', 'The request number has a leading zero.');
  if (BigInt(text) > U64_MAX) throw new MemoError('nonce_overflow', 'The request number is above the largest 64-bit value.');
  return text;
}

/** The exact memo text the transaction carries. Throws `MemoError` for anything the backend would refuse. */
export function encodeReturnMemo(input: ReturnMemoInput, bridge: string | null): string {
  const principal = checkDestinationPrincipal(input.principal, bridge).toText();
  const memo = `${MEMO_TAG}|r|${principal}|${checkSubaccountField(input.subaccount)}|${checkNonce(input.nonce)}`;
  if (new TextEncoder().encode(memo).length > MAX_RETURN_MEMO_BYTES) {
    throw new MemoError('too_long', 'The memo is longer than the 160 bytes the bridge accepts.');
  }
  return memo;
}

/** Parse a return memo, as the backend does (used to check what a saved attempt or a landed transaction says). */
export function parseReturnMemo(text: string, bridge: string | null): ReturnMemoInput & { nonce: bigint } {
  const bytes = new TextEncoder().encode(text);
  if (bytes.length > MAX_RETURN_MEMO_BYTES) throw new MemoError('too_long', 'The memo is longer than 160 bytes.');
  const fields = text.split('|');
  if (fields.length !== 5 || fields[0] !== MEMO_TAG || fields[1] !== 'r') {
    throw new MemoError('principal_syntax', 'This is not a return memo.');
  }
  const principal = checkDestinationPrincipal(fields[2], bridge).toText();
  if (fields[3] === ZERO_SUBACCOUNT) throw new MemoError('subaccount_syntax', 'An all-zero subaccount is spelled -.');
  const subaccount = fields[3] === '-' ? null : checkSubaccountField(fields[3]);
  return { principal, subaccount, nonce: BigInt(checkNonce(fields[4])) };
}
