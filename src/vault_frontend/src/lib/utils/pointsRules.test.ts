import { describe, it, expect } from 'vitest';
import {
  compute3poolMultiplier,
  threePoolHeadline,
  spMultiplier,
  depositMultiplierLabel,
  formatSharePct,
} from './pointsRules';

describe('compute3poolMultiplier — mirrors accrual.rs snapshot_weights', () => {
  it('equal ckUSDC/ckUSDT pair is fully matched at 5x', () => {
    const m = compute3poolMultiplier({ ckusdc: 100, ckusdt: 100 });
    expect(m.matchedUsd).toBe(200); // 2*min(100,100)
    expect(m.unmatchedUsd).toBe(0);
    expect(m.headline).toBe(5);
    expect(m.effective).toBe(5);
  });

  it('matched portion accrues 5x, remainder accrues 3x', () => {
    // 100 ckUSDC + 40 ckUSDT: matched = 2*40 = 80 @5x; unmatched = 60 @3x
    const m = compute3poolMultiplier({ ckusdc: 100, ckusdt: 40 });
    expect(m.matchedUsd).toBe(80);
    expect(m.unmatchedUsd).toBe(60);
    expect(m.headline).toBe(5);
    // weighted = 80*5 + 60*3 = 580; deposited = 140 → 4.14 → 4.1
    expect(m.effective).toBe(4.1);
  });

  it('dust pairing does NOT flip the whole position to 5x blended', () => {
    // 50000 ckUSDC + 1 ckUSDT: only $2 is matched; the rest stays at 3x.
    const m = compute3poolMultiplier({ ckusdc: 50000, ckusdt: 1 });
    expect(m.matchedUsd).toBe(2);
    expect(m.unmatchedUsd).toBe(49999);
    // blended must be ~3x, nowhere near 5x
    expect(m.effective).toBeLessThan(3.1);
    expect(m.effective).toBeGreaterThan(2.9);
    // the form headline must not scream a flat 5x
    expect(threePoolHeadline(m)).not.toContain('Earning 5× — matched');
  });

  it('single-sided ckUSDC earns 3x with a nudge to reach 5x', () => {
    const m = compute3poolMultiplier({ ckusdc: 100 });
    expect(m.matchedUsd).toBe(0);
    expect(m.headline).toBe(3);
    expect(m.effective).toBe(3);
    expect(m.nudge).toBe('Add ckUSDT to reach 5×');
  });

  it('icUSD-only liquidity earns 1x', () => {
    const m = compute3poolMultiplier({ icusd: 100 });
    expect(m.headline).toBe(1);
    expect(m.effective).toBe(1);
    expect(threePoolHeadline(m)).toContain('1×');
  });

  it('empty input earns nothing', () => {
    const m = compute3poolMultiplier({});
    expect(m.headline).toBe(0);
    expect(m.effective).toBe(0);
  });

  it('clamps negative inputs to zero', () => {
    const m = compute3poolMultiplier({ ckusdc: -50, ckusdt: 50 });
    expect(m.matchedUsd).toBe(0);
    expect(m.unmatchedUsd).toBe(50);
    expect(m.headline).toBe(3);
  });
});

describe('spMultiplier', () => {
  it('3USD earns 2x', () => {
    expect(spMultiplier('3USD')).toBe(2);
    expect(spMultiplier('threeusd')).toBe(2);
  });
  it('icUSD and others earn 1x', () => {
    expect(spMultiplier('icUSD')).toBe(1);
    expect(spMultiplier('ckUSDC')).toBe(1);
    expect(spMultiplier(null)).toBe(1);
  });
});

describe('depositMultiplierLabel', () => {
  it('labels per venue/asset', () => {
    expect(depositMultiplierLabel('threePool', 'ckUSDC')).toBe('3–5×');
    expect(depositMultiplierLabel('threePool', 'icUSD')).toBe('1×');
    expect(depositMultiplierLabel('stabilityPool', '3USD')).toBe('2×');
    expect(depositMultiplierLabel('stabilityPool', 'icUSD')).toBe('1×');
    expect(depositMultiplierLabel('amm', 'ICP')).toBe('2×');
    expect(depositMultiplierLabel('vault', 'icUSD')).toBe('1×');
  });
});

describe('formatSharePct', () => {
  it('converts bps of the pool to a percentage', () => {
    expect(formatSharePct(250)).toBe('2.50%');
    expect(formatSharePct(10000)).toBe('100.00%');
  });
  it('returns a dash for non-positive/non-finite', () => {
    expect(formatSharePct(0)).toBe('—');
    expect(formatSharePct(NaN)).toBe('—');
  });
});
