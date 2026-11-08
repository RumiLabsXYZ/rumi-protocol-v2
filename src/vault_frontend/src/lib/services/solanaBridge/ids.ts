/**
 * The id of a finalized return, computed the way the backend does (`ids.rs::return_id`):
 * `sha256(domain || (u32 big-endian length || field)*)` over the genesis hash and the 64-byte first
 * signature of the holder's transaction. It is how the page finds the bridge's record of a return from the
 * signature alone, without trusting any list.
 */

import { concat, sha256, toHex, u32be, type Bytes } from './bytes';

export const RETURN_DOMAIN = 'rumi.icusd.sol.return.v1';

function idHash(domain: string, fields: readonly Bytes[]): Bytes {
  return sha256(concat(new TextEncoder().encode(domain), ...fields.flatMap((f) => [u32be(f.length), f])));
}

export function returnId(genesisHash: Bytes, signature: Bytes): string {
  if (genesisHash.length !== 32 || signature.length !== 64) throw new RangeError('a return id is over a 32-byte genesis hash and a 64-byte signature');
  return toHex(idHash(RETURN_DOMAIN, [genesisHash, signature]));
}
