import { createAlertSink } from "./alerts.js";
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
    minSources: t.minSources,
    pollSec: t.pollSec,
    downtimeIntervals: t.downtimeIntervals,
    nativeDecimals: t.nativeDecimals,
  };
}

async function main(): Promise<void> {
  const cfg = loadConfig(process.env);
  const monitorId = `cfx-monitor:${cfg.target.chainId}:${cfg.target.symbol}`;
  const alertSink = createAlertSink({
    monitorId,
    ...(cfg.slackWebhookUrl ? { slackWebhookUrl: cfg.slackWebhookUrl } : {}),
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
  const aggregateCfg = { minSources: cfg.thresholds.minSources, outlierPct: cfg.thresholds.outlierPct };
  const pollMs = cfg.thresholds.pollSec * 1000;

  let lastSuccessMs = Date.now(); // startup grace
  let downtimeAlerted = false;
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
      });
      logLine(monitorId, "tick", {
        ok: res.ok,
        refreshed: res.refreshed,
        marketE8: res.marketE8?.toString() ?? null,
        reason: res.reason,
        usedSources: res.usedSources,
        alerts: res.alerts.length,
      });
      if (res.ok) {
        lastSuccessMs = nowMs;
        downtimeAlerted = false;
      }
    } catch (err) {
      // runTick is defensive, but never let the loop die on an unexpected throw.
      await alertSink.emit({
        level: "critical",
        code: "tick_exception",
        message: `monitor tick threw unexpectedly: ${err instanceof Error ? err.message : String(err)}`,
        context: {},
      });
    }

    if (shouldAlertDowntime(lastSuccessMs, Date.now(), pollMs, cfg.thresholds.downtimeIntervals)) {
      if (!downtimeAlerted) {
        await alertSink.emit({
          level: "critical",
          code: "monitor_downtime",
          message: `no successful cycle in ${cfg.thresholds.downtimeIntervals} poll intervals — the manual CFX oracle is going unmonitored`,
          context: { lastSuccessMs, downtimeIntervals: cfg.thresholds.downtimeIntervals },
        });
        downtimeAlerted = true;
      }
    }
  };

  const stop = (sig: string): void => {
    running = false;
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
}

main().catch((err) => {
  // eslint-disable-next-line no-console
  console.error(JSON.stringify({ level: "fatal", message: String(err?.stack ?? err) }));
  process.exit(1);
});
