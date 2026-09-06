import { describe, expect, it, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import {
  deserializeManualSolClaims,
  groupManualSolClaimsByVault,
  recoverManualSolClaimsForVault,
  removeManualSolPendingClaim,
  serializeManualSolClaims,
  settleManualSolClaim,
  upsertManualSolPendingClaim,
  upsertManualSolPendingClaims,
  type ManualSolPendingClaim,
  type ManualSolPendingClaimMap,
} from './manualSolLiquidation';

const claimAt = (
  claimId: string,
  vaultId: number,
  overrides: Partial<ManualSolPendingClaim> = {}
): ManualSolPendingClaim => ({
  claimId,
  vaultId,
  payoutAddress: `addr${claimId}`,
  ...overrides,
});

describe('manual SOL liquidation settlement flow', () => {
  it('settles the exact claim id returned by liquidation with the entered address, no destination tag', async () => {
    const settleSolClaim = vi.fn().mockResolvedValue({
      success: true,
      data: { signature: 'ABC123' },
    });

    const result = await settleManualSolClaim(
      { claimId: '91', payoutAddress: 'SolLiquidator' },
      settleSolClaim,
      vi.fn().mockResolvedValue(false)
    );

    expect(settleSolClaim).toHaveBeenCalledWith('91', 'SolLiquidator');
    expect(settleSolClaim.mock.calls[0]).toHaveLength(2); // no destination tag argument
    expect(result.status).toBe('settled');
    expect(result.message).toContain('claim #91 created');
    expect(result.message).toContain('settlement submitted');
  });

  it('keeps a submitted claim retryable until the backend claim disappears', async () => {
    const pending: ManualSolPendingClaim = {
      claimId: '91',
      vaultId: 5,
      payoutAddress: 'SolLiquidator',
    };
    const settleSolClaim = vi.fn().mockResolvedValue({
      success: true,
      data: { signature: 'ABC123' },
    });
    const hasOutstandingClaim = vi.fn().mockResolvedValue(true);

    const result = await settleManualSolClaim(pending, settleSolClaim, hasOutstandingClaim);

    expect(hasOutstandingClaim).toHaveBeenCalledWith('91');
    expect(result.status).toBe('retryable');
    if (result.status !== 'retryable') throw new Error('expected retryable result');
    expect(result.pendingClaim).toEqual(pending);
    expect(result.message).toContain('settlement submitted');
    expect(result.message).toContain('ABC123');
  });

  it('keeps a submitted claim retryable when the outstanding-claim confirmation read fails', async () => {
    const pending: ManualSolPendingClaim = {
      claimId: '92',
      payoutAddress: 'SolLiquidator',
    };
    const settleSolClaim = vi.fn().mockResolvedValue({
      success: true,
      data: { signature: 'DEF456' },
    });
    const hasOutstandingClaim = vi.fn().mockRejectedValue(new Error('query failed'));

    const result = await settleManualSolClaim(pending, settleSolClaim, hasOutstandingClaim);

    expect(result.status).toBe('retryable');
    if (result.status !== 'retryable') throw new Error('expected retryable result');
    expect(result.pendingClaim).toEqual(pending);
    expect(result.error).toBe('query failed');
  });

  it('preserves a retryable pending claim with the same address when settlement fails', async () => {
    const pending: ManualSolPendingClaim = {
      claimId: '91',
      vaultId: 5,
      payoutAddress: 'SolLiquidator',
    };
    const settleSolClaim = vi.fn().mockResolvedValue({
      success: false,
      error: 'Solana submit unavailable',
    });

    const result = await settleManualSolClaim(pending, settleSolClaim, vi.fn());

    expect(result.status).toBe('retryable');
    if (result.status !== 'retryable') throw new Error('expected retryable result');
    expect(result.pendingClaim).toEqual(pending);
    expect(result.message).toContain('claim #91 remains outstanding');
    expect(result.message.toLowerCase()).not.toContain('received sol');
  });

  it('recovers claims by matching custody nonce to vault id (fallback when no claim id came back on the result)', async () => {
    const getMyClaims = vi.fn().mockResolvedValue([
      { claimId: '10', custodyNonce: 7, lamports: 100n },
      { claimId: '11', custodyNonce: 8, lamports: 200n },
      { claimId: '12', vaultId: 7, lamports: 300n },
    ]);

    const recovered = await recoverManualSolClaimsForVault(7, getMyClaims);

    expect(getMyClaims).toHaveBeenCalledOnce();
    expect(recovered.map((claim) => claim.claimId)).toEqual(['10', '12']);
  });

  it('wires manual liquidation retries through the outstanding-claim confirmation read, tagless', () => {
    const source = readFileSync(resolve(__dirname, '../components/liquidations/ManualLiquidations.svelte'), 'utf8');
    const settleCall = source.slice(
      source.indexOf('const settlement = await settleManualSolClaim('),
      source.indexOf('liquidationSuccess = settlement.message;', source.indexOf('const settlement = await settleManualSolClaim('))
    );

    expect(settleCall).toContain('SolVaultService.settleSolClaim(claimId, payoutAddress)');
    expect(settleCall).toContain('SolVaultService.hasOutstandingClaim(claimId)');
    // No destination tag anywhere in the SOL settle call.
    expect(settleCall).not.toContain('destinationTag');
  });
});

describe('manual SOL pending claim store (keyed by claim id)', () => {
  it('keeps two distinct claims on the same vault instead of overwriting the first', () => {
    let map: ManualSolPendingClaimMap = {};
    map = upsertManualSolPendingClaim(map, claimAt('100', 5));
    map = upsertManualSolPendingClaim(map, claimAt('101', 5));

    expect(Object.keys(map).sort()).toEqual(['100', '101']);
    expect(map['100'].vaultId).toBe(5);
    expect(map['101'].vaultId).toBe(5);
  });

  it('replaces an existing claim when re-upserted under the same claim id', () => {
    let map: ManualSolPendingClaimMap = {};
    map = upsertManualSolPendingClaim(map, claimAt('100', 5, { payoutAddress: 'addrOld' }));
    map = upsertManualSolPendingClaim(map, claimAt('100', 5, { payoutAddress: 'addrNew' }));

    expect(Object.keys(map)).toEqual(['100']);
    expect(map['100'].payoutAddress).toBe('addrNew');
  });

  it('adds every recovered claim for the same vault (ambiguous-recovery fallback path)', () => {
    const map = upsertManualSolPendingClaims({}, [claimAt('100', 5), claimAt('101', 5)]);

    expect(Object.keys(map).sort()).toEqual(['100', '101']);
  });

  it('removes a single claim by id without touching its sibling on the same vault', () => {
    let map = upsertManualSolPendingClaims({}, [claimAt('100', 5), claimAt('101', 5)]);
    map = removeManualSolPendingClaim(map, '100');

    expect(Object.keys(map)).toEqual(['101']);
    expect(map['101'].vaultId).toBe(5);
  });

  it('groups multiple same-vault claims under the vault, sorted by claim id numerically', () => {
    const map = upsertManualSolPendingClaims({}, [
      claimAt('101', 5),
      claimAt('100', 5),
      claimAt('50', 9),
    ]);

    const grouped = groupManualSolClaimsByVault(map);

    expect(grouped[5].map((claim) => claim.claimId)).toEqual(['100', '101']);
    expect(grouped[9].map((claim) => claim.claimId)).toEqual(['50']);
  });

  it('round-trips two same-vault claims through serialize/deserialize', () => {
    const map = upsertManualSolPendingClaims({}, [
      claimAt('100', 5, { lamports: 123n }),
      claimAt('101', 5),
    ]);

    const restored = deserializeManualSolClaims(JSON.stringify(serializeManualSolClaims(map)));

    expect(Object.keys(restored).sort()).toEqual(['100', '101']);
    expect(restored['100'].vaultId).toBe(5);
    expect(restored['100'].lamports).toBe(123n);
    expect(restored['101'].vaultId).toBe(5);
  });

  it('drops malformed persisted entries and tolerates missing or invalid input', () => {
    expect(deserializeManualSolClaims(null)).toEqual({});
    expect(deserializeManualSolClaims('not json')).toEqual({});

    const restored = deserializeManualSolClaims(
      JSON.stringify({
        good: { claimId: '100', vaultId: 5, payoutAddress: 'addrA' },
        noAddress: { claimId: '101', vaultId: 5 },
        noClaimId: { vaultId: 5, payoutAddress: 'addrB' },
      })
    );

    expect(Object.keys(restored)).toEqual(['100']);
  });
});
