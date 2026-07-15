import { describe, it, expect } from 'vitest';
import { computeBorrowMax, friendlyBorrowCapError } from './borrowLimits';

// Baseline vault far from any protocol cap: only the collateral CR bounds it.
const FAR = {
  collateralValueUsd: 1000,
  minCr: 1.25,
  currentDebt: 100,
  debtCeiling: 2000,
  aggregateDebt: 200,
  globalCap: 30000,
  globalBorrowed: 3000,
};

describe('computeBorrowMax', () => {
  it('is bounded by collateral capacity when far from every protocol cap', () => {
    const r = computeBorrowMax(FAR);
    // capacity = 1000 / 1.25 - 100 = 700, * 0.995 haircut = 696.5
    expect(r.binding).toBe('collateral');
    expect(r.maxBorrow).toBeCloseTo(696.5, 4);
    expect(r.collateralHeadroom).toBeCloseTo(696.5, 4);
  });

  it('is bounded by the per-collateral debt ceiling when it is the tightest cap (nICP case)', () => {
    // Collateral capacity ~= 228, but only ~66 icUSD of ceiling headroom remains.
    const r = computeBorrowMax({
      collateralValueUsd: 400,
      minCr: 1.25,
      currentDebt: 91.18,
      debtCeiling: 2000,
      aggregateDebt: 1933.76,
      globalCap: 30000,
      globalBorrowed: 3014.87,
    });
    expect(r.binding).toBe('debtCeiling');
    // ceiling headroom = 2000 - 1933.76 = 66.24 (no haircut on the ceiling)
    expect(r.ceilingHeadroom).toBeCloseTo(66.24, 4);
    expect(r.maxBorrow).toBeCloseTo(66.24, 4);
  });

  it('is bounded by the global mint cap when that is the tightest cap', () => {
    const r = computeBorrowMax({
      collateralValueUsd: 100000,
      minCr: 1.25,
      currentDebt: 0,
      debtCeiling: 1_000_000, // huge, not binding
      aggregateDebt: 0,
      globalCap: 30000,
      globalBorrowed: 29950,
    });
    expect(r.binding).toBe('globalCap');
    expect(r.globalHeadroom).toBeCloseTo(50, 6);
    expect(r.maxBorrow).toBeCloseTo(50, 6);
  });

  it('clamps ceiling headroom at zero when aggregate debt already exceeds the ceiling', () => {
    const r = computeBorrowMax({
      ...FAR,
      debtCeiling: 2000,
      aggregateDebt: 2100,
    });
    expect(r.binding).toBe('debtCeiling');
    expect(r.ceilingHeadroom).toBe(0);
    expect(r.maxBorrow).toBe(0);
  });

  it('ignores a non-positive debt ceiling or global cap (treated as no cap)', () => {
    const r = computeBorrowMax({
      ...FAR,
      debtCeiling: 0,
      globalCap: 0,
    });
    expect(r.binding).toBe('collateral');
    expect(r.ceilingHeadroom).toBeNull();
    expect(r.globalHeadroom).toBeNull();
    expect(r.maxBorrow).toBeCloseTo(696.5, 4);
  });

  it('prefers the collateral label on ties so a protocol cap is only flagged when strictly tighter', () => {
    // collateral capacity (no haircut here) exactly equals ceiling headroom.
    const r = computeBorrowMax({
      collateralValueUsd: 1000,
      minCr: 1,
      currentDebt: 900,
      debtCeiling: 500,
      aggregateDebt: 400, // ceiling headroom = 100
      globalCap: 30000,
      globalBorrowed: 0,
      haircut: 1,
    });
    // collateral headroom = (1000 - 900) * 1 = 100, ceiling headroom = 100 -> tie
    expect(r.maxBorrow).toBeCloseTo(100, 6);
    expect(r.binding).toBe('collateral');
  });

  it('never returns a negative max when the vault is already under-collateralized', () => {
    const r = computeBorrowMax({
      ...FAR,
      collateralValueUsd: 100,
      minCr: 1.5,
      currentDebt: 200, // capacity 66.6 - 200 < 0
    });
    expect(r.collateralHeadroom).toBe(0);
    expect(r.maxBorrow).toBe(0);
  });
});

describe('friendlyBorrowCapError', () => {
  it('translates the debt-ceiling guard rejection into a friendly message with remaining headroom', () => {
    const raw =
      'Borrow would exceed debt ceiling incl in-flight (193368693855 + 0 + 6900000000 > 200000000000)';
    const msg = friendlyBorrowCapError(raw);
    expect(msg).not.toBeNull();
    expect(msg).toContain('debt ceiling');
    // remaining = (200000000000 - 193368693855 - 0) / 1e8 = 66.313...
    expect(msg).toContain('66.31');
    // The raw e8s integers must not leak through.
    expect(msg).not.toContain('200000000000');
  });

  it('translates the global mint-cap guard rejection into a friendly message with remaining headroom', () => {
    const raw =
      'Borrow would exceed global icUSD mint cap incl in-flight (500000000000 + 0 + 100000000000 > 550000000000)';
    const msg = friendlyBorrowCapError(raw);
    expect(msg).not.toBeNull();
    expect(msg).toContain('global icUSD mint cap');
    // remaining = (550000000000 - 500000000000 - 0) / 1e8 = 500
    expect(msg).toContain('500');
  });

  it('accounts for in-flight reservations when computing remaining headroom', () => {
    const raw =
      'Borrow would exceed debt ceiling incl in-flight (100000000000 + 50000000000 + 60000000000 > 200000000000)';
    const msg = friendlyBorrowCapError(raw);
    // remaining = (200000000000 - 100000000000 - 50000000000) / 1e8 = 500
    expect(msg).toContain('500');
  });

  it('returns null for unrelated error strings', () => {
    expect(friendlyBorrowCapError('Vault not found. Please check the vault ID.')).toBeNull();
    expect(friendlyBorrowCapError('Borrowing is not allowed for this collateral type.')).toBeNull();
  });
});
