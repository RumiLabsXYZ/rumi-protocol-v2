import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const componentPath = resolve(__dirname, 'ManualLiquidations.svelte');

describe('ManualLiquidations XRP pending claim retries', () => {
  const source = readFileSync(componentPath, 'utf8');

  it('keeps the pending-claim store keyed by claim id, not vault id', () => {
    // The store type and persistence go through the claim-id-keyed helpers so a
    // second claim on the same vault cannot clobber the first.
    expect(source).toContain('let pendingManualXrpClaims: ManualXrpPendingClaimMap = {}');
    expect(source).toContain('upsertManualXrpPendingClaim(pendingManualXrpClaims, pendingClaim)');
    expect(source).toContain('upsertManualXrpPendingClaims(pendingManualXrpClaims, claims)');
    expect(source).toContain('removeManualXrpPendingClaim(pendingManualXrpClaims, claimId)');
    expect(source).toContain('serializeManualXrpClaims(pendingManualXrpClaims)');
    expect(source).toContain('deserializeManualXrpClaims(localStorage.getItem(key))');
    // No vault-id-keyed indexing of the store remains.
    expect(source).not.toContain('pendingManualXrpClaims[vault.vault_id]');
  });

  it('renders every pending claim for a vault, not just one', () => {
    expect(source).toContain('groupManualXrpClaimsByVault(pendingManualXrpClaims)');
    expect(source).toContain('pendingManualXrpClaimsByVault[vault.vault_id] ?? []');
    expect(source).toContain('{#each vaultPendingXrpClaims as pendingXrpClaim (pendingXrpClaim.claimId)}');
  });

  it('settles a pending claim by the claim itself (claim-id identity)', () => {
    expect(source).toContain('settlePendingManualXrpClaim(pendingXrpClaim)');
    expect(source).toContain('async function settlePendingManualXrpClaim(pendingClaim: ManualXrpPendingClaim)');
    expect(source).toContain('clearPendingManualXrpClaim(pendingClaim.claimId)');
    // The old vault-id-addressed settle call must be gone.
    expect(source).not.toContain('settlePendingManualXrpClaim(Number(pendingXrpClaim.vaultId)');
    expect(source).not.toContain('settlePendingManualXrpClaim(vault.vault_id');
  });

  it('registers every recovered claim from the ambiguous-failure path', () => {
    // Recovery must add all claims, never just recovered[0].
    expect(source).toContain('registerRecoveredXrpClaims(recovered)');
    expect(source).not.toContain('recovered[0]');
    expect(source).toContain('addPendingManualXrpClaims(recovered)');
  });

  it('keeps manual XRP claim retries visible after the vault leaves the liquidatable list', () => {
    expect(source).toContain('orphanedPendingManualXrpClaims');
    expect(source).toContain('!sortedVaults.some((vault) => vault.vault_id === claim.vaultId)');
    expect(source).toContain('Vault #{pendingXrpClaim.vaultId}');
    expect(source).toContain('settlePendingManualXrpClaim(pendingXrpClaim)');
  });

  it('persists pending claims per owner in localStorage', () => {
    expect(source).toContain('rumi_manual_xrp_pending_claims:');
    expect(source).toContain('loadPersistedManualXrpClaims(manualXrpClaimsOwner)');
    expect(source).toContain('localStorage.setItem(key, JSON.stringify(rows))');
    expect(source).toContain('localStorage.removeItem(key)');
  });
});
