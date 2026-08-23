import { describe, expect, it } from "vitest";
import publicPolicy from "../.ic-assets.production-public.json";

describe("production-public asset policy", () => {
  it("disables raw access for every production-public asset rule", () => {
    expect(publicPolicy.length).toBeGreaterThan(0);
    expect(publicPolicy.every((rule) => rule.allow_raw_access === false)).toBe(true);
  });
});
