import { describe, it, expect } from "vitest";
import { aggregate, median } from "../src/aggregate.js";
import type { PriceQuote } from "../src/types.js";

const cfg = { minSources: 2, outlierPct: 5, maxSpreadPct: 3 };
const q = (source: string, priceUsd: number): PriceQuote => ({ source, priceUsd, ts: 1000 });

describe("median", () => {
  it("odd count returns the middle", () => {
    expect(median([0.1, 0.3, 0.2])).toBe(0.2);
  });
  it("even count averages the two middles", () => {
    expect(median([0.1, 0.2, 0.3, 0.4])).toBeCloseTo(0.25, 10);
  });
});

describe("aggregate", () => {
  it("returns the median of three in-band sources", () => {
    const out = aggregate([q("a", 0.150), q("b", 0.151), q("c", 0.149)], cfg);
    expect(out.ok).toBe(true);
    if (out.ok) {
      expect(out.result.medianUsd).toBeCloseTo(0.150, 10);
      expect(out.result.used.sort()).toEqual(["a", "b", "c"]);
      expect(out.result.rejected).toEqual([]);
    }
  });

  it("rejects a single outlier and re-medians the survivors", () => {
    // median of [0.15,0.151,0.30] = 0.151; 0.30 is ~99% off -> rejected.
    const out = aggregate([q("a", 0.150), q("b", 0.151), q("bad", 0.30)], cfg);
    expect(out.ok).toBe(true);
    if (out.ok) {
      expect(out.result.used.sort()).toEqual(["a", "b"]);
      expect(out.result.rejected.map((r) => r.source)).toEqual(["bad"]);
      expect(out.result.medianUsd).toBeCloseTo(0.1505, 10);
    }
  });

  it("refuses when fewer than minSources valid quotes exist", () => {
    const out = aggregate([q("only", 0.15)], cfg);
    expect(out.ok).toBe(false);
    if (!out.ok) expect(out.reason).toMatch(/source/i);
  });

  it("refuses when two sources disagree wildly (both outliers)", () => {
    // median of [0.10,0.20]=0.15; both are 33% off -> both rejected -> 0 survive.
    const out = aggregate([q("a", 0.10), q("b", 0.20)], cfg);
    expect(out.ok).toBe(false);
  });

  it("refuses when two in-band sources still spread wider than maxSpreadPct", () => {
    // 0.150 vs 0.156: median 0.153, each ~1.96% off (within 5% outlier so both
    // survive), but spread (0.156-0.150)/0.153 = 3.92% > 3% -> refuse.
    const out = aggregate([q("a", 0.15), q("b", 0.156)], cfg);
    expect(out.ok).toBe(false);
    if (!out.ok) expect(out.reason).toMatch(/spread/i);
  });

  it("ignores zero, negative and NaN prices", () => {
    const out = aggregate(
      [q("a", 0.150), q("b", 0.151), q("z", 0), q("neg", -1), q("nan", NaN)],
      cfg,
    );
    expect(out.ok).toBe(true);
    if (out.ok) {
      expect(out.result.used.sort()).toEqual(["a", "b"]);
    }
  });
});
