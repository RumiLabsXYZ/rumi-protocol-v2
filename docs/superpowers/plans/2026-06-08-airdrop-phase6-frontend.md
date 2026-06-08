# Airdrop Phase 6 Frontend Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the public-facing airdrop points UI in `vault_frontend` — a read-only `/points` (My Points status + earn CTA) and `/points/leaderboard`, backed by a new `pointsService.ts`.

**Architecture:** All data comes from public `query` calls on the `rumi_points` canister via an anonymous `HttpAgent` actor (no wallet signing). Correctness-critical logic (points formatting, season-phase, body-state, rank) lives in pure, unit-tested functions in `points.ts`; Svelte 5 components are thin renderers. The section is gated behind `POINTS_ENABLED` (true once the canister id is configured), so it ships dark and lights up at deploy.

**Tech Stack:** SvelteKit (Svelte 5 runes), TypeScript, Tailwind, `@dfinity/agent`, vitest. Mirrors `src/vault_frontend/src/lib/services/explorer/analyticsService.ts`.

**Working directory for all commands:** `src/vault_frontend`. **Branch:** `feat/airdrop-phase6-frontend` (already created).

**Reference types** (from `src/declarations/rumi_points/rumi_points.did.d.ts`):
- `PublicEpochStatus { open_epoch: [] | [PublicOpenEpoch]; driver_enabled: boolean; current_epoch_index: bigint; ... }`
- `PointsConfig { registered_count: bigint; season_start_ns: bigint; season_end_ns: bigint; current_epoch_index: bigint; ... }`
- `PrincipalState { principal: Principal; registered_at_ns: bigint; total_points: bigint; first_qualifying_action: QualifyingAction; active_deposits: Array<[DepositKey, DepositRecord]>; repayment_events: Array<RepaymentEvent>; last_epoch_processed: bigint; }`
- `LeaderboardEntry { principal: Principal; total_points: bigint; rank: number; estimated_share_bps: number; }`
- `QualifyingAction = {MintIcUsd:null} | {DepositStabilityPool:null} | {Deposit3Pool:null} | {ProvideAmmLiquidity:null} | {RepayVault:null}`
- `DepositKey { asset: AssetType; venue: Venue }`, `Venue = {Amm} | {ThreePool} | {Vault} | {StabilityPool}`
- `_SERVICE.get_principal_state: (Principal) -> ([] | [PrincipalState])`, `get_leaderboard: (number, number) -> Array<LeaderboardEntry>`

---

### Task 1: Pure utilities + tests (`points.ts`)

**Files:**
- Create: `src/vault_frontend/src/lib/utils/points.ts`
- Test: `src/vault_frontend/src/lib/utils/points.test.ts`

- [ ] **Step 1: Write the failing test**

