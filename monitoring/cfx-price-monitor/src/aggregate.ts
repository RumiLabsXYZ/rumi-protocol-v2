import type { AggregateOutcome, PriceQuote } from "./types.js";

export interface AggregateConfig {
  /** Minimum number of healthy, in-band sources required to trust a median. */
  minSources: number;
  /** Reject a quote that deviates more than this % from the provisional median. */
  outlierPct: number;
}

/** Plain median of a non-empty numeric array (average of the two middles for even counts). */
export function median(values: number[]): number {
  const sorted = [...values].sort((a, b) => a - b);
  const mid = Math.floor(sorted.length / 2);
  if (sorted.length % 2 === 1) return sorted[mid]!;
  return (sorted[mid - 1]! + sorted[mid]!) / 2;
}

/**
 * Aggregate raw source quotes into a single trustworthy median CFX/USD price.
 *
 * 1. Drop non-finite / non-positive quotes.
 * 2. If fewer than `minSources` valid quotes remain, refuse.
 * 3. Reject any quote more than `outlierPct`% from the provisional median.
 * 4. Recompute the median over survivors; refuse if fewer than `minSources` remain.
 */
export function aggregate(quotes: PriceQuote[], cfg: AggregateConfig): AggregateOutcome {
  const rejects: { source: string; priceUsd: number; reason: string }[] = [];

  const valid = quotes.filter((qt) => {
    const bad = !Number.isFinite(qt.priceUsd) || qt.priceUsd <= 0;
    if (bad) rejects.push({ source: qt.source, priceUsd: qt.priceUsd, reason: "invalid price" });
    return !bad;
  });

  if (valid.length < cfg.minSources) {
    return {
      ok: false,
      reason: `insufficient sources: ${valid.length} valid < ${cfg.minSources} required`,
      rejected: rejects,
    };
  }

  const provisional = median(valid.map((qt) => qt.priceUsd));
  const survivors = valid.filter((qt) => {
    const deviation = Math.abs(qt.priceUsd - provisional) / provisional;
    const outlier = deviation > cfg.outlierPct / 100;
    if (outlier) {
      rejects.push({
        source: qt.source,
        priceUsd: qt.priceUsd,
        reason: `outlier: ${(deviation * 100).toFixed(2)}% from median ${provisional}`,
      });
    }
    return !outlier;
  });

  if (survivors.length < cfg.minSources) {
    return {
      ok: false,
      reason: `insufficient in-band sources: ${survivors.length} < ${cfg.minSources} required`,
      rejected: rejects,
    };
  }

  return {
    ok: true,
    result: {
      medianUsd: median(survivors.map((qt) => qt.priceUsd)),
      used: survivors.map((qt) => qt.source),
      rejected: rejects,
    },
  };
}
