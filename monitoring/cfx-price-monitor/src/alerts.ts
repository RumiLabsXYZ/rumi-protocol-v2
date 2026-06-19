import type { Alert } from "./types.js";

export interface AlertSinkDeps {
  /** Slack incoming-webhook URL. When unset, alerts go to stdout only. */
  slackWebhookUrl?: string;
  /** Identifier included in every structured line (e.g. "cfx-monitor:1030:CFX"). */
  monitorId: string;
  /** Clock (ms). Injected for tests. */
  now: () => number;
  /** Structured-line writer (default: console.log). Injected for tests. */
  writeLine: (line: string) => void;
  /** Webhook POSTer (default: fetch). Injected for tests. */
  postWebhook: (url: string, body: string) => Promise<void>;
}

export interface AlertSink {
  emit(alert: Alert): Promise<void>;
}

/** Render an alert as a single machine-parseable JSON line. */
export function formatAlertLine(alert: Alert, nowMs: number, monitorId: string): string {
  return JSON.stringify({
    ts: new Date(nowMs).toISOString(),
    monitor: monitorId,
    level: alert.level,
    code: alert.code,
    message: alert.message,
    context: alert.context ?? {},
  });
}

/** Render an alert as a Slack incoming-webhook payload object. */
export function formatSlackPayload(alert: Alert, monitorId: string): { text: string } {
  const icon = alert.level === "critical" ? ":rotating_light:" : ":warning:";
  return {
    text: `${icon} [${alert.level.toUpperCase()}] \`${monitorId}\` ${alert.code}: ${alert.message}`,
  };
}

/** Stable dedup key: same code + same vault/source collapses into one stream. */
export function alertKey(alert: Alert): string {
  const id = alert.context?.["vaultId"] ?? alert.context?.["source"];
  return id !== undefined ? `${alert.code}:${String(id)}` : alert.code;
}

/**
 * Wrap a sink so a persistent fault re-alerts at most once per `cooldownMs`
 * (per key) instead of on every poll. A new key, or a recurrence after the
 * cooldown, still fires — so the signal stays visible without the spam.
 */
export function createDedupingSink(inner: AlertSink, cooldownMs: number, now: () => number = Date.now): AlertSink {
  const lastEmit = new Map<string, number>();
  return {
    async emit(alert: Alert): Promise<void> {
      const key = alertKey(alert);
      const t = now();
      const prev = lastEmit.get(key);
      if (prev !== undefined && t - prev < cooldownMs) return;
      lastEmit.set(key, t);
      await inner.emit(alert);
    },
  };
}

async function defaultPostWebhook(url: string, body: string): Promise<void> {
  const res = await fetch(url, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body,
  });
  if (!res.ok) {
    throw new Error(`webhook responded ${res.status}`);
  }
}

/**
 * Build an alert sink: every alert is written as a structured JSON line, and —
 * if a Slack webhook is configured — also POSTed to Slack. A webhook failure is
 * swallowed (alerting must never crash the monitor) but logged as a fallback line.
 */
export function createAlertSink(deps: Partial<AlertSinkDeps> & { monitorId: string }): AlertSink {
  const monitorId = deps.monitorId;
  const now = deps.now ?? Date.now;
  // eslint-disable-next-line no-console
  const writeLine = deps.writeLine ?? ((line: string) => console.log(line));
  const postWebhook = deps.postWebhook ?? defaultPostWebhook;
  const slackWebhookUrl = deps.slackWebhookUrl;

  return {
    async emit(alert: Alert): Promise<void> {
      writeLine(formatAlertLine(alert, now(), monitorId));
      if (!slackWebhookUrl) return;
      try {
        await postWebhook(slackWebhookUrl, JSON.stringify(formatSlackPayload(alert, monitorId)));
      } catch (err) {
        writeLine(
          formatAlertLine(
            {
              level: "warn",
              code: "alert_webhook_failed",
              message: `Slack webhook POST failed: ${err instanceof Error ? err.message : String(err)}`,
              context: { originalCode: alert.code },
            },
            now(),
            monitorId,
          ),
        );
      }
    },
  };
}
