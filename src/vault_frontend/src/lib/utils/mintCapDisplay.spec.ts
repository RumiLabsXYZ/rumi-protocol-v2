import { describe, expect, it } from 'vitest';
import { formatGlobalMintCap } from './mintCapDisplay';

describe('global mint cap display', () => {
  it.each([
    [null, 'Unavailable'],
    [18_446_744_073_709_551_615n, 'Unlimited'],
    [1n, '0.00000001 icUSD'],
    [0n, '0 icUSD'],
    [100_000_000n, '1 icUSD'],
    [1_000_000_000_000_000n, '10,000,000 icUSD'],
    [123_456_789n, '1.23456789 icUSD'],
    [18_446_744_073_709_551_614n, '184,467,440,737.09551614 icUSD'],
  ])('formats %s as %s', (cap, expected) => {
    expect(formatGlobalMintCap(cap)).toBe(expected);
  });
});
