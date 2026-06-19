import { describe, it, expect } from "vitest";
import { loadConfig } from "../src/config.js";

const required = { CANISTER_ID: "tfesu-vyaaa-aaaap-qrd7a-cai", IDENTITY_PEM: "/keys/pusher.pem" };

describe("loadConfig", () => {
  it("applies the runbook default thresholds", () => {
    const c = loadConfig(required);
    expect(c.thresholds).toEqual({
      driftBps: 200,
      maxAgeSec: 300,
      crWarnBandE4: 16_000n,
      mcrE4: 13_000n,
      outlierPct: 5,
      minSources: 2,
      pollSec: 60,
      downtimeIntervals: 3,
      nativeDecimals: 18,
    });
  });

  it("defaults the target to CFX on chain 1030", () => {
    const c = loadConfig(required);
    expect(c.target).toEqual({ chainId: 1030, symbol: "CFX", coinGeckoId: "conflux-token" });
  });

  it("defaults to the ic network + mainnet host without fetchRootKey", () => {
    const c = loadConfig(required);
    expect(c.network).toBe("ic");
    expect(c.host).toMatch(/icp-api\.io|ic0\.app/);
    expect(c.fetchRootKey).toBe(false);
  });

  it("derives a local host and fetchRootKey when network=local", () => {
    const c = loadConfig({ ...required, IC_NETWORK: "local" });
    expect(c.network).toBe("local");
    expect(c.host).toContain("127.0.0.1");
    expect(c.fetchRootKey).toBe(true);
  });

  it("parses overrides from the environment", () => {
    const c = loadConfig({
      ...required,
      DRIFT_BPS: "100",
      MAX_AGE_SEC: "180",
      CR_WARN_BAND_E4: "18000",
      POLL_SEC: "30",
      CHAIN_ID: "71",
      SYMBOL: "CFX",
      SLACK_WEBHOOK_URL: "https://hooks.slack.test/x",
    });
    expect(c.thresholds.driftBps).toBe(100);
    expect(c.thresholds.maxAgeSec).toBe(180);
    expect(c.thresholds.crWarnBandE4).toBe(18_000n);
    expect(c.thresholds.pollSec).toBe(30);
    expect(c.target.chainId).toBe(71);
    expect(c.slackWebhookUrl).toBe("https://hooks.slack.test/x");
  });

  it("throws when CANISTER_ID is missing", () => {
    expect(() => loadConfig({ IDENTITY_PEM: "/k.pem" })).toThrow(/CANISTER_ID/);
  });

  it("throws when IDENTITY_PEM is missing", () => {
    expect(() => loadConfig({ CANISTER_ID: "x" })).toThrow(/IDENTITY_PEM/);
  });
});
