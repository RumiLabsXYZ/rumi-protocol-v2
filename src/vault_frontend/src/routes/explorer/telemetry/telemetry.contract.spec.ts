import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it, vi } from 'vitest';
import { loadPublicTelemetry, sentinelManagement, type SentinelActor } from '$lib/services/cycleSentinelService';
import type { PublicOverview } from '$declarations/rumi_cycle_sentinel/rumi_cycle_sentinel.did';

const source = readFileSync(resolve(process.cwd(), 'src/routes/explorer/telemetry/+page.svelte'), 'utf8');
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

  it('makes an immediate check available only after signer confirmation and preserves read-only refresh', () => {
    const controls = source.slice(source.indexOf('<div class="check-buttons">'), source.indexOf('</header>'));
    expect(controls).toMatch(/\{#if signer && actor\}[\s\S]*on:click=\{runCheckNow\}/);
    expect(controls).toContain('disabled={checkingNow || loading}');
    expect(controls).toContain("checkingNow ? 'Checking now…' : 'Run check now'");
    expect(controls).toContain('may top up canisters under the current thresholds and spending limits');
    expect(controls).toContain('Refresh telemetry reads saved results.');
    const refreshSource = source.slice(source.indexOf('async function refresh()'), source.indexOf('async function checkOperatorAccess()'));
    expect(refreshSource).not.toContain('runMaintenanceNow');
    expect(refreshSource).not.toContain('createAuthenticatedSentinelActor');
    const checkSource = source.slice(source.indexOf('async function runCheckNow()'), source.indexOf('async function run(action:'));
    expect(checkSource.indexOf('assertCurrentSigner();')).toBeLessThan(checkSource.indexOf('await sentinelManagement.runMaintenanceNow(authenticated)'));
    expect(checkSource).toContain('if (checkingNow) return;');
    expect(checkSource).toContain('epoch === operatorAccessEpoch');
    expect(checkSource).toContain('session === currentWalletSession()');
    expect(checkSource).toContain('actor === authenticated');
    expect(checkSource).toContain('if (!checkSessionIsCurrent()) return;');
    expect(checkSource.lastIndexOf('if (!checkSessionIsCurrent()) return;')).toBeGreaterThan(checkSource.indexOf('await refresh();'));
    expect(checkSource).not.toContain('Action accepted');
    expect(checkSource).toContain('if (checkSessionIsCurrent()) authError');
  });

  it('reads the schedule and source freshness from published policy', () => {
    expect(source).toContain('checkIntervalLabel(funding ? fundingOptionalBigInt(funding.sample_interval_secs) : undefined)');
    expect(source).toContain('cyclesBalanceMaxAgeSecs(snapshot.overview)');
    expect(source).toContain('nextCheckLabel(optional(snapshot.overview.next_sample_at_secs))');
    expect(source).toContain('Automatic checks: <strong>{automaticCheckCadence}</strong>');
    expect(source).not.toContain('Every 4 hours');
  });

  it('never runs maintenance when public telemetry is refreshed', async () => {
    const runMaintenance = vi.fn();
    const publicActor = {
      get_public_overview: vi.fn().mockResolvedValue({} as PublicOverview),
      list_public_targets: vi.fn().mockResolvedValue({ Ok: { items: [], next_cursor: [] } }),
      list_public_alarms: vi.fn().mockResolvedValue({ Ok: { items: [], next_cursor: [] } }),
      run_maintenance_now: runMaintenance,
    } as unknown as SentinelActor;
    await loadPublicTelemetry(publicActor);
    expect(runMaintenance).not.toHaveBeenCalled();
  });

  it('preserves immediate-check failures, including Busy, instead of reporting completion', async () => {
    const runMaintenance = vi.fn().mockResolvedValueOnce({ Err: 'Busy' }).mockResolvedValueOnce({ Err: 'NotSigner' }).mockResolvedValueOnce({ Ok: null });
    const authenticated = { run_maintenance_now: runMaintenance } as unknown as SentinelActor;
    await expect(sentinelManagement.runMaintenanceNow(authenticated)).rejects.toThrow('Busy');
    await expect(sentinelManagement.runMaintenanceNow(authenticated)).rejects.toThrow('NotSigner');
    await expect(sentinelManagement.runMaintenanceNow(authenticated)).resolves.toBeUndefined();
    expect(runMaintenance).toHaveBeenCalledTimes(3);
    expect(runMaintenance).toHaveBeenCalledWith();
  });

  it('exposes every accepted Task 8 operator control as a visible label and call', () => {
    for (const label of [
      'Propose register target', 'Propose target update', 'Propose target removal',
      'Pause target immediately', 'Propose governed unpause', 'Propose add signer',
      'Propose remove signer', 'Propose signer threshold', 'Propose global policy',
      'Approve proposal', 'Execute proposal', 'Cancel proposal', 'Acknowledge',
      'Manual top-up', 'Attach delivery proof', 'Attach refund proof', 'Resolve unknown as spent',
      'Display name', 'Project', 'Tags', 'Environment', 'Criticality', 'Observation mode',
      'Low threshold (T-cycles)', 'Refill amount (T-cycles)', 'Daily cap (T-cycles)', 'Cooldown seconds',
      'Optional burn anomaly limit (T-cycles)', 'Enabled', 'Auto-top-up', 'Unpause timelock seconds',
      'Spend-policy timelock seconds', 'Target-registry timelock seconds', 'Signer-change timelock seconds',
    ]) expect(source).toContain(label);
    for (const method of [
      'proposeRegisterTarget', 'proposeUpdateTarget', 'proposeRemoveTarget', 'pauseTarget',
      'proposeUnpauseTarget', 'proposeAddSigner', 'proposeRemoveSigner', 'proposeSetSignerThreshold',
      'proposeSetGlobalPolicy', 'approveProposal', 'executeProposal', 'cancelProposal',
      'acknowledgeAlarm', 'manualTopUp', 'attachBlockProof', 'attachRefundBlockProof', 'resolveUnknownAsSpent',
    ]) expect(source).toContain(`sentinelManagement.${method}`);
  });
  it('explains an idle registry and shows cycle usage instead of a silent wall of Unavailable', () => {
    expect(source).toContain('snapshot.overview.unobserved_count === snapshot.overview.target_count');
    expect(source).toContain('Observation has not been switched on yet.');
    expect(source).toContain('{#if fundingUnavailable}');
    expect(source).toContain('<th>Burn / day</th>');
    expect(source).toContain('<th>Runway</th>');
    expect(source).toContain('optional(row.burn_cycles_per_day)');
    expect(source).toContain('optional(row.runway_secs)');
    expect(source).toContain('targetStateLabel(row)');
    expect(source).toContain('Awaiting first sample');
  });

  it('lives under the Explorer tab, not the app header, and keeps the old URL working', () => {
    const appLayout = readFileSync(resolve(process.cwd(), 'src/routes/+layout.svelte'), 'utf8');
    const explorerLayout = readFileSync(resolve(process.cwd(), 'src/routes/explorer/+layout.svelte'), 'utf8');
    const legacyRedirect = readFileSync(resolve(process.cwd(), 'src/routes/telemetry/+page.ts'), 'utf8');
    expect(appLayout).not.toContain('href="/telemetry"');
    expect(explorerLayout).toContain("href: '/explorer/telemetry'");
    expect(legacyRedirect).toContain("redirect(308, '/explorer/telemetry')");
  });
});