`src/vault_frontend/src/lib/utils/points.test.ts`:
```ts
import { describe, it, expect } from 'vitest';
import { Principal } from '@dfinity/principal';
import {
  formatPoints,
  qualifyingActionLabel,
  seasonState,
  bodyState,
  deriveRank,
} from './points';
import type {
  PublicEpochStatus,
  PointsConfig,
  PrincipalState,
  LeaderboardEntry,
} from '$declarations/rumi_points/rumi_points.did';

const P1 = Principal.fromText('aaaaa-aa');
const P2 = Principal.fromText('2vxsx-fae');

function status(open: boolean, driver: boolean): PublicEpochStatus {
  return {
    open_epoch: open
      ? [{ epoch_index: 0n, epoch_start_ns: 0n, snapshot_a_ns: 0n, snapshot_b_ns: 0n, epoch_end_ns: 0n }]
      : [],
    snapshot_seed_committed: true,
    driver_interval_secs: 604800n,
    revealed_seed_count: 0n,
    current_epoch_index: 0n,
    driver_enabled: driver,
  };
}
function cfg(endNs: bigint): PointsConfig {
  return {
    admin: P1,
    registered_count: 10n,
    snapshot_seed_committed: true,
    excluded_count: 9,
    season_start_ns: 1_780_272_000_000_000_000n,
    season_end_ns: endNs,
    current_epoch_index: 0n,
  };
}

describe('formatPoints', () => {
  it('renders usd_e8s-days as USD-days', () => {
    // $100 held 1 day = 100 * 1e8 * 1 = 1e10 raw -> "100"
    expect(formatPoints(10_000_000_000n)).toBe('100');
  });
  it('keeps two decimals', () => {
    // 123.45 USD-days = 12_345_000_000 raw
    expect(formatPoints(12_345_000_000n)).toBe('123.45');
  });
  it('groups thousands', () => {
    expect(formatPoints(1_234_567n * 100_000_000n)).toBe('1,234,567');
  });
  it('is zero for empty', () => {
    expect(formatPoints(0n)).toBe('0');
  });
});

describe('qualifyingActionLabel', () => {
  it('maps each variant', () => {
    expect(qualifyingActionLabel({ MintIcUsd: null })).toBe('Minted icUSD');
    expect(qualifyingActionLabel({ DepositStabilityPool: null })).toBe('Deposited to the stability pool');
    expect(qualifyingActionLabel({ Deposit3Pool: null })).toBe('Added 3pool liquidity');
    expect(qualifyingActionLabel({ ProvideAmmLiquidity: null })).toBe('Provided AMM liquidity');
    expect(qualifyingActionLabel({ RepayVault: null })).toBe('Repaid a vault');
  });
});

describe('seasonState', () => {
  const now = 1_784_000_000_000_000_000n;
  it('unknown when data missing', () => {
    expect(seasonState(null, null, now)).toBe('unknown');
  });
  it('ended when now >= season_end', () => {
    expect(seasonState(status(false, false), cfg(now), now)).toBe('ended');
  });
  it('live when an epoch is open', () => {
    expect(seasonState(status(true, true), cfg(now + 1n), now)).toBe('live');
  });
  it('live when driver enabled between epochs', () => {
    expect(seasonState(status(false, true), cfg(now + 1n), now)).toBe('live');
  });
  it('pre when not started and not ended', () => {
    expect(seasonState(status(false, false), cfg(now + 1n), now)).toBe('pre');
  });
});

describe('bodyState', () => {
  const ps = { principal: P1, total_points: 1n } as unknown as PrincipalState;
  it('disconnected', () => {
    expect(bodyState({ connected: false, excluded: false, state: null })).toBe('disconnected');
  });
  it('excluded wins over state', () => {
    expect(bodyState({ connected: true, excluded: true, state: ps })).toBe('excluded');
  });
  it('enrolled when state present', () => {
    expect(bodyState({ connected: true, excluded: false, state: ps })).toBe('enrolled');
  });
  it('not_enrolled when connected without state', () => {
    expect(bodyState({ connected: true, excluded: false, state: null })).toBe('not_enrolled');
  });
});

describe('deriveRank', () => {
  const rows: LeaderboardEntry[] = [
    { principal: P1, total_points: 5n, rank: 1, estimated_share_bps: 0 },
    { principal: P2, total_points: 3n, rank: 2, estimated_share_bps: 0 },
  ];
  it('finds a present principal', () => {
    expect(deriveRank(rows, P2.toText())).toBe(2);
  });
  it('null when outside the slice', () => {
    expect(deriveRank(rows, Principal.fromText('aaaaa-aa').toText())).toBe(1);
    expect(deriveRank([], P1.toText())).toBeNull();
  });
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd src/vault_frontend && npx vitest run src/lib/utils/points.test.ts`
Expected: FAIL — `Failed to resolve import "./points"` / functions not defined.

- [ ] **Step 3: Write minimal implementation**

`src/vault_frontend/src/lib/utils/points.ts`:
```ts
/**
 * points.ts — pure helpers for the airdrop points UI. No I/O, fully unit-tested.
 *
 * `total_points` from the canister is in usd_e8s-days (USD value x 1e8, held
 * over time in days). Display divides by 1e8 to get human-readable "USD-days".
 */
import type {
  PublicEpochStatus,
  PointsConfig,
  PrincipalState,
  LeaderboardEntry,
  QualifyingAction,
} from '$declarations/rumi_points/rumi_points.did';

const POINTS_FORMATTER = new Intl.NumberFormat('en-US', {
  minimumFractionDigits: 0,
  maximumFractionDigits: 2,
});

/** usd_e8s-days (bigint) -> grouped USD-days string. Reduces via bigint first
 *  so very large whale values never overflow Number's safe-integer range. */
export function formatPoints(raw: bigint): string {
  const hundredthsOfUsdDay = raw / 1_000_000n; // 1e8 / 1e6 = 100
  const usdDays = Number(hundredthsOfUsdDay) / 100;
  return POINTS_FORMATTER.format(usdDays);
}

export function qualifyingActionLabel(a: QualifyingAction): string {
  if ('MintIcUsd' in a) return 'Minted icUSD';
  if ('DepositStabilityPool' in a) return 'Deposited to the stability pool';
  if ('Deposit3Pool' in a) return 'Added 3pool liquidity';
  if ('ProvideAmmLiquidity' in a) return 'Provided AMM liquidity';
  if ('RepayVault' in a) return 'Repaid a vault';
  return 'Qualifying action';
}

export type SeasonPhase = 'unknown' | 'pre' | 'live' | 'ended';

/** Derive the season banner phase. `nowNs` is the current time in ns. */
export function seasonState(
  status: PublicEpochStatus | null,
  config: PointsConfig | null,
  nowNs: bigint,
): SeasonPhase {
  if (!status || !config) return 'unknown';
  if (nowNs >= config.season_end_ns) return 'ended';
  if (status.open_epoch.length > 0 || status.driver_enabled) return 'live';
  return 'pre';
}

export type BodyState = 'disconnected' | 'not_enrolled' | 'enrolled' | 'excluded';

export function bodyState(args: {
  connected: boolean;
  excluded: boolean;
  state: PrincipalState | null;
}): BodyState {
  if (!args.connected) return 'disconnected';
  if (args.excluded) return 'excluded';
  if (args.state) return 'enrolled';
  return 'not_enrolled';
}

/** Find a principal's rank in a bounded leaderboard slice; null if absent. */
export function deriveRank(entries: LeaderboardEntry[], principalText: string): number | null {
  const hit = entries.find((e) => e.principal.toText() === principalText);
  return hit ? hit.rank : null;
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd src/vault_frontend && npx vitest run src/lib/utils/points.test.ts`
Expected: PASS — all describe blocks green.

