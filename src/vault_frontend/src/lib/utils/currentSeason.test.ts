import { describe, expect, it } from 'vitest';
import { currentSeasonMessage } from './currentSeason';
import type { CurrentSeason } from '../services/currentSeasonService';
const data = {
  status: { driver_enabled: true, open_epoch: [{ epoch_end_ns: 2_000_000_000n }] },
  config: { season_start_ns: 0n, season_end_ns: 10_000_000_000n },
  ingest: { poll_enabled: true }, highestPoints: 0n, observedAt: 0,
} as unknown as CurrentSeason;
describe('current Season 2 messaging', () => {
  it('shows the stalled week despite enabled timers and zero awards', () => {
    expect(currentSeasonMessage(data, 2000)).toBe('Weekly processing overdue. No Season 2 points awarded yet.');
  });
  it('does not mistake zero-point registration audit rows for awards', () => {
    expect(currentSeasonMessage({ ...data, highestPoints: 0n }, 2000)).toContain('No Season 2 points awarded yet.');
  });
  it('does not call configured timers successful earning', () => {
    expect(currentSeasonMessage(data, 1000)).toBe('Activity tracking configured; weekly awards pending. No Season 2 points awarded yet.');
  });
  it('reports unknown on query failure instead of active or zero', () => {
    expect(currentSeasonMessage(null, 3000)).toBe('Status unavailable — Season 2 awards are unconfirmed.');
  });
  it('distinguishes positive point totals including backfill from paused processing', () => {
    const paused = { ...data, status: { ...data.status, driver_enabled: false }, highestPoints: 2n };
    expect(currentSeasonMessage(paused, 1000)).toBe('Processing paused. Season 2 awards have been recorded.');
  });
});
