import { describe, expect, it, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import {
  deserializeManualXrpClaims,
  groupManualXrpClaimsByVault,
  recoverManualXrpClaimsForVault,
  removeManualXrpPendingClaim,
  serializeManualXrpClaims,
  settleManualXrpClaim,
  upsertManualXrpPendingClaim,
  upsertManualXrpPendingClaims,
  type ManualXrpPendingClaim,
  type ManualXrpPendingClaimMap,
} from './manualXrpLiquidation';

const claimAt = (
  claimId: string,
  vaultId: number,
  overrides: Partial<ManualXrpPendingClaim> = {}
): ManualXrpPendingClaim => ({
  claimId,
  vaultId,
  payoutAddress: `r${claimId}`,
  ...overrides,
});

describe('manual XRP liquidation settlement flow', () => {
  it('settles the exact claim id returned by liquidation with the entered address and tag', async () => {
    const settleXrpClaim = vi.fn().mockResolvedValue({
      success: true,
      data: { txHash: 'ABC123' },
    });

    const result = await settleManualXrpClaim(
      { claimId: '91', payoutAddress: 'rLiquidator', destinationTag: 123 },
      settleXrpClaim,
      vi.fn().mockResolvedValue(false)
    );

    expect(settleXrpClaim).toHaveBeenCalledWith('91', 'rLiquidator', 123);
    expect(result.status).toBe('settled');
    expect(result.message).toContain('claim #91 created');
    expect(result.message).toContain('settlement submitted');
  });

  it('keeps a submitted claim retryable until the backend claim disappears', async () => {
    const pending: ManualXrpPendingClaim = {
      claimId: '91',
      vaultId: 5,
      payoutAddress: 'rLiquidator',
      destinationTag: 321,
    };
    const settleXrpClaim = vi.fn().mockResolvedValue({
      success: true,
      data: { txHash: 'ABC123' },
    });
    const hasOutstandingClaim = vi.fn().mockResolvedValue(true);

    const result = await settleManualXrpClaim(pending, settleXrpClaim, hasOutstandingClaim);

    expect(hasOutstandingClaim).toHaveBeenCalledWith('91');
    expect(result.status).toBe('retryable');
    if (result.status !== 'retryable') throw new Error('expected retryable result');
    expect(result.pendingClaim).toEqual(pending);
    expect(result.message).toContain('settlement submitted');
    expect(result.message).toContain('ABC123');
  });

  it('keeps a submitted claim retryable when the outstanding-claim confirmation read fails', async () => {
    const pending: ManualXrpPendingClaim = {
      claimId: '92',
      payoutAddress: 'rLiquidator',
    };
    const settleXrpClaim = vi.fn().mockResolvedValue({
      success: true,
      data: { txHash: 'DEF456' },
    });
    const hasOutstandingClaim = vi.fn().mockRejectedValue(new Error('query failed'));

    const result = await settleManualXrpClaim(pending, settleXrpClaim, hasOutstandingClaim);

    expect(result.status).toBe('retryable');
    if (result.status !== 'retryable') throw new Error('expected retryable result');
    expect(result.pendingClaim).toEqual(pending);
    expect(result.error).toBe('query failed');
  });

  it('preserves a retryable pending claim with the same address and tag when settlement fails', async () => {
    const pending: ManualXrpPendingClaim = {
      claimId: '91',
      vaultId: 5,
      payoutAddress: 'rLiquidator',
      destinationTag: 321,
    };
    const settleXrpClaim = vi.fn().mockResolvedValue({
      success: false,
      error: 'XRPL submit unavailable',
    });

    const result = await settleManualXrpClaim(pending, settleXrpClaim, vi.fn());

    expect(result.status).toBe('retryable');
    if (result.status !== 'retryable') throw new Error('expected retryable result');
    expect(result.pendingClaim).toEqual(pending);
    expect(result.message).toContain('claim #91 remains outstanding');
    expect(result.message.toLowerCase()).not.toContain('received xrp');
  });

  it('recovers ambiguous liquidation claims by matching custody nonce to vault id', async () => {
    const getMyClaims = vi.fn().mockResolvedValue([
      { claimId: '10', custodyNonce: 7, drops: 100n },
      { claimId: '11', custodyNonce: 8, drops: 200n },
      { claimId: '12', vaultId: 7, drops: 300n },
    ]);

    const recovered = await recoverManualXrpClaimsForVault(7, getMyClaims);

    expect(getMyClaims).toHaveBeenCalledOnce();
    expect(recovered.map((claim) => claim.claimId)).toEqual(['10', '12']);
  });

  it('wires manual liquidation retries through the outstanding-claim confirmation read', () => {
    const source = readFileSync(resolve(__dirname, '../components/liquidations/ManualLiquidations.svelte'), 'utf8');
    const settleCall = source.slice(
      source.indexOf('const settlement = await settleManualXrpClaim('),
      source.indexOf('liquidationSuccess = settlement.message;')
    );

    expect(settleCall).toContain('XrpVaultService.settleXrpClaim(claimId, payoutAddress, destinationTag)');
    expect(settleCall).toContain('XrpVaultService.hasOutstandingClaim(claimId)');
  });
});

describe('manual XRP pending claim store (keyed by claim id)', () => {
  it('keeps two distinct claims on the same vault instead of overwriting the first', () => {
    let map: ManualXrpPendingClaimMap = {};
    map = upsertManualXrpPendingClaim(map, claimAt('100', 5));
    map = upsertManualXrpPendingClaim(map, claimAt('101', 5));

    expect(Object.keys(map).sort()).toEqual(['100', '101']);
    expect(map['100'].vaultId).toBe(5);
    expect(map['101'].vaultId).toBe(5);
  });

  it('replaces an existing claim when re-upserted under the same claim id', () => {
    let map: ManualXrpPendingClaimMap = {};
    map = upsertManualXrpPendingClaim(map, claimAt('100', 5, { payoutAddress: 'rOld' }));
    map = upsertManualXrpPendingClaim(map, claimAt('100', 5, { payoutAddress: 'rNew' }));

    expect(Object.keys(map)).toEqual(['100']);
    expect(map['100'].payoutAddress).toBe('rNew');
  });

  it('adds every recovered claim for the same vault (ambiguous-recovery path)', () => {
    const map = upsertManualXrpPendingClaims({}, [claimAt('100', 5), claimAt('101', 5)]);

    expect(Object.keys(map).sort()).toEqual(['100', '101']);
  });

  it('removes a single claim by id without touching its sibling on the same vault', () => {
    let map = upsertManualXrpPendingClaims({}, [claimAt('100', 5), claimAt('101', 5)]);
    map = removeManualXrpPendingClaim(map, '100');

    expect(Object.keys(map)).toEqual(['101']);
    expect(map['101'].vaultId).toBe(5);
  });

  it('groups multiple same-vault claims under the vault, sorted by claim id numerically', () => {
    const map = upsertManualXrpPendingClaims({}, [
      claimAt('101', 5),
      claimAt('100', 5),
      claimAt('50', 9),
    ]);

    const grouped = groupManualXrpClaimsByVault(map);

    expect(grouped[5].map((claim) => claim.claimId)).toEqual(['100', '101']);
    expect(grouped[9].map((claim) => claim.claimId)).toEqual(['50']);
  });

  it('round-trips two same-vault claims through serialize/deserialize', () => {
    const map = upsertManualXrpPendingClaims({}, [
      claimAt('100', 5, { destinationTag: 7, drops: 123n }),
      claimAt('101', 5),
    ]);

    const restored = deserializeManualXrpClaims(JSON.stringify(serializeManualXrpClaims(map)));

    expect(Object.keys(restored).sort()).toEqual(['100', '101']);
    expect(restored['100'].vaultId).toBe(5);
    expect(restored['100'].destinationTag).toBe(7);
    expect(restored['100'].drops).toBe(123n);
    expect(restored['101'].vaultId).toBe(5);
  });

  it('migrates a legacy vault-id-keyed store to claim-id keys', () => {
    const legacy = JSON.stringify({
      '5': { claimId: '100', vaultId: 5, payoutAddress: 'rA' },
    });

    const restored = deserializeManualXrpClaims(legacy);

    expect(Object.keys(restored)).toEqual(['100']);
    expect(restored['100'].vaultId).toBe(5);
  });

  it('drops malformed persisted entries and tolerates missing or invalid input', () => {
    expect(deserializeManualXrpClaims(null)).toEqual({});
    expect(deserializeManualXrpClaims('not json')).toEqual({});

    const restored = deserializeManualXrpClaims(
      JSON.stringify({
        good: { claimId: '100', vaultId: 5, payoutAddress: 'rA' },
        noAddress: { claimId: '101', vaultId: 5 },
        noClaimId: { vaultId: 5, payoutAddress: 'rB' },
      })
    );

    expect(Object.keys(restored)).toEqual(['100']);
  });
});
