/**
 * Decode a 16-byte rust_decimal::Decimal serialized as a Candid blob.
 *
 * Layout (all little-endian u32):
 *   bytes[0..4]   flags — bits 16-23 = scale (decimal places), bit 31 = sign
 *   bytes[4..8]   lo
 *   bytes[8..12]  mid
 *   bytes[12..16] hi
 *
 * Value = (-1)^sign × (hi × 2^64 + mid × 2^32 + lo) / 10^scale
 */
export function decodeRustDecimal(bytes: Uint8Array | number[]): number {
  const arr = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  if (arr.length < 16) {
    console.warn('decodeRustDecimal: expected 16 bytes, got', arr.length);
    return 0;
  }
  const view = new DataView(arr.buffer, arr.byteOffset, arr.byteLength);
  const flags = view.getUint32(0, true);
  const lo = BigInt(view.getUint32(4, true));
  const mid = BigInt(view.getUint32(8, true));
  const hi = BigInt(view.getUint32(12, true));

  const scale = (flags >> 16) & 0xff;
  const negative = (flags >> 31) & 1;

  const mantissa = (hi << 64n) | (mid << 32n) | lo;
  const divisor = 10n ** BigInt(scale);
  const value = Number(mantissa) / Number(divisor);

  return negative ? -value : value;
}
