import { beforeEach, describe, expect, it, vi } from 'vitest';
import { readCachedPendingDeposits, writeCachedPendingDeposits } from './xrpVaultService';

const OWNER = 'aaaaa-aa';
const CACHE_KEY = `rumi_xrp_pending_deposits:${OWNER}`;

describe('XRP pending-deposit cache', () => {
  beforeEach(() => {
    localStorage.clear();
    vi.restoreAllMocks();
  });

  // Regression: reserveBaseDrops is a nat64 (bigint). JSON.stringify threw
  // "Do not know how to serialize a BigInt" AFTER open_xrp_vault had already
  // landed on-chain, so the UI reported "Could not prepare XRP address" and
  // never showed the custody address.
  it('round-trips bigint drops through localStorage', () => {
    writeCachedPendingDeposits(
      [{ vaultId: 7, custodyAddress: 'rCustody', openedAtMs: 1_700_000_000_000, reserveBaseDrops: 1_000_000n }],
      OWNER
    );

    expect(JSON.parse(localStorage.getItem(CACHE_KEY)!)[0].reserveBaseDrops).toBe('1000000');
    expect(readCachedPendingDeposits(OWNER)).toEqual([
      { vaultId: 7, custodyAddress: 'rCustody', openedAtMs: 1_700_000_000_000, reserveBaseDrops: 1_000_000n },
    ]);
  });

  it('revives legacy number drops and defaults unusable values to zero', () => {
    localStorage.setItem(
      CACHE_KEY,
      JSON.stringify([
        { vaultId: 1, custodyAddress: 'rLegacy', openedAtMs: 1, updatedAtMs: 1, reserveBaseDrops: 1_000_000 },
        { vaultId: 2, custodyAddress: 'rMissing', openedAtMs: 2, updatedAtMs: 2 },
      ])
    );

    expect(readCachedPendingDeposits(OWNER).map((p) => p.reserveBaseDrops)).toEqual([1_000_000n, 0n]);
  });

  it('never throws when localStorage rejects the write', () => {
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('QuotaExceededError');
    });

    expect(() =>
      writeCachedPendingDeposits(
        [{ vaultId: 9, custodyAddress: 'rCustody', openedAtMs: 1, reserveBaseDrops: 1_000_000n }],
        OWNER
      )
    ).not.toThrow();
  });
});
