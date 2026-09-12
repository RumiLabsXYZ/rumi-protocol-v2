import { describe, expect, it } from 'vitest';
import { computeSafeIcusdRepayMax } from './repayLimits';

describe('computeSafeIcusdRepayMax', () => {
  it('caps a 2 icUSD debt below the old 1.995 Max boundary', () => {
    const max = computeSafeIcusdRepayMax(199_600_000n, 200_000_000n, 100_000n, 10_000_000n);

    expect(max).toBe(190_000_000n); // 1.9 icUSD, leaving the 0.1 icUSD minimum debt
    expect(199_500_000n).toBeGreaterThan(max); // old displayed Max: 1.995
  });

  it('does not advertise a near-full repayment that the backend upgrades to full debt', () => {
    const debt = 200_000_259n; // 2 icUSD plus accrued interest from vault #205
    const balance = 199_600_000n; // 1.996 icUSD, observed on the live ledger
    const fee = 100_000n; // live icUSD icrc1_fee()

    expect(computeSafeIcusdRepayMax(balance, debt, fee, 10_000_000n)).toBe(190_000_259n);
  });

  it('allows the exact debt only when balance also covers the transfer fee', () => {
    expect(computeSafeIcusdRepayMax(200_100_000n, 200_000_000n, 100_000n, 10_000_000n)).toBe(200_000_000n);
  });

  it('returns zero when the balance cannot cover the transfer fee', () => {
    expect(computeSafeIcusdRepayMax(100_000n, 200_000_000n, 100_000n, 10_000_000n)).toBe(0n);
  });

  it('leaves the exact minimum debt when it is larger than the snap threshold', () => {
    const max = computeSafeIcusdRepayMax(199_600_000n, 200_000_259n, 100_000n, 10_000_000n);

    expect(max).toBe(190_000_259n);
    expect(200_000_259n - max).toBe(10_000_000n);
  });
});
