/**
 * Keccak-256 for EIP-55 address checksums (`eip55.ts`). It is the original Keccak
 * padding used by Ethereum, NOT NIST SHA3-256.
 *
 * Scope: this hash decides only how an address is cased (EIP-55). It is never used
 * for signing, transaction ids, transfer ids or anything the canister trusts. The
 * bridge canister enforces the same checksum rule independently (`ids::parse_address`,
 * Rust `sha3` crate), so a defect here could only make the page refuse or display an
 * address differently, never move funds to a different address.
 *
 * Why a local implementation: the app has no EVM library, and `@noble/hashes` is only
 * a transitive dependency (importing it would be a phantom dependency, and declaring
 * it would change both lockfiles the deploy uses). `keccak.spec.ts` pins this code to
 * vectors produced by foundry's `cast keccak`, including inputs on both sides of the
 * 136-byte block boundary, and `eip55.spec.ts` checks the EIP-55 vectors.
 *
 * Implementation: Keccak-f[1600] on 25 64-bit lanes held as (lo, hi) 32-bit pairs,
 * rate 136 bytes, capacity 512 bits, pad10*1 with domain byte 0x01.
 */

/** Keccak-256 (the original Keccak padding used by Ethereum, NOT NIST SHA3-256). */
export type Keccak256 = (data: Uint8Array) => Uint8Array;

const RATE_BYTES = 136;

// Round constants as (lo, hi) pairs, generated from the FIPS 202 LFSR (Algorithm 5).
const RC = new Uint32Array([
  0x00000001, 0x00000000, 0x00008082, 0x00000000, 0x0000808a, 0x80000000, 0x80008000, 0x80000000,
  0x0000808b, 0x00000000, 0x80000001, 0x00000000, 0x80008081, 0x80000000, 0x00008009, 0x80000000,
  0x0000008a, 0x00000000, 0x00000088, 0x00000000, 0x80008009, 0x00000000, 0x8000000a, 0x00000000,
  0x8000808b, 0x00000000, 0x0000008b, 0x80000000, 0x00008089, 0x80000000, 0x00008003, 0x80000000,
  0x00008002, 0x80000000, 0x00000080, 0x80000000, 0x0000800a, 0x00000000, 0x8000000a, 0x80000000,
  0x80008081, 0x80000000, 0x00008080, 0x80000000, 0x80000001, 0x00000000, 0x80008008, 0x80000000,
]);

// Rho rotation offsets, indexed by lane x + 5y.
const RHO = [
  0, 1, 62, 28, 27,
  36, 44, 6, 55, 20,
  3, 10, 43, 25, 39,
  41, 45, 15, 21, 8,
  18, 2, 61, 56, 14,
];

// Pi destination of lane x + 5y: B[y, 2x + 3y].
const PI = new Array<number>(25);
for (let x = 0; x < 5; x++) {
  for (let y = 0; y < 5; y++) PI[x + 5 * y] = y + 5 * ((2 * x + 3 * y) % 5);
}

/** Rotate the 64-bit lane (lo, hi) left by n; writes into out[0] = lo, out[1] = hi. */
function rotl(lo: number, hi: number, n: number, out: Uint32Array): void {
  if (n === 0) {
    out[0] = lo;
    out[1] = hi;
  } else if (n < 32) {
    out[0] = (lo << n) | (hi >>> (32 - n));
    out[1] = (hi << n) | (lo >>> (32 - n));
  } else if (n === 32) {
    out[0] = hi;
    out[1] = lo;
  } else {
    const m = n - 32;
    out[0] = (hi << m) | (lo >>> (32 - m));
    out[1] = (lo << m) | (hi >>> (32 - m));
  }
}

/** Keccak-f[1600] on s (50 words: lane i is s[2i] = lo, s[2i + 1] = hi). */
function keccakF(s: Uint32Array): void {
  const c = new Uint32Array(10);
  const b = new Uint32Array(50);
  const t = new Uint32Array(2);
  for (let round = 0; round < 24; round++) {
    // Theta.
    for (let x = 0; x < 5; x++) {
      c[2 * x] = s[2 * x] ^ s[2 * (x + 5)] ^ s[2 * (x + 10)] ^ s[2 * (x + 15)] ^ s[2 * (x + 20)];
      c[2 * x + 1] = s[2 * x + 1] ^ s[2 * (x + 5) + 1] ^ s[2 * (x + 10) + 1] ^ s[2 * (x + 15) + 1] ^ s[2 * (x + 20) + 1];
    }
    for (let x = 0; x < 5; x++) {
      const next = (x + 1) % 5;
      const prev = (x + 4) % 5;
      rotl(c[2 * next], c[2 * next + 1], 1, t);
      const dLo = c[2 * prev] ^ t[0];
      const dHi = c[2 * prev + 1] ^ t[1];
      for (let y = 0; y < 25; y += 5) {
        s[2 * (x + y)] ^= dLo;
        s[2 * (x + y) + 1] ^= dHi;
      }
    }
    // Rho and pi.
    for (let i = 0; i < 25; i++) {
      rotl(s[2 * i], s[2 * i + 1], RHO[i], t);
      b[2 * PI[i]] = t[0];
      b[2 * PI[i] + 1] = t[1];
    }
    // Chi.
    for (let y = 0; y < 25; y += 5) {
      for (let x = 0; x < 5; x++) {
        const i = x + y;
        const i1 = ((x + 1) % 5) + y;
        const i2 = ((x + 2) % 5) + y;
        s[2 * i] = b[2 * i] ^ (~b[2 * i1] & b[2 * i2]);
        s[2 * i + 1] = b[2 * i + 1] ^ (~b[2 * i1 + 1] & b[2 * i2 + 1]);
      }
    }
    // Iota.
    s[0] ^= RC[2 * round];
    s[1] ^= RC[2 * round + 1];
  }
}

/** XOR one RATE_BYTES block (little-endian lanes) into the state and permute. */
function absorb(s: Uint32Array, block: Uint8Array): void {
  for (let j = 0; j < RATE_BYTES; j++) {
    const word = 2 * (j >> 3) + ((j & 7) >> 2);
    s[word] ^= block[j] << (8 * (j & 3));
  }
  keccakF(s);
}

export const keccak256: Keccak256 = (data: Uint8Array): Uint8Array => {
  const s = new Uint32Array(50);
  let offset = 0;
  for (; offset + RATE_BYTES <= data.length; offset += RATE_BYTES) {
    absorb(s, data.subarray(offset, offset + RATE_BYTES));
  }
  // Final block: the remaining bytes, then pad10*1 with the Keccak domain byte 0x01.
  const last = new Uint8Array(RATE_BYTES);
  last.set(data.subarray(offset));
  last[data.length - offset] ^= 0x01;
  last[RATE_BYTES - 1] ^= 0x80;
  absorb(s, last);
  // Squeeze 32 bytes from the first four lanes.
  const out = new Uint8Array(32);
  for (let j = 0; j < 32; j++) {
    out[j] = (s[2 * (j >> 3) + ((j & 7) >> 2)] >>> (8 * (j & 3))) & 0xff;
  }
  return out;
};
