import { describe, expect, it } from 'vitest';
import { keccak256 } from './keccak';

const hex = (bytes: Uint8Array) => Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');

/** Deterministic input of length n: byte i is (7i + 3) mod 256. */
function pattern(n: number): Uint8Array {
  const out = new Uint8Array(n);
  for (let i = 0; i < n; i++) out[i] = (i * 7 + 3) & 0xff;
  return out;
}

// Expected digests produced locally with foundry `cast keccak 0x<pattern(n) as hex>`.
// Lengths straddle the 136-byte rate: 135 (padding fits in one byte), 136 (padding needs
// a whole extra block), 137, and two-block boundaries 271..273.
const VECTORS: Array<[number, string]> = [
  [0, 'c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470'],
  [1, '69c322e3248a5dfc29d73c5b0553b0185a35cd5bb6386747517ef7e53b15e287'],
  [135, '00ef96af9cf4b24c7f269d922294444a197d0a33638c2e56634c57e892103a8f'],
  [136, '742061bcad767ed4c4f5883b1dcb1aad11afdcc140dc469d953759b127b9f9ed'],
  [137, 'e3371f61e770abf254c34239c3b0099ad90594507415bc81dd0a10b9692bbf2a'],
  [271, '4401c4afbe16ff911bdbf2d38e556e5b861f3fdf0f9d4306b1c46f6ae4f73584'],
  [272, 'ac141fd7b0a0ffcd2e967254d508da3ec616596493c36fa304425647d90e6de5'],
  [273, '16192ea86793083e47731cb3c970600f04768414d92bc0540e54ce8607a0fce0'],
  [1000, '80cdc8dd52cbb3dbaea8f383209893fa2bb52efbd5aedbb4b26dcfe307fcdc9b'],
];

describe('keccak256', () => {
  it.each(VECTORS)('matches cast keccak for a %i-byte input', (n, expected) => {
    expect(hex(keccak256(pattern(n)))).toBe(expected);
  });

  it('is Keccak-256, not NIST SHA3-256', () => {
    expect(hex(keccak256(new TextEncoder().encode('abc')))).toBe(
      '4e03657aea45a94fc7d47ba826c8d667c0d1e6e33a64a036ec44f58fa12d6c45',
    );
    // NIST SHA3-256("") would be a7ffc6f8bf1ed76651c14756a061d662f580ff4de43b49fa82d80a4b80f8434a.
    expect(hex(keccak256(new Uint8Array()))).not.toBe('a7ffc6f8bf1ed76651c14756a061d662f580ff4de43b49fa82d80a4b80f8434a');
  });

  it('does not modify its input and is deterministic', () => {
    const input = pattern(300);
    const copy = input.slice();
    const a = hex(keccak256(input));
    expect(hex(keccak256(input))).toBe(a);
    expect(input).toEqual(copy);
  });
});
