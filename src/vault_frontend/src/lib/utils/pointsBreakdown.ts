/**
 * pointsBreakdown.ts — pure helpers that turn rumi_points data into the user
 * points page's two core views:
 *
 *  1. HISTORY  — `summarizeLedger()` decomposes the audit ledger (PointEntry
 *     rows, one per source per epoch) into per-source totals + a per-epoch
 *     timeline, with an active/stopped status per source.
 *  2. NOW      — `buildLivePositions()` mirrors the accrual engine's
 *     `accrual.rs::snapshot_weights` (incl. the 3USD verification cap) over
 *     live position reads, so the page can show what the next snapshot will
 *     credit and why (e.g. a 3pool deposit whose 3USD was sold no longer
 *     counts).
 *
 * Pure and unit-tested; no I/O. The live inputs are fetched by
 * `services/pointsLive.ts` from the SAME endpoints the engine snapshots.
 */
import type {
  PointEntry,
  PointSource,
  EpochSummary,
} from '$declarations/rumi_points/rumi_points.did';

// ── Source metadata (single source of truth for source labels) ──────────────

export type PointSourceKey =
  | 'Registration'
  | 'IcUsdDebt'
  | 'IcUsd3Pool'
  | 'CkStable3PoolMatched'
  | 'CkStable3PoolUnmatched'
  | 'IcUsdStabilityPool'
  | 'ThreeUsdStabilityPool'
  | 'AmmLp'
  | 'VaultRepayment';

export interface SourceMeta {
  /** Human label, matches the docs multiplier table. */
  label: string;
  /** Compact label for dense rows (epoch timeline). */
  short: string;
  /** Venue bucket, for grouping and "already active" checks. */
  venue: 'vault' | 'stabilityPool' | 'threePool' | 'amm';
  /** The multiplier accrual applies (accrual.rs::snapshot_weights). For the
   *  matched pair this is the 5x on the matched dollars (2*min), matching the
   *  user-facing table — not the per-min-side 10x framing. */
  multiplier: number;
  /** Where to act on it. */
  href: string;
}

export const SOURCE_META: Record<PointSourceKey, SourceMeta> = {
  Registration: { label: 'Enrollment', short: 'Enrolled', venue: 'vault', multiplier: 0, href: '/points' },
  IcUsdDebt: { label: 'icUSD borrowed against vaults', short: 'Vault debt', venue: 'vault', multiplier: 1, href: '/' },
  IcUsd3Pool: { label: 'icUSD in the 3pool', short: 'icUSD 3pool', venue: 'threePool', multiplier: 1, href: '/3usd' },
  CkStable3PoolMatched: {
    label: 'ckUSDC + ckUSDT matched in the 3pool',
    short: 'Matched pair',
    venue: 'threePool',
    multiplier: 5,
    href: '/3usd',
  },
  CkStable3PoolUnmatched: {
    label: 'Unmatched ckUSDC/ckUSDT in the 3pool',
    short: 'Unmatched ck',
    venue: 'threePool',
    multiplier: 3,
    href: '/3usd',
  },
  IcUsdStabilityPool: {
    label: 'icUSD in the stability pool',
    short: 'icUSD SP',
    venue: 'stabilityPool',
    multiplier: 1,
    href: '/stability-pool',
  },
  ThreeUsdStabilityPool: {
    label: '3USD in the stability pool',
    short: '3USD SP',
    venue: 'stabilityPool',
    multiplier: 2,
    href: '/stability-pool',
  },
  AmmLp: { label: '3USD/ICP liquidity in the AMM', short: 'AMM LP', venue: 'amm', multiplier: 2, href: '/swap' },
  VaultRepayment: {
    label: 'Vault repaid with ckUSDC/ckUSDT',
    short: 'ck repay',
    venue: 'vault',
    multiplier: 5,
    href: '/',
  },
};

/** Candid variant → its key ('IcUsdDebt', …). */
export function sourceKey(s: PointSource): PointSourceKey {
  return Object.keys(s)[0] as PointSourceKey;
}

/**
 * Meta for a source key, surviving a canister-side variant this build does not
 * know yet (regenerated declarations + a new PointSource must never crash the
 * page — render the raw key at 0x instead).
 */
export function sourceMeta(key: PointSourceKey): SourceMeta {
  return (
    SOURCE_META[key] ?? { label: key, short: key, venue: 'vault', multiplier: 0, href: '/points' }
  );
}

// ── History: ledger decomposition ───────────────────────────────────────────

export interface SourceHistoryRow {
  key: PointSourceKey;
  meta: SourceMeta;
  points: bigint;
  /** Share of the principal's total, in percent (0–100, 1dp precision). */
  sharePct: number;
  firstEpoch: number;
  lastEpoch: number;
  epochCount: number;
  /** 'active' = credited in the principal's most recently processed epoch. */
  status: 'active' | 'stopped';
}

export interface EpochHistoryRow {
  epoch: number;
  points: bigint;
  bySource: Array<{ key: PointSourceKey; points: bigint }>;
}

