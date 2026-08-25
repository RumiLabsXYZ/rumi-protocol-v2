import { describe, expect, it } from "vitest";
import publicPolicy from "../.ic-assets.production-public.json";

describe("production-public asset policy", () => {
  it("disables raw access for every production-public asset rule", () => {
    expect(publicPolicy.length).toBeGreaterThan(0);
    expect(publicPolicy.every((rule) => rule.allow_raw_access === false)).toBe(true);
  });

  it("enables SPA aliasing without permitting the testnet RPC", () => {
    expect(publicPolicy.some((rule) => rule.match === "**/*" && rule.enable_aliasing === true)).toBe(true);
    expect(JSON.stringify(publicPolicy)).toContain("https://evm.confluxrpc.com");
    expect(JSON.stringify(publicPolicy)).not.toContain("evmtestnet.confluxrpc.com");
  });
});