- [ ] **Step 5: Commit**

```bash
git add src/vault_frontend/src/lib/utils/points.ts src/vault_frontend/src/lib/utils/points.test.ts
git commit -m "feat(points): pure utils for airdrop points UI (format, season-state, body-state, rank)"
```

---

### Task 2: Config gate (`config.ts`)

**Files:**
- Modify: `src/vault_frontend/src/lib/config.ts` (CANISTER_IDS object ends at line 33; add a sibling export after it)

- [ ] **Step 1: Add the canister id placeholder + enable flag**

In `src/vault_frontend/src/lib/config.ts`, inside the `CANISTER_IDS` object (after the `ICPSWAP_ICUSD_ICP_POOL` line, before the closing `} as const;`), add:
```ts
  // Rumi Points (airdrop accrual engine). Empty until the canister is reserved
  // + deployed (trio deploy per src/rumi_points/DEPLOY_RUNBOOK.md); the /points
  // section stays hidden while this is "".
  RUMI_POINTS: "",
```
Then, immediately after the `} as const;` that closes `CANISTER_IDS` (line 33), add:
```ts
/** The /points airdrop section is shown only once the rumi_points canister id
 *  is configured above. Flip on by filling RUMI_POINTS at deploy time. */
export const POINTS_ENABLED: boolean = CANISTER_IDS.RUMI_POINTS !== "";
```

- [ ] **Step 2: Type-check**

Run: `cd src/vault_frontend && npm run check`
Expected: completes with no new errors referencing `config.ts` or `POINTS_ENABLED`. (Pre-existing unrelated warnings, if any, are acceptable — compare against a baseline `npm run check` on the untouched file.)

- [ ] **Step 3: Commit**

```bash
git add src/vault_frontend/src/lib/config.ts
git commit -m "feat(points): add RUMI_POINTS canister id slot + POINTS_ENABLED gate"
```

---

### Task 3: Points service (`pointsService.ts`)

**Files:**
- Create: `src/vault_frontend/src/lib/services/pointsService.ts`

- [ ] **Step 1: Write the service**

