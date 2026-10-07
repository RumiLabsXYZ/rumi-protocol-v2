import { describe, expect, it } from 'vitest';
import {
  evaluateThreePoolDepositPolicy,
  THREE_POOL_DEFAULT_DECIMALS,
} from './depositPolicy';

const e8 = (value: bigint) => value * 100_000_000n;
const e6 = (value: bigint) => value * 1_000_000n;
const decimals = THREE_POOL_DEFAULT_DECIMALS;

describe('evaluateThreePoolDepositPolicy', () => {
  it('normalizes six-decimal stablecoins before applying the boundary', () => {
    const result = evaluateThreePoolDepositPolicy(
      [e8(666n), e6(167n), e6(167n)],
      [e8(1n), e6(1n), e6(1n)],
      decimals,
    );
    expect(result.allowed).toBe(true);
    expect(result.reason).toBe('allowed');
  });

  it('accepts a deposit exactly at the 666/1000 boundary', () => {
    const result = evaluateThreePoolDepositPolicy(
      [e8(666n), e6(334n), 0n],
      [e8(666n), e6(334n), 0n],
      decimals,
    );
    expect(result.allowed).toBe(true);
  });

  it('rejects a below-cap deposit that crosses the cap and reports missing stable value', () => {
    const result = evaluateThreePoolDepositPolicy(
      [e8(600n), e6(310n), 0n],
      [e8(100n), 0n, 0n],
      decimals,
    );
    expect(result.allowed).toBe(false);
    expect(result.reason).toBe('cap_exceeded');
    expect(result.missingStableNormalized).toBe(4_105_105_106n);
  });

  it('requires enough stable value for an icUSD deposit when already above cap', () => {
    const result = evaluateThreePoolDepositPolicy(
      [e8(700n), e6(150n), e6(150n)],
      [e8(100n), e6(50n), 0n],
      decimals,
    );
    expect(result.allowed).toBe(false);
    expect(result.reason).toBe('overcap_icusd_requires_stables');
    expect(result.missingStableNormalized).toBe(15_015_016n);
  });

  it('rejects a rounded 200 + 100 corrective pair and reports the exact remainder', () => {
    const result = evaluateThreePoolDepositPolicy(
      [e8(700n), e6(150n), e6(150n)],
      [e8(200n), e6(100n), 0n],
      decimals,
    );
    expect(result.allowed).toBe(false);
    expect(result.missingStableNormalized).toBe(30_030_031n);
  });

  it('accepts an over-cap corrective paired deposit', () => {
    const result = evaluateThreePoolDepositPolicy(
      [e8(700n), e6(150n), e6(150n)],
      [e8(100n), e6(50n), e6(50n)],
      decimals,
    );
    expect(result.allowed).toBe(true);
  });

  it('accepts a stable-only corrective deposit above the cap', () => {
    const result = evaluateThreePoolDepositPolicy(
      [e8(700n), e6(150n), e6(150n)],
      [0n, e6(1n), 0n],
      decimals,
    );
    expect(result.allowed).toBe(true);
  });

  it('accepts either stablecoin as the paired amount', () => {
    const result = evaluateThreePoolDepositPolicy(
      [e8(700n), e6(150n), e6(150n)],
      [e8(100n), 0n, e6(100n)],
      decimals,
    );
    expect(result.allowed).toBe(true);
  });

  it('requires the first deposit to obey the cap', () => {
    const result = evaluateThreePoolDepositPolicy(
      [0n, 0n, 0n],
      [e8(100n), 0n, 0n],
      decimals,
    );
    expect(result.allowed).toBe(false);
    expect(result.reason).toBe('cap_exceeded');
    expect(result.missingStableNormalized).toBe(5_015_015_016n);
  });

  it('returns no_amount for an empty proposal', () => {
    const result = evaluateThreePoolDepositPolicy([0n, 0n, 0n], [0n, 0n, 0n], decimals);
    expect(result.allowed).toBe(false);
    expect(result.reason).toBe('no_amount');
  });
});
