export interface MonitorTarget {
  chainId: number;
  symbol: string;
  coinGeckoId: string;
}

export interface Thresholds {
  driftBps: number;
  maxAgeSec: number;
  crWarnBandE4: bigint;
  mcrE4: bigint;
  outlierPct: number;
  maxSpreadPct: number;
  minSources: number;
  pollSec: number;
  downtimeIntervals: number;
  nativeDecimals: number;
  /** Hard per-call deadline (seconds) for every source + canister request. */
  callTimeoutSec: number;
  /** A persistent fault re-alerts at most once per this many seconds (anti-spam). */
  alertCooldownSec: number;
}

export interface Config {
  network: "ic" | "local";
  host: string;
  /** Local replica only — whether to fetch the root key (insecure; never on ic). */
  fetchRootKey: boolean;
  canisterId: string;
  identityPemPath: string;
  target: MonitorTarget;
  thresholds: Thresholds;
  slackWebhookUrl?: string;
}

/**
 * Build the monitor config from environment variables, applying the runbook
 * defaults. Throws on missing required values (CANISTER_ID, IDENTITY_PEM).
 */
function required(env: Record<string, string | undefined>, key: string): string {
  const v = env[key];
  if (v === undefined || v === "") throw new Error(`missing required env ${key}`);
  return v;
}

function num(env: Record<string, string | undefined>, key: string, fallback: number): number {
  const raw = env[key];
  if (raw === undefined || raw === "") return fallback;
  const n = Number(raw);
  if (!Number.isFinite(n)) throw new Error(`env ${key} is not a number: ${raw}`);
  return n;
}

function validateThresholds(t: Thresholds): void {
  const check = (cond: boolean, msg: string): void => {
    if (!cond) throw new Error(`invalid threshold: ${msg}`);
  };
  // >= 2 sources is the whole point of a median; a single source has no quorum.
  check(t.minSources >= 2, `MIN_SOURCES must be >= 2 (got ${t.minSources})`);
  check(t.outlierPct > 0, `OUTLIER_PCT must be > 0 (got ${t.outlierPct})`);
  check(t.maxSpreadPct > 0, `MAX_SPREAD_PCT must be > 0 (got ${t.maxSpreadPct})`);
  check(t.driftBps > 0, `DRIFT_BPS must be > 0 (got ${t.driftBps})`);
  check(t.maxAgeSec > 0, `MAX_AGE_SEC must be > 0 (got ${t.maxAgeSec})`);
  check(t.pollSec > 0, `POLL_SEC must be > 0 (got ${t.pollSec})`);
  check(t.downtimeIntervals >= 1, `DOWNTIME_INTERVALS must be >= 1 (got ${t.downtimeIntervals})`);
  check(t.nativeDecimals > 0, `NATIVE_DECIMALS must be > 0 (got ${t.nativeDecimals})`);
  check(t.crWarnBandE4 >= t.mcrE4, `CR_WARN_BAND_E4 (${t.crWarnBandE4}) must be >= MCR_E4 (${t.mcrE4})`);
  // A per-call deadline only helps if it is comfortably shorter than the poll.
  check(t.callTimeoutSec > 0, `CALL_TIMEOUT_SEC must be > 0 (got ${t.callTimeoutSec})`);
  check(
    t.callTimeoutSec < t.pollSec,
    `CALL_TIMEOUT_SEC (${t.callTimeoutSec}) must be < POLL_SEC (${t.pollSec}) so a hung call surfaces within one cycle`,
  );
  check(t.alertCooldownSec > 0, `ALERT_COOLDOWN_SEC must be > 0 (got ${t.alertCooldownSec})`);
}

export function loadConfig(env: Record<string, string | undefined>): Config {
  const canisterId = required(env, "CANISTER_ID");
  const identityPemPath = required(env, "IDENTITY_PEM");

  const network = env.IC_NETWORK === "local" ? "local" : "ic";
  const host = env.IC_HOST ?? (network === "local" ? "http://127.0.0.1:4943" : "https://icp-api.io");
  const fetchRootKey = network === "local";

  const target: MonitorTarget = {
    chainId: num(env, "CHAIN_ID", 1030),
    symbol: env.SYMBOL ?? "CFX",
    coinGeckoId: env.COINGECKO_ID ?? "conflux-token",
  };

  const thresholds: Thresholds = {
    driftBps: num(env, "DRIFT_BPS", 200),
    maxAgeSec: num(env, "MAX_AGE_SEC", 300),
    crWarnBandE4: BigInt(num(env, "CR_WARN_BAND_E4", 16_000)),
    mcrE4: BigInt(num(env, "MCR_E4", 13_000)),
    outlierPct: num(env, "OUTLIER_PCT", 5),
    maxSpreadPct: num(env, "MAX_SPREAD_PCT", 3),
    minSources: num(env, "MIN_SOURCES", 2),
    pollSec: num(env, "POLL_SEC", 60),
    downtimeIntervals: num(env, "DOWNTIME_INTERVALS", 3),
    nativeDecimals: num(env, "NATIVE_DECIMALS", 18),
    callTimeoutSec: num(env, "CALL_TIMEOUT_SEC", 15),
    alertCooldownSec: num(env, "ALERT_COOLDOWN_SEC", 900),
  };

  validateThresholds(thresholds);

  return {
    network,
    host,
    fetchRootKey,
    canisterId,
    identityPemPath,
    target,
    thresholds,
    ...(env.SLACK_WEBHOOK_URL ? { slackWebhookUrl: env.SLACK_WEBHOOK_URL } : {}),
  };
}