`src/vault_frontend/src/lib/services/pointsService.ts`:
```ts
/**
 * pointsService.ts — anonymous, TTL-cached query functions for the rumi_points
 * canister. Mirrors analyticsService.ts. All calls are public queries; no wallet.
 */
import type { Principal } from '@dfinity/principal';
import { Actor, HttpAgent } from '@dfinity/agent';
import { CANISTER_IDS, CONFIG } from '$lib/config';
import { idlFactory } from '$declarations/rumi_points/rumi_points.did.js';
import type {
  _SERVICE as PointsService,
  PublicEpochStatus,
  PointsConfig,
  PrincipalState,
  LeaderboardEntry,
} from '$declarations/rumi_points/rumi_points.did';

const TTL = {
  STATUS: 15_000,
  CONFIG: 60_000,
  PRINCIPAL: 15_000,
  LEADERBOARD: 30_000,
} as const;

interface CacheEntry<T> {
  data: T;
  ts: number;
}
const cache = new Map<string, CacheEntry<unknown>>();

function getCached<T>(key: string, ttlMs: number): T | null {
  const entry = cache.get(key);
  if (!entry) return null;
  if (Date.now() - entry.ts > ttlMs) {
    cache.delete(key);
    return null;
  }
  return entry.data as T;
}
function setCache<T>(key: string, data: T): T {
  cache.set(key, { data, ts: Date.now() });
  return data;
}
export function invalidatePointsCache(prefix?: string): void {
  if (!prefix) {
    cache.clear();
    return;
  }
  for (const key of cache.keys()) if (key.startsWith(prefix)) cache.delete(key);
}

let _actor: PointsService | null = null;
function getActor(): PointsService {
  if (_actor) return _actor;
  const host = CONFIG.isLocal ? 'http://localhost:4943' : 'https://icp0.io';
  const agent = new HttpAgent({ host });
  if (CONFIG.isLocal) {
    agent.fetchRootKey().catch((e) => console.warn('[pointsService] fetchRootKey failed', e));
  }
  _actor = Actor.createActor<PointsService>(idlFactory, {
    agent,
    canisterId: CANISTER_IDS.RUMI_POINTS,
  });
  return _actor;
}

export async function getEpochStatus(): Promise<PublicEpochStatus | null> {
  const key = 'points:status';
  const cached = getCached<PublicEpochStatus>(key, TTL.STATUS);
  if (cached) return cached;
  try {
    return setCache(key, await getActor().get_epoch_status());
  } catch (e) {
    console.error('[pointsService] getEpochStatus failed', e);
    return null;
  }
}

export async function getPointsConfig(): Promise<PointsConfig | null> {
  const key = 'points:config';
  const cached = getCached<PointsConfig>(key, TTL.CONFIG);
  if (cached) return cached;
  try {
    return setCache(key, await getActor().get_points_config());
  } catch (e) {
    console.error('[pointsService] getPointsConfig failed', e);
    return null;
  }
}

export async function isRegistered(p: Principal): Promise<boolean> {
  const key = `points:reg:${p.toText()}`;
  const cached = getCached<boolean>(key, TTL.PRINCIPAL);
  if (cached !== null) return cached;
  try {
    return setCache(key, await getActor().is_registered(p));
  } catch (e) {
    console.error('[pointsService] isRegistered failed', e);
    return false;
  }
}

export async function isExcluded(p: Principal): Promise<boolean> {
  const key = `points:excl:${p.toText()}`;
  const cached = getCached<boolean>(key, TTL.PRINCIPAL);
  if (cached !== null) return cached;
  try {
    return setCache(key, await getActor().is_excluded(p));
  } catch (e) {
    console.error('[pointsService] isExcluded failed', e);
    return false;
  }
}

export async function getPrincipalState(p: Principal): Promise<PrincipalState | null> {
  const key = `points:state:${p.toText()}`;
  const cached = getCached<PrincipalState | null>(key, TTL.PRINCIPAL);
  if (cached !== null) return cached;
  try {
    const r = await getActor().get_principal_state(p);
    return setCache(key, r.length > 0 ? r[0] : null);
  } catch (e) {
    console.error('[pointsService] getPrincipalState failed', e);
    return null;
  }
}

export async function getLeaderboard(offset: number, limit: number): Promise<LeaderboardEntry[]> {
  const key = `points:lb:${offset}:${limit}`;
  const cached = getCached<LeaderboardEntry[]>(key, TTL.LEADERBOARD);
  if (cached) return cached;
  try {
    return setCache(key, await getActor().get_leaderboard(offset, limit));
  } catch (e) {
    console.error('[pointsService] getLeaderboard failed', e);
    return [];
  }
}
```

> Note: the `isRegistered`/`isExcluded`/`getPrincipalState` caches store `boolean`/`null` values, so the `getCached` "miss" sentinel must be distinguishable. `getCached` returns `null` only on miss; for the boolean getters a cached `false` is stored as `false` and returned via the `!== null` check, and for `getPrincipalState` a cached `null` (real "no state") would look like a miss — acceptable here because a re-query on a still-absent principal is cheap and returns `null` again. Do not "optimize" this into a wrong cache.

- [ ] **Step 2: Type-check**

Run: `cd src/vault_frontend && npm run check`
Expected: no errors referencing `pointsService.ts`.

- [ ] **Step 3: Commit**

```bash
git add src/vault_frontend/src/lib/services/pointsService.ts
git commit -m "feat(points): anonymous TTL-cached pointsService (mirrors analyticsService)"
```

---

### Task 4: My-points store (`pointsStore.ts`)

**Files:**
- Create: `src/vault_frontend/src/lib/stores/pointsStore.ts`

- [ ] **Step 1: Write the store**

`src/vault_frontend/src/lib/stores/pointsStore.ts`:
```ts
/**
 * pointsStore.ts — the connected wallet's points state. Reset/loaded by the
 * /points page as the principal changes.
 */
import { writable } from 'svelte/store';
import type { Principal } from '@dfinity/principal';
import type { PrincipalState } from '$declarations/rumi_points/rumi_points.did';
import { getPrincipalState, isExcluded } from '$lib/services/pointsService';

export interface MyPointsState {
  loading: boolean;
  loaded: boolean;
  state: PrincipalState | null;
  excluded: boolean;
  error: boolean;
}

const initial: MyPointsState = {
  loading: false,
  loaded: false,
  state: null,
  excluded: false,
  error: false,
};

function createMyPointsStore() {
  const { subscribe, set, update } = writable<MyPointsState>({ ...initial });

  async function load(p: Principal): Promise<void> {
    update((s) => ({ ...s, loading: true, error: false }));
    try {
      const [state, excluded] = await Promise.all([getPrincipalState(p), isExcluded(p)]);
      set({ loading: false, loaded: true, state, excluded, error: false });
    } catch (e) {
      console.error('[pointsStore] load failed', e);
      set({ ...initial, loaded: true, error: true });
    }
  }

  function reset(): void {
    set({ ...initial });
  }

  return { subscribe, load, reset };
}

export const myPointsStore = createMyPointsStore();
```

