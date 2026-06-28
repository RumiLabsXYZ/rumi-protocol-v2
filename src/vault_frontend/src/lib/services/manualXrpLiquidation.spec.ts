import { describe, expect, it, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import {
  recoverManualXrpClaimsForVault,
  settleManualXrpClaim,
  type ManualXrpPendingClaim,
} from './manualXrpLiquidation';

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
