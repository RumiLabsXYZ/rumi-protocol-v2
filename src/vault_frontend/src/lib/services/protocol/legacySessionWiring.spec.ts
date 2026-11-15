import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { parseExactTokenAmount } from '../stabilityPoolConversion';

const vaultCard = readFileSync(resolve(__dirname, '../../components/vault/VaultCard.svelte'), 'utf8');
const vaultPullRecoveryBanner = readFileSync(resolve(__dirname, '../../components/vault/VaultPullRecoveryBanner.svelte'), 'utf8');
const dashboard = readFileSync(resolve(__dirname, '../../../routes/+page.svelte'), 'utf8');
const vaultStore = readFileSync(resolve(__dirname, '../../stores/vaultStore.ts'), 'utf8');

describe('legacy vault UI session-bound action wiring', () => {
  it('routes create-vault through the action context and bound API', () => {
    expect(dashboard).toContain('captureActionBoundContext($principal?.toText())');
    expect(dashboard).toContain('protocolService.openVaultAndBorrowBound(');
    expect(dashboard).toContain('if (!ctx.assertCurrent())');
  });

  it('routes borrow, partial withdrawal, and close through a pinned vault-owner context', () => {
    expect(vaultCard).toContain('captureActionBoundContext(vault.owner)');
    expect(vaultCard).toContain('protocolService.borrowFromVaultBound(');
    expect(vaultCard).toContain('protocolService.withdrawPartialCollateralBound(');
    expect(vaultCard).toContain('protocolService.withdrawCollateralAndCloseVaultBound(');
  });

  it('keeps add-margin on the enabled exact-receipt recovery path', () => {
    expect(vaultCard).toContain('const ADD_MARGIN_V2_ENABLED: boolean = true');
    expect(vaultCard).toContain('parseExactTokenAmount(addCollateralAmount, collateralDecimals)');
    expect(vaultCard).toContain('protocolService.addMarginToVaultBound(actionContext, vault.vaultId, amountRaw, vaultCollateralType)');
    expect(vaultCard).not.toContain('protocolService.addMarginToVault(vault.vaultId, amount');
    expect(vaultCard).toContain("runConfirmedActionRefresh('Collateral deposit', actionContext");
  });

  it('awaits every bound post-action balance refresh so failures stay in the refresh warning path', () => {
    const cardRefreshes = vaultCard.split('\n').filter(line => line.includes('walletStore.refreshBalance({ skipCache: true }'));
    const recoveryRefreshes = vaultPullRecoveryBanner.split('\n').filter(line => line.includes('walletStore.refreshBalance({ skipCache: true }'));
    expect(cardRefreshes).toHaveLength(6);
    expect(cardRefreshes.every(line => line.includes('await walletStore.refreshBalance('))).toBe(true);
    expect(recoveryRefreshes).toHaveLength(2);
    expect(recoveryRefreshes.every(line => line.includes('await walletStore.refreshBalance('))).toBe(true);
    expect(vaultPullRecoveryBanner.match(/runConfirmedActionRefresh\(/g)).toHaveLength(2);
  });

  it('rejects stale fractional input and refreshes after creation with the captured context', () => {
    expect(dashboard).toContain('parseExactTokenAmount(collateralAmountInput, collateralDecimals)');
    expect(dashboard).toContain('parseIcusdAmountToE8s(icusdAmountInput)');
    expect(dashboard).not.toContain('collateralAmount.toFixed(collateralDecimals)');
    expect(dashboard).not.toContain('icusdAmount.toFixed(8)');
    expect(dashboard).toContain('vaultStore.loadVaults(true, ctx)');
    expect(vaultStore).toContain('assertBoundSession(ctx)');
    expect(vaultStore).toContain('async refreshVault(vaultId: number, ctx?: ActionBoundContext)');
    expect(vaultStore).toContain('async loadVaults(forceRefresh = false, ctx?: ActionBoundContext)');
  });

  it('rejects collateral input that would otherwise be silently truncated', () => {
    expect(parseExactTokenAmount('1.12345678', 8)).toBe(112_345_678n);
    expect(() => parseExactTokenAmount('1.123456789', 8)).toThrow('at most 8 decimal places');
  });
});