- [ ] **Step 2: Type-check**

Run: `cd src/vault_frontend && npm run check`
Expected: no errors referencing `pointsStore.ts`.

- [ ] **Step 3: Commit**

```bash
git add src/vault_frontend/src/lib/stores/pointsStore.ts
git commit -m "feat(points): myPointsStore for connected-wallet points state"
```

---

### Task 5: `SeasonBanner.svelte`

**Files:**
- Create: `src/vault_frontend/src/lib/components/points/SeasonBanner.svelte`

- [ ] **Step 1: Write the component**

`src/vault_frontend/src/lib/components/points/SeasonBanner.svelte`:
```svelte
<script lang="ts">
  import type { PublicEpochStatus, PointsConfig } from '$declarations/rumi_points/rumi_points.did';
  import { seasonState } from '$lib/utils/points';

  interface Props {
    status: PublicEpochStatus | null;
    config: PointsConfig | null;
  }
  let { status, config }: Props = $props();

  // ms remaining until season end, for a coarse day countdown.
  const phase = $derived(seasonState(status, config, BigInt(Date.now()) * 1_000_000n));
  const daysLeft = $derived.by(() => {
    if (!config) return null;
    const endMs = Number(config.season_end_ns / 1_000_000n);
    const d = Math.ceil((endMs - Date.now()) / 86_400_000);
    return d > 0 ? d : 0;
  });
  const epochIndex = $derived(status ? Number(status.current_epoch_index) : 0);
</script>

<div class="rounded-xl bg-gray-800/30 border border-gray-700/50 px-4 py-3 flex items-center justify-between gap-3">
  {#if phase === 'live'}
    <div>
      <p class="text-sm font-medium text-teal-400">Season 1 is live</p>
      <p class="text-xs text-gray-400">Epoch {epochIndex}{#if daysLeft !== null} · {daysLeft} day{daysLeft === 1 ? '' : 's'} left{/if}</p>
    </div>
  {:else if phase === 'pre'}
    <div>
      <p class="text-sm font-medium text-gray-200">Season 1 starts soon</p>
      <p class="text-xs text-gray-400">Start earning now — your positions are counted once the season opens.</p>
    </div>
  {:else if phase === 'ended'}
    <div>
      <p class="text-sm font-medium text-gray-200">Season 1 has ended</p>
      <p class="text-xs text-gray-400">Allocations are being finalized. Claiming is coming soon.</p>
    </div>
  {:else}
    <div>
      <p class="text-sm font-medium text-gray-300">Airdrop points</p>
      <p class="text-xs text-gray-500">Loading season status…</p>
    </div>
  {/if}
</div>
```

- [ ] **Step 2: Commit** (component verified by the page typecheck in Task 8)

```bash
git add src/vault_frontend/src/lib/components/points/SeasonBanner.svelte
git commit -m "feat(points): SeasonBanner component"
```

---

### Task 6: `EarnCta.svelte`

**Files:**
- Create: `src/vault_frontend/src/lib/components/points/EarnCta.svelte`

- [ ] **Step 1: Write the component**

`src/vault_frontend/src/lib/components/points/EarnCta.svelte`:
```svelte
<script lang="ts">
  interface Props {
    heading?: string;
  }
  let { heading = 'Ways to earn points' }: Props = $props();

  const actions = [
    { label: 'Mint icUSD', desc: 'Open a vault and borrow icUSD against collateral.', href: '/' },
    { label: 'Deposit to the stability pool', desc: 'Earn while backstopping liquidations.', href: '/stability-pool' },
    { label: 'Provide liquidity', desc: 'Add liquidity to the 3pool or AMM.', href: '/liquidity' },
  ];
</script>

<div class="rounded-xl bg-gray-800/30 border border-gray-700/50 p-4">
  <p class="text-sm font-medium text-gray-200 mb-3">{heading}</p>
  <ul class="flex flex-col gap-2">
    {#each actions as a}
      <li>
        <a
          href={a.href}
          class="flex items-center justify-between gap-3 rounded-lg border border-gray-700/40 bg-gray-900/30 px-3 py-2 hover:border-teal-500/40 transition-colors"
        >
          <span>
            <span class="block text-sm text-gray-100">{a.label}</span>
            <span class="block text-xs text-gray-500">{a.desc}</span>
          </span>
          <span class="text-teal-400 text-sm">→</span>
        </a>
      </li>
    {/each}
  </ul>
</div>
```

