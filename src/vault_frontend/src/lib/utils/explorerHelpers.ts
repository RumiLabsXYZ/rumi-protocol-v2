import { CANISTER_IDS } from '$lib/config';

// ─── Amount Formatting ───────────────────────────────────────────────

/** Convert e8s (or other smallest-unit integer) to human-readable string. */
export function formatE8s(e8s: bigint | number, decimals = 8): string {
  const val = Number(e8s) / 10 ** decimals;
  // Avoid unnecessary trailing zeros but keep at least 2 decimal places for readability
  if (val === 0) return '0';
  if (Math.abs(val) >= 1) {
    return val.toLocaleString('en-US', {
      minimumFractionDigits: 2,
      maximumFractionDigits: decimals,
    });
  }
  return val.toLocaleString('en-US', {
    minimumFractionDigits: 2,
    maximumFractionDigits: decimals,
  });
}

/** Format a USD amount stored in e8s (8 decimals). */
export function formatUsd(e8s: bigint | number): string {
  const val = Number(e8s) / 1e8;
  return `$${val.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
}

/** Format a raw floating-point number as USD. */
export function formatUsdRaw(val: number | bigint): string {
  const n = Number(val);
  return `$${n.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
}

/** Format a ratio as a percentage (0.015 → "1.50%"). */
export function formatPercent(ratio: number | bigint, decimals = 2): string {
  const n = Number(ratio);
  return `${(n * 100).toFixed(decimals)}%`;
}

/** Format a collateral ratio (1.5 → "150.0%"). */
export function formatCR(ratio: number | bigint): string {
  const n = Number(ratio);
  return `${(n * 100).toFixed(1)}%`;
}

/** Format basis points as percentage (500 → "5.00%"). */
export function formatBps(bps: number | bigint): string {
  const n = Number(bps);
  return `${(n / 100).toFixed(2)}%`;
}

// ─── Time Formatting ─────────────────────────────────────────────────

/** Convert nanosecond timestamp to Date. */
export function nsToDate(ns: bigint | number): Date {
  return new Date(Number(ns) / 1_000_000);
}

/** Full locale date-time string from nanosecond timestamp. */
export function formatTimestamp(ns: bigint | number): string {
  return nsToDate(ns).toLocaleString();
}

/** Short date string from nanosecond timestamp. */
export function formatDate(ns: bigint | number): string {
  return nsToDate(ns).toLocaleDateString();
}

/** Relative time string from nanosecond timestamp ("3m ago", "2h ago", "5d ago"). */
export function timeAgo(ns: bigint | number): string {
  const seconds = Math.floor((Date.now() - nsToDate(ns).getTime()) / 1000);
  if (seconds < 0) return 'just now';
  if (seconds < 60) return `${seconds}s ago`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  if (days < 30) return `${days}d ago`;
  const months = Math.floor(days / 30);
  if (months < 12) return `${months}mo ago`;
  const years = Math.floor(days / 365);
  return `${years}y ago`;
}

// ─── Principal / Address ─────────────────────────────────────────────

/** Shorten a principal for display ("tfesu-vyaaa-aaaap-qrd7a-cai" → "tfesu…rd7a-cai"). */
export function shortenPrincipal(principal: string, chars = 5): string {
  if (principal.length <= chars * 2 + 1) return principal;
  return `${principal.slice(0, chars)}…${principal.slice(-chars)}`;
}

// ─── Token Registry ──────────────────────────────────────────────────

export interface TokenInfo {
  symbol: string;
  name: string;
  decimals: number;
}

export const KNOWN_TOKENS: Record<string, TokenInfo> = {
  // Core protocol tokens
  [CANISTER_IDS.ICP_LEDGER]: { symbol: 'ICP', name: 'Internet Computer', decimals: 8 },
  [CANISTER_IDS.ICUSD_LEDGER]: { symbol: 'icUSD', name: 'icUSD Stablecoin', decimals: 8 },
  [CANISTER_IDS.CKUSDT_LEDGER]: { symbol: 'ckUSDT', name: 'Chain-Key USDT', decimals: 6 },
  [CANISTER_IDS.CKUSDC_LEDGER]: { symbol: 'ckUSDC', name: 'Chain-Key USDC', decimals: 6 },
  [CANISTER_IDS.THREEPOOL]: { symbol: '3USD LP', name: 'Rumi 3Pool LP Token', decimals: 18 },
  // Collateral tokens
  'mxzaz-hqaaa-aaaar-qaada-cai': { symbol: 'ckBTC', name: 'Chain-Key Bitcoin', decimals: 8 },
  'ss2fx-dyaaa-aaaar-qacoq-cai': { symbol: 'ckETH', name: 'Chain-Key Ethereum', decimals: 18 },
  'o7oak-6yaaa-aaaap-qhgbq-cai': { symbol: 'ckXAUT', name: 'Chain-Key Gold', decimals: 6 },
  'buwm7-7yaaa-aaaar-qagva-cai': { symbol: 'nICP', name: 'WaterNeuron Staked ICP', decimals: 8 },
  'nza5v-qaaaa-aaaar-qahzq-cai': { symbol: 'ckXAUT', name: 'Chain-Key Gold (XAUT)', decimals: 6 },
  '7pail-xaaaa-aaaas-aabmq-cai': { symbol: 'BOB', name: 'BOB Token', decimals: 8 },
  'rh2pm-ryaaa-aaaan-qeniq-cai': { symbol: 'EXE', name: 'EXE Token', decimals: 8 },
};

/** Register a token dynamically (e.g. from canister discovery). */
export function registerToken(
  principal: string,
  symbol: string,
  name: string,
  decimals: number,
): void {
  KNOWN_TOKENS[principal] = { symbol, name, decimals };
}

/** Get token info by canister principal, or null if unknown. */
export function getTokenInfo(principal: string): TokenInfo | null {
  return KNOWN_TOKENS[principal] ?? null;
}

/** Get token symbol by canister principal, falls back to shortened principal. */
export function getTokenSymbol(principal: string): string {
  return KNOWN_TOKENS[principal]?.symbol ?? shortenPrincipal(principal);
}

/** Get token decimals by canister principal, defaults to 8. */
export function getTokenDecimals(principal: string): number {
  return KNOWN_TOKENS[principal]?.decimals ?? 8;
}

/** Format a token amount using the correct decimals for that token. */
export function formatTokenAmount(amount: bigint | number, tokenPrincipal: string): string {
  const decimals = getTokenDecimals(tokenPrincipal);
  return formatE8s(amount, decimals);
}

// ─── Entity Detection (Smart Search) ─────────────────────────────────

/** Check if input looks like a vault ID (pure digits). */
export function isVaultId(input: string): boolean {
  return /^\d+$/.test(input.trim());
}

/** Check if input looks like an event index (#N, eN, or pure digits). */
export function isEventIndex(input: string): boolean {
  return /^(#\d+|e\d+|\d+)$/i.test(input.trim());
}

/** Parse an event index from "#123", "e123", or "123". */
export function parseEventIndex(input: string): number {
  const trimmed = input.trim();
  if (trimmed.startsWith('#')) return parseInt(trimmed.slice(1), 10);
  if (trimmed.toLowerCase().startsWith('e')) return parseInt(trimmed.slice(1), 10);
  return parseInt(trimmed, 10);
}

/** Check if input looks like a principal (contains dashes, >10 chars). */
export function isPrincipal(input: string): boolean {
  const trimmed = input.trim();
  return trimmed.length > 10 && trimmed.includes('-');
}

const TOKEN_ALIASES: Record<string, string> = {
  icp: CANISTER_IDS.ICP_LEDGER,
  icusd: CANISTER_IDS.ICUSD_LEDGER,
  ckusdt: CANISTER_IDS.CKUSDT_LEDGER,
  ckusdc: CANISTER_IDS.CKUSDC_LEDGER,
  '3usd': CANISTER_IDS.THREEPOOL,
  '3pool': CANISTER_IDS.THREEPOOL,
};

/** Resolve a human-friendly token alias ("icp", "icusd", etc.) to its canister ID. */
export function resolveTokenAlias(input: string): string | null {
  return TOKEN_ALIASES[input.trim().toLowerCase()] ?? null;
}

// ─── Canister Identification ─────────────────────────────────────────

export const KNOWN_CANISTERS: Record<string, string> = {
  [CANISTER_IDS.PROTOCOL]: 'Rumi Protocol',
  [CANISTER_IDS.ICP_LEDGER]: 'ICP Ledger',
  [CANISTER_IDS.ICUSD_LEDGER]: 'icUSD Ledger',
  [CANISTER_IDS.TREASURY]: 'Treasury',
  [CANISTER_IDS.STABILITY_POOL]: 'Stability Pool',
  [CANISTER_IDS.CKUSDT_LEDGER]: 'ckUSDT Ledger',
  [CANISTER_IDS.CKUSDC_LEDGER]: 'ckUSDC Ledger',
  [CANISTER_IDS.ICUSD_INDEX]: 'icUSD Index',
  [CANISTER_IDS.THREEPOOL]: 'Rumi 3Pool',
};

/** Get human-readable name for a canister, or null if unknown. */
export function getCanisterName(principal: string): string | null {
  return KNOWN_CANISTERS[principal] ?? null;
}

/** Check if a principal is a known Rumi canister. */
export function isKnownCanister(principal: string): boolean {
  return principal in KNOWN_CANISTERS;
}

// ─── Vault Health ────────────────────────────────────────────────────

export type HealthStatus = 'healthy' | 'caution' | 'danger' | 'liquidatable';

/**
 * Classify vault health based on collateral ratio, liquidation ratio, and borrow threshold.
 * - liquidatable: CR ≤ liquidation ratio (e.g., ≤ 1.1)
 * - danger: CR < borrow threshold ratio (e.g., < 1.5)
 * - caution: CR < borrow threshold * 1.2 (e.g., < 1.8)
 * - healthy: above that
 *
 * If borrowThreshold is not provided, falls back to liquidationRatio-based thresholds.
 */
export function classifyVaultHealth(
  cr: number | bigint,
  liquidationRatio: number | bigint,
  borrowThreshold?: number | bigint,
): HealthStatus {
  const c = Number(cr);
  const l = Number(liquidationRatio);
  const bt = borrowThreshold != null ? Number(borrowThreshold) : l * 1.36; // fallback heuristic
  if (c <= l) return 'liquidatable';
  if (c < bt) return 'danger';
  // Vaults within 5% of borrow threshold should still feel dangerous
  if (c < bt * 1.05) return 'danger';
  if (c < bt * 1.25) return 'caution';
  return 'healthy';
}

/** Tailwind text color class for a health status. */
export function healthColor(status: HealthStatus): string {
  switch (status) {
    case 'healthy':
      return 'text-green-500';
    case 'caution':
      return 'text-yellow-500';
    case 'danger':
      return 'text-orange-500';
    case 'liquidatable':
      return 'text-red-500';
  }
}

/** Tailwind bg + border classes for a health status. */
export function healthBg(status: HealthStatus): string {
  switch (status) {
    case 'healthy':
      return 'bg-green-500/10 border border-green-500/30';
    case 'caution':
      return 'bg-yellow-500/10 border border-yellow-500/30';
    case 'danger':
      return 'bg-orange-500/10 border border-orange-500/30';
    case 'liquidatable':
      return 'bg-red-500/10 border border-red-500/30';
  }
}
