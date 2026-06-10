import { describe, it, expect, beforeEach } from 'vitest';
import {
  warmRawSnapshots,
  warmRawSnapshot,
  warmRawVaultIds,
  getRawSnapshot,
  getRawVaultIds,
  clearRawSnapshots,
  type RawVaultSnapshot,
} from './rawSnapshotCache';

const PRINCIPAL_A = 'aaaaa-aa';
const PRINCIPAL_B = '2vxsx-fae';

function snap(n: bigint): RawVaultSnapshot {
  return { collateralAmount: n, borrowedIcusd: n * 2n, icpMargin: n * 3n };
}

describe('rawSnapshotCache (FE-002)', () => {
  beforeEach(() => {
    clearRawSnapshots();
  });

  it('FE-002: returns warmed snapshots and vault ids for the same principal', () => {
    warmRawSnapshots(PRINCIPAL_A, [[1, snap(100n)], [2, snap(200n)]]);

    expect(getRawSnapshot(PRINCIPAL_A, 1)).toEqual(snap(100n));
    expect(getRawSnapshot(PRINCIPAL_A, 2)).toEqual(snap(200n));
    expect(getRawVaultIds(PRINCIPAL_A)).toEqual(new Set([1, 2]));
  });

  it('FE-002: never serves a snapshot captured under a different principal', () => {
    warmRawSnapshots(PRINCIPAL_A, [[1, snap(100n)]]);

    expect(getRawSnapshot(PRINCIPAL_B, 1)).toBeNull();
    expect(getRawVaultIds(PRINCIPAL_B)).toBeNull();
  });

  it('FE-002: returns null reads when no principal is connected', () => {
    warmRawSnapshots(PRINCIPAL_A, [[1, snap(100n)]]);

    expect(getRawSnapshot(null, 1)).toBeNull();
    expect(getRawVaultIds(null)).toBeNull();
  });

  it('FE-002: warming under a new principal drops the previous wallet entries entirely', () => {
    warmRawSnapshots(PRINCIPAL_A, [[1, snap(100n)], [2, snap(200n)]]);
    // Account switch: only vault 7 is warmed for B. Vaults 1/2 must not
    // leak through even if B later warms an overlapping id set.
    warmRawSnapshot(PRINCIPAL_B, 7, snap(700n));

    expect(getRawSnapshot(PRINCIPAL_B, 1)).toBeNull();
    expect(getRawSnapshot(PRINCIPAL_B, 2)).toBeNull();
    expect(getRawSnapshot(PRINCIPAL_B, 7)).toEqual(snap(700n));
    expect(getRawSnapshot(PRINCIPAL_A, 1)).toBeNull();
    expect(getRawVaultIds(PRINCIPAL_A)).toBeNull();
  });

  it('FE-002: warmRawVaultIds rekeys on principal change without leaking old snapshots', () => {
    warmRawSnapshots(PRINCIPAL_A, [[1, snap(100n)]]);
    warmRawVaultIds(PRINCIPAL_B, [5, 6]);

    expect(getRawVaultIds(PRINCIPAL_B)).toEqual(new Set([5, 6]));
    expect(getRawSnapshot(PRINCIPAL_B, 1)).toBeNull();
    expect(getRawSnapshot(PRINCIPAL_A, 1)).toBeNull();
  });

  it('FE-002: clearRawSnapshots wipes everything (clearVaultCache / disconnect path)', () => {
    warmRawSnapshots(PRINCIPAL_A, [[1, snap(100n)]]);
    warmRawVaultIds(PRINCIPAL_A, [1]);

    clearRawSnapshots();

    expect(getRawSnapshot(PRINCIPAL_A, 1)).toBeNull();
    expect(getRawVaultIds(PRINCIPAL_A)).toBeNull();
  });

  it('FE-002: upserting a single vault keeps other snapshots for the same principal', () => {
    warmRawSnapshots(PRINCIPAL_A, [[1, snap(100n)], [2, snap(200n)]]);
    warmRawSnapshot(PRINCIPAL_A, 2, snap(999n));

    expect(getRawSnapshot(PRINCIPAL_A, 1)).toEqual(snap(100n));
    expect(getRawSnapshot(PRINCIPAL_A, 2)).toEqual(snap(999n));
  });

  it('returns null for an empty vault-id set (matches verifier "no snapshot" semantics)', () => {
    warmRawVaultIds(PRINCIPAL_A, []);
    expect(getRawVaultIds(PRINCIPAL_A)).toBeNull();
  });

  it('returns a defensive copy of the vault-id set', () => {
    warmRawVaultIds(PRINCIPAL_A, [1]);
    const ids = getRawVaultIds(PRINCIPAL_A)!;
    ids.add(99);
    expect(getRawVaultIds(PRINCIPAL_A)).toEqual(new Set([1]));
  });
});
