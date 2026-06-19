import type { PriceQuote } from "../types.js";
import { coingeckoSource } from "./coingecko.js";
import { krakenSource } from "./kraken.js";
import { okxSource } from "./okx.js";

/** A pluggable CFX/USD price source. */
export interface PriceSource {
  readonly name: string;
  /** Fetch the current CFX/USD price. `fetchImpl` is injectable for tests. */
  fetchCfxUsd(fetchImpl?: typeof fetch): Promise<PriceQuote>;
}

export { coingeckoSource, parseCoingecko } from "./coingecko.js";
export { krakenSource, parseKraken } from "./kraken.js";
export { okxSource, parseOkx } from "./okx.js";

/**
 * The default CFX source set, verified 2026-06-18 to list CFX and be reachable:
 * CoinGecko (conflux-token, USD), Kraken (CFXUSD), OKX (CFX-USDT). Binance is
 * deliberately excluded — its API returns "restricted location" from many hosts.
 */
export function defaultSources(coinGeckoId = "conflux-token"): PriceSource[] {
  return [coingeckoSource(coinGeckoId), krakenSource(), okxSource()];
}

export type { PriceQuote };
