import type { PriceQuote } from "../types.js";
import type { PriceSource } from "./index.js";

const URL = "https://api.coingecko.com/api/v3/simple/price";

/** Parse CoinGecko `simple/price` JSON: `{ "<coinId>": { "usd": 0.049 } }`. */
export function parseCoingecko(json: unknown, coinId: string): number {
  const usd = (json as Record<string, { usd?: unknown }> | null)?.[coinId]?.usd;
  if (typeof usd !== "number" || !Number.isFinite(usd)) {
    throw new Error(`coingecko: missing usd price for ${coinId}`);
  }
  return usd;
}

export function coingeckoSource(coinId: string): PriceSource {
  return {
    name: "coingecko",
    async fetchCfxUsd(fetchImpl: typeof fetch = fetch, signal?: AbortSignal): Promise<PriceQuote> {
      const res = await fetchImpl(`${URL}?ids=${encodeURIComponent(coinId)}&vs_currencies=usd`, { signal });
      if (!res.ok) throw new Error(`coingecko HTTP ${res.status}`);
      return { source: "coingecko", priceUsd: parseCoingecko(await res.json(), coinId), ts: Date.now() };
    },
  };
}
