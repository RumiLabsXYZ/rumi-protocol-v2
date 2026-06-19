import { describe, it, expect, vi } from "vitest";
import { parseCoingecko, coingeckoSource } from "../src/sources/coingecko.js";
import { parseKraken, krakenSource } from "../src/sources/kraken.js";
import { parseOkx, okxSource } from "../src/sources/okx.js";

// Real response fixtures captured 2026-06-18.
const CG = { "conflux-token": { usd: 0.04915384 } };
const KR = {
  error: [],
  result: { CFXUSD: { a: ["0.0492", "2300", "2300"], c: ["0.04919", "29.744"], o: "0.0488" } },
};
const OK = { code: "0", msg: "", data: [{ instId: "CFX-USDT", last: "0.04924", bidPx: "0.04919" }] };

function mockFetch(json: unknown, ok = true): typeof fetch {
  return vi.fn().mockResolvedValue({ ok, status: ok ? 200 : 503, json: async () => json }) as unknown as typeof fetch;
}

describe("parsers", () => {
  it("parseCoingecko reads json[coinId].usd", () => {
    expect(parseCoingecko(CG, "conflux-token")).toBeCloseTo(0.04915384, 10);
  });
  it("parseKraken reads the single result entry's last trade c[0]", () => {
    expect(parseKraken(KR)).toBeCloseTo(0.04919, 10);
  });
  it("parseOkx reads data[0].last", () => {
    expect(parseOkx(OK)).toBeCloseTo(0.04924, 10);
  });
  it("parsers throw on malformed payloads", () => {
    expect(() => parseCoingecko({}, "conflux-token")).toThrow();
    expect(() => parseKraken({ result: {} })).toThrow();
    expect(() => parseOkx({ data: [] })).toThrow();
  });
});

describe("source fetchers (injected fetch)", () => {
  it("coingecko returns a normalized quote", async () => {
    const q = await coingeckoSource("conflux-token").fetchCfxUsd(mockFetch(CG));
    expect(q.source).toBe("coingecko");
    expect(q.priceUsd).toBeCloseTo(0.04915384, 10);
    expect(typeof q.ts).toBe("number");
  });
  it("kraken returns a normalized quote", async () => {
    const q = await krakenSource().fetchCfxUsd(mockFetch(KR));
    expect(q.source).toBe("kraken");
    expect(q.priceUsd).toBeCloseTo(0.04919, 10);
  });
  it("okx returns a normalized quote", async () => {
    const q = await okxSource().fetchCfxUsd(mockFetch(OK));
    expect(q.source).toBe("okx");
    expect(q.priceUsd).toBeCloseTo(0.04924, 10);
  });
  it("a non-200 response throws", async () => {
    await expect(coingeckoSource("conflux-token").fetchCfxUsd(mockFetch(CG, false))).rejects.toThrow(/HTTP 503/);
  });
});
