import { describe, expect, it } from "vitest";
import { amountUnits, positionPreview, groupComposition } from "./borrowMath";

describe("two-field borrow amounts", () => {
  it("preserves exact 18-decimal collateral and 8-decimal debt", () => {
    expect(amountUnits("1.000000000000000001", 18)).toBe(1000000000000000001n);
    expect(amountUnits("200.00000001", 8)).toBe(20000000001n);
  });
  it("rejects ambiguous, rounded, negative and out-of-range amounts", () => {
    for (const value of ["", "-1", "1e3", "Infinity", "NaN", "1,000", "1.000000001", "1x", (2n ** 128n).toString()]) {
      expect(amountUnits(value, 8)).toBeNull();
    }
    expect(amountUnits("0", 18)).toBe(0n);
  });
  it("places 250 percent at 75 percent of the meter, independently of input formatting", () => {
    const p = positionPreview(10000n * 10n ** 18n, 20000000000n, 5000000n, 15000n, 13300n);
    expect(p?.ratioPercent).toBe(250);
    expect(p?.position).toBe(75);
    expect(p?.minPosition).toBe(25);
    expect(p?.liquidationPosition).toBe(16.5);
    expect(p?.liquidationPrice).toBe(0.0266);
    expect(p?.tone).toBe("safe");
    expect(p?.belowMinimum).toBe(false);
  });
  it("never presents missing price or empty debt as a healthy position", () => {
    expect(positionPreview(1n, 1n, null, 15000n, 13300n)).toBeNull();
    expect(positionPreview(1n, 0n, 5000000n, 15000n, 13300n)).toBeNull();
    expect(positionPreview(0n, 1n, 5000000n, 15000n, 13300n)).toBeNull();
  });
  it("keeps the actual low ratio while clamping only the visual marker", () => {
    const p = positionPreview(100n * 10n ** 18n, 1000000000n, 5000000n, 15000n, 13300n);
    expect(p?.ratioPercent).toBe(50);
    expect(p?.position).toBe(0);
    expect(p?.belowMinimum).toBe(true);
    expect(p?.tone).toBe("danger");
  });
});

describe("protocol collateral composition", () => {
  it("groups USD values without pretending different ICP tokens are one asset", () => {
    const rows = groupComposition([
      { symbol: "ICP", amount: 2, usd: 6 },
      { symbol: "nICP", amount: 3, usd: 12 },
      { symbol: "BOB", amount: 50, usd: 4 },
      { symbol: "EXE", amount: 10, usd: 1 },
      { symbol: "ckBTC", amount: 0.001, usd: 100 },
      { symbol: "ckXAUT", amount: 1, usd: 3000 },
    ]);
    expect(rows[0].label).toBe("ICP ecosystem");
    expect(rows[0].value).toBe("$23.00");
    expect(rows[0].details).toHaveLength(4);
    expect(rows.find(r => r.label === "BTC")?.details[0]).toContain("ckBTC");
    expect(rows.find(r => r.label === "XAUT")?.details[0]).toContain("ckXAUT");
  });
  it("does not strip prefixes from arbitrary unknown symbols", () => {
    expect(groupComposition([{ symbol: "ckUNKNOWN", amount: 1, usd: 1 }])[0].label).toBe("ckUNKNOWN");
  });
});
