import { aggregate, type AggregateConfig } from "./aggregate.js";
import type { AlertSink } from "./alerts.js";
import type { CanisterClient } from "./canister.js";
import { decide, type PolicyConfig } from "./policy.js";
import type { PriceSource } from "./sources/index.js";
import type { Alert, PriceQuote } from "./types.js";

export interface RunTickDeps {
  chainId: number;
  symbol: string;
  sources: PriceSource[];
  fetchImpl?: typeof fetch;
  aggregateCfg: AggregateConfig;
  policyCfg: PolicyConfig;
  client: CanisterClient;
  alertSink: AlertSink;
  /** Current time in nanoseconds (the loop passes Date.now() * 1e6). */
  nowNs: bigint;
}

export interface TickResult {
  /** True iff the cycle completed its core job: a trusted price was read and
   *  the on-chain price was either confirmed fresh or successfully refreshed. */
  ok: boolean;
  refreshed: boolean;
  reason: string | null;
  marketE8: bigint | null;
  usedSources: string[];
  alerts: Alert[];
}

/** Convert a USD float price to the backend's e8 fixed point. */
export function usdToE8(usd: number): bigint {
  return BigInt(Math.round(usd * 1e8));
}

/** Watchdog: alert when no successful cycle has completed within N poll intervals. */
export function shouldAlertDowntime(
  lastSuccessMs: number,
  nowMs: number,
  pollMs: number,
  downtimeIntervals: number,
): boolean {
  return nowMs - lastSuccessMs > pollMs * downtimeIntervals;
}

/** Run exactly one monitor cycle. Pure given its injected deps. */
export async function runTick(deps: RunTickDeps): Promise<TickResult> {
  const { chainId, symbol, sources, fetchImpl, aggregateCfg, policyCfg, client, alertSink, nowNs } = deps;

  const emitted: Alert[] = [];
  const emit = async (a: Alert): Promise<void> => {
    emitted.push(a);
    await alertSink.emit(a);
  };

  const quotes = await gatherQuotes(sources, fetchImpl);
  const agg = aggregate(quotes, aggregateCfg);
  if (!agg.ok) {
    await emit({
      level: "critical",
      code: "insufficient_sources",
      message: `cannot refresh CFX price — ${agg.reason}; the manual oracle is going stale`,
      context: { reason: agg.reason, gotSources: quotes.map((q) => q.source) },
    });
    return {
      ok: false,
      refreshed: false,
      reason: agg.reason,
      marketE8: null,
      usedSources: quotes.map((q) => q.source),
      alerts: emitted,
    };
  }

  const marketE8 = usdToE8(agg.result.medianUsd);
  const onChainPrice = await client.getOnChainPrice(chainId, symbol);
  const vaults = await client.listChainVaults(chainId);
  const decision = decide({ marketE8, onChain: onChainPrice, nowNs, vaults }, policyCfg);

  const emitCrAlerts = async (): Promise<void> => {
    for (const a of decision.alerts) await emit(a);
  };

  let refreshed = false;
  if (decision.shouldRefresh) {
    const res = await client.setPrice(chainId, symbol, marketE8);
    if (!res.ok) {
      await emit({
        level: "critical",
        code: "refresh_failed",
        message: `failed to set CFX price to ${marketE8} e8: ${res.error ?? "unknown error"}`,
        context: { marketE8: marketE8.toString(), reason: decision.refreshReason },
      });
      await emitCrAlerts();
      return { ok: false, refreshed: false, reason: res.error ?? "set failed", marketE8, usedSources: agg.result.used, alerts: emitted };
    }

    const after = await client.getOnChainPrice(chainId, symbol);
    if (after === null || after.priceE8 !== marketE8) {
      await emit({
        level: "critical",
        code: "verify_mismatch",
        message: `post-write verification failed: on-chain price ${after?.priceE8 ?? "none"} != written ${marketE8}`,
        context: { wrote: marketE8.toString(), readBack: after?.priceE8?.toString() ?? null },
      });
      await emitCrAlerts();
      return { ok: false, refreshed: false, reason: "verify mismatch", marketE8, usedSources: agg.result.used, alerts: emitted };
    }
    refreshed = true;
  }

  await emitCrAlerts();
  return { ok: true, refreshed, reason: decision.refreshReason, marketE8, usedSources: agg.result.used, alerts: emitted };
}

/** Internal: gather quotes from all sources, tolerating individual failures. */
export async function gatherQuotes(
  sources: PriceSource[],
  fetchImpl?: typeof fetch,
): Promise<PriceQuote[]> {
  const settled = await Promise.allSettled(sources.map((s) => s.fetchCfxUsd(fetchImpl)));
  return settled.flatMap((r) => (r.status === "fulfilled" ? [r.value] : []));
}
