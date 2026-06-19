import { createAlertSink, createDedupingSink } from "./alerts.js";
import { createCanisterClient, createRawBackend } from "./canister.js";
import { loadConfig, type Config } from "./config.js";
import { loadIdentityFromFile } from "./identity.js";
import { runTick, shouldAlertDowntime } from "./runner.js";
import { defaultSources } from "./sources/index.js";

const sleep = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));

function logLine(monitorId: string, message: string, extra: Record<string, unknown> = {}): void {
  // eslint-disable-next-line no-console
  console.log(
    JSON.stringify({ ts: new Date().toISOString(), monitor: monitorId, level: "info", message, ...extra }),
  );
}

function thresholdsForLog(cfg: Config): Record<string, unknown> {
  const t = cfg.thresholds;
  return {
    driftBps: t.driftBps,
    maxAgeSec: t.maxAgeSec,
    crWarnBandE4: t.crWarnBandE4.toString(),
    mcrE4: t.mcrE4.toString(),
    outlierPct: t.outlierPct,
    maxSpreadPct: t.maxSpreadPct,
    minSources: t.minSources,
    pollSec: t.pollSec,
    downtimeIntervals: t.downtimeIntervals,
    nativeDecimals: t.nativeDecimals,
    callTimeoutSec: t.callTimeoutSec,
    alertCooldownSec: t.alertCooldownSec,
  };
}

