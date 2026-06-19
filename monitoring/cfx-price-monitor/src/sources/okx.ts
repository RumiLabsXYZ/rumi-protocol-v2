import type { PriceQuote } from "../types.js";
import type { PriceSource } from "./index.js";

const URL = "https://www.okx.com/api/v5/market/ticker";

/** Parse OKX Ticker JSON: `{ "code": "0", "data": [{ "last": "0.04924" }] }`. */
export function parseOkx(json: unknown): number {
  const last = (json as { data?: Array<{ last?: unknown }> } | null)?.data?.[0]?.last;
  const n = Number(last);
  if (last == null || !Number.isFinite(n) || n <= 0) {
    throw new Error("okx: missing last price");
  }
  return n;
}

export function okxSource(instId = "CFX-USDT"): PriceSource {
  return {
    name: "okx",
    async fetchCfxUsd(fetchImpl: typeof fetch = fetch): Promise<PriceQuote> {
      const res = await fetchImpl(`${URL}?instId=${encodeURIComponent(instId)}`);
      if (!res.ok) throw new Error(`okx HTTP ${res.status}`);
      return { source: "okx", priceUsd: parseOkx(await res.json()), ts: Date.now() };
    },
  };
}
