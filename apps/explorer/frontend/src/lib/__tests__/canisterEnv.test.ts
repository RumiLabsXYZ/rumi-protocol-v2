import { describe, expect, it, vi, beforeEach } from "vitest";

describe("getCanisterEnv", () => {
  beforeEach(() => {
    vi.resetModules();
  });

  it("throws when explorer_bff canister id is not in cookie", async () => {
    vi.doMock("@icp-sdk/core/agent/canister-env", () => ({
      safeGetCanisterEnv: () => undefined,
    }));
    const mod = await import("../canisterEnv");
    expect(() => mod.getCanisterEnv()).toThrow(/explorer_bff canister ID not found/);
  });

  it("returns isLocal=true when hostname is localhost", async () => {
    vi.doMock("@icp-sdk/core/agent/canister-env", () => ({
      safeGetCanisterEnv: () => ({ "PUBLIC_CANISTER_ID:explorer_bff": "rrkah-fqaaa-aaaaa-aaaaq-cai" }),
    }));
    Object.defineProperty(window, "location", {
      value: { hostname: "localhost" },
      writable: true,
    });
    const mod = await import("../canisterEnv");
    const env = mod.getCanisterEnv();
    expect(env.explorerBffId).toBe("rrkah-fqaaa-aaaaa-aaaaq-cai");
    expect(env.isLocal).toBe(true);
  });
});
