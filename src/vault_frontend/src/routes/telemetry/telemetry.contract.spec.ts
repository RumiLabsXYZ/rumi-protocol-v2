import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

const source = readFileSync(resolve(process.cwd(), 'src/routes/telemetry/+page.svelte'), 'utf8');
const serviceSource = readFileSync(resolve(process.cwd(), 'src/lib/services/cycleSentinelService.ts'), 'utf8');

describe('Cycle Sentinel telemetry route contract', () => {
  it('keeps public telemetry anonymous and performs the wallet signer check only from an explicit operator action', () => {
    expect(source).toContain('loadPublicTelemetry(createAnonymousSentinelActor())');
    expect(source).toContain('getPermissions(authenticated)');
    expect(source).toContain('async function checkOperatorAccess()');
    expect(source).toContain('Check operator access');
    expect(source).toContain('checkedSession !== session');
    expect(source).toContain('session !== currentWalletSession()');
    expect(source).toContain('currentWalletType.subscribe');
    expect(source).toContain('walletSessionGeneration.subscribe');
    expect(source).toContain('walletStore.subscribe');
    expect(source).toContain('A partially completed signer check is not authorization');
    expect(source).toContain('assertCurrentSigner();');
    expect(source).toContain('{#if signer && actor}');
    expect(serviceSource).toContain('auth.getActor<SentinelActor>');
    expect(serviceSource).not.toMatch(/\bany\b/);
    const refreshSource = source.slice(source.indexOf('async function refresh()'), source.indexOf('async function checkOperatorAccess()'));
    expect(refreshSource).toContain('loadPublicTelemetry(createAnonymousSentinelActor())');
    expect(refreshSource).not.toContain('getPermissions(');
  });

  it('exposes every accepted Task 8 operator control as a visible label and call', () => {
    for (const label of [
      'Propose register target', 'Propose target update', 'Propose target removal',
      'Pause target immediately', 'Propose governed unpause', 'Propose add signer',
      'Propose remove signer', 'Propose signer threshold', 'Propose global policy',
      'Approve proposal', 'Execute proposal', 'Cancel proposal', 'Acknowledge',
      'Manual top-up', 'Attach delivery proof', 'Attach refund proof', 'Resolve unknown as spent',
      'Display name', 'Project', 'Tags', 'Environment', 'Criticality', 'Observation mode',
      'Low threshold cycles', 'Refill cycles', 'Daily cap cycles', 'Cooldown seconds',
      'Optional burn anomaly limit', 'Enabled', 'Auto-top-up', 'Unpause timelock seconds',
      'Spend-policy timelock seconds', 'Target-registry timelock seconds', 'Signer-change timelock seconds',
    ]) expect(source).toContain(label);
    for (const method of [
      'proposeRegisterTarget', 'proposeUpdateTarget', 'proposeRemoveTarget', 'pauseTarget',
      'proposeUnpauseTarget', 'proposeAddSigner', 'proposeRemoveSigner', 'proposeSetSignerThreshold',
      'proposeSetGlobalPolicy', 'approveProposal', 'executeProposal', 'cancelProposal',
      'acknowledgeAlarm', 'manualTopUp', 'attachBlockProof', 'attachRefundBlockProof', 'resolveUnknownAsSpent',
    ]) expect(source).toContain(`sentinelManagement.${method}`);
  });
});
