import type { CurrentSeason } from '$lib/services/currentSeasonService';

/** Timers being enabled do not prove successful tracking or awarded points. */
export function currentSeasonMessage(data: CurrentSeason | null, nowMs: number): string {
  if (!data) return 'Status unavailable — Season 2 awards are unconfirmed.';
  const now = BigInt(nowMs) * 1_000_000n;
  const open = data.status.open_epoch[0];
  const awards = data.highestPoints === 0n ? 'No Season 2 points awarded yet.' : 'Season 2 awards have been recorded.';
  if (open && now >= open.epoch_end_ns) return `Weekly processing overdue. ${awards}`;
  if (!data.status.driver_enabled || !data.ingest.poll_enabled) return `Processing paused. ${awards}`;
  if (now < data.config.season_start_ns) return `Season 2 starts soon. ${awards}`;
  if (now >= data.config.season_end_ns) return `Season 2 has ended. ${awards}`;
  return `Activity tracking configured; weekly awards pending. ${awards}`;
}

export function pendingCoverageMessage(data: CurrentSeason | null): string | null {
  return data?.forward ? 'Earlier Season 2 activity remains pending reconciliation. Current totals do not settle that period.' : null;
}
