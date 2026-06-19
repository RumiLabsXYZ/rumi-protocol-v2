import type { PriceQuote } from "../types.js";
import type { PriceSource } from "./index.js";

const URL = "https://api.kraken.com/0/public/Ticker";

/**
 * Parse Kraken Ticker JSON. Kraken may return a pair key different from the one
 * requested, so read the single entry under `result` and take `c[0]` (last trade):
 * `{ "error": [], "result": { "CFXUSD": { "c": ["0.04919", "29.7"] } } }`.
 */
export function parseKraken(json: unknown): number {
  const result = (json as { result?: Record<string, { c?: unknown[] }> } | null)?.result;
  const first = result ? Object.values(result)[0] : undefined;
  const last = first?.c?.[0];
  const n = Number(last);
  if (last == null || !Number.isFinite(n) || n <= 0) {
    throw new Error("kraken: missing last-trade price");
  }
  return n;
}

export function krakenSource(pair = "CFXUSD"): PriceSource {
  return {
    name: "kraken",
    async fetchCfxUsd(fetchImpl: typeof fetch = fetch, signal?: AbortSignal): Promise<PriceQuote> {
      const res = await fetchImpl(`${URL}?pair=${encodeURIComponent(pair)}`, { signal });
      if (!res.ok) throw new Error(`kraken HTTP ${res.status}`);
      return { source: "kraken", priceUsd: parseKraken(await res.json()), ts: Date.now() };
    },
  };
}
