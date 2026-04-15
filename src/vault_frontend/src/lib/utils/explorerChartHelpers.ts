/**
 * Shared chart utilities for the explorer.
 * Formatters, scales, and color constants for SVG charts.
 */

/** Format a large number with K/M/B suffixes. */
export function formatCompact(value: number): string {
  if (value >= 1_000_000_000) return `${(value / 1_000_000_000).toFixed(1)}B`;
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}M`;
  if (value >= 1_000) return `${(value / 1_000).toFixed(1)}K`;
  return value.toFixed(0);
}

/** Format basis points as percentage string. */
export function bpsToPercent(bps: number): string {
  return `${(bps / 100).toFixed(1)}%`;
}

/** Format e8s (8-decimal) to human-readable number. */
export function e8sToNumber(e8s: number | bigint): number {
  return Number(e8s) / 1e8;
}

/** Format USD value from e8s. */
export function formatUsdE8s(e8s: number | bigint): string {
  const val = e8sToNumber(e8s);
  if (val >= 1_000_000) return `$${(val / 1_000_000).toFixed(2)}M`;
  if (val >= 1_000) return `$${(val / 1_000).toFixed(1)}K`;
  return `$${val.toFixed(2)}`;
}

/** Convert nanosecond timestamp to JS Date. */
export function nsToDate(ns: bigint | number): Date {
  return new Date(Number(ns) / 1_000_000);
}

/** Format date for chart axis labels. */
export function formatDateShort(date: Date): string {
  return date.toLocaleDateString('en-US', { month: 'short', day: 'numeric' });
}

/** Chart color palette (matching Rumi design tokens). */
export const CHART_COLORS = {
  teal: '#2DD4BF',
  tealDim: 'rgba(45, 212, 191, 0.15)',
  purple: '#d176e8',
  purpleDim: 'rgba(209, 118, 232, 0.15)',
  action: '#34d399',
  danger: '#e06b9f',
  caution: '#a78bfa',
  grid: 'rgba(90, 100, 180, 0.08)',
  text: '#a09bb5',
  textMuted: '#605a75',
} as const;

/** Known collateral symbols by principal. */
export const COLLATERAL_SYMBOLS: Record<string, string> = {
  'ryjl3-tyaaa-aaaaa-aaaba-cai': 'ICP',
  'mxzaz-hqaaa-aaaar-qaada-cai': 'ckBTC',
  'ss2fx-dyaaa-aaaar-qacoq-cai': 'ckETH',
  'nza5v-qaaaa-aaaar-qahzq-cai': 'ckXAUT',
  'buwm7-7yaaa-aaaar-qagva-cai': 'nICP',
  '7pail-xaaaa-aaaas-aabmq-cai': 'BOB',
  'rh2pm-ryaaa-aaaan-qeniq-cai': 'EXE',
};

/** Get symbol for a collateral principal. */
export function getCollateralSymbol(principal: string): string {
  return COLLATERAL_SYMBOLS[principal] ?? principal.slice(0, 5) + '...';
}

/** Collateral brand colors. */
export const COLLATERAL_COLORS: Record<string, string> = {
  ICP: '#29ABE2',
  ckBTC: '#F7931A',
  ckETH: '#627EEA',
  ckXAUT: '#C9A96E',
  nICP: '#5AC4BE',
  BOB: '#FF6B35',
  EXE: '#8B5CF6',
};

/** Time range presets for chart filters. */
export type TimeRange = '7d' | '30d' | '90d' | '1y' | 'all';

export const TIME_RANGES: { key: TimeRange; label: string; days: number }[] = [
  { key: '7d', label: '7D', days: 7 },
  { key: '30d', label: '30D', days: 30 },
  { key: '90d', label: '90D', days: 90 },
  { key: '1y', label: '1Y', days: 365 },
  { key: 'all', label: 'All', days: 0 },
];

/** Filter data points by time range. Returns items within the last N days. */
export function filterByTimeRange<T extends { timestamp_ns: bigint }>(
  data: T[],
  range: TimeRange
): T[] {
  if (range === 'all') return data;
  const preset = TIME_RANGES.find(r => r.key === range);
  if (!preset) return data;
  const cutoff = BigInt(Date.now() - preset.days * 86_400_000) * 1_000_000n;
  return data.filter(d => d.timestamp_ns >= cutoff);
}

/**
 * Compute Y-axis scale for an array of values.
 * Returns { min, max, ticks } with nice round numbers.
 */
export function computeYScale(values: number[]): { min: number; max: number; ticks: number[] } {
  if (values.length === 0) return { min: 0, max: 100, ticks: [0, 25, 50, 75, 100] };
  const rawMin = Math.min(...values);
  const rawMax = Math.max(...values);
  const range = rawMax - rawMin || rawMax * 0.1 || 1;
  const padding = range * 0.05;
  const min = Math.max(0, rawMin - padding);
  const max = rawMax + padding;
  const step = (max - min) / 4;
  const ticks = Array.from({ length: 5 }, (_, i) => min + step * i);
  return { min, max, ticks };
}
