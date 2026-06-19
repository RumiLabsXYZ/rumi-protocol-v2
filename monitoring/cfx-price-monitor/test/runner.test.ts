import { describe, it, expect, vi } from "vitest";
import { runTick, usdToE8, shouldAlertDowntime, gatherQuotes, type RunTickDeps } from "../src/runner.js";
import type { CanisterClient } from "../src/canister.js";
import type { AlertSink } from "../src/alerts.js";
import type { PriceSource } from "../src/sources/index.js";
import type { ChainVault, OnChainPrice, PriceQuote } from "../src/types.js";

const NOW_MS = 1_700_000_000_000;
const NOW_NS = BigInt(NOW_MS) * 1_000_000n;

const aggregateCfg = { minSources: 2, outlierPct: 5 };
const policyCfg = {
  driftBps: 200,
  maxAgeSec: 300,
  crWarnBandE4: 16_000n,
  mcrE4: 13_000n,
  nativeDecimals: 18,
};

function source(name: string, price: number | Error): PriceSource {
  return {
    name,
    fetchCfxUsd: vi.fn(async () => {
      if (price instanceof Error) throw price;
      return { source: name, priceUsd: price, ts: NOW_MS } as PriceQuote;
    }),
  };
}

function fakeClient(over: Partial<CanisterClient>): CanisterClient {
  return {
    getOnChainPrice: vi.fn(async () => null),
    setPrice: vi.fn(async () => ({ ok: true })),
    listChainVaults: vi.fn(async () => [] as ChainVault[]),
    getPricePusher: vi.fn(async () => null),
    ...over,
  };
}

function fakeSink(): AlertSink & { emit: ReturnType<typeof vi.fn> } {
  return { emit: vi.fn(async () => {}) };
}

function deps(over: Partial<RunTickDeps>): RunTickDeps {
  return {
    chainId: 1030,
    symbol: "CFX",
    sources: [source("a", 0.149), source("b", 0.15), source("c", 0.151)], // median 0.150
    aggregateCfg,
    policyCfg,
    client: fakeClient({}),
    alertSink: fakeSink(),
    nowNs: NOW_NS,
    ...over,
  };
}

const onChain = (priceE8: bigint, ageSec: bigint): OnChainPrice => ({
  priceE8,
  setAtNs: NOW_NS - ageSec * 1_000_000_000n,
});

describe("usdToE8", () => {
  it("converts a USD float to e8", () => {
    expect(usdToE8(0.15)).toBe(15_000_000n);
    expect(usdToE8(0.04915384)).toBe(4_915_384n);
  });
});

describe("shouldAlertDowntime", () => {
  it("alerts when no success within N intervals", () => {
    expect(shouldAlertDowntime(NOW_MS - 200_000, NOW_MS, 60_000, 3)).toBe(true); // 200s > 180s
  });
  it("does not alert within the window", () => {
    expect(shouldAlertDowntime(NOW_MS - 120_000, NOW_MS, 60_000, 3)).toBe(false); // 120s < 180s
  });
});

describe("gatherQuotes", () => {
  it("returns only the successful sources", async () => {
    const qs = await gatherQuotes([source("a", 0.15), source("bad", new Error("down")), source("c", 0.16)]);
    expect(qs.map((q) => q.source).sort()).toEqual(["a", "c"]);
  });
});

describe("runTick", () => {
  it("refreshes when the price drifted, then verifies", async () => {
    const setPrice = vi.fn(async () => ({ ok: true }));
    const client = fakeClient({
      // on-chain $0.10 is far from market ~$0.15 -> refresh
      getOnChainPrice: vi
        .fn()
        .mockResolvedValueOnce(onChain(10_000_000n, 60n)) // pre-write read
        .mockResolvedValueOnce(onChain(15_000_000n, 0n)), // post-write verify
      setPrice,
    });
    const r = await runTick(deps({ client }));
    expect(r.ok).toBe(true);
    expect(r.refreshed).toBe(true);
    expect(setPrice).toHaveBeenCalledWith(1030, "CFX", 15_000_000n);
    expect(r.marketE8).toBe(15_000_000n);
  });

  it("does not refresh when fresh and in-band, but still evaluates CR alerts", async () => {
    const setPrice = vi.fn(async () => ({ ok: true }));
    const sink = fakeSink();
    const client = fakeClient({
      getOnChainPrice: vi.fn(async () => onChain(15_000_000n, 60n)),
      // 1 CFX collateral, debt $0.25 -> CR 6000 (way under MCR) at market $0.15
      listChainVaults: vi.fn(async () => [
        { vaultId: 7n, collateralAmountE18: 10n ** 18n, debtE8s: 25_000_000n },
      ]),
      setPrice,
    });
    const r = await runTick(deps({ client, alertSink: sink }));
    expect(r.refreshed).toBe(false);
    expect(setPrice).not.toHaveBeenCalled();
    expect(r.ok).toBe(true);
    // a CR alert was emitted
    const codes = sink.emit.mock.calls.map((c) => (c[0] as { code: string }).code);
    expect(codes).toContain("vault_below_band");
  });

  it("refuses to write and alerts when sources are insufficient", async () => {
    const setPrice = vi.fn(async () => ({ ok: true }));
    const sink = fakeSink();
    const r = await runTick(
      deps({
        sources: [source("a", 0.15), source("bad", new Error("down"))], // only 1 valid < minSources 2
        client: fakeClient({ setPrice }),
        alertSink: sink,
      }),
    );
    expect(r.ok).toBe(false);
    expect(setPrice).not.toHaveBeenCalled();
    const codes = sink.emit.mock.calls.map((c) => (c[0] as { code: string }).code);
    expect(codes).toContain("insufficient_sources");
  });

  it("alerts and reports failure when the on-chain write fails", async () => {
    const sink = fakeSink();
    const client = fakeClient({
      getOnChainPrice: vi.fn(async () => onChain(10_000_000n, 60n)),
      setPrice: vi.fn(async () => ({ ok: false, error: "ChainAdmin: not authorized to set price" })),
    });
    const r = await runTick(deps({ client, alertSink: sink }));
    expect(r.ok).toBe(false);
    expect(r.refreshed).toBe(false);
    const codes = sink.emit.mock.calls.map((c) => (c[0] as { code: string }).code);
    expect(codes).toContain("refresh_failed");
  });

  it("alerts when the post-write verification does not match", async () => {
    const sink = fakeSink();
    const client = fakeClient({
      getOnChainPrice: vi
        .fn()
        .mockResolvedValueOnce(onChain(10_000_000n, 60n)) // pre-write
        .mockResolvedValueOnce(onChain(9_999_999n, 0n)), // post-write != marketE8
      setPrice: vi.fn(async () => ({ ok: true })),
    });
    const r = await runTick(deps({ client, alertSink: sink }));
    expect(r.ok).toBe(false);
    const codes = sink.emit.mock.calls.map((c) => (c[0] as { code: string }).code);
    expect(codes).toContain("verify_mismatch");
  });
});
