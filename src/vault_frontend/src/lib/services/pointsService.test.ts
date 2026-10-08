import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const { createActor } = vi.hoisted(() => ({ createActor: vi.fn() }));

vi.mock('@dfinity/agent', () => ({
  Actor: { createActor },
  HttpAgent: class {
    fetchRootKey = vi.fn().mockResolvedValue(undefined);
  },
}));

import { getFiatStablePointsPolicy, invalidatePointsCache } from './pointsService';

describe('getFiatStablePointsPolicy', () => {
  beforeEach(() => {
    invalidatePointsCache('points:fiat-stable-policy');
    createActor.mockReset();
    vi.spyOn(console, 'warn').mockImplementation(() => {});
  });

  afterEach(() => vi.restoreAllMocks());

  it('returns unknown when actor construction fails', async () => {
    createActor.mockImplementation(() => {
      throw new Error('actor construction failed');
    });

    const policy = await getFiatStablePointsPolicy();

    expect(policy.mode).toBe('unknown');
    expect(policy.source).toBe('runtime');
  });

  it('returns unknown for a read failure and falls back only after method absence is confirmed', async () => {
    const actor: { get_fiat_stable_points_policy?: () => Promise<unknown> } = {
      get_fiat_stable_points_policy: vi.fn().mockRejectedValue(new Error('replica unavailable')),
    };
    createActor.mockReturnValue(actor);

    const failedPolicy = await getFiatStablePointsPolicy();

    expect(failedPolicy.mode).toBe('unknown');
    expect(failedPolicy.source).toBe('runtime');

    actor.get_fiat_stable_points_policy = undefined;
    invalidatePointsCache('points:fiat-stable-policy');
    const missingMethodPolicy = await getFiatStablePointsPolicy();

    expect(missingMethodPolicy.mode).toBe('legacy');
    expect(missingMethodPolicy.source).toBe('legacy-fallback');
  });
});
