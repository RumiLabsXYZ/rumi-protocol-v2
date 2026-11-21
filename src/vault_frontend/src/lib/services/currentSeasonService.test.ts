import { describe, expect, it, vi } from 'vitest';
const { createActor } = vi.hoisted(() => ({ createActor: vi.fn() }));
vi.mock('@dfinity/agent', () => ({ Actor: { createActor }, HttpAgent: class {} }));
vi.mock('$lib/config', () => ({
  CONFIG: { isLocal: false },
  CANISTER_IDS: { RUMI_POINTS: 'bfnu3-6aaaa-aaaab-qhanq-cai', RUMI_POINTS_SEASON2: '2cvsh-3aaaa-aaaaj-qspga-cai' },
}));
import { getCurrentSeason } from './currentSeasonService';
describe('Season 2 query isolation', () => {
  it('reads every current-season field from S2 and rejects a partial failure', async () => {
    const source = {
      get_epoch_status: vi.fn().mockResolvedValue({ driver_enabled: true }),
      get_points_config: vi.fn().mockResolvedValue({}),
      get_ingest_status: vi.fn().mockResolvedValue({ poll_enabled: true }),
      get_point_ledger_len: vi.fn().mockResolvedValue(1n), // Registration audit row, not an award.
      get_leaderboard: vi.fn().mockResolvedValue([{ total_points: 0n }]),
    };
    createActor.mockReturnValue(source);
    expect((await getCurrentSeason()).highestPoints).toBe(0n);
    expect(createActor.mock.calls[0][1].canisterId).toBe('2cvsh-3aaaa-aaaaj-qspga-cai');
    expect(source.get_epoch_status).toHaveBeenCalledTimes(1);
    source.get_leaderboard.mockRejectedValue(new Error('unavailable'));
    await expect(getCurrentSeason(true)).rejects.toThrow('unavailable');
    expect(source.get_point_ledger_len).not.toHaveBeenCalled();
    expect(source.get_leaderboard).toHaveBeenCalledWith(0, 1);
    source.get_leaderboard.mockResolvedValue([{ total_points: 3n }]);
    expect((await getCurrentSeason(true)).highestPoints).toBe(3n);
  });
});
