/**
 * The Gate's return `transfer_id`, recomputed exactly as the Move gate and the canister compute it.
 *
 * `rumi_icusd_gate::compute_transfer_id` (move/rumi_icusd_gate at 23f75ef1) and `ids.rs::transfer_id` both give
 *   sha2-256( bcs(DOMAIN_RETURN) ++ bcs(chain_identifier) ++ gate ++ sender ++ bcs(client_nonce) )
 * where `bcs(string)` and `bcs(vector<u8>)` are a ULEB128 length and the bytes, an address is its 32 raw bytes and a
 * u64 is 8 little-endian bytes. It is the canister's release key (`get_release`) and the Gate's `ReturnRecord` key. It is
 * NEVER a transaction digest: a wallet callback or a digest proves nothing about the payout.
 *
 * Verified in `reference.spec.ts` against vectors produced by the independent Python reference
 * (`move/rumi_icusd_gate/reference/transfer_id_reference.py`), the same vectors the Move and Rust tests pin.
 */

import { concat, sha256, toHex, type Bytes } from '../solanaBridge/bytes';

export const RETURN_DOMAIN = 'rumi.icusd.sui.return.v1';
export const U64_MAX = 18_446_744_073_709_551_615n;

/** ULEB128, the BCS length prefix. */
export function uleb(value: number): Bytes {
  const out: number[] = [];
  let v = value;
  do {
    let byte = v & 0x7f;
    v = Math.floor(v / 128);
    if (v > 0) byte |= 0x80;
    out.push(byte);
  } while (v > 0);
  return Uint8Array.from(out);
}

export function bcsVectorU8(bytes: Bytes): Bytes {
  return concat(uleb(bytes.length), bytes);
}

export function bcsString(text: string): Bytes {
  return bcsVectorU8(new TextEncoder().encode(text));
}

export function bcsU64(value: bigint): Bytes {
  if (value < 0n || value > U64_MAX) throw new RangeError('A u64 must be between 0 and 2^64 - 1.');
  const out = new Uint8Array(8);
  let v = value;
  for (let i = 0; i < 8; i++) {
    out[i] = Number(v & 0xffn);
    v >>= 8n;
  }
  return out;
}

export function transferIdBytes(chainIdentifier: Bytes, gate: Bytes, sender: Bytes, clientNonce: bigint): Bytes {
  if (chainIdentifier.length !== 32 || gate.length !== 32 || sender.length !== 32) throw new RangeError('Chain identifier, Gate and sender must each be 32 bytes.');
  return sha256(concat(bcsString(RETURN_DOMAIN), bcsVectorU8(chainIdentifier), gate, sender, bcsU64(clientNonce)));
}

/** The transfer id as the canister keys it: 64 lowercase hex digits. */
export function transferId(chainIdentifier: Bytes, gate: Bytes, sender: Bytes, clientNonce: bigint): string {
  return toHex(transferIdBytes(chainIdentifier, gate, sender, clientNonce));
}