export interface LedgerSummary {
  total: bigint;
  sources: SourceHistoryRow[];
  /** Most recent epoch first. */
  epochs: EpochHistoryRow[];
}

/**
 * Decompose a principal's ledger rows. `latestClosedEpoch` should be the
 * principal's `last_epoch_processed` (the most recent epoch whose close has
 * credited them) so a chunked in-progress close never misflags sources.
 * Registration rows are markers (0 points) and are skipped.
 */
export function summarizeLedger(
  entries: PointEntry[],
  latestClosedEpoch: number | null,
): LedgerSummary {
  const bySource = new Map<
    PointSourceKey,
    { points: bigint; first: number; last: number; epochs: Set<number> }
  >();
  const byEpoch = new Map<number, Map<PointSourceKey, bigint>>();

  for (const e of entries) {
    const key = sourceKey(e.source);
    if (key === 'Registration') continue;
    const epoch = Number(e.epoch_index);

    const s = bySource.get(key) ?? {
      points: 0n,
      first: epoch,
      last: epoch,
      epochs: new Set<number>(),
    };
    s.points += e.points_delta;
    s.first = Math.min(s.first, epoch);
    s.last = Math.max(s.last, epoch);
    s.epochs.add(epoch);
    bySource.set(key, s);

    const ep = byEpoch.get(epoch) ?? new Map<PointSourceKey, bigint>();
    ep.set(key, (ep.get(key) ?? 0n) + e.points_delta);
    byEpoch.set(epoch, ep);
  }

  const total = [...bySource.values()].reduce((acc, s) => acc + s.points, 0n);

  const sources: SourceHistoryRow[] = [...bySource.entries()]
    .map(([key, s]) => ({
      key,
      meta: sourceMeta(key),
      points: s.points,
      sharePct: total > 0n ? Number((s.points * 1000n) / total) / 10 : 0,
      firstEpoch: s.first,
      lastEpoch: s.last,
      epochCount: s.epochs.size,
      status: (latestClosedEpoch !== null && s.last < latestClosedEpoch
        ? 'stopped'
        : 'active') as 'active' | 'stopped',
    }))
    .sort((a, b) => (b.points > a.points ? 1 : b.points < a.points ? -1 : 0));

  const epochs: EpochHistoryRow[] = [...byEpoch.entries()]
    .map(([epoch, m]) => ({
      epoch,
      points: [...m.values()].reduce((a, b) => a + b, 0n),
      bySource: [...m.entries()]
        .map(([key, points]) => ({ key, points }))
        .sort((a, b) => (b.points > a.points ? 1 : b.points < a.points ? -1 : 0)),
    }))
    .sort((a, b) => b.epoch - a.epoch);

  return { total, sources, epochs };
}

// ── Now: live position weights (mirror of accrual.rs) ───────────────────────

/**
 * Live inputs in display units: token amounts (≈ dollars for the stables) and
 * dollar values. `null` means "the read failed", which is NOT zero — the
 * mirror skips that venue and reports it in `unavailable` (same Option
 * contract as the engine's fetch helpers).
 */
export interface LiveInputs {
  /** Sum of open vault debt, icUSD (face $1). */
  vaultDebtUsd: number | null;
  /** Stability-pool balances, token units (face $1). */
  spIcusd: number | null;
  sp3usd: number | null;
  /** Wallet 3USD/LP balance, token units. */
  wallet3usd: number | null;
  /** The user's share of the 3USD/ICP AMM pool, token units per leg.
   *  `null` = pool exists but the read failed; a missing pool is {0,0}. */
  ammShare3usd: number | null;
  ammShareIcp: number | null;
  /** Recorded 3pool composition (points canister `active_deposits`), dollars. */
  recorded3pool: { icusd: number; ckusdc: number; ckusdt: number };
  /** ICP/USD oracle rate and the 3pool virtual price (1.0-scaled).
   *  A null virtual price means the read failed — verification is then
   *  UNKNOWN, never a false "not counting" alarm (the engine aborts its whole
   *  capture in that case; the mirror degrades per-venue instead). */
  icpUsd: number | null;
  virtualPrice: number | null;
}

export interface LivePositionRow {
  key: PointSourceKey;
  meta: SourceMeta;
  /** Dollar value being credited (post-verification for the 3pool rows). */
  valueUsd: number;
  multiplier: number;
  /** valueUsd × multiplier — its points contribution per day held. */
  weightedUsd: number;
  note?: { tone: 'info' | 'warning'; text: string };
}

export interface ThreePoolVerification {
  recordedUsd: number;
  /** 3USD held across wallet + SP + AMM, valued at the virtual price. */
  verifiedUsd: number;
  /** Dollars of the recorded deposit actually counting after the cap. */
  creditedUsd: number;
  underVerified: boolean;
  /** True when a verification leg failed to load, so no cap was applied. */
  verificationUnknown: boolean;
}