- [ ] **Step 2: Commit**

```bash
git add src/vault_frontend/src/lib/components/points/EarnCta.svelte
git commit -m "feat(points): EarnCta component with deep-links to qualifying flows"
```

---

### Task 7: `PointsSummary.svelte`

**Files:**
- Create: `src/vault_frontend/src/lib/components/points/PointsSummary.svelte`

- [ ] **Step 1: Write the component**

`src/vault_frontend/src/lib/components/points/PointsSummary.svelte`:
```svelte
<script lang="ts">
  import type { PrincipalState, Venue } from '$declarations/rumi_points/rumi_points.did';
  import { formatPoints, qualifyingActionLabel } from '$lib/utils/points';

  interface Props {
    state: PrincipalState;
    rank: number | null;
  }
  let { state, rank }: Props = $props();

  function venueLabel(v: Venue): string {
    if ('Vault' in v) return 'Vault debt';
    if ('StabilityPool' in v) return 'Stability pool';
    if ('ThreePool' in v) return '3pool liquidity';
    if ('Amm' in v) return 'AMM liquidity';
    return 'Position';
  }
  // Distinct venues currently earning, for a short breakdown.
  const venues = $derived(
    Array.from(new Set(state.active_deposits.map(([k]) => venueLabel(k.venue)))),
  );
  const enrolledDate = $derived(
    new Date(Number(state.registered_at_ns / 1_000_000n)).toLocaleDateString(),
  );
</script>

<div class="flex flex-col gap-4">
  <div class="grid grid-cols-2 gap-3">
    <div class="rounded-xl bg-gray-800/30 border border-gray-700/50 p-4">
      <p class="text-xs text-gray-400 uppercase tracking-wider">Your points</p>
      <p class="text-2xl font-semibold text-gray-100 mt-1">{formatPoints(state.total_points)}</p>
      <p class="text-xs text-gray-500">USD-days</p>
    </div>
    <div class="rounded-xl bg-gray-800/30 border border-gray-700/50 p-4">
      <p class="text-xs text-gray-400 uppercase tracking-wider">Rank</p>
      <p class="text-2xl font-semibold text-gray-100 mt-1">{rank !== null ? `#${rank}` : '—'}</p>
      <p class="text-xs text-gray-500">{rank !== null ? 'on the leaderboard' : 'outside the top ranks'}</p>
    </div>
  </div>

  <div class="rounded-xl bg-gray-800/30 border border-gray-700/50 p-4 text-sm text-gray-300">
    <p>Enrolled {enrolledDate} · first action: {qualifyingActionLabel(state.first_qualifying_action)}</p>
    {#if venues.length > 0}
      <p class="text-xs text-gray-500 mt-2">Currently earning from: {venues.join(', ')}</p>
    {/if}
  </div>
</div>
```

- [ ] **Step 2: Commit**

```bash
git add src/vault_frontend/src/lib/components/points/PointsSummary.svelte
git commit -m "feat(points): PointsSummary dashboard card"
```

---

### Task 8: My Points page (`/points`)

**Files:**
- Create: `src/vault_frontend/src/routes/points/+page.svelte`

- [ ] **Step 1: Write the page**

`src/vault_frontend/src/routes/points/+page.svelte`:
```svelte
<script lang="ts">
  import { onMount } from 'svelte';
  import { isConnected, principal } from '$lib/stores/wallet';
  import { myPointsStore } from '$lib/stores/pointsStore';
  import { getEpochStatus, getPointsConfig, getLeaderboard } from '$lib/services/pointsService';
  import { bodyState, deriveRank } from '$lib/utils/points';
  import type { PublicEpochStatus, PointsConfig } from '$declarations/rumi_points/rumi_points.did';
  import SeasonBanner from '$lib/components/points/SeasonBanner.svelte';
  import EarnCta from '$lib/components/points/EarnCta.svelte';
  import PointsSummary from '$lib/components/points/PointsSummary.svelte';

  let status = $state<PublicEpochStatus | null>(null);
  let config = $state<PointsConfig | null>(null);
  let rank = $state<number | null>(null);

  onMount(async () => {
    [status, config] = await Promise.all([getEpochStatus(), getPointsConfig()]);
  });

  // Load / reset the connected wallet's points as the principal changes.
  $effect(() => {
    const p = $principal;
    if ($isConnected && p) {
      myPointsStore.load(p);
      // Best-effort rank from the top slice (no get_my_rank endpoint exists).
      getLeaderboard(0, 1000).then((rows) => {
        rank = deriveRank(rows, p.toText());
      });
    } else {
      myPointsStore.reset();
      rank = null;
    }
  });

  const body = $derived(
    bodyState({
      connected: $isConnected,
      excluded: $myPointsStore.excluded,
      state: $myPointsStore.state,
    }),
  );
</script>

<svelte:head><title>Points · Rumi</title></svelte:head>

<div class="max-w-3xl mx-auto px-4 py-6 flex flex-col gap-4">
  <h1 class="text-xl font-semibold text-gray-100">Airdrop Points</h1>

  <SeasonBanner {status} {config} />

  {#if $myPointsStore.loading}
    <div class="flex justify-center py-12">
      <div class="w-7 h-7 border-2 border-gray-600 border-t-teal-400 rounded-full animate-spin"></div>
    </div>
  {:else if body === 'disconnected'}
    <div class="rounded-xl bg-gray-800/30 border border-gray-700/50 p-4 text-sm text-gray-300">
      Connect your wallet to see your points. Points accrue automatically when you use the protocol.
    </div>
    <EarnCta />
  {:else if body === 'excluded'}
    <div class="rounded-xl bg-gray-800/30 border border-gray-700/50 p-4 text-sm text-gray-300">
      This address is excluded from the airdrop (protocol-owned).
    </div>
  {:else if body === 'enrolled' && $myPointsStore.state}
    <PointsSummary state={$myPointsStore.state} {rank} />
    <EarnCta heading="Earn more" />
  {:else}
    <div class="rounded-xl bg-gray-800/30 border border-gray-700/50 p-4 text-sm text-gray-300">
      You're not earning points yet. Take a qualifying action to enroll automatically.
    </div>
    <EarnCta />
  {/if}
</div>
```

- [ ] **Step 2: Type-check (covers Tasks 5-8 components)**

Run: `cd src/vault_frontend && npm run check`
Expected: no errors referencing `routes/points/+page.svelte` or the `points/` components.

- [ ] **Step 3: Commit**

```bash
git add src/vault_frontend/src/routes/points/+page.svelte
git commit -m "feat(points): My Points page (status + earn CTA)"
```

---

### Task 9: Leaderboard page (`/points/leaderboard`)

**Files:**
- Create: `src/vault_frontend/src/routes/points/leaderboard/+page.svelte`

- [ ] **Step 1: Write the page**

`src/vault_frontend/src/routes/points/leaderboard/+page.svelte`:
```svelte
<script lang="ts">
  import { onMount } from 'svelte';
  import { principal } from '$lib/stores/wallet';
  import { getLeaderboard, getPointsConfig } from '$lib/services/pointsService';
  import { formatPoints } from '$lib/utils/points';
  import { truncatePrincipal } from '$lib/utils/principalHelpers';
  import type { LeaderboardEntry, PointsConfig } from '$declarations/rumi_points/rumi_points.did';
  import DataTable from '$lib/components/explorer/DataTable.svelte';
  import EmptyState from '$lib/components/explorer/EmptyState.svelte';

  const PAGE = 50;
  let rows = $state<LeaderboardEntry[]>([]);
  let config = $state<PointsConfig | null>(null);
  let offset = $state(0);
  let loading = $state(true);

  const columns = [
    { key: 'rank', label: 'Rank', align: 'left' as const, width: '15%' },
    { key: 'principal', label: 'Address', align: 'left' as const },
    { key: 'total_points', label: 'Points (USD-days)', align: 'right' as const },
  ];

  async function loadPage(o: number) {
    loading = true;
    const [page, cfg] = await Promise.all([getLeaderboard(o, PAGE), config ? Promise.resolve(config) : getPointsConfig()]);
    rows = page;
    config = cfg;
    offset = o;
    loading = false;
  }
  onMount(() => loadPage(0));

  const myText = $derived($principal ? $principal.toText() : null);
  const participants = $derived(config ? Number(config.registered_count) : null);
</script>

<svelte:head><title>Leaderboard · Rumi Points</title></svelte:head>

<div class="max-w-4xl mx-auto px-4 py-6 flex flex-col gap-4">
  <div class="flex items-center justify-between">
    <h1 class="text-xl font-semibold text-gray-100">Points Leaderboard</h1>
    {#if participants !== null}
      <span class="text-xs text-gray-400">{participants.toLocaleString()} participants</span>
    {/if}
  </div>

  {#if !loading && rows.length === 0}
    <EmptyState title="No points yet" message="The leaderboard fills in once accrual begins." icon="chart" />
  {:else}
    <DataTable {columns} data={rows} {loading} rowKey={(r) => r.principal.toText()}>
      {#snippet row(entry: LeaderboardEntry)}
        <tr class="border-b border-gray-700/30 {myText && entry.principal.toText() === myText ? 'bg-teal-500/10' : ''}">
          <td class="px-4 py-3 text-gray-300">#{entry.rank}</td>
          <td class="px-4 py-3">
            <a href={`/explorer/e/address/${entry.principal.toText()}`} class="text-teal-400 hover:underline font-mono text-xs">
              {truncatePrincipal(entry.principal.toText())}
            </a>
            {#if myText && entry.principal.toText() === myText}<span class="ml-2 text-xs text-teal-400">you</span>{/if}
          </td>
          <td class="px-4 py-3 text-right text-gray-100">{formatPoints(entry.total_points)}</td>
        </tr>
      {/snippet}
    </DataTable>

    <div class="flex items-center justify-between">
      <button
        class="px-3 py-1.5 rounded-lg text-sm border border-gray-700/50 text-gray-300 disabled:opacity-40"
        disabled={offset === 0 || loading}
        onclick={() => loadPage(Math.max(0, offset - PAGE))}
      >Previous</button>
      <span class="text-xs text-gray-500">Showing {offset + 1}–{offset + rows.length}</span>
      <button
        class="px-3 py-1.5 rounded-lg text-sm border border-gray-700/50 text-gray-300 disabled:opacity-40"
        disabled={rows.length < PAGE || loading}
        onclick={() => loadPage(offset + PAGE)}
      >Next</button>
    </div>
  {/if}
</div>
```

> Verified: `src/vault_frontend/src/routes/explorer/e/address/` exists, so `/explorer/e/address/<principal>` is the correct link target.

- [ ] **Step 2: Type-check**

Run: `cd src/vault_frontend && npm run check`
Expected: no errors referencing the leaderboard page.

- [ ] **Step 3: Commit**

```bash
git add src/vault_frontend/src/routes/points/leaderboard/+page.svelte
git commit -m "feat(points): leaderboard page (rank + truncated address + USD-days)"
```

---

### Task 10: Navigation link (`+layout.svelte`)

**Files:**
- Modify: `src/vault_frontend/src/routes/+layout.svelte` (nav block around line 73-80; script block for the import)

- [ ] **Step 1: Import the gate**

In the `<script>` block of `src/vault_frontend/src/routes/+layout.svelte`, add to the existing config import (or as a new import):
```ts
  import { POINTS_ENABLED } from '$lib/config';
```

- [ ] **Step 2: Add the nav link**

In the `<nav class="top-nav">` block, after the Explorer link (line ~80: `<a href="/explorer" ...><span>Explorer</span></a>`), add:
```svelte
    {#if POINTS_ENABLED}<a href="/points" class="nav-link" class:active={currentPath.startsWith('/points')}><span>Points</span></a>{/if}
```

- [ ] **Step 3: Type-check + build**

Run: `cd src/vault_frontend && npm run check`
Expected: no new errors.

- [ ] **Step 4: Commit**

```bash
git add src/vault_frontend/src/routes/+layout.svelte
git commit -m "feat(points): gated Points nav link (hidden until canister configured)"
```

---

### Task 11: Full verification

- [ ] **Step 1: Run the full test suite**

Run: `cd src/vault_frontend && npm run test`
Expected: all tests pass, including the new `points.test.ts`.

- [ ] **Step 2: Type-check the whole app**

Run: `cd src/vault_frontend && npm run check`
Expected: no new errors attributable to the points feature (baseline-compare any pre-existing warnings).

- [ ] **Step 3: Production build**

Run: `cd src/vault_frontend && npm run build`
Expected: build succeeds. (With `POINTS_ENABLED=false` the routes still compile; they're just unreachable via nav.)

- [ ] **Step 4: Temporary visibility smoke-check (manual, optional)**

Temporarily set `RUMI_POINTS` to any valid-looking canister id in `config.ts`, run `npm run dev`, confirm the `Points` nav item appears and `/points` + `/points/leaderboard` render their empty/disconnected states without runtime errors, then revert the id back to `""`. Do not commit the temporary id.

---

## Self-review notes (author)

- **Spec coverage:** routes + nav (Task 2,10), My Points states incl. excluded (Task 8), earn CTA deep-links (Task 6), season banner (Task 5), leaderboard points+rank only / no estimated_share_bps (Task 9 — `estimated_share_bps` is never read), data layer mirror (Task 3), store (Task 4), USD-days formatting (Task 1), bounded rank lookup top-1000 (Task 8), route gating on canister id (Task 2,10). All covered.
- **No estimated_share_bps** is rendered anywhere (decision 2) — verified in Task 9 markup.
- **Type consistency:** `formatPoints(bigint)`, `seasonState(status,config,bigint)`, `bodyState({connected,excluded,state})`, `deriveRank(entries,string)` used identically across tasks.
- **Verified:** explorer address route `/explorer/e/address/<id>` exists (Task 9 link target); `currentPath` is defined in `+layout.svelte:23` (Task 10 nav `class:active`). No open items.
