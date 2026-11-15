/**
 * Sui addresses and object ids in the strict form the canister and the Move package use: `0x` and exactly 64 lowercase
 * hex digits (`sui::parse_address_strict`). Short forms such as `0x2` are not accepted from a person: padding one would
 * quietly name a different account than the one they meant.
 */

import { fromHex, type Bytes } from '../solanaBridge/bytes';

const STRICT = /^0x[0-9a-f]{64}$/;

export type SuiAddressResult = { ok: true; address: string; bytes: Bytes } | { ok: false; message: string };

export function parseSuiAddress(text: string): SuiAddressResult {
  if (!STRICT.test(text)) return { ok: false, message: 'A Sui address is 0x followed by exactly 64 lowercase hexadecimal characters.' };
  const bytes = fromHex(text.slice(2));
  return bytes && bytes.length === 32 ? { ok: true, address: text, bytes } : { ok: false, message: 'That is not a Sui address.' };
}

/** What a person typed or pasted: trimmed, hex digits case-folded, then held to the strict form. */
export function normalizeSuiAddress(text: string): SuiAddressResult {
  const trimmed = text.trim();
  return parseSuiAddress(/^0[xX]/.test(trimmed) ? `0x${trimmed.slice(2).toLowerCase()}` : trimmed);
}

export function isZeroAddress(address: string): boolean {
  return /^0x0{64}$/.test(address);
}
