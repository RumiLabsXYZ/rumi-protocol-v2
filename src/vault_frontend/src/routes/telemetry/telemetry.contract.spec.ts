import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

const source = readFileSync(resolve(process.cwd(), 'src/routes/telemetry/+page.svelte'), 'utf8');
const serviceSource = readFileSync(resolve(process.cwd(), 'src/lib/services/cycleSentinelService.ts'), 'utf8');

describe('Cycle Sentinel telemetry route contract', () => {
  it('keeps public telemetry anonymous and renders operator actions only after signer confirmation', () => {
    expect(source).toContain('loadPublicTelemetry(createAnonymousSentinelActor())');
    expect(source).toContain('getPermissions(authenticated)');
    expect(source).toContain('{#if signer && actor}');
    expect(serviceSource).toContain('auth.getActor<SentinelActor>');
    expect(serviceSource).not.toMatch(/\bany\b/);
    expect(source.indexOf('loadPublicTelemetry(createAnonymousSentinelActor())')).toBeLessThan(source.indexOf('createAuthenticatedSentinelActor()'));
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
