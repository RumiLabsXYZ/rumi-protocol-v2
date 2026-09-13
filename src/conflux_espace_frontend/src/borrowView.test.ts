import { describe, expect, it } from "vitest";
import { trackGradient } from "./borrowView";

describe("trackGradient", () => {
  it("renders a continuous gradient for known thresholds regardless of tone", () => {
    expect(trackGradient(20, 80)).toBe(
      "linear-gradient(90deg, var(--workspace-pink) 0%, var(--workspace-violet) 20%, var(--workspace-teal) 80%)"
    );
  });

  it("falls back to a neutral scale when liquidation is null", () => {
    expect(trackGradient(null, 80)).toBe("rgba(160, 155, 181, .28)");
  });

  it("falls back to a neutral scale when safe is null", () => {
    expect(trackGradient(20, null)).toBe("rgba(160, 155, 181, .28)");
  });

  it("falls back to a neutral scale when both thresholds are null", () => {
    expect(trackGradient(null, null)).toBe("rgba(160, 155, 181, .28)");
  });

  it("falls back to a neutral scale when either threshold is nonfinite", () => {
    expect(trackGradient(NaN, 80)).toBe("rgba(160, 155, 181, .28)");
    expect(trackGradient(20, Infinity)).toBe("rgba(160, 155, 181, .28)");
  });

  it("clamps out-of-range positions into 0-100", () => {
    expect(trackGradient(-20, 140)).toBe(
      "linear-gradient(90deg, var(--workspace-pink) 0%, var(--workspace-violet) 0%, var(--workspace-teal) 100%)"
    );
  });

  it("prevents reversed stops when liquidation is above safe", () => {
    expect(trackGradient(80, 20)).toBe(
      "linear-gradient(90deg, var(--workspace-pink) 0%, var(--workspace-violet) 20%, var(--workspace-teal) 80%)"
    );
  });
});
