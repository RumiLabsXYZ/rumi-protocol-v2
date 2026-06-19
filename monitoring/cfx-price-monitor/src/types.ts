// Shared types across the monitor's pure modules.

/** A single price observation from one source. */
export interface PriceQuote {
  /** Source identifier, e.g. "coingecko", "binance". */
  source: string;
  /** CFX/USD price as a plain float (e.g. 0.1534). */
  priceUsd: number;
  /** Epoch milliseconds when the quote was fetched. */
  ts: number;
}

/** A source dropped from the aggregate, with why. */
export interface RejectedQuote {
  source: string;
  priceUsd: number;
  reason: string;
}

/** The aggregated market view across sources. */
export interface AggregateResult {
  medianUsd: number;
  /** Sources that survived validation + outlier rejection. */
  used: string[];
  rejected: RejectedQuote[];
}

/** Aggregation either yields a trustworthy median, or refuses. */
export type AggregateOutcome =
  | { ok: true; result: AggregateResult }
  | { ok: false; reason: string; rejected: RejectedQuote[] };

/** A chain vault as returned by `list_chain_vaults` (the fields we need). */
export interface ChainVault {
  vaultId: bigint;
  collateralAmountE18: bigint;
  debtE8s: bigint;
}

/** The on-chain manual price readout from `get_manual_collateral_price`. */
export interface OnChainPrice {
  priceE8: bigint;
  setAtNs: bigint;
}

/** Severity for an alert. */
export type AlertLevel = "warn" | "critical";

/** A structured alert emitted by the policy engine / runner. */
export interface Alert {
  level: AlertLevel;
  /** Stable machine code, e.g. "vault_below_band", "insufficient_sources". */
  code: string;
  message: string;
  /** Arbitrary structured context (vault id, CR, prices, ...). */
  context?: Record<string, unknown>;
}
