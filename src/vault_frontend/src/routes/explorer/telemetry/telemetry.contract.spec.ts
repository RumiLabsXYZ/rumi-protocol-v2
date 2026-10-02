import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it, vi } from 'vitest';
import { loadPublicTelemetry, sentinelManagement, type SentinelActor } from '$lib/services/cycleSentinelService';
import type { PublicOverview } from '$declarations/rumi_cycle_sentinel/rumi_cycle_sentinel.did';

const source = readFileSync(resolve(process.cwd(), 'src/routes/explorer/telemetry/+page.svelte'), 'utf8');
const serviceSource = readFileSync(resolve(process.cwd(), 'src/lib/services/cycleSentinelService.ts'), 'utf8');

describe('Cycle Sentinel telemetry route contract', () => {
  it('loads private telemetry only through an authenticated wallet and keeps signer checks separate', () => {
    expect(source).toContain('loadPublicTelemetry(authenticated)');
    expect(source).toContain('canViewSentinelTelemetry(latestWalletConnection.principal)');
    expect(source).toContain('Sentinel telemetry is private');
    expect(source).toContain('getPermissions(authenticated)');
    expect(source).toContain('async function checkOperatorAccess()');
    expect(source).toContain('Check operator access');
    expect(source).toContain('checkedSession !== session');
    expect(source).toContain('operatorCheckIsCurrent(session, epoch)');
    expect(source).toContain('currentWalletType.subscribe');
    expect(source).toContain('walletSessionGeneration.subscribe');
    expect(source).toContain('walletStore.subscribe');
    expect(source).toContain('OPERATOR_QUERY_TIMEOUT_MS = 25_000');
    expect(source).toContain("Checking signer permission");
    expect(source).toContain('Signer access is confirmed, but the proposal and unresolved-operation lists did not load');
    expect(source).toContain('Retry list loading');
    expect(source).toContain('operatorCheckIsCurrent(session, epoch)');
    expect(source).toContain('assertCurrentSigner();');
    expect(source).toContain('{#if signer && actor}');
    expect(serviceSource).toContain('auth.getActor<SentinelActor>');
    expect(serviceSource).not.toMatch(/\bany\b/);
    const refreshSource = source.slice(source.indexOf('async function refresh()'), source.indexOf('async function checkOperatorAccess()'));
    expect(refreshSource).toContain('loadPublicTelemetry(authenticated)');
    expect(refreshSource).not.toContain('getPermissions(');
  });

  it('makes an immediate check available only after signer confirmation and preserves read-only refresh', () => {
    const controls = source.slice(source.indexOf('<div class="check-buttons">'), source.indexOf('</header>'));
    expect(controls).toMatch(/\{#if signer && actor\}[\s\S]*on:click=\{runCheckNow\}/);
    expect(controls).toContain('disabled={checkingNow || loading}');
    expect(controls).toContain("checkingNow ? 'Checking now…' : 'Run check now'");
    expect(controls).toContain('may refuel Sentinel or top up registered canisters under the current reserves, thresholds and spending limits');
    expect(controls).toContain('Refresh telemetry reads saved results.');
    const refreshSource = source.slice(source.indexOf('async function refresh()'), source.indexOf('async function checkOperatorAccess()'));
    expect(refreshSource).not.toContain('runMaintenanceNow');
    expect(refreshSource).toContain('createAuthenticatedSentinelActor');
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

  it('bounds signer checks and keeps server-confirmed access during ancillary list failures', () => {
    expect(source).toContain('OPERATOR_QUERY_TIMEOUT_MS = 25_000');
    expect(source).toContain("withOperatorQueryTimeout(createAuthenticatedSentinelActor(), 'Connecting to the wallet')");
    expect(source).toContain("withOperatorQueryTimeout(getPermissions(authenticated), 'Checking signer permission')");
    const dataLoadSource = source.slice(source.indexOf('async function loadOperatorData('), source.indexOf('async function checkOperatorAccess()'));
    expect(dataLoadSource).toContain("'Loading signer data'");
    expect(dataLoadSource).toContain('operatorDataError = error instanceof Error');
    expect(dataLoadSource).not.toContain('invalidateOperatorAccess()');
    expect(source).toContain('assertCurrentSigner();');
    expect(source).toContain('Retry list loading');
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
    const dataLoadSource = source.slice(source.indexOf('async function loadOperatorData('), source.indexOf('async function checkOperatorAccess()'));
    expect(dataLoadSource).toContain("nextUnresolved.find((item) => variant(item.trigger) === 'ManualTopup')");
    expect(dataLoadSource).toContain('authorizedSession: session');
    expect(dataLoadSource.indexOf('if (manualTopUpLock?.inFlight)')).toBeLessThan(dataLoadSource.indexOf("nextUnresolved.find((item) => variant(item.trigger) === 'ManualTopup')"));
    expect(dataLoadSource).toContain('operationId: unresolvedManualTopUp.id');
    expect(dataLoadSource).toContain('Do not submit another top-up until it is reconciled.');
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
