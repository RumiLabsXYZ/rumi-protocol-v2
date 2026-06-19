import { describe, it, expect } from "vitest";
import { withTimeout } from "../src/util.js";

const never = new Promise<never>(() => {});

describe("withTimeout", () => {
  it("resolves with the value when the promise settles in time", async () => {
    await expect(withTimeout(Promise.resolve(42), 1000, "fast")).resolves.toBe(42);
  });

  it("rejects with a timeout error when the promise hangs", async () => {
    await expect(withTimeout(never, 20, "hung-call")).rejects.toThrow(/hung-call timed out after 20ms/);
  });

  it("propagates the underlying rejection", async () => {
    await expect(withTimeout(Promise.reject(new Error("boom")), 1000, "x")).rejects.toThrow("boom");
  });
});
