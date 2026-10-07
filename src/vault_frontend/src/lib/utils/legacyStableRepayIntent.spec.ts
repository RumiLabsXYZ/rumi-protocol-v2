import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import path from 'node:path';
import { legacyStableRepayIntentKey, parseLegacyStableRepayIntent } from './legacyStableRepayIntent';

const apiClientPath = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../services/protocol/apiClient.ts');

describe('legacy stable repayment recovery intent', () => {
  it('scopes a hold to owner, network, vault, and stable ledger token', () => {
    const key = legacyStableRepayIntentKey('owner-a', 'mainnet', 12, 'CKUSDT');
    expect(key).not.toBe(legacyStableRepayIntentKey('owner-b', 'mainnet', 12, 'CKUSDT'));
    expect(key).not.toBe(legacyStableRepayIntentKey('owner-a', 'local', 12, 'CKUSDT'));
    expect(key).not.toBe(legacyStableRepayIntentKey('owner-a', 'mainnet', 13, 'CKUSDT'));
    expect(key).not.toBe(legacyStableRepayIntentKey('owner-a', 'mainnet', 12, 'CKUSDC'));
  });

  it('accepts only exact persisted intent fields and stages', () => {
    const raw = JSON.stringify({ version: 1, owner: 'owner-a', network: 'mainnet', vaultId: '12', token: 'CKUSDT', requestedAmountRaw: '1234567', stage: 'backend_dispatch_attempted', updatedAt: 1 });
    expect(parseLegacyStableRepayIntent(raw)?.requestedAmountRaw).toBe('1234567');
    expect(parseLegacyStableRepayIntent(raw.replace('backend_dispatch_attempted', 'unknown'))).toBeNull();
    expect(parseLegacyStableRepayIntent(null)).toBeNull();
  });

  it('fails closed before approval on the legacy wrapper and routes the card through V2', () => {
    const source = readFileSync(apiClientPath, 'utf8');
    const start = source.indexOf('static async repayToVaultWithStable(');
    const end = source.indexOf('static async closeVault(', start);
    const method = source.slice(start, end);
    expect(start).toBeGreaterThanOrEqual(0);
    expect(method).toContain('return ApiClient.legacyStableRepaymentDisabled()');
    expect(method).not.toContain('icrc2_approve');
    expect(method).not.toContain('repay_to_vault_with_stable(');
    expect(source).toContain('actor.repay_to_vault_with_stable_v2(requestId');
    const card = readFileSync(path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../components/vault/VaultCard.svelte'), 'utf8');
    expect(card).toContain('protocolManager.repayToVaultWithStableV2(');
    expect(card).not.toContain('protocolManager.repayToVaultWithStable(');
  });
});
