import { describe, it, expect } from "vitest";
import { collateralRatioE4, U64_MAX } from "../src/cr.js";

// CFX is 18 native decimals; debt is e8s; price is e8. These vectors are
// computed by hand against the Rust `collateral_ratio_e4` integer math.
const CFX_DECIMALS = 18;
const ONE_CFX = 10n ** 18n;

describe("collateralRatioE4", () => {
  it("returns U64_MAX for a debt-free vault", () => {
    expect(collateralRatioE4(ONE_CFX, CFX_DECIMALS, 15_000_000n, 0n)).toBe(U64_MAX);
  });

  it("computes 200.00% for 1 CFX @ $0.30 against $0.15 debt", () => {
    // collateral_usd_e8 = 1e18 * 30_000_000 / 1e18 = 30_000_000 ($0.30)
    // cr_e4 = 30_000_000 * 10_000 / 15_000_000 = 20_000
    expect(collateralRatioE4(ONE_CFX, CFX_DECIMALS, 30_000_000n, 15_000_000n)).toBe(20_000n);
  });

  it("computes 150.00% for 1.5 CFX @ $1.00 against $1.00 debt", () => {
    const collateral = 15n * 10n ** 17n; // 1.5e18
    // usd_e8 = 1.5e18 * 1e8 / 1e18 = 1.5e8 ; cr = 1.5e8 * 1e4 / 1e8 = 15_000
    expect(collateralRatioE4(collateral, CFX_DECIMALS, 100_000_000n, 100_000_000n)).toBe(15_000n);
  });

  it("truncates like Rust integer division (no rounding up)", () => {
    // 1 CFX @ $0.10 = usd_e8 10_000_000 ; debt $0.07 = 7_000_000
    // cr = 10_000_000 * 10_000 / 7_000_000 = 14_285 (14285.714... truncated)
    expect(collateralRatioE4(ONE_CFX, CFX_DECIMALS, 10_000_000n, 7_000_000n)).toBe(14_285n);
  });

  it("drops below the 130% MCR when the price falls", () => {
    // 1 CFX @ $0.12 = usd_e8 12_000_000 ; debt $0.10 = 10_000_000
    // cr = 12_000_000 * 10_000 / 10_000_000 = 12_000 (120.00% < 13000 MCR)
    const cr = collateralRatioE4(ONE_CFX, CFX_DECIMALS, 12_000_000n, 10_000_000n);
    expect(cr).toBe(12_000n);
    expect(cr < 13_000n).toBe(true);
  });
});