export interface LivePositions {
  rows: LivePositionRow[];
  /** Σ weighted × 7 — upper-bound estimate of USD-day points per week. */
  weeklyEstimateUsdDays: number;
  /** Sources whose live read failed (shown as "couldn't check"). */
  unavailable: string[];
  threePool: ThreePoolVerification | null;
}

/** Spec Section 5: 0.5% upward tolerance on verified 3USD. */
const VERIFICATION_TOLERANCE = 1.005;

/**
 * Mirror of `accrual.rs::snapshot_weights` + `build_snapshot_inputs` over live
 * reads. Zero-value rows are omitted (matching `by_source`).
 */
export function buildLivePositions(inp: LiveInputs): LivePositions {
  const rows: LivePositionRow[] = [];
  const unavailable: string[] = [];

  // Vault debt @1x.
  if (inp.vaultDebtUsd === null) unavailable.push('vault debt');
  else if (inp.vaultDebtUsd > 0) {
    rows.push(row('IcUsdDebt', inp.vaultDebtUsd));
  }

  // Stability pool @1x / @2x (position value is $1 face, like the engine).
  if (inp.spIcusd === null || inp.sp3usd === null) unavailable.push('stability pool');
  else {
    if (inp.spIcusd > 0) rows.push(row('IcUsdStabilityPool', inp.spIcusd));
    if (inp.sp3usd > 0) rows.push(row('ThreeUsdStabilityPool', inp.sp3usd));
  }

  // AMM LP @2x: 3USD leg at $1 face + ICP leg at the oracle rate.
  const ammUnavailable =
    inp.ammShare3usd === null || inp.ammShareIcp === null || inp.icpUsd === null;
  if (ammUnavailable) unavailable.push('AMM liquidity');
  else {
    const ammValue = inp.ammShare3usd! + inp.ammShareIcp! * inp.icpUsd!;
    if (ammValue > 0) rows.push(row('AmmLp', ammValue));
  }

  // 3pool: recorded composition scaled by the 3USD-holding verification cap.
  const rec = inp.recorded3pool;
  const recordedUsd = rec.icusd + rec.ckusdc + rec.ckusdt;
  let threePool: ThreePoolVerification | null = null;
  if (recordedUsd > 0) {
    const verificationUnknown =
      inp.wallet3usd === null ||
      inp.sp3usd === null ||
      ammUnavailable ||
      inp.virtualPrice === null;
    const held3usd = verificationUnknown
      ? 0
      : inp.wallet3usd! + inp.sp3usd! + inp.ammShare3usd!;
    const verifiedUsd = verificationUnknown ? 0 : held3usd * inp.virtualPrice!;
    // Unknown verification: show the recorded position uncapped with a note,
    // never a false "not counting" alarm.
    const cap = verificationUnknown ? recordedUsd : verifiedUsd * VERIFICATION_TOLERANCE;
    const capped = Math.min(cap, recordedUsd);
    const factor = capped / recordedUsd;
    const effIcusd = rec.icusd * factor;
    const effUsdc = rec.ckusdc * factor;
    const effUsdt = rec.ckusdt * factor;
    const matched = 2 * Math.min(effUsdc, effUsdt);
    const unmatched = Math.abs(effUsdc - effUsdt);
    if (effIcusd > 0) rows.push(row('IcUsd3Pool', effIcusd));
    if (matched > 0) rows.push(row('CkStable3PoolMatched', matched));
    if (unmatched > 0) rows.push(row('CkStable3PoolUnmatched', unmatched));
    threePool = {
      recordedUsd,
      verifiedUsd,
      creditedUsd: capped,
      underVerified: !verificationUnknown && capped < recordedUsd - 0.005,
      verificationUnknown,
    };
  }

  const weekly = rows.reduce((acc, r) => acc + r.weightedUsd, 0) * 7;
  return { rows, weeklyEstimateUsdDays: weekly, unavailable, threePool };
}

function row(key: PointSourceKey, valueUsd: number): LivePositionRow {
  const meta = SOURCE_META[key];
  return { key, meta, valueUsd, multiplier: meta.multiplier, weightedUsd: valueUsd * meta.multiplier };
}

// ── Epoch date helpers ──────────────────────────────────────────────────────

const EPOCH_DATE = new Intl.DateTimeFormat('en-US', {
  month: 'short',
  day: 'numeric',
  timeZone: 'UTC',
});

/** "Aug 10 – Aug 17" (UTC boundaries, matching the epoch schedule). */
export function epochDateRange(startNs: bigint, endNs: bigint): string {
  const s = new Date(Number(startNs / 1_000_000n));
  const e = new Date(Number(endNs / 1_000_000n));
  return `${EPOCH_DATE.format(s)} – ${EPOCH_DATE.format(e)}`;
}

/** Epoch index → its summary, for the timeline's date column. */
export function epochRangeByIndex(
  epochs: EpochSummary[],
  index: number,
): string | null {
  const hit = epochs.find((e) => Number(e.epoch_index) === index);
  return hit ? epochDateRange(hit.epoch_start_ns, hit.epoch_end_ns) : null;
}
