import { describe, it, expect, vi } from "vitest";
import { createAlertSink, createDedupingSink, formatAlertLine, formatSlackPayload } from "../src/alerts.js";
import type { AlertSink } from "../src/alerts.js";
import type { Alert } from "../src/types.js";

const alert: Alert = {
  level: "critical",
  code: "vault_below_band",
  message: "vault 3 under-collateralized",
  context: { vaultId: "3", crE4: "12000" },
};

describe("formatAlertLine", () => {
  it("produces a single-line JSON with the key fields", () => {
    const line = formatAlertLine(alert, 1_700_000_000_000, "cfx-monitor:1030:CFX");
    expect(line).not.toContain("\n");
    const parsed = JSON.parse(line);
    expect(parsed.level).toBe("critical");
    expect(parsed.code).toBe("vault_below_band");
    expect(parsed.message).toBe("vault 3 under-collateralized");
    expect(parsed.monitor).toBe("cfx-monitor:1030:CFX");
    expect(parsed.context.vaultId).toBe("3");
    expect(typeof parsed.ts).toBe("string"); // ISO timestamp
  });
});

describe("formatSlackPayload", () => {
  it("includes severity and message", () => {
    const p = formatSlackPayload(alert, "cfx-monitor:1030:CFX");
    expect(p.text).toContain("CRITICAL");
    expect(p.text).toContain("vault 3 under-collateralized");
  });
});

describe("createAlertSink", () => {
  it("always writes a structured line", async () => {
    const writeLine = vi.fn();
    const sink = createAlertSink({ monitorId: "m", writeLine, now: () => 0 });
    await sink.emit(alert);
    expect(writeLine).toHaveBeenCalledTimes(1);
    expect(() => JSON.parse(writeLine.mock.calls[0]![0])).not.toThrow();
  });

  it("POSTs to Slack when a webhook is configured", async () => {
    const postWebhook = vi.fn().mockResolvedValue(undefined);
    const sink = createAlertSink({
      monitorId: "m",
      slackWebhookUrl: "https://hooks.slack.test/abc",
      writeLine: vi.fn(),
      postWebhook,
      now: () => 0,
    });
    await sink.emit(alert);
    expect(postWebhook).toHaveBeenCalledTimes(1);
    expect(postWebhook.mock.calls[0]![0]).toBe("https://hooks.slack.test/abc");
    expect(postWebhook.mock.calls[0]![1]).toContain("CRITICAL");
  });

  it("does NOT POST when no webhook is configured", async () => {
    const postWebhook = vi.fn();
    const sink = createAlertSink({ monitorId: "m", writeLine: vi.fn(), postWebhook, now: () => 0 });
    await sink.emit(alert);
    expect(postWebhook).not.toHaveBeenCalled();
  });

  it("does not throw when the Slack POST fails (resilient), and logs a fallback", async () => {
    const writeLine = vi.fn();
    const postWebhook = vi.fn().mockRejectedValue(new Error("network down"));
    const sink = createAlertSink({
      monitorId: "m",
      slackWebhookUrl: "https://hooks.slack.test/abc",
      writeLine,
      postWebhook,
      now: () => 0,
    });
    await expect(sink.emit(alert)).resolves.toBeUndefined();
    // one line for the alert + one fallback line for the webhook failure
    expect(writeLine).toHaveBeenCalledTimes(2);
    expect(writeLine.mock.calls[1]![0]).toMatch(/webhook|slack/i);
  });
});

describe("createDedupingSink", () => {
  function counting(): AlertSink & { count: number } {
    const s = { count: 0, async emit() { s.count += 1; } };
    return s;
  }

  it("suppresses repeats of the same key within the cooldown", async () => {
    const inner = counting();
    let t = 0;
    const sink = createDedupingSink(inner, 1000, () => t);
    await sink.emit(alert);
    await sink.emit(alert);
    expect(inner.count).toBe(1);
  });

  it("re-alerts the same key after the cooldown elapses", async () => {
    const inner = counting();
    let t = 0;
    const sink = createDedupingSink(inner, 1000, () => t);
    await sink.emit(alert);
    t = 1500;
    await sink.emit(alert);
    expect(inner.count).toBe(2);
  });

  it("treats different codes and different vaults as independent streams", async () => {
    const inner = counting();
    const sink = createDedupingSink(inner, 1000, () => 0);
    await sink.emit({ ...alert, context: { vaultId: "3" } });
    await sink.emit({ ...alert, context: { vaultId: "4" } }); // different vault
    await sink.emit({ ...alert, code: "insufficient_sources", context: {} }); // different code
    expect(inner.count).toBe(3);
  });
});
