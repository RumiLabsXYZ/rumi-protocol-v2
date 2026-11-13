import { describe, expect, it } from 'vitest';
import { computeSafeIcusdRepayMax, repaymentPreview, normalizedRepaymentE8s } from './repayLimits';

describe('repayment authorization and settlement estimates', () => {
  it('uses the spendable balance without increasing a near-full payment', () => {
    const max = computeSafeIcusdRepayMax(199_600_000n, 200_000_259n, 100_000n);
    expect(max).toBe(199_500_000n);
    expect(repaymentPreview(200_000_259n, max)).toEqual({
      paidE8s: 199_500_000n, forgivenE8s: 500_259n, remainingE8s: 0n,
    });
  });

  it('caps payment at debt and reserves one approval fee', () => {
    expect(computeSafeIcusdRepayMax(200_100_000n, 200_000_000n, 100_000n)).toBe(200_000_000n);
    expect(computeSafeIcusdRepayMax(100_000n, 200_000_000n, 100_000n)).toBe(0n);
    expect(computeSafeIcusdRepayMax(500_000n, 0n, 100_000n)).toBe(0n);
  });

  it('forgives strictly below 0.1, retaining exactly 0.1 and above', () => {
    expect(repaymentPreview(109_999_999n, 100_000_000n).forgivenE8s).toBe(9_999_999n);
    expect(repaymentPreview(110_000_000n, 100_000_000n)).toEqual({
      paidE8s: 100_000_000n, forgivenE8s: 0n, remainingE8s: 10_000_000n,
    });
    expect(repaymentPreview(110_000_001n, 100_000_000n).remainingE8s).toBe(10_000_001n);
  });

  it('does not project forgiveness without a positive payment', () => {
    expect(repaymentPreview(1n, 0n)).toEqual({ paidE8s: 0n, forgivenE8s: 0n, remainingE8s: 1n });
    expect(repaymentPreview(1n, -1n).remainingE8s).toBe(1n);
    expect(repaymentPreview(1n, 10n)).toEqual({ paidE8s: 1n, forgivenE8s: 0n, remainingE8s: 0n });
  });
});

describe('claim repayment maximum', () => {
  it('includes a bounded cushion without exceeding spendable funds', async () => {
    const { claimRepayMaximum } = await import('./repayLimits');
    expect(claimRepayMaximum(0.05, 0.1, 0.05)).toBeCloseTo(0.06);
    expect(claimRepayMaximum(0.05, 0.055, 0.05)).toBe(0.055);
    expect(claimRepayMaximum(0.05, 0.03, 0.02)).toBe(0.02);
    expect(claimRepayMaximum(0, 1, 0)).toBe(0);
  });
});

describe('stable repayment precision', () => {
  it('retains the exact threshold after stable truncation, unlike icUSD', () => {
    expect(repaymentPreview(110_000_000n, normalizedRepaymentE8s(100_000_001n, true)).remainingE8s).toBe(10_000_000n);
    expect(repaymentPreview(110_000_000n, normalizedRepaymentE8s(100_000_001n)).forgivenE8s).toBe(9_999_999n);
    expect(normalizedRepaymentE8s(5_000_001n, true)).toBeLessThan(5_000_001n);
  });
});
