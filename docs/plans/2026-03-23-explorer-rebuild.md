# Explorer Rebuild — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Rebuild the Rumi Protocol Explorer from scratch — a robust, Etherscan-quality protocol explorer with a data-rich dashboard, clickable everything, human-readable formatting, and full historical data from all 3 canisters.

**Architecture:** Rewrite the explorer's data layer, formatters, and all 8 page routes. Keep the existing route structure (`/explorer/*`) and the `publicActor` pattern for canister queries. Replace the current stores with a single reactive data service using Svelte 5 runes. Rebuild every page to be data-dense, fully linked, and readable.

**Tech Stack:** SvelteKit, Svelte 5 (runes: `$state`, `$derived`, `$effect`), Tailwind CSS, IC agent via `publicActor`, Layerchart for historical charts.

---

## File Map

```
src/vault_frontend/src/
├── lib/
│   ├── services/explorer/
│   │   ├── explorerService.ts      ← NEW: unified data service (replaces store + data layer)
│   │   └── explorerFormatters.ts   ← REWRITE: comprehensive event→human-readable formatting
│   ├── components/explorer/
│   │   ├── EntityLink.svelte       ← REWRITE: smart clickable links for any entity
│   │   ├── DataTable.svelte        ← REWRITE: sortable, filterable table with rich cells
│   │   ├── SearchBar.svelte        ← REWRITE: smart search with autocomplete hints
│   │   ├── StatCard.svelte         ← NEW: replaces DashboardCard with richer display
│   │   ├── TokenBadge.svelte       ← KEEP: minor polish
│   │   ├── VaultHealthBar.svelte   ← KEEP: minor polish
│   │   ├── EventRow.svelte         ← REWRITE: rich event row with all details inline
│   │   ├── Pagination.svelte       ← KEEP: works fine
│   │   ├── TimeAgo.svelte          ← NEW: reactive relative timestamps
│   │   ├── AmountDisplay.svelte    ← NEW: formatted token amounts with USD values
│   │   ├── CopyButton.svelte       ← NEW: reusable copy-to-clipboard
│   │   └── StatusBadge.svelte      ← NEW: protocol mode / vault status badges
│   └── utils/
│       └── explorerHelpers.ts      ← NEW: shared formatting utilities (e8s→decimal, time, etc.)
├── routes/explorer/
│   ├── +layout.svelte              ← REWRITE: nav + search chrome
│   ├── +page.svelte                ← REWRITE: dashboard (the big one)
│   ├── events/+page.svelte         ← REWRITE: full event log
│   ├── event/[index]/+page.svelte  ← REWRITE: event detail
│   ├── vault/[id]/+page.svelte     ← REWRITE: vault detail
│   ├── address/[principal]/+page.svelte ← REWRITE: address/principal detail
│   ├── token/[id]/+page.svelte     ← REWRITE: token/collateral detail
│   ├── liquidations/+page.svelte   ← REWRITE: liquidation history
│   └── stats/+page.svelte          ← REWRITE: historical charts + stats
```

---

## Phase 1: Foundation — Data Service & Utilities

### Task 1.1: Create `explorerHelpers.ts` — Shared Formatting Utilities

**File:** `src/vault_frontend/src/lib/utils/explorerHelpers.ts`

This file provides all the low-level formatting functions used across every explorer page. No canister calls — pure functions only.

**Create the file with these exports:**

```typescript
// ============================================================
// explorerHelpers.ts — Pure formatting utilities for the explorer
// ============================================================

import { CANISTER_IDS } from '$lib/config';

// --- Amount formatting ---

/** Convert e8s (bigint or number) to a human-readable decimal string */
export function formatE8s(e8s: bigint | number, decimals: number = 8): string {
  const n = typeof e8s === 'bigint' ? e8s : BigInt(Math.round(Number(e8s)));
  const divisor = BigInt(10 ** decimals);
  const whole = n / divisor;
  const frac = n % divisor;
  const fracStr = frac.toString().padStart(decimals, '0').replace(/0+$/, '');
  if (!fracStr) return whole.toLocaleString();
  return `${whole.toLocaleString()}.${fracStr}`;
}

/** Format a USD value from e8s */
export function formatUsd(e8s: bigint | number): string {
  const val = Number(typeof e8s === 'bigint' ? e8s : e8s) / 1e8;
  return new Intl.NumberFormat('en-US', { style: 'currency', currency: 'USD' }).format(val);
}

/** Format a USD value from a raw float */
export function formatUsdRaw(val: number): string {
  return new Intl.NumberFormat('en-US', { style: 'currency', currency: 'USD' }).format(val);
}

/** Format a percentage (0.015 → "1.50%") */
export function formatPercent(ratio: number, decimals: number = 2): string {
  return `${(ratio * 100).toFixed(decimals)}%`;
}

/** Format a collateral ratio for display (1.5 → "150%") */
export function formatCR(ratio: number): string {
  return `${(ratio * 100).toFixed(1)}%`;
}

/** Format BPS to percentage (500 → "5.00%") */
export function formatBps(bps: number): string {
  return `${(bps / 100).toFixed(2)}%`;
}

// --- Time formatting ---

/** Convert nanosecond timestamp to Date */
export function nsToDate(ns: bigint | number): Date {
  return new Date(Number(BigInt(ns) / 1_000_000n));
}

/** Format a nanosecond timestamp to locale string */
export function formatTimestamp(ns: bigint | number): string {
  return nsToDate(ns).toLocaleString();
}

/** Format a nanosecond timestamp to short date */
export function formatDate(ns: bigint | number): string {
  return nsToDate(ns).toLocaleDateString();
}

/** Relative time: "3m ago", "2h ago", "5d ago" */
export function timeAgo(ns: bigint | number): string {
  const date = nsToDate(ns);
  const seconds = Math.floor((Date.now() - date.getTime()) / 1000);
  if (seconds < 60) return 'just now';
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
  if (seconds < 86400) return `${Math.floor(seconds / 3600)}h ago`;
  if (seconds < 2592000) return `${Math.floor(seconds / 86400)}d ago`;
  return formatDate(ns);
}

// --- Principal / address formatting ---

/** Shorten a principal for display: "tfesu-vyaaa...qrd7a-cai" */
export function shortenPrincipal(principal: string, chars: number = 5): string {
  if (principal.length <= chars * 2 + 3) return principal;
  return `${principal.slice(0, chars)}…${principal.slice(-chars)}`;
}

// --- Token / collateral symbol mapping ---

const KNOWN_TOKENS: Record<string, { symbol: string; name: string; decimals: number }> = {
  [CANISTER_IDS.ICP_LEDGER]: { symbol: 'ICP', name: 'Internet Computer', decimals: 8 },
  [CANISTER_IDS.ICUSD_LEDGER]: { symbol: 'icUSD', name: 'icUSD Stablecoin', decimals: 8 },
  [CANISTER_IDS.CKUSDT_LEDGER]: { symbol: 'ckUSDT', name: 'Chain-Key USDT', decimals: 6 },
  [CANISTER_IDS.CKUSDC_LEDGER]: { symbol: 'ckUSDC', name: 'Chain-Key USDC', decimals: 6 },
  [CANISTER_IDS.THREEPOOL]: { symbol: '3USD', name: '3Pool LP Token', decimals: 18 },
};

// Additional collateral tokens will be added dynamically from collateral configs
const dynamicTokens: Record<string, { symbol: string; name: string; decimals: number }> = {};

export function registerToken(principal: string, symbol: string, name: string, decimals: number) {
  dynamicTokens[principal] = { symbol, name, decimals };
}

export function getTokenInfo(principal: string): { symbol: string; name: string; decimals: number } | null {
  return KNOWN_TOKENS[principal] || dynamicTokens[principal] || null;
}

export function getTokenSymbol(principal: string): string {
  return getTokenInfo(principal)?.symbol || shortenPrincipal(principal);
}

export function getTokenDecimals(principal: string): number {
  return getTokenInfo(principal)?.decimals ?? 8;
}

/** Format a token amount using proper decimals for that token */
export function formatTokenAmount(amount: bigint | number, tokenPrincipal: string): string {
  const decimals = getTokenDecimals(tokenPrincipal);
  return formatE8s(amount, decimals);
}

// --- Entity detection (for smart search) ---

export function isVaultId(input: string): boolean {
  return /^\d+$/.test(input.trim());
}

export function isEventIndex(input: string): boolean {
  return /^[#e]?\d+$/i.test(input.trim());
}

export function parseEventIndex(input: string): number {
  return parseInt(input.replace(/^[#e]/i, ''), 10);
}

export function isPrincipal(input: string): boolean {
  return /^[a-z0-9-]{10,}$/i.test(input.trim()) && input.includes('-');
}

const TOKEN_ALIASES: Record<string, string> = {
  icp: CANISTER_IDS.ICP_LEDGER,
  icusd: CANISTER_IDS.ICUSD_LEDGER,
  ckusdt: CANISTER_IDS.CKUSDT_LEDGER,
  ckusdc: CANISTER_IDS.CKUSDC_LEDGER,
  '3usd': CANISTER_IDS.THREEPOOL,
};

export function resolveTokenAlias(input: string): string | null {
  return TOKEN_ALIASES[input.toLowerCase().trim()] || null;
}

// --- Canister identification ---

const KNOWN_CANISTERS: Record<string, string> = {
  [CANISTER_IDS.PROTOCOL]: 'Rumi Backend',
  [CANISTER_IDS.ICP_LEDGER]: 'ICP Ledger',
  [CANISTER_IDS.ICUSD_LEDGER]: 'icUSD Ledger',
  [CANISTER_IDS.TREASURY]: 'Treasury',
  [CANISTER_IDS.STABILITY_POOL]: 'Stability Pool',
  [CANISTER_IDS.CKUSDT_LEDGER]: 'ckUSDT Ledger',
  [CANISTER_IDS.CKUSDC_LEDGER]: 'ckUSDC Ledger',
  [CANISTER_IDS.THREEPOOL]: '3Pool AMM',
};

export function getCanisterName(principal: string): string | null {
  return KNOWN_CANISTERS[principal] || null;
}

export function isKnownCanister(principal: string): boolean {
  return principal in KNOWN_CANISTERS;
}

// --- Vault health classification ---

export type HealthStatus = 'healthy' | 'caution' | 'danger' | 'liquidatable';

export function classifyVaultHealth(cr: number, liquidationRatio: number): HealthStatus {
  if (cr <= liquidationRatio) return 'liquidatable';
  if (cr < liquidationRatio * 1.1) return 'danger';
  if (cr < liquidationRatio * 1.3) return 'caution';
  return 'healthy';
}

export function healthColor(status: HealthStatus): string {
  switch (status) {
    case 'healthy': return 'text-emerald-400';
    case 'caution': return 'text-yellow-400';
    case 'danger': return 'text-orange-400';
    case 'liquidatable': return 'text-red-400';
  }
}

export function healthBg(status: HealthStatus): string {
  switch (status) {
    case 'healthy': return 'bg-emerald-400/10 border-emerald-400/20';
    case 'caution': return 'bg-yellow-400/10 border-yellow-400/20';
    case 'danger': return 'bg-orange-400/10 border-orange-400/20';
    case 'liquidatable': return 'bg-red-400/10 border-red-400/20';
  }
}
```

