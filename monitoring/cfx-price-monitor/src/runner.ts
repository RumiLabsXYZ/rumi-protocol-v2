import { aggregate, type AggregateConfig } from "./aggregate.js";
import type { AlertSink } from "./alerts.js";
import type { CanisterClient } from "./canister.js";
import { decide, type PolicyConfig } from "./policy.js";
import type { PriceSource } from "./sources/index.js";
import type { Alert, PriceQuote } from "./types.js";
import { withTimeout } from "./util.js";

/** The backend caps `list_chain_vaults` at this many entries (MAX_CHAIN_VAULTS_RETURNED). */
export const MAX_CHAIN_VAULTS_RETURNED = 500;

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
  /** Hard per-call deadline (ms) for every source + canister request. Must be << pollMs. */
  callTimeoutMs: number;
}

export interface SourceFailure {
  source: string;
  reason: string;
}

export interface TickResult {
  /** True iff the cycle completed its core job: a trusted price was read and
   *  the on-chain price was either confirmed fresh or successfully refreshed. */
  ok: boolean;
  refreshed: boolean;
  reason: string | null;
  marketE8: bigint | null;
  usedSources: string[];
  sourceFailures: SourceFailure[];
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

export interface GatherResult {
  quotes: PriceQuote[];
  failures: SourceFailure[];
}

/**
 * Gather quotes from all sources concurrently, each under a hard `timeoutMs`
 * deadline (so a single hung endpoint cannot stall the gather forever). Returns
 * the successful quotes and a per-source failure list (reasons are surfaced, not
 * silently dropped).
 */
export async function gatherQuotes(
  sources: PriceSource[],
  fetchImpl: typeof fetch | undefined,
  timeoutMs: number,
): Promise<GatherResult> {
  const signal = typeof AbortSignal !== "undefined" && AbortSignal.timeout ? AbortSignal.timeout(timeoutMs) : undefined;
  const settled = await Promise.allSettled(
    sources.map((s) => withTimeout(s.fetchCfxUsd(fetchImpl, signal), timeoutMs, `source:${s.name}`)),
  );
  const quotes: PriceQuote[] = [];
  const failures: SourceFailure[] = [];
  settled.forEach((r, i) => {
    if (r.status === "fulfilled") {
      quotes.push(r.value);
    } else {
      failures.push({
        source: sources[i]!.name,
        reason: r.reason instanceof Error ? r.reason.message : String(r.reason),
      });
    }
  });
  return { quotes, failures };
}

/** Run exactly one monitor cycle. Defensive: a network hang surfaces as a failed
 *  cycle, never an indefinite stall. */
export async function runTick(deps: RunTickDeps): Promise<TickResult> {
  const { chainId, symbol, sources, fetchImpl, aggregateCfg, policyCfg, client, alertSink, nowNs, callTimeoutMs } =
    deps;

  const emitted: Alert[] = [];
  const emit = async (a: Alert): Promise<void> => {
    emitted.push(a);
    await alertSink.emit(a);
  };

  const { quotes, failures } = await gatherQuotes(sources, fetchImpl, callTimeoutMs);
  const fail = (ok: boolean, reason: string | null, marketE8: bigint | null, used: string[]): TickResult => ({
    ok,
    refreshed: false,
    reason,
    marketE8,
    usedSources: used,
    sourceFailures: failures,
    alerts: emitted,
  });

  const agg = aggregate(quotes, aggregateCfg);
  if (!agg.ok) {
    await emit({
      level: "critical",
      code: "insufficient_sources",
      message: `cannot refresh CFX price — ${agg.reason}; the manual oracle is going stale`,
      context: { reason: agg.reason, gotSources: quotes.map((q) => q.source), failures },
    });
    return fail(false, agg.reason, null, quotes.map((q) => q.source));
  }

  const marketE8 = usdToE8(agg.result.medianUsd);
  if (marketE8 <= 0n) {
    await emit({
      level: "critical",
      code: "price_underflow",
      message: `aggregated CFX market price ${agg.result.medianUsd} underflowed the e8 scale to ${marketE8}; refusing to write`,
      context: { medianUsd: agg.result.medianUsd, marketE8: marketE8.toString() },
    });
    return fail(false, "price underflow", marketE8, agg.result.used);
  }

  const onChainPrice = await withTimeout(client.getOnChainPrice(chainId, symbol), callTimeoutMs, "getOnChainPrice");
  const vaults = await withTimeout(client.listChainVaults(chainId), callTimeoutMs, "listChainVaults");
  if (vaults.length >= MAX_CHAIN_VAULTS_RETURNED) {
    await emit({
      level: "warn",
      code: "vault_coverage_truncated",
      message: `list_chain_vaults returned the full page (${vaults.length}); CR coverage may be incomplete beyond the cap`,
      context: { returned: vaults.length, cap: MAX_CHAIN_VAULTS_RETURNED },
    });
  }

  const decision = decide({ marketE8, onChain: onChainPrice, nowNs, vaults }, policyCfg);
  const emitCrAlerts = async (): Promise<void> => {
    for (const a of decision.alerts) await emit(a);
  };

  let refreshed = false;
  if (decision.shouldRefresh) {
    const res = await withTimeout(client.setPrice(chainId, symbol, marketE8), callTimeoutMs, "setPrice");
    if (!res.ok) {
      await emit({
        level: "critical",
        code: "refresh_failed",
        message: `failed to set CFX price to ${marketE8} e8: ${res.error ?? "unknown error"}`,
        context: { marketE8: marketE8.toString(), reason: decision.refreshReason },
      });
      await emitCrAlerts();
      return fail(false, res.error ?? "set failed", marketE8, agg.result.used);
    }

    // Verify the write landed. A read FAILURE (network) is distinct from a value
    // MISMATCH: the write already succeeded, so a failed re-read does not mean the
    // oracle is stale — it is a softer, warn-level signal.
    let after;
    try {
      after = await withTimeout(client.getOnChainPrice(chainId, symbol), callTimeoutMs, "verifyRead");
    } catch (err) {
      await emit({
        level: "warn",
        code: "verify_read_failed",
        message: `price write succeeded but post-write verification read failed: ${
          err instanceof Error ? err.message : String(err)
        }`,
        context: { wrote: marketE8.toString() },
      });
      await emitCrAlerts();
      return {
        ok: true,
        refreshed: true,
        reason: decision.refreshReason,
        marketE8,
        usedSources: agg.result.used,
        sourceFailures: failures,
        alerts: emitted,
      };
    }

    if (after === null || after.priceE8 !== marketE8) {
      await emit({
        level: "critical",
        code: "verify_mismatch",
        message: `post-write verification failed: on-chain price ${after?.priceE8 ?? "none"} != written ${marketE8}`,
        context: { wrote: marketE8.toString(), readBack: after?.priceE8?.toString() ?? null },
      });
      await emitCrAlerts();
      return fail(false, "verify mismatch", marketE8, agg.result.used);
    }
    refreshed = true;
  }

  await emitCrAlerts();
  return {
    ok: true,
    refreshed,
    reason: decision.refreshReason,
    marketE8,
    usedSources: agg.result.used,
    sourceFailures: failures,
    alerts: emitted,
  };
}
