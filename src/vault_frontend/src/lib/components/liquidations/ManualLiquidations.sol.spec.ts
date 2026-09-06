import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const componentPath = resolve(__dirname, 'ManualLiquidations.svelte');

describe('ManualLiquidations SOL pending claim retries', () => {
  const source = readFileSync(componentPath, 'utf8');

  it('keeps the pending-claim store keyed by claim id, not vault id', () => {
    expect(source).toContain('let pendingManualSolClaims: ManualSolPendingClaimMap = {}');
    expect(source).toContain('upsertManualSolPendingClaim(pendingManualSolClaims, pendingClaim)');
    expect(source).toContain('upsertManualSolPendingClaims(pendingManualSolClaims, claims)');
    expect(source).toContain('removeManualSolPendingClaim(pendingManualSolClaims, claimId)');
    expect(source).toContain('serializeManualSolClaims(pendingManualSolClaims)');
    expect(source).toContain('deserializeManualSolClaims(localStorage.getItem(key))');
    expect(source).not.toContain('pendingManualSolClaims[vault.vault_id]');
  });

  it('renders every pending claim for a vault, not just one', () => {
    expect(source).toContain('groupManualSolClaimsByVault(pendingManualSolClaims)');
    expect(source).toContain('pendingManualSolClaimsByVault[vault.vault_id] ?? []');
    expect(source).toContain('{#each vaultPendingSolClaims as pendingSolClaim (pendingSolClaim.claimId)}');
  });

  it('settles a pending claim by the claim itself (claim-id identity)', () => {
    expect(source).toContain('settlePendingManualSolClaim(pendingSolClaim)');
    expect(source).toContain('async function settlePendingManualSolClaim(pendingClaim: ManualSolPendingClaim)');
    expect(source).toContain('clearPendingManualSolClaim(pendingClaim.claimId)');
  });

  it('settles directly off result.xrpClaimId when a SOL liquidation returns one (fast path, same as XRP)', () => {
    // result.xrpClaimId is the shared native-custody claim-id field: for a
    // native-SOL vault it carries the SOL claim id, so the success branch
    // should settle it directly instead of always scanning for it.
    const solBranchStart = source.indexOf('} else if (isSol && solPayout?.ok) {');
    const solBranch = source.slice(
      solBranchStart,
      source.indexOf('liquidationSuccess = `Liquidated vault #', solBranchStart)
    );

    expect(solBranch).toContain('if (result.xrpClaimId)');
    expect(solBranch).toContain('claimId: result.xrpClaimId');
    expect(solBranch).toContain('settlePendingManualSolClaim(pendingClaim)');
    // There is no separate solClaimId field; the shared field is reused.
    expect(source).not.toContain('result.solClaimId');
  });

  it('falls back to recoverSolClaimsForVault only when no claim id came back on the result', () => {
    const solBranchStart = source.indexOf('} else if (isSol && solPayout?.ok) {');
    const solBranch = source.slice(
      solBranchStart,
      source.indexOf('liquidationSuccess = `Liquidated vault #', solBranchStart)
    );
    const fallback = solBranch.slice(solBranch.indexOf('} else {'));

    expect(fallback).toContain('recoverSolClaimsForVault(vault)');
  });

  it('leaves the XRP fast path untouched by the SOL fast path', () => {
    const xrpBranch = source.slice(
      source.indexOf('if (isXrp && xrpPayout?.ok) {'),
      source.indexOf('} else if (isSol && solPayout?.ok) {')
    );

    expect(xrpBranch).toContain('if (result.xrpClaimId)');
    expect(xrpBranch).toContain('settlePendingManualXrpClaim(pendingClaim)');
    expect(xrpBranch).toContain('recoverXrpClaimsForVault(vault)');
  });

  it('registers every recovered claim from the SOL recovery path', () => {
    expect(source).toContain('registerRecoveredSolClaims(recovered)');
    expect(source).toContain('addPendingManualSolClaims(recovered)');
  });

  it('keeps manual SOL claim retries visible after the vault leaves the liquidatable list', () => {
    expect(source).toContain('orphanedPendingManualSolClaims');
    expect(source).toContain('settlePendingManualSolClaim(pendingSolClaim)');
  });

  it('persists pending claims per owner in localStorage under its own key prefix', () => {
    expect(source).toContain('rumi_manual_sol_pending_claims:');
    expect(source).toContain('loadPersistedManualSolClaims(manualSolClaimsOwner)');
  });

  it('requires a SOL payout address before enabling the liquidate button, with no destination-tag input', () => {
    const cardRight = source.slice(source.indexOf('<div class="card-right">'), source.indexOf('btn-liquidate'));
    expect(cardRight).toContain('SOL payout address');
    expect(cardRight).toContain('bind:value={solPayoutAddresses[vault.vault_id]}');
    expect(cardRight).not.toContain('sol-tag-input');
    expect(source).toContain("isSol && !(solPayoutAddresses[vault.vault_id] ?? '').trim()");
  });

  it('validates the SOL payout address with validateSolPayoutInput before submitting a liquidation', () => {
    expect(source).toContain('validateSolPayoutInput(solPayoutAddresses[vault.vault_id] ?? \'\')');
  });
});