**Commit:** `feat(explorer): add shared formatting utilities for explorer rebuild`

---

### Task 1.2: Create `explorerService.ts` — Unified Data Service

**File:** `src/vault_frontend/src/lib/services/explorer/explorerService.ts`

This replaces both `explorerStore.ts` and `explorerDataLayer.ts` with a single service that handles caching, fetching, and state. Uses Svelte 5 runes for reactivity.

```typescript
// ============================================================
// explorerService.ts — Unified data service for the explorer
// ============================================================

import { getPublicActor } from '$lib/stores/authStore';
import { getStabilityPoolService } from '$lib/services/stabilityPoolService';
import { getThreePoolService } from '$lib/services/threePoolService';
import { registerToken, getTokenDecimals } from '$lib/utils/explorerHelpers';
import type { CandidVault, Event, CollateralConfig } from '$declarations/rumi_protocol_backend/rumi_protocol_backend.did';

// --- Cache layer ---
interface CacheEntry<T> {
  data: T;
  timestamp: number;
}

const cache = new Map<string, CacheEntry<any>>();

function getCached<T>(key: string, ttlMs: number): T | null {
  const entry = cache.get(key);
  if (entry && Date.now() - entry.timestamp < ttlMs) return entry.data as T;
  return null;
}

function setCache<T>(key: string, data: T): T {
  cache.set(key, { data, timestamp: Date.now() });
  return data;
}

export function invalidateCache(prefix?: string) {
  if (!prefix) { cache.clear(); return; }
  for (const key of cache.keys()) {
    if (key.startsWith(prefix)) cache.delete(key);
  }
}

// --- TTLs ---
const TTL = {
  STATUS: 15_000,       // 15s — changes frequently
  VAULTS: 30_000,       // 30s
  COLLATERAL: 60_000,   // 1min — config rarely changes
  EVENTS: 10_000,       // 10s — new events come in
  SNAPSHOTS: 300_000,   // 5min — hourly snapshots
  TREASURY: 60_000,     // 1min
  LIQUIDATIONS: 30_000, // 30s
  POOL: 30_000,         // 30s
};

// --- Protocol Status ---

export async function fetchProtocolStatus() {
  const cached = getCached('protocolStatus', TTL.STATUS);
  if (cached) return cached;
  const actor = await getPublicActor();
  const status = await actor.get_protocol_status();
  return setCache('protocolStatus', status);
}

// --- Collateral ---

export async function fetchCollateralConfigs(): Promise<Map<string, any>> {
  const cached = getCached<Map<string, any>>('collateralConfigs', TTL.COLLATERAL);
  if (cached) return cached;
  const actor = await getPublicActor();
  const types = await actor.get_supported_collateral_types();
  const configs = new Map<string, any>();
  await Promise.all(
    types.map(async ([principal, _status]) => {
      const pid = principal.toText();
      const config = await actor.get_collateral_config(principal);
      if (config && config.length > 0) {
        configs.set(pid, config[0]);
        // Register token for symbol lookup
        const symbol = getCollateralSymbol(pid, config[0]);
        registerToken(pid, symbol, symbol, Number(config[0].decimals));
      }
    })
  );
  return setCache('collateralConfigs', configs);
}

function getCollateralSymbol(principal: string, config: any): string {
  // Try to derive from known principals, or fall back
  const known: Record<string, string> = {
    'ryjl3-tyaaa-aaaaa-aaaba-cai': 'ICP',
  };
  return known[principal] || `COL-${principal.slice(0, 5)}`;
}

export async function fetchCollateralTotals() {
  const cached = getCached('collateralTotals', TTL.COLLATERAL);
  if (cached) return cached;
  const actor = await getPublicActor();
  const totals = await actor.get_collateral_totals();
  return setCache('collateralTotals', totals);
}

export async function fetchCollateralPrices() {
  const cached = getCached('collateralPrices', TTL.STATUS);
  if (cached) return cached;
  const actor = await getPublicActor();
  const prices = await actor.get_latest_collateral_prices();
  return setCache('collateralPrices', prices);
}

// --- Vaults ---

export async function fetchAllVaults(): Promise<CandidVault[]> {
  const cached = getCached<CandidVault[]>('allVaults', TTL.VAULTS);
  if (cached) return cached;
  const actor = await getPublicActor();
  const vaults = await actor.get_vaults();
  return setCache('allVaults', vaults);
}

export async function fetchVault(vaultId: number) {
  const actor = await getPublicActor();
  const result = await actor.get_vault(BigInt(vaultId));
  if ('Ok' in result) return result.Ok;
  throw new Error(`Vault ${vaultId} not found`);
}

export async function fetchVaultInterestRate(vaultId: number): Promise<number> {
  const actor = await getPublicActor();
  const result = await actor.get_vault_interest_rate(BigInt(vaultId));
  if ('Ok' in result) return Number(result.Ok);
  return 0;
}

export async function fetchVaultsByOwner(principal: any): Promise<bigint[]> {
  const actor = await getPublicActor();
  return await actor.get_vaults_by_principal(principal);
}

export async function fetchVaultHistory(vaultId: number): Promise<Event[]> {
  const actor = await getPublicActor();
  return await actor.get_vault_history(BigInt(vaultId));
}

// --- Events ---

export async function fetchEventCount(): Promise<number> {
  const cached = getCached<number>('eventCount', TTL.EVENTS);
  if (cached) return cached;
  const actor = await getPublicActor();
  const count = Number(await actor.get_event_count());
  return setCache('eventCount', count);
}

export async function fetchEvents(start: number, length: number): Promise<Event[]> {
  const actor = await getPublicActor();
  // get_events_filtered excludes AccrueInterest noise
  try {
    const result = await actor.get_events_filtered({
      start: BigInt(start),
      length: BigInt(length),
      event_types: [],  // empty = all except AccrueInterest
    });
    if ('Ok' in result) return result.Ok;
    return [];
  } catch {
    // Fallback to unfiltered
    return await actor.get_events(BigInt(start), BigInt(length));
  }
}

export async function fetchEventsByPrincipal(principal: any): Promise<Event[]> {
  const actor = await getPublicActor();
  const result = await actor.get_events_by_principal(principal);
  if (Array.isArray(result)) return result;
  if ('Ok' in (result as any)) return (result as any).Ok;
  return [];
}

// --- Liquidations ---

export async function fetchLiquidationRecords(start: number, length: number) {
  const actor = await getPublicActor();
  return await actor.get_liquidation_records(BigInt(start), BigInt(length));
}

export async function fetchLiquidationCount(): Promise<number> {
  const actor = await getPublicActor();
  return Number(await actor.get_liquidation_record_count());
}

export async function fetchPendingLiquidations() {
  const actor = await getPublicActor();
  return await actor.get_pending_liquidations();
}

export async function fetchBotStats() {
  const cached = getCached('botStats', TTL.LIQUIDATIONS);
  if (cached) return cached;
  const actor = await getPublicActor();
  const stats = await actor.get_bot_stats();
  return setCache('botStats', stats);
}

// --- Treasury ---

export async function fetchTreasuryStats() {
  const cached = getCached('treasuryStats', TTL.TREASURY);
  if (cached) return cached;
  const actor = await getPublicActor();
  const stats = await actor.get_treasury_stats();
  return setCache('treasuryStats', stats);
}

export async function fetchInterestSplit() {
  const cached = getCached('interestSplit', TTL.TREASURY);
  if (cached) return cached;
  const actor = await getPublicActor();
  const split = await actor.get_interest_split();
  return setCache('interestSplit', split);
}

// --- Protocol Snapshots (for charts) ---

export async function fetchSnapshotCount(): Promise<number> {
  const cached = getCached<number>('snapshotCount', TTL.SNAPSHOTS);
  if (cached) return cached;
  const actor = await getPublicActor();
  const count = Number(await actor.get_protocol_snapshot_count());
  return setCache('snapshotCount', count);
}

export async function fetchSnapshots(start: number, length: number) {
  const actor = await getPublicActor();
  return await actor.get_protocol_snapshots(BigInt(start), BigInt(length));
}

export async function fetchAllSnapshots() {
  const count = await fetchSnapshotCount();
  const batchSize = 2000;
  const batches: Promise<any[]>[] = [];
  for (let i = 0; i < count; i += batchSize) {
    batches.push(fetchSnapshots(i, Math.min(batchSize, count - i)));
  }
  const results = await Promise.all(batches);
  return results.flat();
}

// --- Stability Pool ---

export async function fetchStabilityPoolStatus() {
  const cached = getCached('spStatus', TTL.POOL);
  if (cached) return cached;
  try {
    const service = await getStabilityPoolService();
    const status = await service.get_status();
    return setCache('spStatus', status);
  } catch { return null; }
}

export async function fetchStabilityPoolLiquidations(limit: number = 100) {
  try {
    const service = await getStabilityPoolService();
    return await service.get_liquidation_history([BigInt(limit)]);
  } catch { return []; }
}

// --- 3Pool ---

export async function fetchThreePoolStatus() {
  const cached = getCached('3poolStatus', TTL.POOL);
  if (cached) return cached;
  try {
    const service = await getThreePoolService();
    const status = await service.get_pool_status();
    return setCache('3poolStatus', status);
  } catch { return null; }
}

export async function fetchSwapEvents(start: number, length: number) {
  try {
    const service = await getThreePoolService();
    return await service.get_swap_events(BigInt(start), BigInt(length));
  } catch { return []; }
}

export async function fetchSwapEventCount(): Promise<number> {
  try {
    const service = await getThreePoolService();
    return Number(await service.get_swap_event_count());
  } catch { return 0; }
}

// --- Recovery / Mode ---

export async function fetchProtocolMode() {
  const actor = await getPublicActor();
  return await actor.get_protocol_mode();
}
```