async function main(): Promise<void> {
  const cfg = loadConfig(process.env);
  const monitorId = `cfx-monitor:${cfg.target.chainId}:${cfg.target.symbol}`;

  // baseSink: always emits. alertSink: dedups persistent faults so a stuck
  // condition re-alerts at most once per cooldown instead of every poll.
  const baseSink = createAlertSink({
    monitorId,
    ...(cfg.slackWebhookUrl ? { slackWebhookUrl: cfg.slackWebhookUrl } : {}),
  });
  const alertSink = createDedupingSink(baseSink, cfg.thresholds.alertCooldownSec * 1000);

  // Process-level guards: a stray rejection/exception must surface as a critical
  // alert rather than silently killing the monitor (a dead monitor = frozen
  // oracle = the F-01 risk). Use baseSink so these are never deduped away.
  process.on("unhandledRejection", (reason) => {
    void baseSink.emit({
      level: "critical",
      code: "unhandled_rejection",
      message: `unhandled promise rejection: ${reason instanceof Error ? reason.message : String(reason)}`,
      context: {},
    });
  });
  process.on("uncaughtException", (err) => {
    void baseSink.emit({
      level: "critical",
      code: "uncaught_exception",
      message: `uncaught exception: ${err instanceof Error ? err.message : String(err)}`,
      context: {},
    });
  });

  logLine(monitorId, "starting CFX price monitor (audit F-01 mitigation)", {
    network: cfg.network,
    host: cfg.host,
    canisterId: cfg.canisterId,
    target: cfg.target,
    thresholds: thresholdsForLog(cfg),
    slackConfigured: Boolean(cfg.slackWebhookUrl),
  });

  const identity = loadIdentityFromFile(cfg.identityPemPath);
  const myPrincipal = identity.getPrincipal().toText();
  logLine(monitorId, "loaded price-pusher identity", { principal: myPrincipal });

  const raw = await createRawBackend({
    canisterId: cfg.canisterId,
    host: cfg.host,
    identity,
    fetchRootKey: cfg.fetchRootKey,
  });
  const client = createCanisterClient(raw);

  // Preflight: warn loudly if this identity isn't the registered pusher. It may
  // still be the developer (also allowed), so this is a warning, not a hard stop.
  try {
    const pusher = await client.getPricePusher();
    if (pusher && pusher.toText() !== myPrincipal) {
      await alertSink.emit({
        level: "warn",
        code: "identity_not_registered_pusher",
        message: `configured identity ${myPrincipal} is not the registered price-pusher (${pusher.toText()}); set_manual_collateral_price will fail unless this identity is the developer`,
        context: { configured: myPrincipal, registered: pusher.toText() },
      });
    } else if (!pusher) {
      logLine(monitorId, "no price-pusher registered yet; relying on this identity being the developer", {});
    }
  } catch (err) {
    logLine(monitorId, "preflight getPricePusher failed (non-fatal)", { error: String(err) });
  }

  const sources = defaultSources(cfg.target.coinGeckoId);
  const policyCfg = {
    driftBps: cfg.thresholds.driftBps,
    maxAgeSec: cfg.thresholds.maxAgeSec,
    crWarnBandE4: cfg.thresholds.crWarnBandE4,
    mcrE4: cfg.thresholds.mcrE4,
    nativeDecimals: cfg.thresholds.nativeDecimals,
  };
  const aggregateCfg = {
    minSources: cfg.thresholds.minSources,
    outlierPct: cfg.thresholds.outlierPct,
    maxSpreadPct: cfg.thresholds.maxSpreadPct,
  };
  const pollMs = cfg.thresholds.pollSec * 1000;
  const callTimeoutMs = cfg.thresholds.callTimeoutSec * 1000;

  let lastSuccessMs = Date.now(); // startup grace
  let running = true;

  const tickOnce = async (): Promise<void> => {
    const nowMs = Date.now();
    try {
      const res = await runTick({
        chainId: cfg.target.chainId,
        symbol: cfg.target.symbol,
        sources,
        aggregateCfg,
        policyCfg,
        client,
        alertSink,
        nowNs: BigInt(nowMs) * 1_000_000n,
        callTimeoutMs,
      });
      logLine(monitorId, "tick", {
        ok: res.ok,
        refreshed: res.refreshed,
        marketE8: res.marketE8?.toString() ?? null,
        reason: res.reason,
        usedSources: res.usedSources,
        alerts: res.alerts.length,
        sourceFailures: res.sourceFailures.length,
      });
      for (const f of res.sourceFailures) {
        logLine(monitorId, "source failed", { source: f.source, reason: f.reason });
      }
      if (res.ok) lastSuccessMs = nowMs;
    } catch (err) {
      await alertSink.emit({
        level: "critical",
        code: "tick_exception",
        message: `monitor tick threw unexpectedly: ${err instanceof Error ? err.message : String(err)}`,
        context: {},
      });
    }
  };

  // Independent downtime watchdog. It runs on its OWN timer, decoupled from the
  // tick loop, so a tick that stalls (despite per-call timeouts) can never
  // suppress the very alert meant to catch it. Dedup throttles repeats.
  const watchdogMs = Math.max(5_000, Math.floor(pollMs / 2));
  const watchdog = setInterval(() => {
    if (shouldAlertDowntime(lastSuccessMs, Date.now(), pollMs, cfg.thresholds.downtimeIntervals)) {
      void alertSink.emit({
        level: "critical",
        code: "monitor_downtime",
        message: `no successful cycle in ${cfg.thresholds.downtimeIntervals} poll intervals — the manual CFX oracle is going unmonitored`,
        context: { lastSuccessMs, downtimeIntervals: cfg.thresholds.downtimeIntervals },
      });
    }
  }, watchdogMs);

  const stop = (sig: string): void => {
    running = false;
    clearInterval(watchdog);
    logLine(monitorId, `received ${sig}, shutting down`, {});
  };
  process.on("SIGINT", () => stop("SIGINT"));
  process.on("SIGTERM", () => stop("SIGTERM"));

  await tickOnce();
  while (running) {
    await sleep(pollMs);
    if (!running) break;
    await tickOnce();
  }
  clearInterval(watchdog);
}

main().catch((err) => {
  // eslint-disable-next-line no-console
  console.error(JSON.stringify({ level: "fatal", message: String(err?.stack ?? err) }));
  process.exit(1);
});
