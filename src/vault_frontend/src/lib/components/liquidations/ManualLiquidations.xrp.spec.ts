import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const componentPath = resolve(__dirname, 'ManualLiquidations.svelte');

describe('ManualLiquidations XRP pending claim retries', () => {
  it('keeps manual XRP claim retries visible after the vault leaves the liquidatable list', () => {
    const source = readFileSync(componentPath, 'utf8');

    expect(source).toContain('orphanedPendingManualXrpClaims');
    expect(source).toContain('!sortedVaults.some((vault) => vault.vault_id === claim.vaultId)');
    expect(source).toContain('Vault #{pendingXrpClaim.vaultId}');
    expect(source).toContain('settlePendingManualXrpClaim(Number(pendingXrpClaim.vaultId), pendingXrpClaim)');
    expect(source).toContain('rumi_manual_xrp_pending_claims:');
    expect(source).toContain('loadPersistedManualXrpClaims(manualXrpClaimsOwner)');
    expect(source).toContain('localStorage.setItem(key, JSON.stringify(rows))');
    expect(source).toContain('localStorage.removeItem(key)');
  });
});