**Notes for implementor:**
- The `getPublicActor`, `getStabilityPoolService`, `getThreePoolService` functions already exist in the codebase — check `authStore.ts` and `services/` for exact imports and adjust as needed.
- The cache is module-level (singleton). TTLs prevent hammering canisters while keeping data fresh.
- Each function can be called independently — pages compose what they need.

**Commit:** `feat(explorer): add unified explorer data service with caching`

---

### Task 1.3: Rewrite `explorerFormatters.ts` — Human-Readable Event Formatting

**File:** `src/vault_frontend/src/lib/utils/explorerFormatters.ts`

Complete rewrite. The formatter must handle all 46 event types and produce:
1. A **one-line summary** for event lists (e.g., "Vault #50 opened with 1.5 ICP, borrowed 100 icUSD")
2. **Structured detail fields** for event detail pages — each field has a type so the UI knows how to render it (link to vault, link to address, formatted amount, etc.)
3. A **category** for filtering (vault_ops, liquidation, redemption, stability_pool, admin, system)

```typescript
// ============================================================
// explorerFormatters.ts — Event formatting for human-readable display
// ============================================================

import {
  formatE8s, formatTokenAmount, formatPercent, formatCR, formatBps,
  timeAgo, formatTimestamp, getTokenSymbol, getTokenDecimals,
  shortenPrincipal, getCanisterName
} from '$lib/utils/explorerHelpers';

// --- Types ---

export type EventCategory = 'vault_ops' | 'liquidation' | 'redemption' | 'stability_pool' | 'threepool' | 'admin' | 'system';

export type FieldType = 'text' | 'amount' | 'usd' | 'percentage' | 'address' | 'vault' | 'token' | 'event' | 'timestamp' | 'json' | 'canister' | 'block_index' | 'ratio';

export interface EventField {
  label: string;
  value: string;
  type: FieldType;
  /** For links: the raw ID/principal to link to */
  linkTarget?: string;
  /** For amounts: the token principal (to determine decimals) */
  tokenPrincipal?: string;
}

export interface FormattedEvent {
  /** Short one-line summary for event lists */
  summary: string;
  /** Human-readable event type name */
  typeName: string;
  /** Category for filtering */
  category: EventCategory;
  /** Color class for the type badge */
  badgeColor: string;
  /** Structured fields for detail view */
  fields: EventField[];
}

// --- Badge colors by category ---

const BADGE_COLORS: Record<EventCategory, string> = {
  vault_ops: 'bg-blue-500/20 text-blue-300 border-blue-500/30',
  liquidation: 'bg-red-500/20 text-red-300 border-red-500/30',
  redemption: 'bg-purple-500/20 text-purple-300 border-purple-500/30',
  stability_pool: 'bg-teal-500/20 text-teal-300 border-teal-500/30',
  threepool: 'bg-cyan-500/20 text-cyan-300 border-cyan-500/30',
  admin: 'bg-amber-500/20 text-amber-300 border-amber-500/30',
  system: 'bg-gray-500/20 text-gray-300 border-gray-500/30',
};

// --- Helper: extract the event variant key ---

function getEventKey(event: any): string {
  if (!event?.event_type) return 'Unknown';
  const eventType = event.event_type;
  // Candid unions are objects with a single key
  const keys = Object.keys(eventType);
  return keys[0] || 'Unknown';
}

function getEventData(event: any): any {
  const key = getEventKey(event);
  return event.event_type?.[key] || {};
}

// --- Field builder helpers ---

function field(label: string, value: string, type: FieldType = 'text', extra?: Partial<EventField>): EventField {
  return { label, value, type, ...extra };
}

function vaultField(label: string, vaultId: bigint | number): EventField {
  return field(label, `#${vaultId}`, 'vault', { linkTarget: String(vaultId) });
}

function addressField(label: string, principal: any): EventField {
  const pid = typeof principal === 'string' ? principal : principal?.toText?.() || String(principal);
  const name = getCanisterName(pid);
  return field(label, name || shortenPrincipal(pid), 'address', { linkTarget: pid });
}

function tokenField(label: string, principal: any): EventField {
  const pid = typeof principal === 'string' ? principal : principal?.toText?.() || String(principal);
  return field(label, getTokenSymbol(pid), 'token', { linkTarget: pid });
}

function amountField(label: string, amount: bigint | number, tokenPrincipal?: string): EventField {
  const pid = tokenPrincipal || '';
  const symbol = pid ? getTokenSymbol(pid) : '';
  const formatted = pid ? formatTokenAmount(amount, pid) : formatE8s(amount);
  return field(label, `${formatted} ${symbol}`.trim(), 'amount', { tokenPrincipal: pid });
}

function blockIndexField(label: string, blockIndex: bigint | number): EventField {
  return field(label, String(blockIndex), 'block_index');
}

function timestampField(label: string, ts: bigint | number): EventField {
  return field(label, formatTimestamp(ts), 'timestamp');
}

function percentField(label: string, value: number | string): EventField {
  const num = typeof value === 'string' ? parseFloat(value) : value;
  return field(label, formatPercent(num), 'percentage');
}

function ratioField(label: string, value: number | string): EventField {
  const num = typeof value === 'string' ? parseFloat(value) : value;
  return field(label, formatCR(num), 'ratio');
}

// --- Main formatter ---

