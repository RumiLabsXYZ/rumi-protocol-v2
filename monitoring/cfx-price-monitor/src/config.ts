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
  minSources: number;
  pollSec: number;
  downtimeIntervals: number;
  nativeDecimals: number;
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
    minSources: num(env, "MIN_SOURCES", 2),
    pollSec: num(env, "POLL_SEC", 60),
    downtimeIntervals: num(env, "DOWNTIME_INTERVALS", 3),
    nativeDecimals: num(env, "NATIVE_DECIMALS", 18),
  };

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
