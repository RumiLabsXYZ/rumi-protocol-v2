import { describe, it, expect } from "vitest";
import { decide, type PolicyConfig, type PolicyInput } from "../src/policy.js";
import type { ChainVault, OnChainPrice } from "../src/types.js";

const cfg: PolicyConfig = {
  driftBps: 200,
  maxAgeSec: 300,
  crWarnBandE4: 16_000n,
  mcrE4: 13_000n,
  nativeDecimals: 18,
};

const NOW = 1_000_000_000_000_000_000n; // arbitrary ns
const SEC = 1_000_000_000n;
const onChain = (priceE8: bigint, ageSec: bigint): OnChainPrice => ({
  priceE8,
  setAtNs: NOW - ageSec * SEC,
});
const ONE_CFX = 10n ** 18n;
const vault = (vaultId: bigint, debtE8s: bigint): ChainVault => ({
  vaultId,
  collateralAmountE18: ONE_CFX,
  debtE8s,
});

const base = (over: Partial<PolicyInput>): PolicyInput => ({
  marketE8: 15_000_000n,
  onChain: onChain(15_000_000n, 60n),
  nowNs: NOW,
  vaults: [],
  ...over,
});

describe("decide — refresh logic", () => {
  it("refreshes when no on-chain price is set", () => {
    const d = decide(base({ onChain: null }), cfg);
    expect(d.shouldRefresh).toBe(true);
    expect(d.refreshReason).toMatch(/no on-chain price/i);
  });

  it("refreshes when drift exceeds the band", () => {
    // 15_400_000 vs 15_000_000 = ~266 bps > 200
    const d = decide(base({ marketE8: 15_400_000n }), cfg);
    expect(d.shouldRefresh).toBe(true);
    expect(d.refreshReason).toMatch(/drift/i);
  });

  it("does NOT refresh when drift is within band and price is fresh", () => {
    // 15_100_000 vs 15_000_000 = ~66 bps < 200, age 60s < 300s
    const d = decide(base({ marketE8: 15_100_000n }), cfg);
    expect(d.shouldRefresh).toBe(false);
    expect(d.refreshReason).toBeNull();
  });

  it("refreshes when the price is stale even with no drift", () => {
    const d = decide(base({ onChain: onChain(15_000_000n, 600n) }), cfg);
    expect(d.shouldRefresh).toBe(true);
    expect(d.refreshReason).toMatch(/stale|age/i);
  });

  it("treats a pre-V5 price (set_at_ns = 0) as stale", () => {
    const d = decide(base({ onChain: { priceE8: 15_000_000n, setAtNs: 0n } }), cfg);
    expect(d.shouldRefresh).toBe(true);
    expect(d.refreshReason).toMatch(/stale|age/i);
  });

  it("refreshes when the on-chain price is zero", () => {
    const d = decide(base({ onChain: { priceE8: 0n, setAtNs: NOW } }), cfg);
    expect(d.shouldRefresh).toBe(true);
  });
});

describe("decide — CR alerts (evaluated at the market price)", () => {
  // Fresh, no drift -> refresh isolated out; market = $0.30 for clean CR math.
  const stable = (vaults: ChainVault[]): PolicyInput =>
    base({ marketE8: 30_000_000n, onChain: onChain(30_000_000n, 60n), vaults });

  it("no alert for a healthy vault (CR above warn band)", () => {
    // 1 CFX @ $0.30, debt $0.15 -> CR 200% > 160%
    const d = decide(stable([vault(1n, 15_000_000n)]), cfg);
    expect(d.alerts).toEqual([]);
  });

  it("warns when a vault is below the warn band but above MCR", () => {
    // debt $0.19 -> CR 15789 (between 13000 and 16000)
    const d = decide(stable([vault(2n, 19_000_000n)]), cfg);
    expect(d.alerts).toHaveLength(1);
    expect(d.alerts[0]!.level).toBe("warn");
    expect(d.alerts[0]!.code).toBe("vault_below_band");
    expect(d.alerts[0]!.context?.vaultId).toBe("2");
  });

  it("criticals when a vault is below MCR (under-collateralized)", () => {
    // debt $0.25 -> CR 12000 < 13000
    const d = decide(stable([vault(3n, 25_000_000n)]), cfg);
    expect(d.alerts).toHaveLength(1);
    expect(d.alerts[0]!.level).toBe("critical");
  });

  it("ignores debt-free vaults", () => {
    const d = decide(stable([vault(4n, 0n)]), cfg);
    expect(d.alerts).toEqual([]);
  });

  it("evaluates CR at the MARKET price, not the (stale) on-chain price", () => {
    // On-chain still says $0.30 (healthy), but market has crashed to $0.12.
    // 1 CFX @ $0.12, debt $0.10 -> CR 12000 < MCR -> critical, despite on-chain.
    const d = decide(
      base({ marketE8: 12_000_000n, onChain: onChain(30_000_000n, 60n), vaults: [vault(5n, 10_000_000n)] }),
      cfg,
    );
    const crit = d.alerts.find((a) => a.code === "vault_below_band");
    expect(crit?.level).toBe("critical");
  });

  it("emits one alert per breaching vault and skips healthy ones", () => {
    const d = decide(stable([vault(1n, 15_000_000n), vault(2n, 19_000_000n), vault(3n, 25_000_000n)]), cfg);
    const ids = d.alerts.map((a) => a.context?.vaultId).sort();
    expect(ids).toEqual(["2", "3"]);
  });
});