export function formatEvent(event: any): FormattedEvent {
  const key = getEventKey(event);
  const data = getEventData(event);
  const ts = event.timestamp ? Number(event.timestamp) : 0;

  switch (key) {
    // ─── Vault Lifecycle ───
    case 'OpenVault': {
      const collateral = data.collateral_amount ? formatE8s(data.collateral_amount) : '?';
      const debt = data.borrowed_icusd_amount ? formatE8s(data.borrowed_icusd_amount) : '?';
      const colType = data.collateral_type?.toText?.() || '';
      const symbol = colType ? getTokenSymbol(colType) : 'ICP';
      return {
        summary: `Vault #${data.vault_id ?? '?'} opened with ${collateral} ${symbol}, borrowed ${debt} icUSD`,
        typeName: 'Open Vault',
        category: 'vault_ops',
        badgeColor: BADGE_COLORS.vault_ops,
        fields: [
          ...(data.vault_id != null ? [vaultField('Vault', data.vault_id)] : []),
          ...(data.owner ? [addressField('Owner', data.owner)] : []),
          ...(data.collateral_type ? [tokenField('Collateral Type', data.collateral_type)] : []),
          ...(data.collateral_amount ? [amountField('Collateral Deposited', data.collateral_amount, colType)] : []),
          ...(data.borrowed_icusd_amount ? [amountField('icUSD Borrowed', data.borrowed_icusd_amount)] : []),
          ...(data.fee_amount ? [amountField('Borrowing Fee', data.fee_amount)] : []),
          ...(data.block_index != null ? [blockIndexField('Block Index', data.block_index)] : []),
        ],
      };
    }

    case 'CloseVault':
      return {
        summary: `Vault #${data.vault_id ?? '?'} closed`,
        typeName: 'Close Vault',
        category: 'vault_ops',
        badgeColor: BADGE_COLORS.vault_ops,
        fields: [
          ...(data.vault_id != null ? [vaultField('Vault', data.vault_id)] : []),
          ...(data.caller ? [addressField('Closed By', data.caller)] : []),
          ...(data.block_index != null ? [blockIndexField('Block Index', data.block_index)] : []),
        ],
      };

    case 'WithdrawAndCloseVault':
    case 'VaultWithdrawnAndClosed':
      return {
        summary: `Vault #${data.vault_id ?? '?'} withdrawn and closed (${data.amount ? formatE8s(data.amount) + ' ICP' : ''})`,
        typeName: 'Withdraw & Close',
        category: 'vault_ops',
        badgeColor: BADGE_COLORS.vault_ops,
        fields: [
          ...(data.vault_id != null ? [vaultField('Vault', data.vault_id)] : []),
          ...(data.caller ? [addressField('Owner', data.caller)] : []),
          ...(data.amount ? [amountField('Collateral Returned', data.amount)] : []),
          ...(data.block_index != null ? [blockIndexField('Block Index', data.block_index)] : []),
        ],
      };

    // ─── Borrowing & Repayment ───
    case 'BorrowFromVault':
      return {
        summary: `Borrowed ${data.borrowed_amount ? formatE8s(data.borrowed_amount) : '?'} icUSD from Vault #${data.vault_id ?? '?'}`,
        typeName: 'Borrow',
        category: 'vault_ops',
        badgeColor: BADGE_COLORS.vault_ops,
        fields: [
          ...(data.vault_id != null ? [vaultField('Vault', data.vault_id)] : []),
          ...(data.caller ? [addressField('Borrower', data.caller)] : []),
          ...(data.borrowed_amount ? [amountField('Amount Borrowed', data.borrowed_amount)] : []),
          ...(data.fee_amount ? [amountField('Borrowing Fee', data.fee_amount)] : []),
          ...(data.block_index != null ? [blockIndexField('Block Index', data.block_index)] : []),
        ],
      };

    case 'RepayToVault':
      return {
        summary: `Repaid ${data.repayed_amount ? formatE8s(data.repayed_amount) : '?'} icUSD to Vault #${data.vault_id ?? '?'}`,
        typeName: 'Repay',
        category: 'vault_ops',
        badgeColor: BADGE_COLORS.vault_ops,
        fields: [
          ...(data.vault_id != null ? [vaultField('Vault', data.vault_id)] : []),
          ...(data.caller ? [addressField('Repayer', data.caller)] : []),
          ...(data.repayed_amount ? [amountField('Amount Repaid', data.repayed_amount)] : []),
          ...(data.block_index != null ? [blockIndexField('Block Index', data.block_index)] : []),
        ],
      };

    case 'DustForgiven':
      return {
        summary: `Dust forgiven on Vault #${data.vault_id ?? '?'} (${data.amount ? formatE8s(data.amount) : '?'} icUSD)`,
        typeName: 'Dust Forgiven',
        category: 'vault_ops',
        badgeColor: BADGE_COLORS.vault_ops,
        fields: [
          ...(data.vault_id != null ? [vaultField('Vault', data.vault_id)] : []),
          ...(data.amount ? [amountField('Dust Amount', data.amount)] : []),
        ],
      };

    // ─── Collateral Management ───
    case 'AddMarginToVault':
      return {
        summary: `Added ${data.margin_added ? formatE8s(data.margin_added) : '?'} collateral to Vault #${data.vault_id ?? '?'}`,
        typeName: 'Add Collateral',
        category: 'vault_ops',
        badgeColor: BADGE_COLORS.vault_ops,
        fields: [
          ...(data.vault_id != null ? [vaultField('Vault', data.vault_id)] : []),
          ...(data.caller ? [addressField('Depositor', data.caller)] : []),
          ...(data.margin_added ? [amountField('Amount Added', data.margin_added)] : []),
          ...(data.block_index != null ? [blockIndexField('Block Index', data.block_index)] : []),
        ],
      };

    case 'CollateralWithdrawn':
    case 'PartialCollateralWithdrawn':
      return {
        summary: `Withdrew ${data.amount ? formatE8s(data.amount) : '?'} collateral from Vault #${data.vault_id ?? '?'}`,
        typeName: key === 'PartialCollateralWithdrawn' ? 'Partial Withdrawal' : 'Withdraw Collateral',
        category: 'vault_ops',
        badgeColor: BADGE_COLORS.vault_ops,
        fields: [
          ...(data.vault_id != null ? [vaultField('Vault', data.vault_id)] : []),
          ...(data.caller ? [addressField('Owner', data.caller)] : []),
          ...(data.amount ? [amountField('Amount Withdrawn', data.amount)] : []),
          ...(data.block_index != null ? [blockIndexField('Block Index', data.block_index)] : []),
        ],
      };

    case 'MarginTransfer':
      return {
        summary: `Margin transfer for Vault #${data.vault_id ?? '?'}`,
        typeName: 'Margin Transfer',
        category: 'vault_ops',
        badgeColor: BADGE_COLORS.vault_ops,
        fields: [
          ...(data.vault_id != null ? [vaultField('Vault', data.vault_id)] : []),
          ...(data.block_index != null ? [blockIndexField('Block Index', data.block_index)] : []),
        ],
      };

    // ─── Liquidations ───
    case 'LiquidateVault':
      return {
        summary: `Vault #${data.vault_id ?? '?'} fully liquidated`,
        typeName: 'Full Liquidation',
        category: 'liquidation',
        badgeColor: BADGE_COLORS.liquidation,
        fields: [
          ...(data.vault_id != null ? [vaultField('Vault', data.vault_id)] : []),
          ...(data.liquidator ? [addressField('Liquidator', data.liquidator)] : []),
          ...(data.icp_rate ? [field('ICP Price', `$${data.icp_rate}`, 'usd')] : []),
          ...(data.mode ? [field('Protocol Mode', JSON.stringify(data.mode), 'text')] : []),
        ],
      };

    case 'PartialLiquidateVault':
      return {
        summary: `Vault #${data.vault_id ?? '?'} partially liquidated (${data.liquidator_payment ? formatE8s(data.liquidator_payment) + ' icUSD' : ''})`,
        typeName: 'Partial Liquidation',
        category: 'liquidation',
        badgeColor: BADGE_COLORS.liquidation,
        fields: [
          ...(data.vault_id != null ? [vaultField('Vault', data.vault_id)] : []),
          ...(data.liquidator ? [addressField('Liquidator', data.liquidator)] : []),
          ...(data.liquidator_payment ? [amountField('Debt Covered', data.liquidator_payment)] : []),
          ...(data.icp_to_liquidator ? [amountField('Collateral to Liquidator', data.icp_to_liquidator)] : []),
          ...(data.protocol_fee_collateral ? [amountField('Protocol Fee', data.protocol_fee_collateral)] : []),
          ...(data.icp_rate ? [field('ICP Price', `$${data.icp_rate}`, 'usd')] : []),
        ],
      };

    case 'RedistributeVault':
      return {
        summary: `Vault #${data.vault_id ?? '?'} redistributed`,
        typeName: 'Redistribution',
        category: 'liquidation',
        badgeColor: BADGE_COLORS.liquidation,
        fields: [
          ...(data.vault_id != null ? [vaultField('Vault', data.vault_id)] : []),
        ],
      };

    case 'BotLiquidationClaimed':
      return {
        summary: `Bot claimed liquidation rights on Vault #${data.vault_id ?? '?'}`,
        typeName: 'Bot Claim',
        category: 'liquidation',
        badgeColor: BADGE_COLORS.liquidation,
        fields: [
          ...(data.vault_id != null ? [vaultField('Vault', data.vault_id)] : []),
          ...(data.bot ? [addressField('Bot', data.bot)] : []),
        ],
      };

    case 'BotLiquidationConfirmed':
      return {
        summary: `Bot confirmed liquidation of Vault #${data.vault_id ?? '?'}`,
        typeName: 'Bot Confirmed',
        category: 'liquidation',
        badgeColor: BADGE_COLORS.liquidation,
        fields: [
          ...(data.vault_id != null ? [vaultField('Vault', data.vault_id)] : []),
          ...(data.debt_burned ? [amountField('Debt Burned', data.debt_burned)] : []),
          ...(data.collateral_seized ? [amountField('Collateral Seized', data.collateral_seized)] : []),
          ...(data.liquidator_fee_bps != null ? [field('Liquidator Fee', formatBps(Number(data.liquidator_fee_bps)), 'percentage')] : []),
        ],
      };

    case 'BotLiquidationCanceled':
      return {
        summary: `Bot liquidation canceled for Vault #${data.vault_id ?? '?'}`,
        typeName: 'Bot Canceled',
        category: 'liquidation',
        badgeColor: BADGE_COLORS.liquidation,
        fields: [
          ...(data.vault_id != null ? [vaultField('Vault', data.vault_id)] : []),
        ],
      };

    // ─── Stability Pool ───
    case 'ProvideLiquidity':
      return {
        summary: `Deposited ${data.amount ? formatE8s(data.amount) : '?'} to Stability Pool`,
        typeName: 'SP Deposit',
        category: 'stability_pool',
        badgeColor: BADGE_COLORS.stability_pool,
        fields: [
          ...(data.caller ? [addressField('Depositor', data.caller)] : []),
          ...(data.amount ? [amountField('Amount', data.amount)] : []),
          ...(data.block_index != null ? [blockIndexField('Block Index', data.block_index)] : []),
        ],
      };

    case 'WithdrawLiquidity':
      return {
        summary: `Withdrew ${data.amount ? formatE8s(data.amount) : '?'} from Stability Pool`,
        typeName: 'SP Withdrawal',
        category: 'stability_pool',
        badgeColor: BADGE_COLORS.stability_pool,
        fields: [
          ...(data.caller ? [addressField('Withdrawer', data.caller)] : []),
          ...(data.amount ? [amountField('Amount', data.amount)] : []),
          ...(data.block_index != null ? [blockIndexField('Block Index', data.block_index)] : []),
        ],
      };

    case 'ClaimLiquidityReturns':
      return {
        summary: `Claimed ${data.amount ? formatE8s(data.amount) : '?'} ICP from Stability Pool`,
        typeName: 'SP Claim',
        category: 'stability_pool',
        badgeColor: BADGE_COLORS.stability_pool,
        fields: [
          ...(data.caller ? [addressField('Claimer', data.caller)] : []),
          ...(data.amount ? [amountField('Amount Claimed', data.amount)] : []),
          ...(data.block_index != null ? [blockIndexField('Block Index', data.block_index)] : []),
        ],
      };

    // ─── Redemptions ───
    case 'RedemptionOnVaults':
      return {
        summary: `Redeemed ${data.icusd_amount ? formatE8s(data.icusd_amount) : '?'} icUSD across vaults`,
        typeName: 'Redemption',
        category: 'redemption',
        badgeColor: BADGE_COLORS.redemption,
        fields: [
          ...(data.owner ? [addressField('Redeemer', data.owner)] : []),
          ...(data.icusd_amount ? [amountField('icUSD Redeemed', data.icusd_amount)] : []),
          ...(data.fee_amount ? [amountField('Redemption Fee', data.fee_amount)] : []),
          ...(data.current_icp_rate ? [field('ICP Price', `$${data.current_icp_rate}`, 'usd')] : []),
          ...(data.icusd_block_index != null ? [blockIndexField('icUSD Block Index', data.icusd_block_index)] : []),
        ],
      };

    case 'RedemptionTransfered':
      return {
        summary: `Redemption collateral transferred`,
        typeName: 'Redemption Transfer',
        category: 'redemption',
        badgeColor: BADGE_COLORS.redemption,
        fields: [
          ...(data.icusd_block_index != null ? [blockIndexField('icUSD Block Index', data.icusd_block_index)] : []),
          ...(data.icp_block_index != null ? [blockIndexField('ICP Block Index', data.icp_block_index)] : []),
        ],
      };

    case 'ReserveRedemption':
      return {
        summary: `Reserve redemption: ${data.icusd_amount ? formatE8s(data.icusd_amount) : '?'} icUSD`,
        typeName: 'Reserve Redemption',
        category: 'redemption',
        badgeColor: BADGE_COLORS.redemption,
        fields: [
          ...(data.owner ? [addressField('Redeemer', data.owner)] : []),
          ...(data.icusd_amount ? [amountField('icUSD Redeemed', data.icusd_amount)] : []),
          ...(data.fee_amount ? [amountField('Fee (icUSD)', data.fee_amount)] : []),
          ...(data.stable_token_ledger ? [tokenField('Stablecoin Received', data.stable_token_ledger)] : []),
          ...(data.stable_amount_sent ? [amountField('Stablecoin Amount', data.stable_amount_sent, data.stable_token_ledger?.toText?.())] : []),
          ...(data.fee_stable_amount ? [amountField('Fee (Stablecoin)', data.fee_stable_amount, data.stable_token_ledger?.toText?.())] : []),
        ],
      };

    // ─── Admin: Collateral Management ───
    case 'AddCollateralType':
      return {
        summary: `New collateral type added: ${data.collateral_type?.toText ? shortenPrincipal(data.collateral_type.toText()) : '?'}`,
        typeName: 'Add Collateral Type',
        category: 'admin',
        badgeColor: BADGE_COLORS.admin,
        fields: [
          ...(data.collateral_type ? [tokenField('Collateral', data.collateral_type)] : []),
          field('Details', 'See configuration', 'text'),
        ],
      };

    case 'UpdateCollateralStatus':
      return {
        summary: `Collateral status updated to ${data.status ? JSON.stringify(data.status) : '?'}`,
        typeName: 'Collateral Status',
        category: 'admin',
        badgeColor: BADGE_COLORS.admin,
        fields: [
          ...(data.collateral_type ? [tokenField('Collateral', data.collateral_type)] : []),
          ...(data.status ? [field('New Status', JSON.stringify(data.status), 'text')] : []),
        ],
      };

    case 'UpdateCollateralConfig':
      return {
        summary: `Collateral config updated`,
        typeName: 'Config Update',
        category: 'admin',
        badgeColor: BADGE_COLORS.admin,
        fields: [
          ...(data.collateral_type ? [tokenField('Collateral', data.collateral_type)] : []),
          field('Details', 'Configuration changed', 'text'),
        ],
      };

    // ─── Admin: Fee/Rate Changes ───
    case 'SetBorrowingFee':
    case 'SetCollateralBorrowingFee':
    case 'SetLiquidationBonus':
    case 'SetInterestRate':
    case 'SetInterestSplit':
    case 'SetRedemptionFeeFloor':
    case 'SetRedemptionFeeCeiling':
    case 'SetCkstableRepayFee':
    case 'SetMinIcusdAmount':
    case 'SetGlobalIcusdMintCap':
    case 'SetMaxPartialLiquidationRatio':
    case 'SetRecoveryTargetCr':
    case 'SetRecoveryCrMultiplier':
    case 'SetLiquidationProtocolShare':
    case 'SetInterestPoolShare':
    case 'SetReserveRedemptionsEnabled':
    case 'SetReserveRedemptionFee': {
      const typeName = key.replace(/^Set/, '').replace(/([A-Z])/g, ' $1').trim();
      const value = data.rate || data.amount || data.share || data.multiplier || data.fee || data.split || data.enabled?.toString() || data.cap || JSON.stringify(data);
      return {
        summary: `${typeName} set to ${typeof value === 'string' ? value : JSON.stringify(value)}`,
        typeName: `Set ${typeName}`,
        category: 'admin',
        badgeColor: BADGE_COLORS.admin,
        fields: [
          field('Parameter', typeName, 'text'),
          field('New Value', String(value), 'text'),
          ...(data.collateral_type ? [tokenField('Collateral', data.collateral_type)] : []),
        ],
      };
    }

    // ─── Admin: Rate Curves ───
    case 'SetRateCurveMarkers':
    case 'SetRecoveryRateCurve':
    case 'SetBorrowingFeeCurve':
      return {
        summary: `${key.replace(/^Set/, '').replace(/([A-Z])/g, ' $1').trim()} updated`,
        typeName: key.replace(/^Set/, 'Set ').replace(/([A-Z])/g, ' $1').trim(),
        category: 'admin',
        badgeColor: BADGE_COLORS.admin,
        fields: [
          ...(data.collateral_type ? [field('Collateral', typeof data.collateral_type === 'string' ? data.collateral_type : 'Global', 'text')] : []),
          field('Markers', data.markers || JSON.stringify(data), 'json'),
        ],
      };

    // ─── Admin: RMR Parameters ───
    case 'SetRmrFloor':
    case 'SetRmrCeiling':
    case 'SetRmrFloorCr':
    case 'SetRmrCeilingCr':
      return {
        summary: `${key.replace(/^Set/, '').replace(/([A-Z])/g, ' $1').trim()} set to ${data.value || '?'}`,
        typeName: key.replace(/^Set/, 'Set ').replace(/([A-Z])/g, ' $1').trim(),
        category: 'admin',
        badgeColor: BADGE_COLORS.admin,
        fields: [
          field('Parameter', key.replace(/^Set/, ''), 'text'),
          field('Value', String(data.value || '?'), 'text'),
        ],
      };

    // ─── Admin: Principals & Config ───
    case 'SetStableLedgerPrincipal':
    case 'SetTreasuryPrincipal':
    case 'SetStabilityPoolPrincipal':
    case 'SetLiquidationBotPrincipal':
    case 'SetThreePoolCanister':
      return {
        summary: `${key.replace(/^Set/, '').replace(/([A-Z])/g, ' $1').trim()} updated`,
        typeName: key.replace(/^Set/, 'Set ').replace(/([A-Z])/g, ' $1').trim(),
        category: 'admin',
        badgeColor: BADGE_COLORS.admin,
        fields: [
          ...(data.principal ? [addressField('New Principal', data.principal)] : []),
          ...(data.canister ? [addressField('Canister', data.canister)] : []),
          ...(data.token_type ? [field('Token Type', String(data.token_type), 'text')] : []),
        ],
      };

    case 'SetBotBudget':
      return {
        summary: `Bot budget set to ${data.total_e8s ? formatE8s(data.total_e8s) : '?'} ICP`,
        typeName: 'Set Bot Budget',
        category: 'admin',
        badgeColor: BADGE_COLORS.admin,
        fields: [
          ...(data.total_e8s ? [amountField('Budget', data.total_e8s)] : []),
          ...(data.start_timestamp ? [timestampField('Start', data.start_timestamp)] : []),
        ],
      };

    case 'SetBotAllowedCollateralTypes':
      return {
        summary: `Bot allowed collateral types updated`,
        typeName: 'Bot Collateral Types',
        category: 'admin',
        badgeColor: BADGE_COLORS.admin,
        fields: [
          field('Collateral Types', JSON.stringify(data.collateral_types || []), 'json'),
        ],
      };

    case 'SetStableTokenEnabled':
      return {
        summary: `${data.token_type || '?'} ${data.enabled ? 'enabled' : 'disabled'}`,
        typeName: 'Toggle Stablecoin',
        category: 'admin',
        badgeColor: BADGE_COLORS.admin,
        fields: [
          field('Token Type', String(data.token_type || '?'), 'text'),
          field('Enabled', String(data.enabled ?? '?'), 'text'),
        ],
      };

    case 'SetHealthyCr':
      return {
        summary: `Healthy CR set to ${data.healthy_cr || 'cleared'}`,
        typeName: 'Set Healthy CR',
        category: 'admin',
        badgeColor: BADGE_COLORS.admin,
        fields: [
          ...(data.collateral_type ? [field('Collateral', String(data.collateral_type), 'text')] : []),
          field('Healthy CR', data.healthy_cr || 'None (cleared)', 'text'),
        ],
      };

    case 'SetRecoveryParameters':
      return {
        summary: `Recovery parameters updated for ${data.collateral_type ? shortenPrincipal(String(data.collateral_type)) : 'collateral'}`,
        typeName: 'Recovery Params',
        category: 'admin',
        badgeColor: BADGE_COLORS.admin,
        fields: [
          ...(data.collateral_type ? [tokenField('Collateral', data.collateral_type)] : []),
          ...(data.recovery_borrowing_fee ? [field('Recovery Borrowing Fee', String(data.recovery_borrowing_fee), 'percentage')] : []),
          ...(data.recovery_interest_rate_apr ? [field('Recovery Interest Rate', String(data.recovery_interest_rate_apr), 'percentage')] : []),
        ],
      };

    // ─── Admin: Corrections ───
    case 'AdminVaultCorrection':
      return {
        summary: `Admin correction on Vault #${data.vault_id ?? '?'}: ${data.reason || ''}`,
        typeName: 'Admin Correction',
        category: 'admin',
        badgeColor: BADGE_COLORS.admin,
        fields: [
          ...(data.vault_id != null ? [vaultField('Vault', data.vault_id)] : []),
          ...(data.old_amount ? [amountField('Old Amount', data.old_amount)] : []),
          ...(data.new_amount ? [amountField('New Amount', data.new_amount)] : []),
          ...(data.reason ? [field('Reason', data.reason, 'text')] : []),
        ],
      };

    case 'AdminMint':
      return {
        summary: `Admin minted ${data.amount ? formatE8s(data.amount) : '?'} icUSD: ${data.reason || ''}`,
        typeName: 'Admin Mint',
        category: 'admin',
        badgeColor: BADGE_COLORS.admin,
        fields: [
          ...(data.amount ? [amountField('Amount', data.amount)] : []),
          ...(data.to ? [addressField('Recipient', data.to)] : []),
          ...(data.reason ? [field('Reason', data.reason, 'text')] : []),
          ...(data.block_index != null ? [blockIndexField('Block Index', data.block_index)] : []),
        ],
      };

    case 'AdminSweepToTreasury':
      return {
        summary: `Swept ${data.amount ? formatE8s(data.amount) : '?'} ICP to treasury`,
        typeName: 'Sweep to Treasury',
        category: 'admin',
        badgeColor: BADGE_COLORS.admin,
        fields: [
          ...(data.amount ? [amountField('Amount', data.amount)] : []),
          ...(data.treasury ? [addressField('Treasury', data.treasury)] : []),
          ...(data.reason ? [field('Reason', data.reason, 'text')] : []),
          ...(data.block_index != null ? [blockIndexField('Block Index', data.block_index)] : []),
        ],
      };

    // ─── System ───
    case 'Init':
      return {
        summary: 'Protocol initialized',
        typeName: 'Init',
        category: 'system',
        badgeColor: BADGE_COLORS.system,
        fields: [field('Details', 'Protocol initialized with default configuration', 'text')],
      };

    case 'Upgrade':
      return {
        summary: `Protocol upgraded${data.description ? ': ' + data.description : ''}`,
        typeName: 'Upgrade',
        category: 'system',
        badgeColor: BADGE_COLORS.system,
        fields: [
          ...(data.description ? [field('Description', data.description, 'text')] : []),
          ...(data.mode ? [field('Mode', JSON.stringify(data.mode), 'text')] : []),
        ],
      };

    case 'AccrueInterest':
      return {
        summary: 'Interest accrued (system)',
        typeName: 'Accrue Interest',
        category: 'system',
        badgeColor: BADGE_COLORS.system,
        fields: [],
      };

    // ─── Fallback ───
    default:
      return {
        summary: `${key.replace(/([A-Z])/g, ' $1').trim()}`,
        typeName: key.replace(/([A-Z])/g, ' $1').trim(),
        category: 'system',
        badgeColor: BADGE_COLORS.system,
        fields: [field('Raw Data', JSON.stringify(data, null, 2), 'json')],
      };
  }
}

