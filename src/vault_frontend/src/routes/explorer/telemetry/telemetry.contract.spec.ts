import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it, vi } from 'vitest';
import { Principal } from '@dfinity/principal';
import { loadPublicTelemetry, sentinelManagement, type SentinelActor } from '$lib/services/cycleSentinelService';
import type { PublicOverview, TargetUpdate } from '$declarations/rumi_cycle_sentinel/rumi_cycle_sentinel.did';

const source = readFileSync(resolve(process.cwd(), 'src/routes/explorer/telemetry/+page.svelte'), 'utf8');
const serviceSource = readFileSync(resolve(process.cwd(), 'src/lib/services/cycleSentinelService.ts'), 'utf8');

describe('Cycle Sentinel telemetry route contract', () => {
  it('loads telemetry and initial operator state from one authenticated dashboard query', () => {
    expect(source).toContain('loadOperatorDashboard(authenticated)');
    expect(source).toContain('canViewSentinelTelemetry(latestWalletConnection.principal)');
    expect(source).toContain('Sentinel telemetry is private');
    expect(source).toContain('signer = next.governance.is_signer');
    expect(source).toContain('proposals = next.proposals');
    expect(source).toContain('unresolved = next.unresolved');
    expect(source).toContain('walletStore.subscribe');
    expect(source).toContain('OPERATOR_QUERY_TIMEOUT_MS = 25_000');
    expect(source).toContain('assertCurrentSigner();');
    expect(source).toContain('{#if signer && actor}');
    expect(serviceSource).toContain('actor.get_operator_dashboard()');
    expect(serviceSource).not.toMatch(/\bany\b/);
    expect(source).not.toContain('async function checkOperatorAccess()');
    expect(source).not.toContain('Check operator access');
    const refreshSource = source.slice(source.indexOf('async function refresh()'), source.indexOf('function reconcileManualTopUpLock('));
    expect(refreshSource).toContain('loadOperatorDashboard(authenticated)');
    expect(refreshSource).not.toContain('runMaintenanceNow');
    expect(refreshSource).not.toContain('getPermissions(');
  });

  it('makes an immediate check available only after signer confirmation and preserves read-only refresh', () => {
    const controls = source.slice(source.indexOf('<div class="check-buttons">'), source.indexOf('</header>'));
    expect(controls).toMatch(/\{#if signer && actor\}[\s\S]*on:click=\{runCheckNow\}/);
    expect(controls).toContain('disabled={checkingNow || loading}');
    expect(controls).toContain("checkingNow ? 'Checking now…' : 'Run check now'");
    expect(controls).toContain('may refuel Sentinel or top up registered canisters under the current reserves, thresholds and spending limits');
    expect(controls).toContain('Refresh telemetry reads saved results.');
    const refreshSource = source.slice(source.indexOf('async function refresh()'), source.indexOf('function reconcileManualTopUpLock('));
    expect(refreshSource).not.toContain('runMaintenanceNow');
    expect(refreshSource).toContain('loadOperatorDashboard(authenticated)');
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

  it('bounds the combined dashboard query and keeps signer-only record loading explicit', () => {
    expect(source).toContain('OPERATOR_QUERY_TIMEOUT_MS = 25_000');
    expect(source).toContain("withOperatorQueryTimeout(loadOperatorDashboard(authenticated), 'Loading Sentinel telemetry and operator status')");
    const dataLoadSource = source.slice(source.indexOf('async function refresh()'), source.indexOf('function reconcileManualTopUpLock('));
    expect(dataLoadSource).toContain('const authenticated = await createAuthenticatedSentinelActor()');
    expect(dataLoadSource).toContain('operatorDataError = signer && !next.operatorRecordsAvailable');
    expect(source).toContain('async function loadAllOperatorRecords()');
    expect(source).toContain("'Loading remaining operator records'");
    expect(source).toContain('assertCurrentSigner();');
    expect(source).toContain('Load all proposal and unresolved records');
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
      'Manual top-up', 'Funding source', 'Amount in T-cycles', 'Amount in ICP', 'Confirm top-up',
      'Attach delivery proof', 'Attach refund proof', 'Resolve unknown as spent',
      'Display name', 'Project', 'Tags', 'Environment', 'Criticality', 'Observation mode',
      'Low threshold (T-cycles)', 'Refill amount (T-cycles)', 'Daily cap (T-cycles)', 'Cooldown seconds',
      'Optional burn anomaly limit (T-cycles)', 'Enabled', 'Auto-top-up', 'Unpause timelock seconds',
      'Spend-policy timelock seconds', 'Target-registry timelock seconds', 'Signer-change timelock seconds',
    ]) expect(source).toContain(label);
    for (const method of [
      'proposeRegisterTarget', 'proposeUpdateTarget', 'proposeRemoveTarget', 'pauseTarget',
      'proposeUnpauseTarget', 'proposeAddSigner', 'proposeRemoveSigner', 'proposeSetSignerThreshold',
      'proposeSetGlobalPolicy', 'approveProposal', 'executeProposal', 'cancelProposal',
      'manualTopUpWithAmount', 'attachBlockProof', 'attachRefundBlockProof', 'resolveUnknownAsSpent',
    ]) expect(source).toContain(`sentinelManagement.${method}`);
  });

  it('proposes bulk target flags once and reports the proposal ID and remaining governance steps', () => {
    expect(source).toContain('`Enable monitoring · ${monitoringPendingCount}`');
    expect(source).toContain("'Monitoring enabled for all'");
    expect(source).toContain('`Enable auto top-up · ${autoTopupPendingCount}`');
    expect(source).toContain("'Auto top-up enabled for all'");
    expect(source).toContain('Monitoring is already enabled for every observable target.');
    expect(source).toContain('Auto top-up is already enabled for every eligible target.');
    expect(source).toContain('Unobserved targets are skipped');
    expect(source).toContain('await refresh();');
    expect(source).toContain('const updates: TargetUpdate[] = candidates.map');
    expect(source).toContain('await sentinelManagement.proposeUpdateTargets(requestActor, updates)');
    expect(source).toContain('proposalId = createdProposalId.toString()');
    const bulkSource = source.slice(source.indexOf('async function proposeBulkTargetFlags('), source.indexOf('function targetPatchFor('));
    expect(bulkSource).toContain('requestActor !== actor');
    expect(bulkSource).toContain('Proposal #${proposalId} created for ${candidates.length} target');
    expect(bulkSource).not.toContain('loadAllOperatorRecords(');
    expect(bulkSource).not.toContain('refresh()');
    expect(source).toContain('The configured on-chain approval threshold and target-registry timelock apply.');
  });

  it('sends one batch call for a bulk target proposal and rejects duplicate principals', async () => {
    const actor = {
      propose_update_targets: vi.fn().mockResolvedValue({ Ok: 42n }),
    } as unknown as SentinelActor;
    const patch: TargetUpdate['patch'] = {
      display_name: [], project: [], tags: [], environment: [], criticality: [],
      observation_mode: [], funding_policy: [], enabled: [true], auto_topup: [true],
    };
    const update = (text: string): TargetUpdate => ({ principal: Principal.fromText(text), patch });
    const updates = [
      update('bfnu3-6aaaa-aaaab-qhanq-cai'),
      update('ucjxv-nqaaa-aaaaj-qrsaq-cai'),
    ];

    await expect(sentinelManagement.proposeUpdateTargets(actor, updates)).resolves.toBe(42n);
    expect(actor.propose_update_targets).toHaveBeenCalledTimes(1);
    expect(actor.propose_update_targets).toHaveBeenCalledWith(updates);
    expect(() => sentinelManagement.proposeUpdateTargets(actor, [updates[0], updates[0]])).toThrow('A bulk update cannot include a target more than once.');
  });

  it('clears alerts from the current browser without a wallet transaction', () => {
    expect(source).toContain('on:click={(event) => acknowledgeAlarmLocally(alarm, event.currentTarget)}');
    expect(source).not.toContain('sentinelManagement.acknowledgeAlarm(');
    expect(source).toContain("localStorage.setItem(`${acknowledgedAlarmsStoragePrefix(identityKey)}${id}`, '1')");
    expect(source).toContain('loadAcknowledgedAlarms(nextSession ? latestWalletConnection.principal?.toText() : undefined)');
    expect(source).toContain("window.addEventListener('storage', onAcknowledgedAlarmsStorageChange)");
    expect(source).toContain('!acknowledgedAlarmIds.has(alarm.id.toString())');
    expect(source).toContain('Alerts to review</span><strong>{openAlarmCount}');
    expect(source).toContain('aria-live="polite" aria-atomic="true">{openAlarmCount} remaining');
    expect(source).toContain('aria-label={`Acknowledge ${variant(alarm.kind)} alert for ${alarm.target[0]?.toText() ?? \'Sentinel\'}`}');
    expect(source).toContain('nextFocus?.isConnected) nextFocus.focus()');
  });

  it('requires an outcome-neutral explicit acknowledgement before clearing a durable top-up lock', () => {
    expect(source).toContain('requiresOperatorAcknowledgement: true');
    expect(source).toContain('This does not confirm whether funds moved');
    expect(source).toContain('the UI cannot independently confirm whether the prior request moved funds');
    expect(source).toContain('does not assert success or failure');
    expect(source).toContain('I reviewed the outcome and checked balances — clear retry lock');
    expect(source).toContain('function acknowledgeManualTopUpOutcomeReviewed()');
    expect(source).toContain('lockedTargetRow.recent_topups.slice(-3)');
    expect(source).toContain('Sentinel funding balances: cycles');
  });

  it('closes the manual top-up modal only after the backend reports completion', () => {
    const submitSource = source.slice(source.indexOf('async function confirmManualTopUp()'), source.indexOf('const opt ='));
    const completedBranch = submitSource.slice(
      submitSource.indexOf("if (result.disposition === 'completed')"),
      submitSource.indexOf("} else if (result.disposition === 'terminal')"),
    );
    expect(completedBranch).toContain('Top-up confirmed:');
    expect(completedBranch).toContain('topupTarget = null;');
    expect(submitSource).toContain("else if (result.disposition === 'terminal')");
    expect(submitSource).toContain("else if (result.disposition === 'uncertain')");
    expect(submitSource).toContain('is still pending');
  });

  it('restores unresolved manual top-up locks on reload and preserves active dispatches', () => {
    const dataLoadSource = source.slice(source.indexOf('async function refresh()'), source.indexOf('function reconcileManualTopUpLock('));
    expect(dataLoadSource).toContain('loadOperatorDashboard(authenticated)');
    expect(dataLoadSource).toContain('reconcileManualTopUpLock(next, session)');
    const lockSource = source.slice(source.indexOf('function reconcileManualTopUpLock('), source.indexOf('async function retryOperatorData()'));
    expect(lockSource).toContain("data.unresolved.find((item) => variant(item.trigger) === 'ManualTopup')");
    expect(lockSource).toContain('authorizedSession: session');
    expect(lockSource.indexOf('if (manualTopUpLock?.inFlight) return;')).toBeLessThan(lockSource.indexOf("data.unresolved.find((item) => variant(item.trigger) === 'ManualTopup')"));
    expect(lockSource).toContain('operationId: unresolvedManualTopUp.id');
    expect(lockSource).toContain('Do not submit another top-up until it is reconciled.');
    expect(source).toContain('sessionStorage.getItem(manualTopUpMarkerStorageKey(identityKey))');
    expect(source).toContain("return principal.toText();");
    const walletObserveSource = source.slice(source.indexOf('function observeWalletSession()'), source.indexOf('function fundingPolicy()'));
    expect(walletObserveSource).toContain('manualTopUpLock.identityKey !== currentWalletIdentityKey()');
    expect(walletObserveSource).toContain('restoreManualTopUpMarker();');
    const submitSource = source.slice(source.indexOf('async function confirmManualTopUp()'), source.indexOf('const opt ='));
    expect(submitSource.indexOf('storeManualTopUpMarker({ identityKey: requestIdentityKey')).toBeLessThan(submitSource.indexOf('await sentinelManagement.manualTopUpWithAmount'));
    expect(submitSource).toContain('clearManualTopUpMarker(requestIdentityKey)');
    expect(source).toContain('!clearManualTopUpMarker(manualTopUpLock.identityKey)');
  });

  it('only lets the current confirmed signer acknowledge an uncertain top-up lock', () => {
    const authoritySource = source.slice(source.indexOf('function hasCurrentManualTopUpAuthority()'), source.indexOf('async function confirmManualTopUp()'));
    expect(authoritySource).toContain('checkedSession === session');
    expect(authoritySource).toContain('manualTopUpLock.authorizedSession === session');
    expect(authoritySource).toContain('|| !hasCurrentManualTopUpAuthority()');
    expect(source).toContain('{#if manualTopUpLock.requiresOperatorAcknowledgement && hasCurrentManualTopUpAuthority()}');
  });

  it('explains an idle registry and shows cycle usage instead of a silent wall of Unavailable', () => {
    expect(source).toContain('snapshot.overview.unobserved_count === snapshot.overview.target_count');
    expect(source).toContain('Observation has not been switched on yet.');
    expect(source).toContain('{#if fundingUnavailable}');
    expect(source).toContain('<th scope="col">Burn / day</th>');
    expect(source).toContain('<th scope="col">Runway</th>');
    expect(source).toContain('<span class="metric-label">Burn / day</span>');
    expect(source).toContain('<span class="metric-label">Runway</span>');
    expect(source).toContain('optional(row.burn_cycles_per_day)');
    expect(source).toContain('optional(row.runway_secs)');
    expect(source).toContain('targetStateLabel(row)');
    expect(source).toContain('Awaiting first sample');
  });

  it('does not erase an unknown burn anomaly guard from inline rule proposals', () => {
    const ruleSource = source.slice(source.indexOf('async function saveRule('), source.indexOf('async function toggleTargetEnabled('));
    expect(ruleSource).toContain('if (current.burn_anomaly_limit_cycles_per_day === undefined)');
    expect(ruleSource).toContain('burn_anomaly_limit_cycles_per_day: current.burn_anomaly_limit_cycles_per_day');
    expect(ruleSource).not.toContain('burn_anomaly_limit_cycles_per_day ?? []');
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