/** Get just the category for an event (for filtering without full format) */
export function getEventCategory(event: any): EventCategory {
  const key = getEventKey(event);
  if (['LiquidateVault', 'PartialLiquidateVault', 'RedistributeVault', 'BotLiquidationClaimed', 'BotLiquidationConfirmed', 'BotLiquidationCanceled'].includes(key)) return 'liquidation';
  if (['RedemptionOnVaults', 'RedemptionTransfered', 'ReserveRedemption'].includes(key)) return 'redemption';
  if (['ProvideLiquidity', 'WithdrawLiquidity', 'ClaimLiquidityReturns'].includes(key)) return 'stability_pool';
  if (['OpenVault', 'CloseVault', 'WithdrawAndCloseVault', 'VaultWithdrawnAndClosed', 'BorrowFromVault', 'RepayToVault', 'DustForgiven', 'AddMarginToVault', 'CollateralWithdrawn', 'PartialCollateralWithdrawn', 'MarginTransfer'].includes(key)) return 'vault_ops';
  if (key.startsWith('Set') || key.startsWith('Admin') || key.startsWith('Add') || key.startsWith('Update')) return 'admin';
  return 'system';
}

/** Get all available categories with labels */
export const EVENT_CATEGORIES: { key: EventCategory; label: string }[] = [
  { key: 'vault_ops', label: 'Vault Operations' },
  { key: 'liquidation', label: 'Liquidations' },
  { key: 'redemption', label: 'Redemptions' },
  { key: 'stability_pool', label: 'Stability Pool' },
  { key: 'threepool', label: '3Pool' },
  { key: 'admin', label: 'Admin' },
  { key: 'system', label: 'System' },
];
```

**Commit:** `feat(explorer): rewrite event formatters with full coverage of all 46 event types`

---

## Phase 2: Core Components

### Task 2.1: Rewrite `EntityLink.svelte`

**File:** `src/vault_frontend/src/lib/components/explorer/EntityLink.svelte`

The most important component — makes everything clickable. Must handle: vault IDs, principals (users & canisters), tokens, events.

```svelte
<script lang="ts">
  import { shortenPrincipal, getCanisterName, getTokenSymbol, isKnownCanister } from '$lib/utils/explorerHelpers';

  interface Props {
    type: 'vault' | 'address' | 'token' | 'event' | 'canister' | 'block_index';
    value: string;
    label?: string;
    short?: boolean;
  }

  let { type, value, label, short = true }: Props = $props();

  const href = $derived.by(() => {
    switch (type) {
      case 'vault': return `/explorer/vault/${value}`;
      case 'address': return `/explorer/address/${value}`;
      case 'token': return `/explorer/token/${value}`;
      case 'event': return `/explorer/event/${value}`;
      case 'canister': return `/explorer/address/${value}`;
      case 'block_index': return null; // no page for block indices
      default: return null;
    }
  });

  const displayText = $derived.by(() => {
    if (label) return label;
    switch (type) {
      case 'vault': return `Vault #${value}`;
      case 'event': return `Event #${value}`;
      case 'token': return getTokenSymbol(value);
      case 'block_index': return `#${value}`;
      case 'address':
      case 'canister': {
        const name = getCanisterName(value);
        if (name) return name;
        return short ? shortenPrincipal(value) : value;
      }
      default: return value;
    }
  });

  const icon = $derived.by(() => {
    switch (type) {
      case 'vault': return '🏦';
      case 'address': return isKnownCanister(value) ? '📦' : '👤';
      case 'token': return '🪙';
      case 'event': return '📋';
      case 'canister': return '📦';
      case 'block_index': return '🔗';
      default: return '';
    }
  });
</script>

{#if href}
  <a
    {href}
    class="inline-flex items-center gap-1 text-blue-400 hover:text-blue-300 hover:underline font-mono text-sm transition-colors"
    title={value}
  >
    <span class="text-xs">{icon}</span>
    <span>{displayText}</span>
  </a>
{:else}
  <span class="inline-flex items-center gap-1 font-mono text-sm text-gray-300" title={value}>
    <span class="text-xs">{icon}</span>
    <span>{displayText}</span>
  </span>
{/if}
```

**Commit:** `feat(explorer): rewrite EntityLink with smart display for all entity types`

---

### Task 2.2: Create `StatCard.svelte`, `AmountDisplay.svelte`, `CopyButton.svelte`, `StatusBadge.svelte`, `TimeAgo.svelte`

Create all 5 small utility components. Each is simple and focused.

**File:** `src/vault_frontend/src/lib/components/explorer/StatCard.svelte`
```svelte
<script lang="ts">
  interface Props {
    label: string;
    value: string;
    subtitle?: string;
    trend?: 'up' | 'down' | 'neutral';
    trendValue?: string;
    size?: 'sm' | 'md' | 'lg';
  }

  let { label, value, subtitle, trend, trendValue, size = 'md' }: Props = $props();

  const trendColor = $derived(
    trend === 'up' ? 'text-emerald-400' : trend === 'down' ? 'text-red-400' : 'text-gray-400'
  );
  const trendIcon = $derived(trend === 'up' ? '↑' : trend === 'down' ? '↓' : '→');
  const valueSize = $derived(
    size === 'lg' ? 'text-3xl' : size === 'sm' ? 'text-lg' : 'text-2xl'
  );
</script>

<div class="bg-gray-800/50 border border-gray-700/50 rounded-xl p-4 hover:border-gray-600/50 transition-colors">
  <div class="text-xs font-medium text-gray-400 uppercase tracking-wider mb-1">{label}</div>
  <div class="{valueSize} font-bold text-white font-mono">{value}</div>
  {#if subtitle}
    <div class="text-xs text-gray-500 mt-1">{subtitle}</div>
  {/if}
  {#if trend && trendValue}
    <div class="flex items-center gap-1 mt-1 text-xs {trendColor}">
      <span>{trendIcon}</span>
      <span>{trendValue}</span>
    </div>
  {/if}
</div>
```

**File:** `src/vault_frontend/src/lib/components/explorer/AmountDisplay.svelte`
```svelte
<script lang="ts">
  import { formatTokenAmount, formatUsdRaw, getTokenSymbol } from '$lib/utils/explorerHelpers';

  interface Props {
    amount: bigint | number;
    tokenPrincipal?: string;
    showUsd?: boolean;
    price?: number;
    size?: 'sm' | 'md' | 'lg';
  }

  let { amount, tokenPrincipal = '', showUsd = false, price, size = 'md' }: Props = $props();

  const formatted = $derived(tokenPrincipal ? formatTokenAmount(amount, tokenPrincipal) : String(amount));
  const symbol = $derived(tokenPrincipal ? getTokenSymbol(tokenPrincipal) : '');
  const usdValue = $derived.by(() => {
    if (!showUsd || !price) return null;
    const numAmount = Number(typeof amount === 'bigint' ? amount : amount) / 1e8;
    return formatUsdRaw(numAmount * price);
  });

  const textSize = $derived(size === 'lg' ? 'text-lg' : size === 'sm' ? 'text-xs' : 'text-sm');
</script>

<span class="inline-flex items-baseline gap-1 {textSize}">
  <span class="font-mono font-medium text-white">{formatted}</span>
  {#if symbol}
    <span class="text-gray-400">{symbol}</span>
  {/if}
  {#if usdValue}
    <span class="text-gray-500">({usdValue})</span>
  {/if}
</span>
```

**File:** `src/vault_frontend/src/lib/components/explorer/CopyButton.svelte`
```svelte
<script lang="ts">
  interface Props {
    text: string;
    size?: 'sm' | 'md';
  }

  let { text, size = 'sm' }: Props = $props();
  let copied = $state(false);

  async function copy() {
    await navigator.clipboard.writeText(text);
    copied = true;
    setTimeout(() => (copied = false), 2000);
  }

  const iconSize = $derived(size === 'sm' ? 'w-3.5 h-3.5' : 'w-4 h-4');
</script>

<button
  onclick={copy}
  class="inline-flex items-center text-gray-400 hover:text-gray-200 transition-colors"
  title="Copy to clipboard"
>
  {#if copied}
    <svg class="{iconSize} text-emerald-400" fill="none" stroke="currentColor" viewBox="0 0 24 24">
      <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 13l4 4L19 7" />
    </svg>
  {:else}
    <svg class="{iconSize}" fill="none" stroke="currentColor" viewBox="0 0 24 24">
      <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M8 16H6a2 2 0 01-2-2V6a2 2 0 012-2h8a2 2 0 012 2v2m-6 12h8a2 2 0 002-2v-8a2 2 0 00-2-2h-8a2 2 0 00-2 2v8a2 2 0 002 2z" />
    </svg>
  {/if}
</button>
```

**File:** `src/vault_frontend/src/lib/components/explorer/StatusBadge.svelte`
```svelte
<script lang="ts">
  interface Props {
    status: string;
    size?: 'sm' | 'md';
  }

  let { status, size = 'sm' }: Props = $props();

  const styles = $derived.by(() => {
    const s = status.toLowerCase();
    if (s === 'normal' || s === 'active' || s === 'healthy') return 'bg-emerald-500/20 text-emerald-300 border-emerald-500/30';
    if (s === 'recovery' || s === 'caution' || s === 'paused') return 'bg-yellow-500/20 text-yellow-300 border-yellow-500/30';
    if (s === 'frozen' || s === 'danger' || s === 'liquidatable' || s === 'deprecated' || s === 'sunset') return 'bg-red-500/20 text-red-300 border-red-500/30';
    return 'bg-gray-500/20 text-gray-300 border-gray-500/30';
  });

  const padding = $derived(size === 'sm' ? 'px-2 py-0.5 text-xs' : 'px-3 py-1 text-sm');
</script>

<span class="inline-flex items-center rounded-full border font-medium {styles} {padding}">
  {status}
</span>
```

**File:** `src/vault_frontend/src/lib/components/explorer/TimeAgo.svelte`
```svelte
<script lang="ts">
  import { timeAgo, formatTimestamp } from '$lib/utils/explorerHelpers';

  interface Props {
    timestamp: bigint | number;
    showFull?: boolean;
  }

  let { timestamp, showFull = false }: Props = $props();

  const relative = $derived(timeAgo(timestamp));
  const full = $derived(formatTimestamp(timestamp));
</script>

{#if showFull}
  <span class="text-gray-300" title={full}>{full}</span>
{:else}
  <span class="text-gray-400 cursor-help" title={full}>{relative}</span>
{/if}
```

**Commit:** `feat(explorer): add StatCard, AmountDisplay, CopyButton, StatusBadge, TimeAgo components`

---

### Task 2.3: Rewrite `DataTable.svelte` and `EventRow.svelte`

**File:** `src/vault_frontend/src/lib/components/explorer/DataTable.svelte`

A generic, sortable table component that handles loading states, empty states, and flexible column definitions.

```svelte
<script lang="ts" generics="T">
  import type { Snippet } from 'svelte';

  interface Column<T> {
    key: string;
    label: string;
    sortable?: boolean;
    align?: 'left' | 'center' | 'right';
    width?: string;
  }

  interface Props {
    columns: Column<T>[];
    data: T[];
    loading?: boolean;
    emptyMessage?: string;
    rowKey?: (item: T) => string;
    row: Snippet<[T, number]>;
    compact?: boolean;
  }

  let { columns, data, loading = false, emptyMessage = 'No data', rowKey, row, compact = false }: Props = $props();

  let sortKey = $state('');
  let sortDir = $state<'asc' | 'desc'>('desc');

  function toggleSort(key: string) {
    if (sortKey === key) {
      sortDir = sortDir === 'asc' ? 'desc' : 'asc';
    } else {
      sortKey = key;
      sortDir = 'desc';
    }
  }

  const padding = $derived(compact ? 'px-3 py-2' : 'px-4 py-3');
</script>

<div class="overflow-x-auto rounded-xl border border-gray-700/50">
  <table class="w-full text-sm">
    <thead>
      <tr class="border-b border-gray-700/50 bg-gray-800/30">
        {#each columns as col}
          <th
            class="{padding} text-{col.align || 'left'} text-xs font-medium text-gray-400 uppercase tracking-wider {col.width || ''}"
            class:cursor-pointer={col.sortable}
            onclick={() => col.sortable && toggleSort(col.key)}
          >
            <span class="inline-flex items-center gap-1">
              {col.label}
              {#if col.sortable && sortKey === col.key}
                <span class="text-blue-400">{sortDir === 'asc' ? '↑' : '↓'}</span>
              {/if}
            </span>
          </th>
        {/each}
      </tr>
    </thead>
    <tbody class="divide-y divide-gray-800/50">
      {#if loading}
        <tr>
          <td colspan={columns.length} class="{padding} text-center text-gray-500">
            <div class="flex items-center justify-center gap-2">
              <div class="w-4 h-4 border-2 border-blue-400 border-t-transparent rounded-full animate-spin"></div>
              Loading...
            </div>
          </td>
        </tr>
      {:else if data.length === 0}
        <tr>
          <td colspan={columns.length} class="{padding} text-center text-gray-500">{emptyMessage}</td>
        </tr>
      {:else}
        {#each data as item, index (rowKey ? rowKey(item) : index)}
          {@render row(item, index)}
        {/each}
      {/if}
    </tbody>
  </table>
</div>
```

**File:** `src/vault_frontend/src/lib/components/explorer/EventRow.svelte`

Rich event row with inline details — shows type badge, summary, timestamp, and links.

```svelte
<script lang="ts">
  import { formatEvent } from '$lib/utils/explorerFormatters';
  import EntityLink from './EntityLink.svelte';
  import TimeAgo from './TimeAgo.svelte';

  interface Props {
    event: any;
    index: number;
    showTimestamp?: boolean;
  }

  let { event, index, showTimestamp = true }: Props = $props();

  const formatted = $derived(formatEvent(event));
</script>

<tr class="hover:bg-gray-800/30 transition-colors">
  <td class="px-3 py-2 font-mono text-xs text-gray-500 w-16">
    <EntityLink type="event" value={String(index)} />
  </td>
  {#if showTimestamp}
    <td class="px-3 py-2 text-xs w-24">
      {#if event.timestamp}
        <TimeAgo timestamp={event.timestamp} />
      {:else}
        <span class="text-gray-600">—</span>
      {/if}
    </td>
  {/if}
  <td class="px-3 py-2 w-36">
    <span class="inline-flex items-center rounded-full border px-2 py-0.5 text-xs font-medium {formatted.badgeColor}">
      {formatted.typeName}
    </span>
  </td>
  <td class="px-3 py-2 text-sm text-gray-300">
    {formatted.summary}
  </td>
  <td class="px-3 py-2 text-right">
    <a href="/explorer/event/{index}" class="text-xs text-blue-400 hover:text-blue-300 hover:underline">
      Details →
    </a>
  </td>
</tr>
```

**Commit:** `feat(explorer): rewrite DataTable and EventRow components`

---

## Phase 3: Page Rebuilds — Dashboard

### Task 3.1: Rewrite the Dashboard (`/explorer/+page.svelte`)

This is the crown jewel. The dashboard should show at a glance:

**Section 1 — Hero Stats (top row of cards):**
- Protocol Mode (Normal/Recovery/Frozen) with status badge
- Total TVL (USD)
- Total Debt (icUSD)
- System Collateral Ratio
- Total Vaults (active)
- Total Events

**Section 2 — Collateral Breakdown:**
- Table: one row per collateral type
- Columns: Token, Price, Total Locked, Total Locked (USD), Total Debt, Vault Count, Utilization (debt/ceiling %), Interest Rate, Status

**Section 3 — Pools:**
- Stability Pool card: Total deposited, depositor count, total liquidations executed, emergency status
- 3Pool card: Token balances, virtual price, swap fee, total swaps

**Section 4 — Treasury & Revenue:**
- Total accrued interest
- Interest split visualization (where interest flows)
- Pending treasury amounts
- Liquidation protocol share

**Section 5 — Liquidation Health:**
- Bot stats: budget remaining, total liquidations, fees paid
- At-risk vaults (sorted by CR ascending, top 10)
- Pending liquidations

**Section 6 — Recent Activity:**
- Last 20 events with EventRow component

**Implementation:** This is a large file (~400-500 lines). The implementor should:

1. Import all needed service functions from `explorerService.ts`
2. Use `$state` for all data, `$effect` for fetching on mount
3. Use `$derived` for computed values (e.g., filtering at-risk vaults)
4. Auto-refresh every 30 seconds
5. Show skeleton/loading states per section (not one big spinner)
6. Every number, address, vault ID, and token should be clickable via `EntityLink`

**Key data fetches on mount (parallel):**
```typescript
const [status, collateralTotals, prices, vaults, eventCount, treasuryStats, interestSplit, botStats, spStatus, threePoolStatus] = await Promise.all([
  fetchProtocolStatus(),
  fetchCollateralTotals(),
  fetchCollateralPrices(),
  fetchAllVaults(),
  fetchEventCount(),
  fetchTreasuryStats(),
  fetchInterestSplit(),
  fetchBotStats(),
  fetchStabilityPoolStatus(),
  fetchThreePoolStatus(),
]);
```

Plus fetch recent events (last 20) and collateral configs.

**Commit:** `feat(explorer): rebuild dashboard with comprehensive protocol overview`

---

## Phase 4: Page Rebuilds — Events & Event Detail

### Task 4.1: Rewrite Events List (`/explorer/events/+page.svelte`)

**Features:**
- Category filter tabs: All | Vault Ops | Liquidations | Redemptions | Stability Pool | 3Pool | Admin
- Paginated event table (100 per page) with total count
- Each row uses `EventRow` component
- Newest-first ordering
- Search/filter within current page (client-side text filter on summary)
- Show total event count prominently

**Data flow:**
1. Fetch `eventCount` on mount
2. Calculate page: `start = Math.max(0, eventCount - (page * pageSize))`
3. Fetch events for current page
4. Filter by selected category client-side (using `getEventCategory`)
5. Render with `DataTable` + `EventRow`
6. `Pagination` component at bottom

**Commit:** `feat(explorer): rebuild events list with category filters and pagination`

---

### Task 4.2: Rewrite Event Detail (`/explorer/event/[index]/+page.svelte`)

**Features:**
- Header: Event #N, type badge, timestamp (both relative and absolute)
- Structured fields section: render each `EventField` with appropriate component
  - `vault` → `EntityLink type="vault"`
  - `address` → `EntityLink type="address"` + `CopyButton`
  - `token` → `EntityLink type="token"`
  - `amount` → `AmountDisplay`
  - `timestamp` → `TimeAgo showFull`
  - `percentage` / `ratio` → formatted text
  - `json` → collapsible `<pre>` block
  - `block_index` → formatted number
  - `text` → plain text
- "Related Entities" sidebar: extract all vault IDs, addresses, tokens from fields and list as links
- "Raw Event Data" collapsible section at bottom showing full JSON
- Navigation: ← Previous Event | Next Event → links

**Data flow:**
1. Get index from route params
2. Fetch single event: `fetchEvents(index, 1)`
3. Format with `formatEvent()`
4. Render fields dynamically based on field type

**Commit:** `feat(explorer): rebuild event detail page with rich field rendering`

---

## Phase 5: Page Rebuilds — Vault Detail

### Task 5.1: Rewrite Vault Detail (`/explorer/vault/[id]/+page.svelte`)

**Features:**

**Header:**
- Vault #N, status badge (active/closed/liquidated), collateral type token badge
- Owner as clickable EntityLink + CopyButton

**Stats Cards Row:**
- Collateral Amount (with USD value using current price)
- Debt (icUSD borrowed + accrued interest)
- Collateral Ratio with health classification + VaultHealthBar
- Current Interest Rate (dynamic, from `get_vault_interest_rate`)
- Liquidation Price (calculated: debt × liquidation_ratio / collateral_amount)

**Collateral Config Section:**
- Table showing the config for this vault's collateral type
- Liquidation ratio, borrow threshold, borrowing fee, interest rate, debt ceiling, min debt

**Vault History Timeline:**
- Full event history from `get_vault_history()`
- Each event rendered as EventRow
- Chronological order (oldest first for timeline feel, toggle available)

**Data flow:**
1. Fetch vault: `fetchVault(id)`
2. Parallel: `fetchVaultInterestRate(id)`, `fetchCollateralConfigs()`, `fetchCollateralPrices()`, `fetchVaultHistory(id)`
3. Compute CR: `(collateral_amount × price) / (borrowed + interest)`
4. Compute liquidation price: `(borrowed + interest) × liquidation_ratio / collateral_amount`

**Commit:** `feat(explorer): rebuild vault detail with full stats, config, and history timeline`

---

## Phase 6: Page Rebuilds — Address Detail

### Task 6.1: Rewrite Address Detail (`/explorer/address/[principal]/+page.svelte`)

**Features:**

**Header:**
- Principal ID (full, with CopyButton)
- If known canister: show canister name + "Canister" badge
- If user: show "User Account" label

**Summary Cards:**
- Total Vaults (active / total)
- Total Collateral Value (USD)
- Total Debt (icUSD)
- Weighted Average CR

**Vaults Section:**
- Table of all vaults owned by this principal
- Columns: Vault ID (link), Collateral Type, Collateral Amount, Debt, CR, Status
- Each vault row clickable

**Activity Feed:**
- All events involving this principal (from `get_events_by_principal`)
- Rendered with EventRow components
- Same category filter tabs as the main events page

**Data flow:**
1. Parse principal from route params
2. Fetch vaults: `fetchVaultsByOwner(principal)` → then `fetchVault(id)` for each
3. Fetch events: `fetchEventsByPrincipal(principal)`
4. Fetch collateral configs + prices for CR/USD calculations

**Commit:** `feat(explorer): rebuild address detail with vaults summary and activity feed`

---

## Phase 7: Page Rebuilds — Token Detail

### Task 7.1: Rewrite Token Detail (`/explorer/token/[id]/+page.svelte`)

**Features:**

**Header:**
- Token symbol + badge, full principal + CopyButton
- Status badge (Active/Paused/Frozen)
- Current price

**Stats Cards:**
- Total Collateral Locked (in this token)
- Total Collateral Value (USD)
- Total Debt Minted Against This Collateral
- Active Vaults Using This Token
- Debt Ceiling Utilization (bar + percentage)

**Configuration Table:**
- Full CollateralConfig display: liquidation ratio, borrow threshold, borrowing fee, interest rate, debt ceiling, min vault debt, ledger fee, price source, recovery parameters, rate curve markers

**Vaults Using This Token:**
- Table of all vaults with this collateral type
- Columns: Vault ID, Owner, Collateral, Debt, CR, Status

**Recent Activity:**
- Events filtered to this collateral type
- EventRow rendering

**Data flow:**
1. Get token principal from route params
2. Fetch collateral config for this token
3. Fetch all vaults, filter to this collateral type
4. Fetch collateral totals for aggregate stats
5. Fetch recent events, filter client-side to events mentioning this collateral

**Commit:** `feat(explorer): rebuild token detail with collateral config, vaults, and activity`

---

## Phase 8: Page Rebuilds — Liquidations & Stats

### Task 8.1: Rewrite Liquidations Page (`/explorer/liquidations/+page.svelte`)

**Features:**

**Summary Cards:**
- Total Liquidations (from `get_liquidation_record_count`)
- Pending Liquidations (count)
- Bot Budget Remaining
- Bot Total Liquidations

**Filter Tabs:**
- All | Full Liquidations | Partial Liquidations | Redistributions | Bot Liquidations

**Liquidation Table:**
- Fetch from `get_liquidation_records()` with pagination
- Columns: Event #, Time, Vault, Type (full/partial/redistribution/bot), Debt Covered, Collateral Seized, Liquidator, ICP Price at Time
- Each row links to event detail

**Pending Liquidations Section:**
- Table of in-flight liquidations from `get_pending_liquidations()`

**Bot Stats Section:**
- Budget: remaining / total
- Per-collateral: total liquidated, fees paid, collateral received

**Stability Pool Liquidations:**
- From `get_liquidation_history()` on the stability pool canister
- Show how much debt was absorbed and collateral distributed

**Commit:** `feat(explorer): rebuild liquidations page with bot stats and pending liquidations`

---

### Task 8.2: Rewrite Stats Page (`/explorer/stats/+page.svelte`)

**Features:**

**Current Metrics (cards):**
- Same as dashboard hero stats but with more detail

**Per-Collateral Breakdown Table:**
- Expanded view of each collateral: price, locked amount, debt, vault count, interest rate, utilization, status

**Historical Charts (using Layerchart):**
- TVL Over Time (from protocol snapshots)
- Total Debt Over Time
- System CR Over Time (derived: TVL/debt)
- Vault Count Over Time (if available in snapshots)
- Per-collateral TVL stacked area chart

**Time Range Selector:**
- 24h | 7d | 30d | 90d | All

**Data flow:**
1. Fetch all snapshots (batched: `fetchAllSnapshots()`)
2. Filter by selected time range
3. Transform for chart rendering
4. Fetch current status for the "current metrics" section

**Commit:** `feat(explorer): rebuild stats page with historical charts and time range filters`

---

## Phase 9: Layout & Navigation

### Task 9.1: Rewrite Explorer Layout (`/explorer/+layout.svelte`)

**Features:**
- Explorer header bar with Rumi logo + "Explorer" label
- Navigation tabs: Dashboard | Events | Liquidations | Stats
- Smart search bar (right-aligned)
  - Detects vault IDs, event indices, token symbols, principals
  - Shows hint text: "Search vault ID, address, token, or event #"
  - Routes to appropriate detail page
- Breadcrumb trail for sub-pages (e.g., Dashboard > Vault #50)
- Mobile hamburger menu for nav tabs

**Search logic (reuse from `explorerHelpers.ts`):**
```typescript
function handleSearch(query: string) {
  const q = query.trim();
  if (isVaultId(q)) goto(`/explorer/vault/${q}`);
  else if (isEventIndex(q)) goto(`/explorer/event/${parseEventIndex(q)}`);
  else if (resolveTokenAlias(q)) goto(`/explorer/token/${resolveTokenAlias(q)}`);
  else if (isPrincipal(q)) goto(`/explorer/address/${q}`);
}
```

**Commit:** `feat(explorer): rebuild layout with nav tabs, smart search, and breadcrumbs`

---

## Phase 10: Integration & Polish

### Task 10.1: Wire Up All Imports and Test Build

After all files are written:
1. Run `npm run build` (or the SvelteKit build command) and fix any import errors
2. Verify all components import from correct paths
3. Fix any TypeScript errors from Candid type mismatches
4. Test that the `/explorer` route loads without errors in dev mode

**Command:** `cd src/vault_frontend && npm run build`

**Commit:** `fix(explorer): resolve build errors and import paths`

---

### Task 10.2: Polish Pass — Loading States, Error Handling, Empty States

Go through each page and ensure:
1. Every fetch has a loading state (skeleton or spinner)
2. Every fetch has error handling (try/catch → error message display)
3. Empty states show helpful messages ("No vaults found for this address")
4. All amounts display with correct decimal places for their token type
5. All timestamps show both relative and full on hover
6. Mobile responsive — tables scroll horizontally, cards stack vertically

**Commit:** `fix(explorer): add loading states, error handling, and responsive polish`

---

### Task 10.3: Final Verification

1. Navigate to each route and verify it loads data:
   - `/explorer` — dashboard loads all sections
   - `/explorer/events` — events paginate correctly
   - `/explorer/event/0` — first event shows detail
   - `/explorer/vault/1` — a vault shows full detail
   - `/explorer/address/{some-principal}` — address page works
   - `/explorer/token/{ICP-ledger-id}` — token page works
   - `/explorer/liquidations` — liquidation history loads
   - `/explorer/stats` — charts render
2. Test smart search with various inputs
3. Verify all entity links navigate correctly
4. Check that auto-refresh works on dashboard (30s interval)

**Commit:** `feat(explorer): explorer rebuild complete — all pages verified`

---

## Summary

| Phase | Tasks | Description |
|-------|-------|-------------|
| 1 | 1.1–1.3 | Foundation: helpers, data service, formatters |
| 2 | 2.1–2.3 | Core components: EntityLink, StatCard, DataTable, etc. |
| 3 | 3.1 | Dashboard rebuild |
| 4 | 4.1–4.2 | Events list + event detail |
| 5 | 5.1 | Vault detail |
| 6 | 6.1 | Address detail |
| 7 | 7.1 | Token detail |
| 8 | 8.1–8.2 | Liquidations + stats pages |
| 9 | 9.1 | Layout & navigation |
| 10 | 10.1–10.3 | Integration, polish, verification |

**Total: 14 tasks across 10 phases**

Each task produces a working commit. The data service and formatters (Phase 1) unblock all page rebuilds. Components (Phase 2) are shared across all pages. Pages (Phases 3-8) can be built in parallel by subagents since they're independent files. Layout (Phase 9) ties it together. Polish (Phase 10) is the final pass.
