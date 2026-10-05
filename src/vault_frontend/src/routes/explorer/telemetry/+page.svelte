<script lang="ts">
  import { goto } from '$app/navigation';
  import { onMount, tick } from 'svelte';
  import { Principal } from '@dfinity/principal';
  import { walletStore } from '$lib/stores/wallet';
  import { currentWalletType, walletSessionGeneration } from '$lib/services/auth';
  import { canViewSentinelTelemetry } from '$lib/services/cycleSentinelAccess';
  import { CANISTER_IDS } from '$lib/config';
  import { targetStateLabel } from '$lib/services/cycleSentinelTelemetry';
  import {
    ageLabel,
    checkIntervalLabel,
    cyclesBalanceMaxAgeSecs,
    CYCLES_LEDGER_PRINCIPAL,
    ICP_LEDGER_PRINCIPAL,
    formatIcp,
    formatTCycles,
    fundingOwner,
    fundingOverview,
    fundingTarget,
    isStale,
    legacyIcpAccountIdentifier,
    optionalBigInt as fundingOptionalBigInt,
    parseIcp,
    parseTCycles,
    targetFundingPolicyChanged,
    variantLabel,
  } from '$lib/services/cycleSentinelFunding';
  import {
    createAuthenticatedSentinelActor,
    loadOperatorDashboard,
    listProposals,
    listUnresolvedFundingOperations,
    operatorDashboardSnapshot,
    manualTopUpDisposition,
    parseNat,
    parseNat32,
    parsePrincipal,
    requireText,
    sentinelManagement,
    isCycleSentinelConfigured,
    type SentinelActor,
    type OperatorDashboardSnapshot,
    type TelemetrySnapshot,
  } from '$lib/services/cycleSentinelService';
  import type {
    Criticality,
    Environment,
    ObservationMode,
    ProposalRecord,
    PublicAlarm,
    PublicTopupSummary,
    TargetArgs,
    TargetFundingPolicy,
    TargetPatch,
    TargetUpdate,
    GlobalPolicyArgs,
    FundingOperation,
    FundingRail,
  } from '$declarations/rumi_cycle_sentinel/rumi_cycle_sentinel.did';

  let snapshot: OperatorDashboardSnapshot | null = null;
  let proposals: ProposalRecord[] = [];
  let unresolved: FundingOperation[] = [];
  let signer = false;
  let operatorChecked = false;
  let singleOperatorMode = false;
  let operatorDataLoading = false;
  let operatorDataLoaded = false;
  let operatorDataError = '';
  let checkingNow = false;
  let checkedSession: string | undefined;
  let operatorAccessEpoch = 0;
  let observedWalletSession: string | undefined | null = null;
  let telemetryLoadEpoch = 0;
  let latestWalletConnection: { isConnected: boolean; principal: Principal | null } = { isConnected: false, principal: null };
  let latestWalletType: string | null = null;
  let latestWalletSessionGeneration = 0;
  let loading = true;
  let publicError = '';
  let authError = '';
  let actionMessage = '';
  let proposingBulkTargets: 'enable' | 'auto-top-up' | null = null;
  let configuringSingleOperator = false;
  let loadingAllOperatorRecords = false;
  let copyMessage = '';
  let copyError = '';
  let selectedTargetPrincipal = '';
  let actor: SentinelActor | undefined;
  let alarmsOpen = false;
  let alarmStorageWarning = '';
  let acknowledgedAlarmIds = new Set<string>();
  let acknowledgedAlarmIdentity: string | undefined;
  let topupTarget: TelemetrySnapshot['targets'][number] | null = null;
  let topupRail: 'CyclesLedger' | 'IcpCmc' = 'CyclesLedger';
  let topupCyclesAmount = '';
  let topupIcpAmount = '';
  let submittingTopup = false;
  let topupError = '';
  let topupResult: { id: bigint; disposition: 'completed' | 'terminal' | 'pending' | 'uncertain'; state: string; amount: string; source: string } | null = null;
  let manualTopUpLock: { session: string; authorizedSession?: string; identityKey: string; target: Principal; rail?: 'CyclesLedger' | 'IcpCmc'; amount?: string; disposition: 'dispatching' | 'pending' | 'uncertain'; operationId?: bigint; inFlight: boolean; requiresOperatorAcknowledgement?: boolean } | null = null;
  const MANUAL_TOPUP_MARKER_KEY = 'rumi:cycle-sentinel:manual-topup';
  const ACKNOWLEDGED_ALARMS_KEY = 'rumi:cycle-sentinel:acknowledged-alarms';
  let ruleDrafts: Record<string, { lowThreshold: string; refill: string; dailyCap: string; cooldown: string }> = {};
  let ruleErrors: Record<string, string> = {};

  let targetPrincipal = '';
  let displayName = '';
  let project = '';
  let tags = '';
  let environment: keyof typeof EnvironmentVariant = 'Production';
  let criticality: keyof typeof CriticalityVariant = 'Standard';
  let observationMode: keyof typeof ObservationModeVariant = 'SelfReport';
  let lowThreshold = '1';
  let refill = '1';
  let dailyCap = '10';
  let cooldown = '3600';
  let burnAnomalyLimit = '';
  let enabled = false;
  let autoTopup = false;
  let loadedTarget: {
    principal: string;
    displayName: string;
    project: string;
    environment: keyof typeof EnvironmentVariant;
    criticality: keyof typeof CriticalityVariant;
    observationMode: keyof typeof ObservationModeVariant;
    lowThreshold: string;
    refill: string;
    dailyCap: string;
    cooldown: string;
    burnAnomalyLimit: string;
    tags: string;
    tagsKnown: boolean;
    burnAnomalyKnown: boolean;
    enabled: boolean;
    autoTopup: boolean;
  } | null = null;

  let signerPrincipal = '';
  let signerThreshold = '1';
  let proposalId = '';
  let operationId = '';
  let blockIndex = '';

  let globalDailyCap = '100000000000000';
  let sampleInterval = '300';
  let staleAfter = '900';
  let minIcpReserve = '0';
  let selfRefill = '1000000000000';
  let selfLowThreshold = '1000000000000';
  let selfDailyCap = '10000000000000';
  let protectedReserve = '1000000000000';
  let unpauseTimelock = '3600';
  let spendTimelock = '3600';
  let targetTimelock = '3600';
  let signerTimelock = '86400';
  let observableTargets: TelemetrySnapshot['targets'] = [];
  let autoTopupEligibleTargets: TelemetrySnapshot['targets'] = [];
  let monitoringEnabledCount = 0;
  let monitoringPendingCount = 0;
  let autoTopupEnabledCount = 0;
  let autoTopupPendingCount = 0;
  let unresolvedRecordsComplete = false;
  let unobservedTargetCount = 0;
  let pausedTargetCount = 0;

  const variant = (value: Record<string, unknown>): string => Object.keys(value)[0] ?? 'Unknown';
  const OPERATOR_QUERY_TIMEOUT_MS = 25_000;
  function withOperatorQueryTimeout<T>(promise: Promise<T>, stage: string): Promise<T> {
    let timer: ReturnType<typeof setTimeout>;
    const timeout = new Promise<never>((_, reject) => {
      timer = setTimeout(() => reject(new Error(`${stage} timed out. Check that your wallet request completed, then retry.`)), OPERATOR_QUERY_TIMEOUT_MS);
    });
    return Promise.race([promise, timeout]).finally(() => clearTimeout(timer));
  }
  const format = (value: bigint | undefined): string => value === undefined ? 'Unavailable' : value.toLocaleString();
  const optional = <T,>(value: [] | [T]): T | undefined => value.length ? value[0] : undefined;

  // Cycle counts are 12-to-15 digit integers. Render them in T/B units so a
  // balance, a burn rate, and a threshold can be compared at a glance; the
  // exact integer stays available in the cell's title attribute.
  function formatCycles(value: bigint | undefined): string {
    if (value === undefined) return 'Unavailable';
    if (value >= 1_000_000_000_000n) return `${(Number(value) / 1e12).toFixed(2)}T`;
    if (value >= 1_000_000_000n) return `${(Number(value) / 1e9).toFixed(2)}B`;
    return value.toLocaleString();
  }

  function formatRunway(secs: bigint | undefined): string {
    if (secs === undefined) return 'Unavailable';
    const days = Number(secs) / 86_400;
    if (days >= 1) return `${days.toFixed(1)}d`;
    return `${(Number(secs) / 3_600).toFixed(1)}h`;
  }

  function nextCheckLabel(value: bigint | undefined): string {
    if (value === undefined) return 'Unavailable';
    return new Date(Number(value) * 1000).toLocaleString();
  }

  function topupOrigin(item: PublicTopupSummary): string {
    const trigger = optional(item.trigger);
    if (!trigger) return 'Origin unavailable';
    switch (variant(trigger)) {
      case 'ManualTopup': return 'Manual';
      case 'LowBalanceAutoTopup': return 'Automatic';
      case 'SelfRecovery': return 'Sentinel self-recovery';
      default: return 'Unknown';
    }
  }

  function topupTimeLabel(seconds: bigint): string {
    const value = Number(seconds);
    if (!Number.isSafeInteger(value)) return 'Unavailable';
    return new Date(value * 1000).toLocaleString();
  }

  function topupTargetLabel(item: PublicTopupSummary): string {
    return snapshot?.targets.find((row) => row.principal.toText() === item.target.toText())?.display_name
      ?? item.target.toText();
  }

  function copyLabel(label: string, value: string): void {
    copyError = '';
    copyMessage = '';
    if (!value || value === 'Unavailable') {
      copyError = `${label} is unavailable until the Sentinel publishes its funding account.`;
      return;
    }
    if (!navigator.clipboard) {
      copyError = 'Clipboard access is unavailable. Select and copy the address manually.';
      return;
    }
    void navigator.clipboard.writeText(value).then(() => {
      copyMessage = `${label} copied.`;
    }).catch(() => {
      copyError = `Could not copy ${label.toLowerCase()}. Select the visible address and copy it manually.`;
    });
  }

  function selectTarget(row: TelemetrySnapshot['targets'][number]): void {
    const current = fundingTarget(row);
    selectedTargetPrincipal = current.principal.toText();
    targetPrincipal = current.principal.toText();
    displayName = current.display_name;
    project = current.project;
    tags = current.tags?.join(', ') ?? '';
    lowThreshold = formatTCycles(current.low_balance_threshold_cycles);
    refill = formatTCycles(current.refill_cycles);
    dailyCap = formatTCycles(current.daily_cap_cycles ?? current.refill_cycles);
    cooldown = current.cooldown_secs?.toString() ?? cooldown;
    const anomalyLimit = fundingOptionalBigInt(current.burn_anomaly_limit_cycles_per_day);
    burnAnomalyLimit = anomalyLimit === undefined
      ? ''
      : formatTCycles(anomalyLimit);
    enabled = current.enabled ?? false;
    autoTopup = current.auto_topup ?? false;
    environment = variant(current.environment) as keyof typeof EnvironmentVariant;
    criticality = variant(current.criticality) as keyof typeof CriticalityVariant;
    observationMode = variant(current.observation_mode) as keyof typeof ObservationModeVariant;
    loadedTarget = { principal: current.principal.toText(), displayName, project, environment, criticality, observationMode, lowThreshold, refill, dailyCap, cooldown, burnAnomalyLimit, tags, tagsKnown: current.tags !== undefined, burnAnomalyKnown: current.burn_anomaly_limit_cycles_per_day !== undefined, enabled, autoTopup };
    actionMessage = `${current.display_name} settings loaded into the operator form.`;
    const scrollBehavior = window.matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth';
    const settingsField = document.querySelector<HTMLInputElement>('.operator article:nth-of-type(2) input');
    settingsField?.focus({ preventScroll: true });
    settingsField?.scrollIntoView({ behavior: scrollBehavior, block: 'start' });
  }

  function ruleDraft(row: TelemetrySnapshot['targets'][number]) {
    const principal = row.principal.toText();
    ruleDrafts[principal] ??= {
      lowThreshold: formatTCycles(row.low_balance_threshold_cycles),
      refill: formatTCycles(row.refill_cycles),
      dailyCap: fundingTarget(row).daily_cap_cycles === undefined ? '' : formatTCycles(fundingTarget(row).daily_cap_cycles),
      cooldown: fundingTarget(row).cooldown_secs?.toString() ?? '',
    };
    return ruleDrafts[principal];
  }

  async function updateTarget(row: TelemetrySnapshot['targets'][number], patch: TargetPatch, success: string): Promise<void> {
    actionMessage = '';
    authError = '';
    try {
      assertCurrentSigner();
      await sentinelManagement.proposeUpdateTarget(actor!, row.principal, patch);
      actionMessage = success;
    } catch (error) {
      authError = error instanceof Error ? error.message : String(error);
    }
  }

  async function proposeBulkTargetFlags(mode: 'enable' | 'auto-top-up'): Promise<void> {
    if (proposingBulkTargets !== null) return;
    proposingBulkTargets = mode;
    authError = '';
    actionMessage = '';
    try {
      assertCurrentSigner();
      const requestSession = checkedSession;
      const requestEpoch = operatorAccessEpoch;
      const requestActor = actor;
      if (!requestSession || !requestActor) throw new Error('Refresh telemetry to confirm operator access before proposing target changes.');
      const current = snapshot;
      if (!current) throw new Error(publicError || 'Refresh telemetry before proposing target changes.');
      if (current.targets.length === 0) {
        actionMessage = 'There are no registered targets to update.';
        return;
      }

      const unobserved = current.targets.filter((row) => variant(row.observation_mode) === 'Unobserved').length;
      const paused = mode === 'auto-top-up'
        ? current.targets.filter((row) => variant(row.observation_mode) !== 'Unobserved' && fundingTarget(row).paused).length
        : 0;
      const candidates = current.targets.filter((row) => {
        const target = fundingTarget(row);
        if (variant(row.observation_mode) === 'Unobserved') return false;
        if (mode === 'auto-top-up' && target.paused) return false;
        return mode === 'enable' ? !target.enabled : !target.enabled || !target.auto_topup;
      });
      if (candidates.length === 0) {
        const skipped = [
          unobserved ? `${unobserved} unobserved skipped` : '',
          paused ? `${paused} paused skipped` : '',
        ].filter(Boolean).join('; ');
        actionMessage = `${mode === 'enable' ? 'All observable targets are already enabled.' : 'All eligible targets already have monitoring and auto top-up enabled.'}${skipped ? ` ${skipped}.` : ''}`;
        return;
      }

      const updates: TargetUpdate[] = candidates.map((row) => ({
        principal: row.principal,
        patch: targetPatchFor(mode === 'enable' ? { enabled: [true] } : { enabled: [true], auto_topup: [true] }),
      }));
      const createdProposalId = await sentinelManagement.proposeUpdateTargets(requestActor, updates);
      if (requestSession !== currentWalletSession()
        || requestEpoch !== operatorAccessEpoch
        || requestActor !== actor
        || checkedSession !== requestSession
        || !signer) return;

      proposalId = createdProposalId.toString();
      const skipped = [
        unobserved ? `${unobserved} unobserved skipped` : '',
        paused ? `${paused} paused skipped` : '',
      ].filter(Boolean).join('; ');
      actionMessage = `Proposal #${proposalId} created for ${candidates.length} target${candidates.length === 1 ? '' : 's'}${skipped ? `; ${skipped}` : ''}. The configured on-chain approval threshold and target-registry timelock apply.`;
    } catch (error) {
      authError = error instanceof Error ? error.message : String(error);
    } finally {
      proposingBulkTargets = null;
    }
  }

  function targetPatchFor(changes: Partial<TargetPatch>): TargetPatch {
    return {
      display_name: changes.display_name ?? [],
      project: changes.project ?? [],
      tags: changes.tags ?? [],
      environment: changes.environment ?? [],
      criticality: changes.criticality ?? [],
      observation_mode: changes.observation_mode ?? [],
      funding_policy: changes.funding_policy ?? [],
      enabled: changes.enabled ?? [],
      auto_topup: changes.auto_topup ?? [],
    };
  }

  async function saveRule(row: TelemetrySnapshot['targets'][number]): Promise<void> {
    const principal = row.principal.toText();
    ruleErrors = { ...ruleErrors, [principal]: '' };
    authError = '';
    try {
      const draft = ruleDraft(row);
      const current = fundingTarget(row);
      if (current.burn_anomaly_limit_cycles_per_day === undefined) {
        throw new Error('This target does not expose its burn anomaly limit yet. Refresh telemetry after the backend update before changing its funding policy.');
      }
      const policy: TargetFundingPolicy = {
        low_balance_threshold_cycles: parseTCycles(draft.lowThreshold, 'Low balance threshold'),
        refill_cycles: parseTCycles(draft.refill, 'Refill amount'),
        daily_cap_cycles: parseTCycles(draft.dailyCap, 'Daily cap'),
        cooldown_secs: parseNat(draft.cooldown, 'Cooldown'),
        burn_anomaly_limit_cycles_per_day: current.burn_anomaly_limit_cycles_per_day,
      };
      await updateTarget(row, targetPatchFor({ funding_policy: [policy] }), `${row.display_name}: rule change proposed. The configured approval threshold and target-registry timelock apply.`);
      if (authError) ruleErrors = { ...ruleErrors, [principal]: authError };
    } catch (error) {
      ruleErrors = { ...ruleErrors, [principal]: error instanceof Error ? error.message : String(error) };
    }
  }

  async function toggleTargetEnabled(row: TelemetrySnapshot['targets'][number], value: boolean): Promise<void> {
    await updateTarget(row, targetPatchFor({ enabled: [value] }), `${row.display_name}: ${value ? 'enable' : 'disable'} change proposed. The configured approval threshold and target-registry timelock apply.`);
  }

  async function toggleAutoTopup(row: TelemetrySnapshot['targets'][number], value: boolean): Promise<void> {
    await updateTarget(row, targetPatchFor({ auto_topup: [value] }), `${row.display_name}: auto-top-up change proposed. The configured approval threshold and target-registry timelock apply.`);
  }

  function openManualTopUp(row: TelemetrySnapshot['targets'][number]): void {
    if (manualTopUpLock || !unresolvedRecordsComplete) return;
    topupTarget = row;
    topupRail = 'CyclesLedger';
    topupCyclesAmount = formatTCycles(row.refill_cycles);
    topupIcpAmount = '';
    topupError = '';
    topupResult = null;
  }

  function selectedTopUpAmount(): bigint {
    return topupRail === 'CyclesLedger'
      ? parseTCycles(topupCyclesAmount, 'Top-up amount')
      : parseIcp(topupIcpAmount, 'Top-up amount');
  }

  function selectedTopUpAmountLabel(): string {
    try {
      const amount = selectedTopUpAmount();
      return topupRail === 'CyclesLedger' ? `${formatTCycles(amount)} T-cycles` : `${formatIcp(amount)} ICP`;
    } catch {
      return 'Enter a valid amount';
    }
  }

  function manualTopUpRail(): FundingRail {
    return topupRail === 'CyclesLedger' ? { CyclesLedger: null } : { IcpCmc: null };
  }

  function validTopUpAmount(): boolean {
    try { return selectedTopUpAmount() > 0n; } catch { return false; }
  }

  function hasCurrentManualTopUpAuthority(): boolean {
    const session = currentWalletSession();
    return !!session && signer && !!actor && checkedSession === session
      && !!manualTopUpLock?.authorizedSession
      && manualTopUpLock.authorizedSession === session;
  }

  function acknowledgeManualTopUpOutcomeReviewed(): void {
    if (!manualTopUpLock || manualTopUpLock.inFlight || !manualTopUpLock.requiresOperatorAcknowledgement || !hasCurrentManualTopUpAuthority()) return;
    if (manualTopUpLock.identityKey !== currentWalletIdentityKey() || !clearManualTopUpMarker(manualTopUpLock.identityKey)) return;
    manualTopUpLock = null;
    topupTarget = null;
    topupResult = null;
    submittingTopup = false;
    topupError = 'Retry lock cleared after your review. The UI could not independently confirm whether funds moved; keep your ledger and balance checks as the source of truth.';
  }

  async function confirmManualTopUp(): Promise<void> {
    if (!topupTarget || submittingTopup || manualTopUpLock || topupResult) return;
    let requestSession: string;
    let requestEpoch: number;
    let requestActor: SentinelActor;
    let requestTarget: Principal;
    let requestTargetName: string;
    let requestRail: FundingRail;
    let requestAmount: bigint;
    let requestAmountLabel: string;
    let requestSource: string;
    try {
      assertCurrentSigner();
      const session = currentWalletSession();
      const requestIdentityKey = currentWalletIdentityKey();
      const capturedActor = actor;
      const capturedTarget = topupTarget;
      if (!session || !requestIdentityKey || !capturedActor || !checkedSession || checkedSession !== session) {
        throw new Error('Operator access changed. Refresh telemetry to confirm signer access before submitting an action.');
      }
      requestSession = session;
      requestEpoch = operatorAccessEpoch;
      requestActor = capturedActor;
      requestTarget = capturedTarget.principal;
      requestTargetName = capturedTarget.display_name;
      requestRail = manualTopUpRail();
      requestAmount = selectedTopUpAmount();
      if (requestAmount <= 0n) throw new Error('Top-up amount must be greater than zero.');
      requestAmountLabel = topupRail === 'CyclesLedger' ? `${formatTCycles(requestAmount)} T-cycles` : `${formatIcp(requestAmount)} ICP`;
      requestSource = topupRail === 'CyclesLedger'
        ? 'Sentinel funding account · Cycles Ledger'
        : 'Sentinel funding account · ICP via NNS Cycles Minting Canister';
      const isCurrentRequest = (): boolean => currentWalletSession() === requestSession
        && operatorAccessEpoch === requestEpoch
        && actor === requestActor
        && signer
        && checkedSession === requestSession;

      storeManualTopUpMarker({ identityKey: requestIdentityKey, target: requestTarget.toText(), rail: topupRail, amount: requestAmountLabel });
      submittingTopup = true;
      topupError = '';
      actionMessage = '';
      manualTopUpLock = { session: requestSession, authorizedSession: requestSession, identityKey: requestIdentityKey, target: requestTarget, rail: topupRail, amount: requestAmountLabel, disposition: 'dispatching', inFlight: true };
      try {
        const operation = await sentinelManagement.manualTopUpWithAmount(requestActor, requestTarget, requestRail, requestAmount);
        const result = manualTopUpDisposition(operation);
        if (!isCurrentRequest()) {
          if (manualTopUpLock?.session === requestSession && manualTopUpLock.target.toText() === requestTarget.toText()) manualTopUpLock = { ...manualTopUpLock, inFlight: false };
          return;
        }
        if (result.disposition === 'pending' || result.disposition === 'uncertain') {
          manualTopUpLock = { session: requestSession, authorizedSession: requestSession, identityKey: requestIdentityKey, target: requestTarget, rail: topupRail, amount: requestAmountLabel, disposition: result.disposition, operationId: operation.id, inFlight: false };
          storeManualTopUpMarker({ identityKey: requestIdentityKey, target: requestTarget.toText(), rail: topupRail, amount: requestAmountLabel, operationId: operation.id, state: result.state });
        } else {
          clearManualTopUpMarker(requestIdentityKey);
          manualTopUpLock = null;
        }
        topupResult = { id: operation.id, disposition: result.disposition, state: result.state, amount: requestAmountLabel, source: requestSource };
        if (result.disposition === 'completed') {
          actionMessage = `Top-up confirmed: ${requestTargetName}, operation #${operation.id.toString()} completed (${requestAmountLabel}, ${requestSource}). Refresh telemetry to reload balances and history.`;
          topupTarget = null;
        } else if (result.disposition === 'terminal') {
          topupError = `Operation #${operation.id.toString()} ended in ${result.state}; the backend reports no completed top-up. Review the operation before trying again.`;
        } else if (result.disposition === 'uncertain') {
          topupError = `Operation #${operation.id.toString()} is uncertain (${result.state}). Do not submit again. Reconcile this operation in the operator console before retrying.`;
        } else {
          topupError = `Operation #${operation.id.toString()} is still pending (${result.state}). Do not submit again until it settles; review it in the operator console.`;
        }
      } catch (error) {
        // A rejected wallet/network promise does not prove the canister rejected
        // the call: the update may have reached execution already.
        if (!isCurrentRequest()) {
          if (manualTopUpLock?.session === requestSession && manualTopUpLock.target.toText() === requestTarget.toText()) manualTopUpLock = { ...manualTopUpLock, inFlight: false };
          return;
        }
        manualTopUpLock = { session: requestSession, authorizedSession: requestSession, identityKey: requestIdentityKey, target: requestTarget, rail: topupRail, amount: requestAmountLabel, disposition: 'uncertain', inFlight: false };
        topupError = `The request outcome could not be confirmed${error instanceof Error ? ` (${error.message})` : ''}. Do not submit it again. Check unresolved funding operations in the operator console before retrying.`;
      } finally {
        if (isCurrentRequest()) submittingTopup = false;
      }
    } catch (error) {
      topupError = error instanceof Error ? error.message : String(error);
    }
  }
  const opt = <T,>(value: T | undefined): [] | [T] => value === undefined ? [] : [value];
  const principalVariant = (value: keyof typeof EnvironmentVariant): Record<string, null> => ({ [value]: null });
  const EnvironmentVariant = { Local: null, Production: null, Test: null, Archived: null, Staging: null } as const;
  const criticalityVariant = (value: keyof typeof CriticalityVariant): Criticality => ({ [value]: null } as Criticality);
  const CriticalityVariant = { Important: null, Experimental: null, Critical: null, Standard: null } as const;
  const observationVariant = (value: keyof typeof ObservationModeVariant): ObservationMode => ({ [value]: null } as ObservationMode);
  const ObservationModeVariant = { SelfReport: null, BlackholeRelay: null, Unobserved: null } as const;

  function walletSession(
    connection = latestWalletConnection,
    walletType = latestWalletType,
    generation = latestWalletSessionGeneration,
  ): string | undefined {
    if (!connection.isConnected || !connection.principal) return undefined;
    // Wallet type is part of the binding: a Plug/Oisy/II switch must never
    // retain an actor or signer result, even where the principal is the same.
    return `${generation}:${walletType ?? 'unclassified'}:${connection.principal.toText()}`;
  }

  function currentWalletSession(): string | undefined {
    return walletSession(
      { isConnected: $walletStore.isConnected, principal: $walletStore.principal },
      $currentWalletType,
      $walletSessionGeneration,
    );
  }

  function currentWalletIdentityKey(): string | undefined {
    const principal = $walletStore.principal;
    if (!$walletStore.isConnected || !principal) return undefined;
    return principal.toText();
  }

  function acknowledgedAlarmsStoragePrefix(identityKey: string): string {
    return `${ACKNOWLEDGED_ALARMS_KEY}:${CANISTER_IDS.CYCLE_SENTINEL}:${encodeURIComponent(identityKey)}:`;
  }

  function loadAcknowledgedAlarms(identityKey: string | undefined): void {
    acknowledgedAlarmIdentity = identityKey;
    acknowledgedAlarmIds = new Set();
    alarmStorageWarning = '';
    if (!identityKey) return;
    try {
      const prefix = acknowledgedAlarmsStoragePrefix(identityKey);
      const ids = new Set<string>();
      for (let index = 0; index < localStorage.length; index += 1) {
        const key = localStorage.key(index);
        if (key?.startsWith(prefix) && localStorage.getItem(key) === '1') {
          const id = key.slice(prefix.length);
          if (/^\d+$/.test(id)) ids.add(id);
        }
      }
      acknowledgedAlarmIds = ids;
    } catch {
      // Browser storage may be disabled. Acknowledgement still works until reload.
    }
  }

  function onAcknowledgedAlarmsStorageChange(event: StorageEvent): void {
    const identityKey = latestWalletConnection.isConnected ? latestWalletConnection.principal?.toText() : undefined;
    if (!identityKey) return;
    if (event.key === null) {
      loadAcknowledgedAlarms(identityKey);
      return;
    }
    const prefix = acknowledgedAlarmsStoragePrefix(identityKey);
    if (!event.key.startsWith(prefix)) return;
    const id = event.key.slice(prefix.length);
    if (!/^\d+$/.test(id)) return;
    const next = new Set(acknowledgedAlarmIds);
    if (event.newValue === '1') next.add(id);
    else next.delete(id);
    acknowledgedAlarmIds = next;
  }

  async function acknowledgeAlarmLocally(alarm: PublicAlarm, button: HTMLButtonElement): Promise<void> {
    const identityKey = currentWalletIdentityKey();
    if (!identityKey) return;
    if (acknowledgedAlarmIdentity !== identityKey) loadAcknowledgedAlarms(identityKey);
    const popover = button.closest('.alarm-popover');
    const buttons = [...(popover?.querySelectorAll<HTMLButtonElement>('[data-alert-ack]') ?? [])];
    const index = buttons.indexOf(button);
    const nextFocus = buttons[index + 1] ?? buttons[index - 1] ?? popover?.querySelector<HTMLButtonElement>('[aria-label="Close alerts"]');
    const next = new Set(acknowledgedAlarmIds);
    const id = alarm.id.toString();
    next.add(id);
    alarmStorageWarning = '';
    try {
      localStorage.setItem(`${acknowledgedAlarmsStoragePrefix(identityKey)}${id}`, '1');
    } catch {
      alarmStorageWarning = 'Browser storage is unavailable; this alert may reappear after refresh or reconnect.';
    }
    acknowledgedAlarmIds = next;
    await tick();
    if (currentWalletIdentityKey() === identityKey && nextFocus?.isConnected) nextFocus.focus();
  }

  function manualTopUpMarkerStorageKey(identityKey: string): string {
    return `${MANUAL_TOPUP_MARKER_KEY}:${encodeURIComponent(identityKey)}`;
  }

  function storeManualTopUpMarker(marker: { identityKey: string; target: string; rail: 'CyclesLedger' | 'IcpCmc'; amount: string; operationId?: bigint; state?: string }): void {
    sessionStorage.setItem(manualTopUpMarkerStorageKey(marker.identityKey), JSON.stringify({
      principal: marker.identityKey,
      target: marker.target,
      rail: marker.rail,
      amount: marker.amount,
      operationId: marker.operationId?.toString(),
      state: marker.state,
    }));
  }

  function clearManualTopUpMarker(identityKey: string): boolean {
    try {
      sessionStorage.removeItem(manualTopUpMarkerStorageKey(identityKey));
      return true;
    } catch {
      return false;
    }
  }

  function restoreManualTopUpMarker(): void {
    const session = currentWalletSession();
    const identityKey = currentWalletIdentityKey();
    if (!session || !identityKey || manualTopUpLock) return;
    try {
      const raw = sessionStorage.getItem(manualTopUpMarkerStorageKey(identityKey));
      if (!raw) return;
      const marker = JSON.parse(raw) as { principal?: string; target?: string; rail?: string; amount?: string; operationId?: string };
      if (marker.principal !== identityKey || !marker.target || !marker.amount || (marker.rail !== 'CyclesLedger' && marker.rail !== 'IcpCmc')) return;
      manualTopUpLock = {
        session,
        identityKey,
        target: Principal.fromText(marker.target),
        rail: marker.rail,
        amount: marker.amount,
        disposition: 'uncertain',
        operationId: marker.operationId ? BigInt(marker.operationId) : undefined,
        inFlight: false,
        requiresOperatorAcknowledgement: true,
      };
      topupError = 'A prior manual top-up request was restored from this wallet session. Its outcome is unconfirmed; reconcile it and verify balances before retrying.';
    } catch {
      // If browser storage is unavailable or malformed, do not infer an operation.
    }
  }

  function invalidateOperatorAccess(): void {
    operatorAccessEpoch += 1;
    actionMessage = '';
    topupTarget = null;
    topupError = '';
    topupResult = null;
    submittingTopup = false;
    signer = false;
    operatorChecked = false;
    operatorDataLoading = false;
    operatorDataLoaded = false;
    operatorDataError = '';
    checkedSession = undefined;
    actor = undefined;
    proposals = [];
    unresolved = [];
  }

  function assertCurrentSigner(): void {
    const session = currentWalletSession();
    if (!signer || !actor || !checkedSession || checkedSession !== session) {
      invalidateOperatorAccess();
      throw new Error('Operator access changed. Refresh telemetry to confirm signer access before submitting an action.');
    }
  }

  function observeWalletSession(): void {
    const nextSession = walletSession();
    if (nextSession === observedWalletSession) return;
    observedWalletSession = nextSession;
    loadAcknowledgedAlarms(nextSession ? latestWalletConnection.principal?.toText() : undefined);
    if (manualTopUpLock && manualTopUpLock.identityKey !== currentWalletIdentityKey()) manualTopUpLock = null;
    // Store subscriptions observe every connect, disconnect, and wallet-type
    // transition. This prevents a same-principal reconnection from retaining
    // a previously-created authenticated actor.
    invalidateOperatorAccess();
    restoreManualTopUpMarker();
    snapshot = null;
    if (!canViewSentinelTelemetry(latestWalletConnection.principal)) {
      void goto('/explorer', { replaceState: true });
      return;
    }
    void refresh();
  }

  function fundingPolicy(): TargetFundingPolicy {
    return {
      low_balance_threshold_cycles: parseTCycles(lowThreshold, 'Low balance threshold'),
      refill_cycles: parseTCycles(refill, 'Refill cycles'),
      daily_cap_cycles: parseTCycles(dailyCap, 'Daily cap'),
      cooldown_secs: parseNat(cooldown, 'Cooldown'),
      burn_anomaly_limit_cycles_per_day: opt(burnAnomalyLimit.trim() ? parseTCycles(burnAnomalyLimit, 'Burn anomaly limit') : undefined),
    };
  }

  function targetArgs(): TargetArgs {
    const principal = parsePrincipal(targetPrincipal, 'Target principal');
    return {
      principal,
      display_name: requireText(displayName, 'Display name'),
      project: requireText(project, 'Project'),
      tags: tags.split(',').map((tag) => tag.trim()).filter(Boolean),
      environment: principalVariant(environment) as Environment,
      criticality: criticalityVariant(criticality),
      observation_mode: observationVariant(observationMode),
      funding_policy: fundingPolicy(),
    };
  }

  function targetPatch(): TargetPatch {
    const currentFunding = { lowThreshold, refill, dailyCap, cooldown, burnAnomalyLimit };
    const fundingChanged = loadedTarget
      ? targetFundingPolicyChanged(loadedTarget.burnAnomalyKnown ? loadedTarget : { ...loadedTarget, burnAnomalyLimit: '' }, currentFunding)
      : true;
    if (loadedTarget && fundingChanged && !loadedTarget.burnAnomalyKnown) {
      throw new Error('This target does not expose its burn anomaly limit yet. Refresh telemetry after the backend update before changing its funding policy.');
    }
    return {
      display_name: loadedTarget && loadedTarget.displayName === displayName ? [] : [requireText(displayName, 'Display name')],
      project: loadedTarget && loadedTarget.project === project ? [] : [requireText(project, 'Project')],
      tags: loadedTarget && !tags.trim() && !loadedTarget.tagsKnown ? [] : [tags.split(',').map((tag) => tag.trim()).filter(Boolean)],
      environment: loadedTarget && loadedTarget.environment === environment ? [] : [principalVariant(environment) as Environment],
      criticality: loadedTarget && loadedTarget.criticality === criticality ? [] : [criticalityVariant(criticality)],
      observation_mode: loadedTarget && loadedTarget.observationMode === observationMode ? [] : [observationVariant(observationMode)],
      funding_policy: fundingChanged ? [fundingPolicy()] : [],
      enabled: loadedTarget && loadedTarget.enabled === enabled ? [] : [enabled],
      auto_topup: loadedTarget && loadedTarget.autoTopup === autoTopup ? [] : [autoTopup],
    };
  }

  function globalPolicy(): GlobalPolicyArgs {
    return {
      global_daily_cap_cycles: parseNat(globalDailyCap, 'Global daily cap'),
      sample_interval_secs: parseNat(sampleInterval, 'Sample interval'),
      stale_after_secs: parseNat(staleAfter, 'Stale-after'),
      min_icp_reserve_e8s: parseNat(minIcpReserve, 'Minimum ICP reserve'),
      self_recovery_policy: {
        refill_cycles: parseNat(selfRefill, 'Self-recovery refill'),
        low_balance_threshold_cycles: parseNat(selfLowThreshold, 'Self-recovery low threshold'),
        daily_cap_cycles: parseNat(selfDailyCap, 'Self-recovery daily cap'),
        protected_reserve_cycles: parseNat(protectedReserve, 'Protected reserve'),
      },
      timelocks: {
        unpause_secs: parseNat(unpauseTimelock, 'Unpause timelock'),
        spend_policy_secs: parseNat(spendTimelock, 'Spend-policy timelock'),
        target_registry_secs: parseNat(targetTimelock, 'Target-registry timelock'),
        signer_change_secs: parseNat(signerTimelock, 'Signer-change timelock'),
      },
    };
  }

  // Every target reads Unobserved when it is disabled OR its observation mode
  // is Unobserved (see rumi_cycle_sentinel public_api.rs `target_row`). In that
  // state the sampler never runs, so balance, burn, and runway are genuinely
  // absent rather than zero. Say so explicitly: a table of "Unavailable" with
  // no explanation reads like a broken page.
  $: observableTargets = snapshot?.targets.filter((row) => variant(row.observation_mode) !== 'Unobserved') ?? [];
  $: autoTopupEligibleTargets = observableTargets.filter((row) => !fundingTarget(row).paused);
  $: monitoringEnabledCount = observableTargets.filter((row) => fundingTarget(row).enabled).length;
  $: monitoringPendingCount = observableTargets.length - monitoringEnabledCount;
  $: autoTopupEnabledCount = autoTopupEligibleTargets.filter((row) => fundingTarget(row).enabled && fundingTarget(row).auto_topup).length;
  $: autoTopupPendingCount = autoTopupEligibleTargets.length - autoTopupEnabledCount;
  $: unobservedTargetCount = snapshot?.targets.filter((row) => variant(row.observation_mode) === 'Unobserved').length ?? 0;
  $: pausedTargetCount = observableTargets.filter((row) => fundingTarget(row).paused).length;
  $: registryIdle = !!snapshot
    && snapshot.overview.target_count > 0n
    && snapshot.overview.unobserved_count === snapshot.overview.target_count;
  $: neverSampled = !!snapshot && optional(snapshot.overview.last_sample_at_secs) === undefined;
  $: funding = snapshot ? fundingOverview(snapshot.overview) : undefined;
  $: if (loadedTarget && targetPrincipal.trim() !== loadedTarget.principal && selectedTargetPrincipal === loadedTarget.principal) {
    loadedTarget = null;
    selectedTargetPrincipal = '';
  }
  $: fundingPrincipal = snapshot
    ? fundingOwner(snapshot.overview, CANISTER_IDS.CYCLE_SENTINEL)
    : isCycleSentinelConfigured
      ? Principal.fromText(CANISTER_IDS.CYCLE_SENTINEL)
      : undefined;
  $: fundingOwnerText = fundingPrincipal?.toText() ?? 'Unavailable';
  $: icpAccountText = fundingPrincipal ? legacyIcpAccountIdentifier(fundingPrincipal) : 'Unavailable';
  $: fundingCyclesBalance = funding ? fundingOptionalBigInt(funding.cycles_ledger_balance_cycles) : undefined;
  $: fundingCyclesBalanceAsOf = funding ? fundingOptionalBigInt(funding.cycles_ledger_balance_as_of_secs) : undefined;
  $: fundingCyclesBalanceStale = fundingCyclesBalanceAsOf !== undefined && !!snapshot
    && isStale(fundingCyclesBalanceAsOf, undefined, cyclesBalanceMaxAgeSecs(snapshot.overview));
  $: fundingCyclesAvailable = funding ? fundingOptionalBigInt(funding.cycles_ledger_available_cycles) : undefined;
  $: fundingProtectedCycles = funding ? fundingOptionalBigInt(funding.protected_self_reserve_cycles) : undefined;
  $: fundingIcpBalance = funding ? fundingOptionalBigInt(funding.icp_ledger_balance_e8s) : undefined;
  $: fundingIcpBalanceAsOf = funding ? fundingOptionalBigInt(funding.icp_ledger_balance_as_of_secs) : undefined;
  $: fundingIcpBalanceStale = fundingIcpBalanceAsOf !== undefined && isStale(fundingIcpBalanceAsOf, undefined, 600n);
  $: fundingIcpAvailable = funding ? fundingOptionalBigInt(funding.icp_available_e8s) : undefined;
  $: fundingMinIcpReserve = funding ? fundingOptionalBigInt(funding.min_icp_reserve_e8s) : undefined;
  $: fundingConversionStatus = funding ? variantLabel(funding.shared_reserve_conversion_status) : 'Unavailable';
  $: automaticCheckCadence = checkIntervalLabel(funding ? fundingOptionalBigInt(funding.sample_interval_secs) : undefined);
  $: fundingNextCheck = snapshot ? nextCheckLabel(optional(snapshot.overview.next_sample_at_secs)) : 'Unavailable';
  $: fundingUnavailable = !!funding
    && fundingCyclesBalance === undefined
    && fundingCyclesAvailable === undefined
    && fundingIcpBalance === undefined
    && fundingIcpAvailable === undefined;
  $: fundingEmpty = !!funding
    && fundingCyclesAvailable === 0n
    && fundingIcpAvailable === 0n;
  $: visibleAlarms = snapshot?.alarms.filter((alarm) => alarmCanBeAcknowledged(alarm) && !acknowledgedAlarmIds.has(alarm.id.toString())) ?? [];
  $: openAlarmCount = visibleAlarms.length;
  $: singleOperatorMode = snapshot?.governance.is_single_operator_mode ?? false;
  $: unresolvedRecordsComplete = !!snapshot
    && snapshot.operatorRecordsAvailable
    && snapshot.unresolvedNextCursor.length === 0;

  async function refresh(): Promise<void> {
    const epoch = ++telemetryLoadEpoch;
    const accessEpoch = ++operatorAccessEpoch;
    const session = currentWalletSession();
    loading = true;
    operatorDataLoading = true;
    publicError = '';
    snapshot = null;
    if (!session || !canViewSentinelTelemetry(latestWalletConnection.principal) || !isCycleSentinelConfigured) {
      publicError = 'Connect with an approved principal to view private Sentinel telemetry.';
      loading = false;
      operatorDataLoading = false;
      return;
    }
    try {
      const authenticated = await createAuthenticatedSentinelActor();
      const next = await withOperatorQueryTimeout(loadOperatorDashboard(authenticated), 'Loading Sentinel telemetry and operator status');
      if (epoch === telemetryLoadEpoch && accessEpoch === operatorAccessEpoch && session === currentWalletSession()) {
        actor = authenticated;
        snapshot = next;
        signer = next.governance.is_signer;
        operatorChecked = true;
        checkedSession = session;
        proposals = next.proposals;
        unresolved = next.unresolved;
        operatorDataLoaded = signer && next.operatorRecordsAvailable;
        operatorDataError = signer && !next.operatorRecordsAvailable
          ? 'The dashboard response omitted signer-only records. Refresh telemetry before managing proposals or funding operations.'
          : '';
        reconcileManualTopUpLock(next, session);
      }
    } catch {
      if (epoch === telemetryLoadEpoch && accessEpoch === operatorAccessEpoch && session === currentWalletSession()) {
        publicError = 'Sentinel telemetry is private and unavailable to this identity.';
        signer = false;
        operatorChecked = false;
        checkedSession = undefined;
        actor = undefined;
        proposals = [];
        unresolved = [];
        operatorDataLoaded = false;
      }
    } finally {
      if (epoch === telemetryLoadEpoch) {
        loading = false;
        operatorDataLoading = false;
      }
    }
  }

  function reconcileManualTopUpLock(data: OperatorDashboardSnapshot, session: string): void {
    if (!data.operatorRecordsAvailable) return;
    if (manualTopUpLock?.inFlight) return;
    if (manualTopUpLock && manualTopUpLock.identityKey !== currentWalletIdentityKey()) return;
    const complete = data.unresolvedNextCursor.length === 0;
    if (manualTopUpLock) {
      const tracked = manualTopUpLock.operationId !== undefined
        ? data.unresolved.find((item) => item.id === manualTopUpLock?.operationId)
        : data.unresolved.find((item) => item.target.toText() === manualTopUpLock!.target.toText() && variant(item.trigger) === 'ManualTopup');
      if (!tracked) {
        if (!complete) {
          topupError = 'More unresolved records are available. Load them before deciding whether this top-up moved or retrying.';
          return;
        }
        manualTopUpLock = { ...manualTopUpLock, authorizedSession: session, disposition: 'uncertain', inFlight: false, requiresOperatorAcknowledgement: true };
        topupError = 'No unresolved manual top-up was found. This does not confirm whether funds moved. Verify recent top-ups, the target balance, and the Sentinel funding ledger before deciding whether to retry.';
      } else {
        const result = manualTopUpDisposition(tracked);
        if (result.disposition === 'completed' || result.disposition === 'terminal') {
          manualTopUpLock = { ...manualTopUpLock, authorizedSession: session, disposition: 'uncertain', operationId: tracked.id, inFlight: false, requiresOperatorAcknowledgement: true };
          topupError = `Operation #${tracked.id.toString()} is ${result.state} in the unresolved-operation history. Verify the target balance and funding ledger before explicitly unlocking retries.`;
        } else {
          manualTopUpLock = { ...manualTopUpLock, authorizedSession: session, disposition: result.disposition, operationId: tracked.id, inFlight: false, requiresOperatorAcknowledgement: false };
          if (manualTopUpLock.rail && manualTopUpLock.amount) storeManualTopUpMarker({ identityKey: manualTopUpLock.identityKey, target: manualTopUpLock.target.toText(), rail: manualTopUpLock.rail, amount: manualTopUpLock.amount, operationId: tracked.id, state: result.state });
          topupError = `Operation #${tracked.id.toString()} remains unresolved (${result.state}). Do not submit again; reconcile it before retrying.`;
        }
      }
    } else if (complete) {
      const unresolvedManualTopUp = data.unresolved.find((item) => variant(item.trigger) === 'ManualTopup');
      if (unresolvedManualTopUp) {
        const result = manualTopUpDisposition(unresolvedManualTopUp);
        manualTopUpLock = {
          session,
          authorizedSession: session,
          identityKey: currentWalletIdentityKey()!,
          target: unresolvedManualTopUp.target,
          disposition: result.disposition === 'pending' ? 'pending' : 'uncertain',
          operationId: unresolvedManualTopUp.id,
          inFlight: false,
          requiresOperatorAcknowledgement: result.disposition === 'completed' || result.disposition === 'terminal',
        };
        topupError = result.disposition === 'completed' || result.disposition === 'terminal'
          ? `Operation #${unresolvedManualTopUp.id.toString()} is ${result.state} in the unresolved-operation history. Verify the target balance and funding ledger before explicitly unlocking retries.`
          : `Operation #${unresolvedManualTopUp.id.toString()} was restored from the unresolved-operation list (${result.state}). Do not submit another top-up until it is reconciled.`;
      }
    }
  }

  async function retryOperatorData(): Promise<void> {
    await refresh();
  }

  async function loadAllOperatorRecords(): Promise<void> {
    if (loadingAllOperatorRecords) return;
    loadingAllOperatorRecords = true;
    try {
      assertCurrentSigner();
      if (!actor || !snapshot) throw new Error('Refresh telemetry before loading operator records.');
      const requestActor = actor;
      const session = checkedSession;
      const [allProposals, allUnresolved] = await withOperatorQueryTimeout(
        Promise.all([listProposals(requestActor), listUnresolvedFundingOperations(requestActor)]),
        'Loading remaining operator records',
      );
      if (!session || session !== currentWalletSession() || requestActor !== actor || !signer || !snapshot) return;
      proposals = allProposals;
      unresolved = allUnresolved;
      snapshot = { ...snapshot, proposals: allProposals, unresolved: allUnresolved, proposalsNextCursor: [], unresolvedNextCursor: [] };
      operatorDataLoaded = true;
      operatorDataError = '';
      reconcileManualTopUpLock(snapshot, session);
    } catch (error) {
      authError = error instanceof Error ? error.message : String(error);
    } finally {
      loadingAllOperatorRecords = false;
    }
  }

  async function runCheckNow(): Promise<void> {
    if (checkingNow) return;
    actionMessage = '';
    authError = '';
    try { assertCurrentSigner(); }
    catch (error) { authError = error instanceof Error ? error.message : String(error); return; }
    const authenticated = actor!;
    const session = checkedSession;
    const epoch = operatorAccessEpoch;
    const checkSessionIsCurrent = (): boolean => epoch === operatorAccessEpoch
      && session === currentWalletSession()
      && checkedSession === session
      && actor === authenticated
      && signer;
    checkingNow = true;
    try {
      await sentinelManagement.runMaintenanceNow(authenticated);
      if (!checkSessionIsCurrent()) return;
      actionMessage = 'Check completed. Refresh telemetry to read the saved results.';
    } catch (error) {
      if (checkSessionIsCurrent()) authError = error instanceof Error ? error.message : String(error);
    } finally {
      checkingNow = false;
    }
  }

  async function run(action: () => Promise<unknown>): Promise<void> {
    actionMessage = '';
    authError = '';
    try {
      assertCurrentSigner();
      const result = await action();
      if (typeof result === 'bigint') proposalId = result.toString();
      actionMessage = 'Action accepted by Cycle Sentinel. Refresh telemetry when you want to reload saved state.';
    }
    catch (error) { authError = error instanceof Error ? error.message : String(error); }
  }

  async function configureSingleOperatorGovernance(): Promise<void> {
    if (configuringSingleOperator) return;
    const requestActor = actor;
    const session = currentWalletSession();
    authError = '';
    actionMessage = '';
    if (!requestActor || !session || session !== checkedSession || !snapshot) {
      authError = 'Refresh telemetry with one of the configured operator principals before changing signer access.';
      return;
    }
    configuringSingleOperator = true;
    try {
      const result = await requestActor.configure_single_operator_governance();
      if (session !== currentWalletSession() || requestActor !== actor || session !== checkedSession) return;
      if ('Err' in result) {
        const error = Object.keys(result.Err)[0] ?? 'Unknown';
        authError = error === 'NotOperator'
          ? 'This principal is not one of the three configured Cycle Sentinel operators.'
          : error === 'InvalidProjection'
            ? 'The dashboard could not be prepared, so signer access was not changed. Refresh telemetry and try again.'
            : error === 'SetupAlreadyUsed'
              ? 'The one-time signer setup has already been used. Signer changes now follow the configured governance delay.'
            : `Cycle Sentinel rejected the signer update: ${error}`;
        return;
      }
      const wasSingleOperatorMode = snapshot.governance.is_single_operator_mode;
      const next = operatorDashboardSnapshot(result.Ok);
      snapshot = next;
      signer = next.governance.is_signer;
      operatorChecked = true;
      operatorDataLoaded = next.operatorRecordsAvailable;
      operatorDataError = next.operatorRecordsAvailable ? '' : 'The operator dashboard is incomplete. Refresh telemetry before using signer-only record lists.';
      proposals = next.proposals;
      unresolved = next.unresolved;
      reconcileManualTopUpLock(next, session);
      actionMessage = wasSingleOperatorMode
        ? 'Single-signature access was already active: any one of the three configured operator principals can manage Sentinel.'
        : 'Single-signature access is active: any one of the three configured operator principals can manage Sentinel. This setup bypassed the signer-change timelock and cleared approvals on open proposals; funding policy and its other timelocks are unchanged.';
    } catch (error) {
      authError = `The update response could not be confirmed${error instanceof Error ? ` (${error.message})` : ''}. It may have applied; refresh telemetry to check before retrying.`;
    } finally {
      configuringSingleOperator = false;
    }
  }

  function id(value: string, label: string): bigint { return parseNat(value, label); }
  function target(): Principal { return parsePrincipal(targetPrincipal, 'Target principal'); }
  function operation(): bigint { return id(operationId, 'Operation ID'); }
  function proposal(): bigint { return id(proposalId, 'Proposal ID'); }
  function alarmCanBeAcknowledged(alarm: PublicAlarm): boolean { return variant(alarm.status) === 'Open'; }

  onMount(() => {
    window.addEventListener('storage', onAcknowledgedAlarmsStorageChange);
    const unsubscribeWallet = walletStore.subscribe((state) => {
      latestWalletConnection = { isConnected: state.isConnected, principal: state.principal };
      observeWalletSession();
    });
    const unsubscribeWalletType = currentWalletType.subscribe((walletType) => {
      latestWalletType = walletType;
      observeWalletSession();
    });
    const unsubscribeWalletSessionGeneration = walletSessionGeneration.subscribe((generation) => {
      latestWalletSessionGeneration = generation;
      observeWalletSession();
    });
    return () => {
      window.removeEventListener('storage', onAcknowledgedAlarmsStorageChange);
      unsubscribeWallet();
      unsubscribeWalletType();
      unsubscribeWalletSessionGeneration();
    };
  });
</script>

<svelte:head>
  {#if canViewSentinelTelemetry($walletStore.principal)}<title>Cycle Sentinel Telemetry | Rumi</title>{/if}
</svelte:head>
<section class="telemetry-page">
  {#if canViewSentinelTelemetry($walletStore.principal)}
  {#if loading}
    <div class="login-note">Verifying access to private Sentinel telemetry…</div>
  {:else if !snapshot}
    <div class="login-note">{publicError || 'Sentinel telemetry is private. Connect with an approved principal.'}</div>
  {:else}
  <header class="hero">
    <div class="hero-copy"><p class="eyebrow">RUMI OPERATIONS</p><h1>Cycle Sentinel</h1><p class="lede">Private runtime health and auditable cycle maintenance. Operator controls appear only after the canister confirms signer permission.</p></div>
    <div class="check-controls">
      <p class="check-schedule">Automatic checks: <strong>{automaticCheckCadence}</strong></p>
      <div class="check-buttons">
        <button on:click={refresh} disabled={loading || checkingNow}>{loading ? 'Refreshing telemetry…' : 'Refresh telemetry'}</button>
        {#if signer && actor}
          <button class="run-check" on:click={runCheckNow} disabled={checkingNow || loading} aria-describedby="manual-check-description">{checkingNow ? 'Checking now…' : 'Run check now'}</button>
        {/if}
      </div>
      <p class="fine-print">Refresh telemetry reads saved results.</p>
      {#if signer && actor}<p id="manual-check-description" class="fine-print">Run check now checks balances immediately and may refuel Sentinel or top up registered canisters under the current reserves, thresholds and spending limits.</p>{/if}
    </div>
  </header>
  {#if publicError}<div class="notice error" role="alert">Public telemetry unavailable: {publicError}</div>{/if}
  {#if authError}<div class="notice error" role="alert">Operator query/action: {authError}</div>{/if}
  {#if actionMessage}<div class="notice success">{actionMessage}</div>{/if}
  {#if topupError && !topupTarget}<div class="notice error" role="alert">{topupError}</div>{/if}
  {#if manualTopUpLock}
    <div class="notice error" role="alert">
      {#if manualTopUpLock.requiresOperatorAcknowledgement}
        This retry lock is held because the UI cannot independently confirm whether the prior request moved funds. Review the target balance, its recent top-up records, and both Sentinel funding balances below. After reviewing the relevant ledgers and balances, acknowledge to clear the retry lock; this acknowledgment does not assert success or failure.
      {:else}
        A previous manual top-up is still pending or uncertain. Do not submit another until you reconcile it in the operator console.
      {/if}
      {#if manualTopUpLock.session === currentWalletSession() && manualTopUpLock.operationId !== undefined} Operation #{manualTopUpLock.operationId.toString()}.{/if}
      {#if snapshot}
        {@const lockedTargetRow = snapshot.targets.find((row) => row.principal.toText() === manualTopUpLock?.target.toText())}
        <div class="manual-lock-evidence">
          {#if lockedTargetRow}
            <p><strong>{lockedTargetRow.display_name}</strong> · latest sampled balance {lockedTargetRow.advisory_balance_overflowed ? 'Overflow' : formatCycles(optional(lockedTargetRow.advisory_balance_cycles))} cycles · sampled {ageLabel(lockedTargetRow.as_of_secs)}</p>
            {#if lockedTargetRow.recent_topups.length}
              <ul>{#each lockedTargetRow.recent_topups.slice(-3) as item}<li>{variant(item.outcome)} · {variant(item.rail)} · {formatTCycles(item.amount_cycles)} T-cycles · {ageLabel(item.resolved_at_secs)}</li>{/each}</ul>
            {:else}<p>No recent top-up summary is recorded for this target.</p>{/if}
          {:else}<p>Latest target balance and top-up summaries are unavailable.</p>{/if}
          <p>Sentinel funding balances: cycles {formatTCycles(fundingCyclesBalance)} T ({fundingCyclesBalanceStale ? 'stale' : ageLabel(fundingCyclesBalanceAsOf)}), ICP {formatIcp(fundingIcpBalance)} ({fundingIcpBalanceStale ? 'stale' : ageLabel(fundingIcpBalanceAsOf)}). Spendable: {formatTCycles(fundingCyclesAvailable)} T and {formatIcp(fundingIcpAvailable)} ICP.</p>
        </div>
      {/if}
      {#if signer && actor && !manualTopUpLock.inFlight} <button on:click={retryOperatorData} disabled={operatorDataLoading}>Recheck unresolved operations</button>{/if}
      {#if manualTopUpLock.requiresOperatorAcknowledgement && hasCurrentManualTopUpAuthority()}<button on:click={acknowledgeManualTopUpOutcomeReviewed}>I reviewed the outcome and checked balances — clear retry lock</button>{/if}
    </div>
  {/if}
  {#if !isCycleSentinelConfigured}<div class="notice">Cycle Sentinel is not configured yet. This page remains fail-closed until the authoritative Task 10 deployment.</div>
  {:else}
    {#if loading}<p class="muted">Loading private telemetry…</p>{/if}
    {#if snapshot}<div class="stats"><div><span>Targets</span><strong>{format(snapshot.overview.target_count)}</strong></div><div><span>Healthy</span><strong>{format(snapshot.overview.healthy_count)}</strong></div><div><span>Runtime fuel</span><strong>{formatTCycles(snapshot.overview.runtime_cycles)} T</strong><small>This is Sentinel's own operating fuel.</small></div><div><span>Alerts to review</span><strong>{openAlarmCount}</strong></div></div>{/if}
    <section class="funding-wallet" aria-labelledby="funding-wallet-heading">
      <div class="funding-heading">
        <div>
          <p class="eyebrow">AUTOMATIC TOP-UPS</p>
          <h2 id="funding-wallet-heading">Sentinel funding wallet</h2>
          <p class="muted">Deposit here to fund target top-ups. This reserve is separate from the <strong>Runtime fuel</strong> number above.</p>
        </div>
        <span class="badge confirmed">Uses cycles first · ICP fallback</span>
      </div>
        <div class="funding-balances">
          <article class="funding-balance">
            <div class="balance-title"><h3>Deposit cycles</h3><span class="network">ICRC Cycles Ledger</span></div>
            <div class="balance-grid"><div><span>Ledger balance</span><strong>{formatTCycles(fundingCyclesBalance)} T</strong><small class:stale={fundingCyclesBalanceStale}>{fundingCyclesBalanceStale ? 'Stale · ' : ''}{ageLabel(fundingCyclesBalanceAsOf)}</small></div><div><span>Spendable</span><strong>{formatTCycles(fundingCyclesAvailable)} T</strong><small>After protected reserves</small></div><div><span>Protected</span><strong>{formatTCycles(fundingProtectedCycles)} T</strong><small>Kept for Sentinel recovery</small></div></div>
            <p class="deposit-copy">Send cycles to the Cycles Ledger using the Sentinel principal as the owner and the default subaccount.</p>
            <div class="address-row"><span class="address-label">Cycles deposit owner</span><code>{fundingOwnerText}</code><button on:click={() => copyLabel('Cycles deposit owner', fundingOwnerText)}>Copy</button></div>
            <p class="service-reference">Cycles Ledger service reference: <code>{CYCLES_LEDGER_PRINCIPAL}</code></p>
            <p class="fine-print">Use a Cycles Ledger transfer to the Sentinel owner/default subaccount above. Sending runtime cycles directly to the Sentinel or a target canister does not fund this ledger pool; it only changes that canister's runtime balance.</p>
          </article>
          <article class="funding-balance">
            <div class="balance-title"><h3>Deposit ICP</h3><span class="network">ICP Ledger · mainnet</span></div>
            <div class="balance-grid"><div><span>Ledger balance</span><strong>{formatIcp(fundingIcpBalance)} ICP</strong><small class:stale={fundingIcpBalanceStale}>{fundingIcpBalanceStale ? 'Stale · ' : ''}{ageLabel(fundingIcpBalanceAsOf)}</small></div><div><span>Spendable</span><strong>{formatIcp(fundingIcpAvailable)} ICP</strong><small>After reserve policy</small></div><div><span>Minimum reserve</span><strong>{formatIcp(fundingMinIcpReserve)} ICP</strong><small>Held back from conversion</small></div></div>
            <p class="deposit-copy">Send ICP to the Sentinel owner on the ICP Ledger using the default subaccount. ICRC wallets can use the principal below; legacy wallets can use the 64-character account identifier. Sentinel converts ICP through the NNS Cycles Minting Canister only when spendable cycles cannot cover a refill.</p>
            <div class="address-row"><span class="address-label">ICRC owner principal</span><code>{fundingOwnerText}</code><button on:click={() => copyLabel('ICRC owner principal', fundingOwnerText)}>Copy</button></div>
            <div class="address-row"><span class="address-label">ICP account identifier</span><code>{icpAccountText}</code><button on:click={() => copyLabel('ICP account identifier', icpAccountText)}>Copy</button></div>
            <p class="service-reference">ICP Ledger service reference: <code>{ICP_LEDGER_PRINCIPAL}</code></p>
            <p class="fine-print">The ICRC owner uses the empty/default subaccount. The legacy identifier is the same owner account encoded for older ICP Ledger send forms.</p>
          </article>
        </div>
        <div class="funding-footer"><span>Conversion status: <strong>{fundingConversionStatus}</strong></span><span>Funding owner: <code>{fundingOwnerText}</code></span><span>Next scheduled check: <strong>{fundingNextCheck}</strong></span></div>
        {#if fundingEmpty}<p class="notice idle"><strong>Automation is configured but awaiting funds.</strong> Deposit cycles or ICP above; the next scheduled Sentinel check will recognize the reserve. Refreshing this page reads current public state but does not force a funding run.</p>{/if}
        {#if !funding}<p class="notice idle"><strong>Funding balances are unavailable right now.</strong> The deposit destinations above are ready; balances and automation status will appear after the Sentinel publishes its public funding status.</p>{/if}
        {#if copyMessage}<p class="notice success" role="status">{copyMessage}</p>{/if}
        {#if copyError}<p class="notice error" role="alert">{copyError}</p>{/if}
    </section>
    {#if snapshot}
      {#if registryIdle}
      <div class="notice idle">
        <strong>Observation has not been switched on yet.</strong>
        <p>All {snapshot.overview.target_count.toString()} registered targets are still disabled or set to <em>Unobserved</em>, so the Sentinel has never sampled them{neverSampled ? '' : ' recently'}. That is why balance, burn rate, and runway read <em>Unavailable</em> rather than zero — the values are genuinely unknown, not missing from the page.</p>
        <p class="muted">Registration is deliberately fail-closed: a target is created with monitoring off and auto-top-up off. Turning observation on is a separate governed change; the active approval threshold and target-registry timelock apply.</p>
        {#if fundingUnavailable}<p class="muted">The Sentinel also reports no funding source yet (no cycles-ledger balance and no ICP), so top-ups would be rejected even for an enabled target.</p>{/if}
      </div>
      {/if}
    <div class="registry-shell">
      <article class="registry-card">
        <div class="registry-heading"><div class="registry-title"><h2>Target registry</h2>
          {#if snapshot}<div class="registry-state-summary" aria-label="Target monitoring and auto top-up status">
            <span class="registry-count"><strong>Monitoring</strong><b>{monitoringEnabledCount}/{observableTargets.length}</b><small>enabled</small></span>
            <span class="registry-count"><strong>Auto top-up</strong><b>{autoTopupEnabledCount}/{autoTopupEligibleTargets.length}</b><small>enabled · eligible</small></span>
            {#if unobservedTargetCount || pausedTargetCount}<small class="registry-exclusions">{#if unobservedTargetCount}{unobservedTargetCount} unobserved skipped{/if}{#if unobservedTargetCount && pausedTargetCount} · {/if}{#if pausedTargetCount}{pausedTargetCount} paused for auto top-up{/if}</small>{/if}
          </div>{/if}</div>
          {#if signer && actor}<div class="registry-bulk-controls"><div class="registry-bulk-actions" aria-label="Bulk target controls">
            <button type="button" disabled={proposingBulkTargets !== null || loading || monitoringPendingCount === 0} title={monitoringPendingCount ? `Create one proposal to enable monitoring for ${monitoringPendingCount} observable target${monitoringPendingCount === 1 ? '' : 's'}.` : 'Monitoring is already enabled for every observable target.'} on:click={() => proposeBulkTargetFlags('enable')}>{proposingBulkTargets === 'enable' ? 'Submitting…' : monitoringPendingCount ? `Enable monitoring · ${monitoringPendingCount}` : 'Monitoring enabled for all'}</button>
            <button type="button" class="bulk-primary" disabled={proposingBulkTargets !== null || loading || autoTopupPendingCount === 0} title={autoTopupPendingCount ? `Create one proposal to enable monitoring and auto top-up for ${autoTopupPendingCount} eligible target${autoTopupPendingCount === 1 ? '' : 's'}.` : 'Auto top-up is already enabled for every eligible target.'} on:click={() => proposeBulkTargetFlags('auto-top-up')}>{proposingBulkTargets === 'auto-top-up' ? 'Submitting…' : autoTopupPendingCount ? `Enable auto top-up · ${autoTopupPendingCount}` : 'Auto top-up enabled for all'}</button>
          </div><small>Unobserved targets are skipped; paused targets are skipped for auto top-up. Each action creates one proposal; the on-chain approval threshold and delay apply.</small></div>{/if}
          <div class="alarm-menu">
            <button class="alarm-trigger" aria-label={`Alerts${openAlarmCount ? `, ${openAlarmCount} to review` : ', none to review'}`} aria-expanded={alarmsOpen} on:click={() => alarmsOpen = !alarmsOpen}>
              <span aria-hidden="true">🔔</span>{#if openAlarmCount}<i class="alarm-indicator"></i>{/if}<span>Alerts</span>{#if openAlarmCount}<b>{openAlarmCount}</b>{/if}
            </button>
            {#if alarmsOpen}<section class="alarm-popover" aria-label="Sentinel alerts">
              <div class="alarm-popover-heading"><strong>Alerts to review</strong><span class="muted" role="status" aria-live="polite" aria-atomic="true">{openAlarmCount} remaining</span><button class="quiet-button" aria-label="Close alerts" on:click={() => alarmsOpen = false}>×</button></div>
              {#if alarmStorageWarning}<p class="fine-print" role="alert">{alarmStorageWarning}</p>{/if}
              {#if visibleAlarms.length}<p class="fine-print">Acknowledge hides an alert in this browser. Monitoring continues, and a new incident will appear.</p>{#each visibleAlarms as alarm (alarm.id.toString())}<div class="alarm-row"><span class="alarm-dot dot-open"></span><div class="alarm-copy"><strong>{variant(alarm.kind)}</strong><small>{alarm.target[0]?.toText() ?? 'Sentinel'}</small></div><button class="quiet-button alarm-status" data-alert-ack aria-label={`Acknowledge ${variant(alarm.kind)} alert for ${alarm.target[0]?.toText() ?? 'Sentinel'}`} on:click={(event) => acknowledgeAlarmLocally(alarm, event.currentTarget)}>Acknowledge</button></div>{/each}{:else}<p class="muted">No alerts to review.</p>{/if}
            </section>{/if}
          </div>
        </div>
        {#if snapshot.targets.length}<div class="table-scroll"><table><thead><tr><th scope="col">Target</th><th scope="col">State</th><th scope="col">Balance</th><th scope="col">Burn / day</th><th scope="col">Runway</th><th scope="col">Top-up rule</th><th scope="col">Actions</th></tr></thead><tbody>{#each snapshot.targets as row}<tr class:selected-row={selectedTargetPrincipal === row.principal.toText()}>
          <td class="target-cell"><strong>{row.display_name}</strong><button class="principal-copy" title="Copy canister ID" on:click={() => copyLabel('Canister ID', row.principal.toText())}>{row.principal.toText()}</button><small>{variant(row.environment)} · {variant(row.observation_mode)}</small></td>
          <td class="state-cell" title={targetStateLabel(row) === 'Awaiting first sample' ? 'Monitoring is enabled. The next scheduled observation has not completed yet.' : undefined}><span class:state-low={!fundingTarget(row).paused && targetStateLabel(row) === 'Low'} class:state-healthy={!fundingTarget(row).paused && targetStateLabel(row) === 'Healthy'} class="state-pill">{fundingTarget(row).paused ? 'Paused' : targetStateLabel(row)}</span><div class="target-flags"><span class:flag-off={!fundingTarget(row).enabled} class="target-flag"><i aria-hidden="true"></i>Enabled <strong>{fundingTarget(row).enabled ? 'ON' : 'OFF'}</strong></span><span class:flag-off={!fundingTarget(row).auto_topup} class="target-flag"><i aria-hidden="true"></i>Auto top-up <strong>{fundingTarget(row).auto_topup ? 'ON' : 'OFF'}</strong></span></div></td>
          <td class="metric-cell" title={format(optional(row.advisory_balance_cycles))}><span class="metric-label">Balance</span><strong>{row.advisory_balance_overflowed ? 'Overflow' : formatCycles(optional(row.advisory_balance_cycles))}</strong></td>
          <td class="metric-cell" title={format(optional(row.burn_cycles_per_day))}><span class="metric-label">Burn / day</span><strong>{formatCycles(optional(row.burn_cycles_per_day))}</strong></td>
          <td class="metric-cell"><span class="metric-label">Runway</span><strong>{formatRunway(optional(row.runway_secs))}</strong></td>
          <td class="rule-cell"><span class="metric-label">Top-up rule</span><div class="rule-summary"><strong>At {formatTCycles(row.low_balance_threshold_cycles)}T</strong><span>add {formatTCycles(row.refill_cycles)}T</span></div><small>{fundingTarget(row).daily_cap_cycles === undefined ? 'Cap unavailable' : `Cap ${formatTCycles(fundingTarget(row).daily_cap_cycles)}T/day`} · {fundingTarget(row).cooldown_secs === undefined ? 'Cooldown unavailable' : `${fundingTarget(row).cooldown_secs.toString()}s cooldown`}</small>{#if row.recent_topups.length}<small>Last: {variant(row.recent_topups[row.recent_topups.length - 1].outcome)} · {formatTCycles(row.recent_topups[row.recent_topups.length - 1].amount_cycles)}T</small>{/if}{#if signer && actor}<details class="row-disclosure rule-disclosure"><summary aria-label={`Edit rule for ${row.display_name}`}>Edit rule</summary><div class="rule-editor"><label>Threshold <span><input aria-label={`Low balance threshold for ${row.display_name}, T-cycles`} inputmode="decimal" value={ruleDraft(row).lowThreshold} on:input={(event) => ruleDraft(row).lowThreshold = event.currentTarget.value} /> T</span></label><label>Refill <span><input aria-label={`Refill amount for ${row.display_name}, T-cycles`} inputmode="decimal" value={ruleDraft(row).refill} on:input={(event) => ruleDraft(row).refill = event.currentTarget.value} /> T</span></label><label>Daily cap <span><input aria-label={`Daily cap for ${row.display_name}, T-cycles`} inputmode="decimal" value={ruleDraft(row).dailyCap} on:input={(event) => ruleDraft(row).dailyCap = event.currentTarget.value} /> T</span></label><label>Cooldown <span><input aria-label={`Cooldown for ${row.display_name}, seconds`} inputmode="numeric" value={ruleDraft(row).cooldown} on:input={(event) => ruleDraft(row).cooldown = event.currentTarget.value} /> s</span></label><button class="row-action" on:click={() => saveRule(row)}>Propose rule</button></div>{#if ruleErrors[row.principal.toText()]}<small class="rule-error" role="alert">{ruleErrors[row.principal.toText()]}</small>{/if}</details>{/if}</td>
          <td class="row-actions">{#if signer && actor}<details class="row-disclosure"><summary aria-label={`Manage ${row.display_name}`}>Manage</summary><div class="manage-controls"><label class="inline-toggle"><input type="checkbox" checked={fundingTarget(row).enabled} on:change={(event) => toggleTargetEnabled(row, event.currentTarget.checked)} /> Enabled</label><label class="inline-toggle"><input type="checkbox" checked={fundingTarget(row).auto_topup} on:change={(event) => toggleAutoTopup(row, event.currentTarget.checked)} /> Auto-top-up</label>{#if fundingTarget(row).paused}<button class="row-action" on:click={() => run(() => sentinelManagement.proposeUnpauseTarget(actor!, row.principal))}>Propose unpause</button>{:else}<button class="row-action" on:click={() => run(() => sentinelManagement.pauseTarget(actor!, row.principal))}>Pause</button>{/if}<button class="row-action" disabled={!!manualTopUpLock || !unresolvedRecordsComplete} title={unresolvedRecordsComplete ? 'Request a manual top-up.' : 'Load all unresolved-operation pages before submitting a manual top-up.'} on:click={() => openManualTopUp(row)}>Manual top-up</button><button class="row-action" on:click={() => selectTarget(row)}>Use settings</button></div></details>{/if}</td>
        </tr>{/each}</tbody></table></div>{:else}<p class="muted">No targets have been published.</p>{/if}
      </article>
    </div>
    <article class="topup-history">
      <div class="history-heading"><div><h2>Top-up history</h2><p class="muted">Manual requests, automatic low-balance top-ups, and Sentinel self-recovery in one timeline.</p></div><strong>{snapshot.topupHistory.length} retained record{snapshot.topupHistory.length === 1 ? '' : 's'}</strong></div>
      <p class="fine-print">Sentinel retains up to 512 terminal records. Older records may have aged out. Records saved before origin tracking was added show “Origin unavailable.”</p>
      {#if snapshot.topupHistory.length}<div class="table-scroll"><table><thead><tr><th scope="col">Date</th><th scope="col">Target</th><th scope="col">Origin</th><th scope="col">Outcome</th><th scope="col">Rail</th><th scope="col">Amount</th></tr></thead><tbody>
        {#each snapshot.topupHistory as item, index (`${item.resolved_at_secs.toString()}-${item.target.toText()}-${index}`)}<tr><td>{topupTimeLabel(item.resolved_at_secs)}</td><td><strong>{topupTargetLabel(item)}</strong><small>{item.target.toText()}</small></td><td>{topupOrigin(item)}</td><td>{variant(item.outcome)}</td><td>{variant(item.rail)}</td><td>{formatTCycles(item.amount_cycles)} T-cycles</td></tr>{/each}
      </tbody></table></div>{:else}<p class="muted">No completed or terminal top-ups are currently retained.</p>{/if}
    </article>
    {/if}
  {/if}

  {#if $walletStore.isConnected && isCycleSentinelConfigured}<section class="operator"><h2>Operator console <span class:confirmed={signer} class="badge">{signer ? `Signer · ${snapshot?.governance.approval_threshold ?? 0} of ${snapshot?.governance.signers.length ?? 0}` : operatorChecked ? 'Telemetry access confirmed' : 'Access not loaded'}</span></h2>
    {#if snapshot}<article class="operator-access"><h3>Signer access</h3><p>Current on-chain approval threshold: <strong>{snapshot.governance.approval_threshold} of {snapshot.governance.signers.length}</strong>. {snapshot.governance.is_signer ? 'This wallet can manage Cycle Sentinel.' : 'This wallet can view telemetry but cannot manage Cycle Sentinel yet.'}</p>
      <details open><summary>Current signer principals</summary><ul>{#each snapshot.governance.signers as principal}<li><code>{principal.toText()}</code></li>{/each}</ul></details>
      {#if singleOperatorMode}<p class="notice success">Single-signature mode is active. Any one of the three configured operator principals can manage the Sentinel alone.</p>
      {:else if snapshot.governance.single_operator_setup_available}<p class="muted">These are the three operator principals authorized by this canister. This one-time setup immediately replaces the signer list with them and sets the threshold to one, bypassing the signer-change timelock. It clears existing approvals on open proposals. Funding policy and its other timelocks do not change. The setup cannot be repeated; future signer changes follow governance delays.</p><details><summary>Configured operator principals</summary><ul>{#each snapshot.governance.configured_operator_principals as principal}<li><code>{principal.toText()}</code></li>{/each}</ul></details><button on:click={configureSingleOperatorGovernance} disabled={configuringSingleOperator || !operatorChecked}>{configuringSingleOperator ? 'Updating signer access…' : 'Use one-time single-signature setup'}</button>{:else}<p class="muted">The one-time signer setup has already been used. Future signer changes follow the configured governance delay.</p>{/if}
    </article>{/if}
    {#if signer && actor}
      {#if operatorDataLoading}<p class="muted" role="status">Loading telemetry and operator data…</p>{/if}
      {#if operatorDataError}<div class="notice error" role="alert">{operatorDataError}<button on:click={retryOperatorData} disabled={operatorDataLoading}>Refresh telemetry</button></div>{/if}
      {#if snapshot?.operatorRecordsAvailable && (snapshot.proposalsNextCursor.length || snapshot.unresolvedNextCursor.length)}<div class="notice idle"><p>More than one page of operator records is retained. Load all pages only when needed; each additional wallet-routed query may ask for approval.</p><button on:click={loadAllOperatorRecords} disabled={loadingAllOperatorRecords}>{loadingAllOperatorRecords ? 'Loading all records…' : 'Load all proposal and unresolved records'}</button></div>{/if}
      <article><h3>Register target</h3><p class="muted">Registration is fail-closed: the canister creates the target disabled with auto-top-up off. Enablement is a separate governed update.</p><div class="form-grid"><label>Target principal<input bind:value={targetPrincipal} /></label><label>Display name<input bind:value={displayName} /></label><label>Project<input bind:value={project} /></label><label>Tags (comma separated)<input bind:value={tags} /></label><label>Low threshold (T-cycles)<input bind:value={lowThreshold} inputmode="decimal" /></label><label>Refill amount (T-cycles)<input bind:value={refill} inputmode="decimal" /></label><label>Daily cap (T-cycles)<input bind:value={dailyCap} inputmode="decimal" /></label><label>Cooldown seconds<input bind:value={cooldown} inputmode="numeric" /></label><label>Optional burn anomaly limit (T-cycles)<input bind:value={burnAnomalyLimit} inputmode="decimal" /></label><label>Environment<select bind:value={environment}>{#each Object.keys(EnvironmentVariant) as value}<option value={value}>{value}</option>{/each}</select></label><label>Criticality<select bind:value={criticality}>{#each Object.keys(CriticalityVariant) as value}<option value={value}>{value}</option>{/each}</select></label><label>Observation mode<select bind:value={observationMode}>{#each Object.keys(ObservationModeVariant) as value}<option value={value}>{value}</option>{/each}</select></label></div><p class="fine-print">For example: threshold 2, refill 6 means “At 2T, add 6T.” Values are converted to exact cycle integers before submission.</p><button on:click={() => run(() => sentinelManagement.proposeRegisterTarget(actor!, targetArgs()))}>Propose register target</button></article>
      <article><h3>Update or remove target</h3><p class="muted">Update includes funding policy, enabled, and auto-top-up choices. Removal is governed and remains fail-closed while unresolved operations exist.</p><div class="form-grid"><label>Target principal<input bind:value={targetPrincipal} /></label><label>Display name<input bind:value={displayName} /></label><label>Project<input bind:value={project} /></label><label>Tags<input bind:value={tags} /></label><label>Low threshold (T-cycles)<input bind:value={lowThreshold} inputmode="decimal" /></label><label>Refill amount (T-cycles)<input bind:value={refill} inputmode="decimal" /></label><label>Daily cap (T-cycles)<input bind:value={dailyCap} inputmode="decimal" /></label><label>Cooldown seconds<input bind:value={cooldown} inputmode="numeric" /></label><label>Burn anomaly limit (T-cycles)<input bind:value={burnAnomalyLimit} inputmode="decimal" /></label><label>Environment<select bind:value={environment}>{#each Object.keys(EnvironmentVariant) as value}<option value={value}>{value}</option>{/each}</select></label><label>Criticality<select bind:value={criticality}>{#each Object.keys(CriticalityVariant) as value}<option value={value}>{value}</option>{/each}</select></label><label>Observation mode<select bind:value={observationMode}>{#each Object.keys(ObservationModeVariant) as value}<option value={value}>{value}</option>{/each}</select></label><label class="check"><input type="checkbox" bind:checked={enabled} /> Enabled</label><label class="check"><input type="checkbox" bind:checked={autoTopup} /> Auto-top-up</label></div><p class="fine-print">Use “Use settings” in the registry to load the selected target’s current threshold, refill, cap, cooldown, and switches before editing.</p><div class="actions"><button on:click={() => run(() => sentinelManagement.proposeUpdateTarget(actor!, target(), targetPatch()))}>Propose target update</button><button on:click={() => run(() => sentinelManagement.proposeRemoveTarget(actor!, target()))}>Propose target removal</button><button on:click={() => run(() => sentinelManagement.pauseTarget(actor!, target()))}>Pause target immediately</button><button on:click={() => run(() => sentinelManagement.proposeUnpauseTarget(actor!, target()))}>Propose governed unpause</button></div></article>
      <article><h3>Signer governance</h3><div class="form-grid"><label>Signer principal<input bind:value={signerPrincipal} /></label><label>Signer threshold (nat32)<input bind:value={signerThreshold} inputmode="numeric" /></label></div><div class="actions"><button on:click={() => run(() => sentinelManagement.proposeAddSigner(actor!, parsePrincipal(signerPrincipal, 'Signer principal')))}>Propose add signer</button><button on:click={() => run(() => sentinelManagement.proposeRemoveSigner(actor!, parsePrincipal(signerPrincipal, 'Signer principal')))}>Propose remove signer</button><button on:click={() => run(() => sentinelManagement.proposeSetSignerThreshold(actor!, parseNat32(signerThreshold, 'Signer threshold')))}>Propose signer threshold</button></div></article>
      <article><h3>Global policy governance</h3><div class="form-grid"><label>Global daily cap<input bind:value={globalDailyCap} inputmode="numeric" /></label><label>Sample interval seconds<input bind:value={sampleInterval} inputmode="numeric" /></label><label>Stale-after seconds<input bind:value={staleAfter} inputmode="numeric" /></label><label>Minimum ICP reserve e8s<input bind:value={minIcpReserve} inputmode="numeric" /></label><label>Self-recovery refill<input bind:value={selfRefill} inputmode="numeric" /></label><label>Self-recovery low threshold<input bind:value={selfLowThreshold} inputmode="numeric" /></label><label>Self-recovery daily cap<input bind:value={selfDailyCap} inputmode="numeric" /></label><label>Protected reserve cycles<input bind:value={protectedReserve} inputmode="numeric" /></label><label>Unpause timelock seconds<input bind:value={unpauseTimelock} inputmode="numeric" /></label><label>Spend-policy timelock seconds<input bind:value={spendTimelock} inputmode="numeric" /></label><label>Target-registry timelock seconds<input bind:value={targetTimelock} inputmode="numeric" /></label><label>Signer-change timelock seconds<input bind:value={signerTimelock} inputmode="numeric" /></label></div><button on:click={() => run(() => sentinelManagement.proposeSetGlobalPolicy(actor!, globalPolicy()))}>Propose global policy</button></article>
      <article><h3>Proposal list and actions</h3><label>Proposal ID<input bind:value={proposalId} inputmode="numeric" /></label><div class="actions"><button on:click={() => run(() => sentinelManagement.approveProposal(actor!, proposal()))}>Approve proposal</button><button on:click={() => run(() => sentinelManagement.executeProposal(actor!, proposal()))}>Execute proposal</button><button on:click={() => run(() => sentinelManagement.cancelProposal(actor!, proposal()))}>Cancel proposal</button></div>{#if !operatorDataLoaded}<p class="muted">{operatorDataLoading ? 'Loading proposals…' : operatorDataError ? 'Proposal list unavailable; retry list loading above.' : 'Proposal list has not been loaded.'}</p>{:else if proposals.length}<ul>{#each proposals as item}<li>#{item.id.toString()} · {variant(item.status)} · {variant(item.payload)} · {item.approvals.length} approval(s)</li>{/each}</ul>{:else}<p class="muted">No proposals returned.</p>{/if}</article>
      <article><h3>Unresolved funding operations</h3><div class="form-grid"><label>Operation ID<input bind:value={operationId} inputmode="numeric" /></label><label>Ledger block index<input bind:value={blockIndex} inputmode="numeric" /></label></div><div class="actions"><button on:click={() => run(() => sentinelManagement.attachBlockProof(actor!, operation(), id(blockIndex, 'Block index')))}>Attach delivery proof</button><button on:click={() => run(() => sentinelManagement.attachRefundBlockProof(actor!, operation(), id(blockIndex, 'Block index')))}>Attach refund proof</button><button on:click={() => run(() => sentinelManagement.resolveUnknownAsSpent(actor!, operation()) )}>Resolve unknown as spent</button></div>{#if !operatorDataLoaded}<p class="muted">{operatorDataLoading ? 'Loading unresolved operations…' : operatorDataError ? 'Unresolved-operation list unavailable; retry list loading above.' : 'Unresolved-operation list has not been loaded.'}</p>{:else if unresolved.length}<ul>{#each unresolved as item}<li>#{item.id.toString()} · {variant(item.state)} · target {item.target.toText()} · reserved {item.reserved_amount_cycles.toString()} cycles</li>{/each}</ul>{:else}<p class="muted">No unresolved funding operations returned.</p>{/if}</article>
    {:else}<p class="muted">{operatorChecked ? 'Telemetry and signer access were read together. After single-signature setup, any of the three listed operator principals can manage the Sentinel alone.' : 'Refresh telemetry to read telemetry and signer access together.'}</p>{/if}
  </section>{:else}<div class="login-note">Connect a wallet to check signer permissions and access operator controls.</div>{/if}
  {/if}
  {/if}
</section>

{#if topupTarget && signer && actor}
  <div class="modal-backdrop" role="presentation">
    <div class="topup-modal" role="dialog" aria-modal="true" aria-labelledby="topup-heading" aria-describedby="topup-description">
      <div class="modal-heading"><h2 id="topup-heading">Manual top-up</h2><button aria-label="Close manual top-up" on:click={() => topupTarget = null}>×</button></div>
      <p id="topup-description">Choose the amount and funding source for <strong>{topupTarget.display_name}</strong>.</p>
      <div class="topup-form">
        <label>Funding source
          <select bind:value={topupRail} disabled={submittingTopup || !!topupResult}>
            <option value="CyclesLedger">T-cycles from Cycles Ledger</option>
            <option value="IcpCmc">ICP from Sentinel account via NNS CMC</option>
          </select>
        </label>
        {#if topupRail === 'CyclesLedger'}
          <label>Amount in T-cycles
            <input bind:value={topupCyclesAmount} inputmode="decimal" autocomplete="off" aria-label="Top-up amount in T-cycles" disabled={submittingTopup || !!topupResult} />
          </label>
          <p class="fine-print">Available from Sentinel cycles account: <strong>{formatTCycles(fundingCyclesAvailable)} T-cycles</strong> · {fundingCyclesBalanceStale ? 'Stale · ' : ''}{ageLabel(fundingCyclesBalanceAsOf)}. Enter up to 12 decimal places. The requested amount is the cycles transfer; any Cycles Ledger fee is additional.</p>
        {:else}
          <label>Amount in ICP
            <input bind:value={topupIcpAmount} inputmode="decimal" autocomplete="off" aria-label="Top-up amount in ICP" disabled={submittingTopup || !!topupResult} />
          </label>
          <p class="fine-print">Spends from the Sentinel funding account through the NNS Cycles Minting Canister. Available estimate: <strong>{formatIcp(fundingIcpAvailable)} ICP</strong> · {fundingIcpBalanceStale ? 'Stale · ' : ''}{ageLabel(fundingIcpBalanceAsOf)}. The amount is the ICP transfer; the ICP Ledger fee is additional. Enter up to 8 decimal places. Conversion output varies with the rate at execution.</p>
        {/if}
      </div>
      <dl class="topup-summary"><div><dt>Canister</dt><dd>{topupTarget.display_name}</dd></div><div><dt>Amount</dt><dd>{selectedTopUpAmountLabel()}</dd></div><div><dt>Funding source</dt><dd>{topupRail === 'CyclesLedger' ? 'Sentinel cycles account · Cycles Ledger' : 'Sentinel ICP account · NNS CMC'}</dd></div></dl>
      <p class="fine-print">Available balances and freshness are advisory. The canister checks signer permission, target eligibility, cooldown, reserves, fees, and target/global daily caps at submission. There is no live fee or conversion quote before submission.</p>
      {#if topupError}<div class="notice error" role="alert">{topupError}</div>{/if}
      {#if topupResult}<div class:notice={true} class:success={topupResult.disposition === 'completed'} class:error={topupResult.disposition !== 'completed'} role="status">
        Operation #{topupResult.id.toString()} · {topupResult.state} · {topupResult.amount} · {topupResult.source}
      </div>{/if}
      <div class="modal-actions"><button on:click={() => topupTarget = null}>Close</button><button class="confirm-topup" disabled={submittingTopup || !!manualTopUpLock || !!topupResult || !unresolvedRecordsComplete || !validTopUpAmount()} on:click={confirmManualTopUp}>{submittingTopup ? 'Submitting…' : manualTopUpLock ? 'Awaiting confirmation' : 'Confirm top-up'}</button></div>
    </div>
  </div>
{/if}

<style>
  .telemetry-page{max-width:1120px;margin:0 auto;color:var(--rumi-text-primary)}.hero{display:flex;align-items:flex-end;justify-content:space-between;gap:1rem;flex-wrap:wrap;margin-bottom:2rem}.hero h1{margin:.1rem 0;font-size:2.5rem}.lede{max-width:700px;color:var(--rumi-text-secondary)}.eyebrow{color:var(--rumi-teal);font-size:.72rem;letter-spacing:.14em}.stats{display:grid;grid-template-columns:repeat(4,1fr);gap:.75rem;margin-bottom:1rem}.stats div,article,.operator,.login-note{padding:1rem;background:var(--rumi-bg-surface-1);border:1px solid var(--rumi-border);border-radius:.5rem}.stats span,.muted,small{display:block;color:var(--rumi-text-muted);font-size:.78rem}.stats strong{font-size:1.25rem}.grid{display:grid;grid-template-columns:2fr 1fr;gap:1rem}h2,h3{margin-top:0}table{width:100%;border-collapse:collapse}th,td{text-align:left;padding:.55rem;border-bottom:1px solid var(--rumi-border);font-size:.82rem;vertical-align:top}.alarm{display:flex;gap:.5rem;align-items:center;border-bottom:1px solid var(--rumi-border);padding:.65rem 0}.alarm>span:nth-last-of-type(1){margin-left:auto;font-size:.75rem}.dot{width:.45rem;height:.45rem;background:#e05252;border-radius:50%}.operator{margin-top:1rem;display:grid;gap:1rem}.operator>h2,.operator>p{margin-bottom:0}.badge{font-size:.7rem;padding:.25rem .5rem;border-radius:99px;color:var(--rumi-text-muted);background:var(--rumi-bg-surface3)}.badge.confirmed{color:var(--rumi-teal);background:rgba(45,212,191,.12)}.form-grid{display:grid;grid-template-columns:repeat(3,1fr);gap:.7rem}.form-grid label{display:block;font-size:.75rem;color:var(--rumi-text-muted)}input,select{display:block;width:100%;box-sizing:border-box;margin-top:.25rem;padding:.5rem;background:var(--rumi-bg-surface3);border:1px solid var(--rumi-border);color:inherit;border-radius:.3rem}.check{display:flex!important;gap:.5rem;align-items:center}.check input{width:auto}.actions{display:flex;gap:.4rem;flex-wrap:wrap;margin-top:.7rem}button{border:1px solid var(--rumi-border-hover);background:var(--rumi-bg-surface3);color:inherit;border-radius:.4rem;padding:.55rem .8rem;cursor:pointer;font-size:.78rem}button:hover{border-color:var(--rumi-action)}button:disabled{opacity:.5}.notice{padding:.7rem;margin-bottom:1rem;border-radius:.4rem}.error{color:#ff9b9b;background:rgba(224,82,82,.12)}.idle{background:var(--rumi-bg-surface-1);border:1px solid var(--rumi-border)}.idle strong{display:block;margin-bottom:.35rem}.idle p{margin:.35rem 0;font-size:.82rem;color:var(--rumi-text-secondary)}.idle p.muted{font-size:.78rem}.success{color:var(--rumi-teal);background:rgba(45,212,191,.1)}.login-note{margin-top:1rem}li{margin:.4rem 0;font-size:.82rem}.funding-wallet{padding:1.2rem;background:linear-gradient(135deg,rgba(31,48,78,.75),rgba(19,29,52,.95));border:1px solid rgba(45,212,191,.35);border-radius:.65rem;margin-bottom:1rem}.funding-heading{display:flex;justify-content:space-between;gap:1rem;align-items:flex-start;margin-bottom:1rem}.funding-heading h2{margin:.1rem 0 .35rem}.funding-heading p{margin:0}.funding-balances{display:grid;grid-template-columns:1fr 1fr;gap:.8rem}.funding-balance{padding:1rem;background:rgba(8,15,30,.35);border-color:rgba(255,255,255,.12)}.balance-title{display:flex;align-items:baseline;justify-content:space-between;gap:.5rem}.balance-title h3{margin:0}.network{font-size:.7rem;color:var(--rumi-teal)}.balance-grid{display:grid;grid-template-columns:repeat(3,1fr);gap:.5rem;margin:1rem 0}.balance-grid>div{min-width:0;padding:.6rem;background:rgba(255,255,255,.04);border-radius:.35rem}.balance-grid strong{display:block;min-width:0;overflow-wrap:anywhere;word-break:break-word;font-size:1.05rem;margin-top:.2rem}.deposit-copy{color:var(--rumi-text-secondary);font-size:.82rem;line-height:1.45}.address-row{display:grid;grid-template-columns:9.5rem minmax(0,1fr) auto;align-items:center;gap:.5rem;margin-top:.5rem}.address-label{font-size:.72rem;color:var(--rumi-text-muted)}code{font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:.73rem;overflow-wrap:anywhere;user-select:all}.address-row code{padding:.45rem;background:rgba(0,0,0,.22);border-radius:.25rem}.address-row button{padding:.4rem .55rem}.fine-print{font-size:.72rem;color:var(--rumi-text-muted);line-height:1.4}.funding-footer{display:flex;justify-content:space-between;gap:1rem;flex-wrap:wrap;margin-top:.8rem;color:var(--rumi-text-muted);font-size:.75rem}.rule{display:block;white-space:nowrap}.selected-row{background:rgba(45,212,191,.07)}.select-target{padding:.4rem .5rem;white-space:nowrap}@media(max-width:768px){.stats{grid-template-columns:repeat(2,1fr)}.grid,.form-grid,.funding-balances{grid-template-columns:1fr}.balance-grid{grid-template-columns:1fr 1fr}.address-row{grid-template-columns:1fr}.address-row button{justify-self:start}table{font-size:.72rem}}
  .hero-copy{flex:1 1 30rem}.check-controls{flex:1 1 18rem;max-width:390px}.check-schedule{margin:0 0 .5rem;font-size:.82rem;color:var(--rumi-text-secondary)}.check-buttons{display:flex;flex-wrap:wrap;gap:.5rem}.check-controls .fine-print{margin:.4rem 0 0}.run-check{border-color:var(--rumi-teal);color:var(--rumi-teal)}
  .funding-wallet .deposit-copy,.funding-wallet .address-label,.funding-wallet .fine-print,.funding-wallet .service-reference{color:var(--rumi-text-secondary)}.service-reference{margin:.5rem 0 0;font-size:.72rem;line-height:1.4}.service-reference code{padding:0;background:transparent;color:inherit}
  .registry-shell{display:block}.registry-card{min-width:0}.registry-heading{display:flex;align-items:center;justify-content:space-between;gap:1rem}.registry-heading h2{margin-bottom:0}.table-scroll{overflow-x:auto;margin-top:.75rem}.table-scroll table{min-width:1000px}.registry-card th{white-space:nowrap}.registry-card td{vertical-align:middle}.principal-copy{display:block;margin:.2rem 0 0;padding:0;border:0;background:transparent;color:var(--rumi-text-muted);font:inherit;font-size:.72rem;text-align:left;overflow-wrap:anywhere;user-select:text}.principal-copy:hover{color:var(--rumi-teal)}.inline-toggle{display:flex;align-items:center;gap:.3rem;margin-top:.25rem;color:var(--rumi-text-muted);font-size:.7rem;white-space:nowrap}.inline-toggle input{width:auto;margin:0}.row-action{margin-top:.3rem;padding:.35rem .5rem;font-size:.69rem;white-space:nowrap}.rule-editor{display:grid;grid-template-columns:repeat(2,minmax(5rem,1fr));gap:.2rem .4rem;min-width:12rem}.rule-editor label{display:flex;align-items:center;gap:.2rem;color:var(--rumi-text-muted);font-size:.68rem;white-space:nowrap}.rule-editor input{min-width:0;width:4.5rem;margin:0;padding:.28rem .3rem;font-size:.7rem}.rule-editor .row-action{grid-column:1/-1;justify-self:start}.alarm-menu{position:relative}.alarm-trigger{position:relative;display:flex;align-items:center;gap:.45rem}.alarm-trigger>span:first-child{font-size:1.1rem}.alarm-indicator{position:absolute;left:1.38rem;top:.25rem;width:.45rem;height:.45rem;border-radius:50%;background:#ef5350;box-shadow:0 0 0 2px var(--rumi-bg-surface-1)}.alarm-trigger b{display:grid;place-items:center;min-width:1.1rem;height:1.1rem;padding:0 .15rem;border-radius:99px;background:#b4232c;color:white;font-size:.65rem}.alarm-popover{position:absolute;z-index:20;right:0;top:calc(100% + .5rem);width:min(27rem,calc(100vw - 3rem));max-height:65vh;overflow:auto;padding:.8rem;background:var(--rumi-bg-surface-1);border:1px solid var(--rumi-border-hover);border-radius:.55rem;box-shadow:0 14px 36px rgba(0,0,0,.42)}.alarm-popover-heading{display:flex;justify-content:space-between;align-items:center;padding-bottom:.45rem;border-bottom:1px solid var(--rumi-border)}.alarm-row{display:flex;align-items:center;gap:.55rem;padding:.65rem 0;border-bottom:1px solid var(--rumi-border)}.alarm-dot{flex:none;width:.48rem;height:.48rem;border-radius:50%;background:var(--rumi-text-muted)}.alarm-dot.dot-open{background:#ef5350}.alarm-copy{min-width:0}.alarm-copy small{overflow-wrap:anywhere}.alarm-status{margin-left:auto;color:var(--rumi-text-muted);font-size:.7rem;white-space:nowrap}.quiet-button{padding:.35rem .5rem;white-space:nowrap}.alarm-popover .muted{padding:.5rem 0}
  @media(max-width:768px){.registry-heading{align-items:flex-start}.alarm-popover{right:-.5rem}.table-scroll table{min-width:1050px}}
  .modal-backdrop{position:fixed;z-index:1000;inset:0;display:grid;place-items:center;padding:1rem;background:rgba(2,8,20,.72);backdrop-filter:blur(3px)}.topup-modal{width:min(30rem,100%);padding:1.2rem;background:var(--rumi-bg-surface-1);border:1px solid var(--rumi-border-hover);border-radius:.7rem;box-shadow:0 22px 60px rgba(0,0,0,.5)}.modal-heading{display:flex;align-items:center;justify-content:space-between;gap:1rem}.modal-heading h2{margin:0}.modal-heading button{font-size:1.15rem;padding:.25rem .55rem}.topup-summary{margin:1rem 0}.topup-summary>div{display:grid;grid-template-columns:8rem 1fr;gap:.7rem;padding:.55rem 0;border-bottom:1px solid var(--rumi-border)}.topup-summary dt{color:var(--rumi-text-muted);font-size:.75rem}.topup-summary dd{margin:0;font-size:.82rem}.modal-actions{display:flex;justify-content:flex-end;gap:.5rem;margin-top:1rem}.confirm-topup{border-color:var(--rumi-teal);color:var(--rumi-teal)}
  .topup-history{margin-top:1rem}.history-heading{display:flex;align-items:flex-start;justify-content:space-between;gap:1rem}.history-heading h2{margin-bottom:.2rem}.history-heading p{margin:.25rem 0}.history-heading>strong{white-space:nowrap;color:var(--rumi-teal);font-size:.8rem}.topup-history table{min-width:820px}.topup-history td small{max-width:16rem;overflow-wrap:anywhere}.operator-access details{margin:.6rem 0}.operator-access code{overflow-wrap:anywhere}.operator-access .notice{margin:.5rem 0}

  /* Cycle Sentinel: a wide, calm control room for long-lived operational data. */
  .telemetry-page {
    --sentinel-line: rgba(170, 190, 222, .17);
    --sentinel-teal: #52dfc8;
    max-width: 1540px;
    width: calc(100vw - 4rem);
    position: relative;
    margin: 0 0 0 calc(50% - 50vw + 2rem);
    padding: 1.6rem 0 3rem;
    color: var(--rumi-text-primary, #f2f6ff);
    font-size: .94rem;
    line-height: 1.5;
  }
  .hero {
    align-items: center;
    margin: 0 0 1.35rem;
    padding: .4rem 0 1.5rem;
    border-bottom: 1px solid var(--sentinel-line);
  }
  .hero-copy { flex: 1 1 36rem; }
  .eyebrow {
    margin: 0 0 .35rem;
    color: var(--sentinel-teal);
    font-size: .73rem;
    font-weight: 650;
    letter-spacing: .08em;
  }
  .hero h1 { margin: 0; font-size: clamp(2rem, 3vw, 2.7rem); line-height: 1.1; letter-spacing: -.035em; }
  .lede { max-width: 58rem; margin: .65rem 0 0; color: var(--rumi-text-secondary, #bbc7dc); font-size: 1rem; line-height: 1.55; }
  .check-controls { flex: 0 1 20rem; max-width: 24rem; padding: 1rem 1.1rem; border: 1px solid var(--sentinel-line); border-radius: .65rem; background: rgba(20, 33, 55, .72); }
  .check-schedule { font-size: .88rem; }
  .stats {
    grid-template-columns: repeat(4, minmax(0, 1fr));
    gap: 0;
    margin: 0 0 1.25rem;
    overflow: hidden;
    border: 1px solid var(--sentinel-line);
    border-radius: .7rem;
    background: linear-gradient(110deg, rgba(23, 39, 65, .82), rgba(15, 25, 44, .92));
  }
  .stats > div { min-width: 0; padding: 1rem 1.25rem; border: 0; border-right: 1px solid var(--sentinel-line); border-radius: 0; background: transparent; }
  .stats > div:last-child { border-right: 0; }
  .stats span, .stats small { color: #b9c7de; font-size: .79rem; }
  .stats strong { display: block; margin-top: .25rem; font-size: 1.45rem; font-weight: 650; letter-spacing: -.025em; font-variant-numeric: tabular-nums; }
  .funding-wallet {
    margin-bottom: 1.25rem;
    padding: 1.4rem;
    background: linear-gradient(120deg, rgba(23, 42, 68, .96), rgba(15, 25, 44, .98));
    border: 1px solid rgba(82, 223, 200, .34);
    border-left: 3px solid var(--sentinel-teal);
    border-radius: .75rem;
  }
  .funding-heading { align-items: center; margin-bottom: 1.15rem; }
  .funding-heading h2 { margin: .1rem 0 .35rem; font-size: 1.35rem; letter-spacing: -.02em; }
  .funding-heading .muted { color: #c4d0e4; font-size: .88rem; }
  .funding-heading .badge { flex: none; color: #8af1df; background: rgba(82, 223, 200, .11); border: 1px solid rgba(82, 223, 200, .25); }
  .funding-balances { gap: 1rem; }
  .funding-balance {
    padding: 1.1rem 1.15rem;
    background: rgba(8, 15, 29, .45);
    border: 1px solid rgba(189, 205, 231, .16);
    border-radius: .58rem;
  }
  .balance-title h3 { font-size: 1rem; }
  .network { color: #82dece; font-size: .76rem; }
  .balance-grid { gap: .65rem; margin: .9rem 0; }
  .balance-grid > div { padding: .75rem; background: rgba(208, 224, 249, .055); border: 1px solid rgba(208, 224, 249, .08); border-radius: .45rem; }
  .balance-grid span { color: #bdcbe0; font-size: .76rem; }
  .balance-grid strong { margin-top: .25rem; font-size: 1.15rem; font-variant-numeric: tabular-nums; }
  .balance-grid small { margin-top: .15rem; color: #9fadc5; font-size: .71rem; }
  .deposit-copy { color: #ccd6e7; font-size: .86rem; line-height: 1.55; }
  .address-row { grid-template-columns: 10rem minmax(0, 1fr) auto; margin-top: .65rem; }
  .address-label { color: #b6c4dc; font-size: .76rem; }
  .address-row code { color: #e5edf9; background: rgba(4, 10, 21, .58); border: 1px solid rgba(189, 205, 231, .12); }
  .service-reference { color: #aebbd2; }
  .funding-wallet .fine-print { color: #aebbd2; font-size: .75rem; }
  .funding-footer { padding-top: .8rem; border-top: 1px solid rgba(189, 205, 231, .13); color: #c0cce0; font-variant-numeric: tabular-nums; }
  .funding-footer code { color: #d8e2f2; }
  .registry-shell { margin: 0; }
  .registry-card {
    padding: 1.35rem 1.4rem 1rem;
    background: linear-gradient(180deg, rgba(17, 28, 48, .98), rgba(12, 20, 36, .99));
    border: 1px solid var(--sentinel-line);
    border-radius: .75rem;
    box-shadow: 0 12px 35px rgba(1, 7, 18, .2);
  }
  .registry-heading { align-items: flex-start; padding-bottom: .9rem; border-bottom: 1px solid var(--sentinel-line); }
  .registry-title { display: grid; gap: .65rem; min-width: 15rem; }
  .registry-heading h2 { margin: 0; font-size: 1.3rem; letter-spacing: -.02em; }
  .registry-state-summary { display: flex; flex-wrap: wrap; align-items: stretch; gap: .4rem; }
  .registry-count { display: grid; grid-template-columns: auto auto; align-items: baseline; column-gap: .35rem; padding: .35rem .55rem; border: 1px solid rgba(174, 195, 224, .16); border-radius: .45rem; background: rgba(8, 15, 29, .34); }
  .registry-count strong { color: #c6d3e7; font-size: .7rem; font-weight: 550; }
  .registry-count b { color: #edf2fa; font-size: .78rem; font-variant-numeric: tabular-nums; }
  .registry-count small { grid-column: 1 / -1; color: #8f9fb9; font-size: .62rem; line-height: 1.2; }
  .registry-state-summary .registry-exclusions { align-self: center; color: #9eacc3; font-size: .66rem; }
  .registry-bulk-controls { display: grid; justify-items: end; gap: .2rem; margin-left: auto; }
  .registry-bulk-controls small { color: #aebbd2; font-size: .68rem; text-align: right; }
  .registry-bulk-actions { display: flex; flex-wrap: wrap; justify-content: end; gap: .45rem; }
  .registry-bulk-actions button { min-height: 2.45rem; }
  .registry-bulk-actions .bulk-primary { color: #071b1c; background: #69dfcb; border-color: #69dfcb; font-weight: 700; }
  .registry-bulk-actions .bulk-primary:hover:not(:disabled) { background: #a1f3e5; border-color: #a1f3e5; }
  .table-scroll { margin: .25rem -1.4rem 0; padding: 0 1.4rem .5rem; scrollbar-color: #435776 #111c30; }
  .table-scroll table { min-width: 1380px; border-collapse: separate; border-spacing: 0; }
  .registry-card th { padding: .8rem .7rem; color: #b4c3da; font-size: .75rem; font-weight: 600; letter-spacing: .025em; border-bottom: 1px solid rgba(174, 195, 224, .23); }
  .registry-card td { padding: .8rem .7rem; color: #edf2fa; font-size: .83rem; border-bottom: 1px solid rgba(174, 195, 224, .11); }
  .registry-card tbody tr { transition: background-color .15s ease; }
  .registry-card tbody tr:hover { background: rgba(102, 132, 175, .09); }
  .registry-card tbody tr:last-child td { border-bottom: 0; }
  .registry-card td:first-child { min-width: 11rem; }
  .registry-card td:nth-child(2) { min-width: 10.5rem; }
  .registry-card td:nth-child(3) { min-width: 17rem; }
  .registry-card td:nth-child(4), .registry-card td:nth-child(5), .registry-card td:nth-child(6) { white-space: nowrap; font-variant-numeric: tabular-nums; }
  .registry-card td:first-child > strong { display: block; font-size: .87rem; font-weight: 650; }
  .principal-copy { margin-top: .25rem; color: #b6c5dc; font-size: .72rem; }
  .registry-card td small { margin-top: .2rem; color: #a8b6cc; font-size: .72rem; }
  .rule-editor { min-width: 16rem; gap: .4rem .65rem; }
  .rule-editor label { gap: .3rem; color: #bdc9dc; font-size: .73rem; }
  .rule-editor input { width: 4.8rem; padding: .42rem .5rem; border-radius: .35rem; font-size: .78rem; }
  .rule-editor input:disabled { color: #d6deeb; opacity: 1; background: #202e47; border-color: rgba(181, 200, 227, .17); }
  .registry-card select { min-width: 8rem; padding: .48rem 2rem .48rem .6rem; background-color: #1c2a43; border-color: rgba(181, 200, 227, .22); color: #eaf0fa; }
  .inline-toggle { margin-top: .4rem; color: #bdc9dc; font-size: .74rem; }
  .inline-toggle input { accent-color: var(--sentinel-teal); }
  .row-action { margin-top: .4rem; padding: .42rem .62rem; border-radius: .42rem; font-size: .74rem; }
  .registry-card .row-action:not(:disabled) { border-color: rgba(82, 223, 200, .3); }
  .row-actions { min-width: 9rem; }
  .alarm-trigger { padding: .55rem .8rem; background: #17243a; border-color: rgba(180, 198, 226, .2); font-size: .8rem; }
  .alarm-trigger > span:first-child { font-size: 1rem; filter: grayscale(1); }
  .alarm-indicator { left: 1.45rem; top: .42rem; box-shadow: 0 0 0 2px #17243a; }
  .alarm-trigger b { background: #bf343c; }
  .alarm-popover { width: min(29rem, calc(100vw - 2rem)); padding: 1rem; background: #17243a; border-color: rgba(180, 198, 226, .24); border-radius: .7rem; box-shadow: 0 20px 60px rgba(0, 0, 0, .55); }
  .alarm-popover-heading { padding-bottom: .7rem; }
  .alarm-popover-heading strong { font-size: 1rem; }
  .alarm-row { gap: .7rem; padding: .75rem .1rem; border-color: rgba(180, 198, 226, .12); }
  .alarm-copy strong { font-size: .83rem; }
  .alarm-copy small { color: #b5c3da; }
  .alarm-status { color: #d0d9e7; }
  .operator { margin-top: 1.25rem; padding: 1.3rem; gap: 1rem; background: #111c30; border-color: var(--sentinel-line); border-radius: .75rem; }
  .operator > h2 { margin: 0; font-size: 1.2rem; }
  .operator article { padding: 1.1rem; background: rgba(24, 38, 64, .62); border-color: rgba(170, 190, 222, .15); border-radius: .55rem; }
  .operator article h3 { margin-bottom: .45rem; font-size: 1rem; }
  .operator .muted, .operator .fine-print { color: #b7c5da; }
  input, select { min-height: 2.35rem; padding: .55rem .65rem; background-color: #17243a; border-color: rgba(181, 200, 227, .23); border-radius: .4rem; color: #edf2fa; font-size: .84rem; }
  input:focus, select:focus, button:focus-visible { outline: 2px solid var(--sentinel-teal); outline-offset: 2px; }
  button { min-height: 2.3rem; padding: .58rem .85rem; border-color: rgba(181, 200, 227, .23); border-radius: .45rem; background: #1b2a43; color: #edf2fa; font-size: .82rem; transition: background-color .14s ease, border-color .14s ease, transform .14s ease; }
  button:hover:not(:disabled) { border-color: rgba(82, 223, 200, .62); background: #243751; }
  button:active:not(:disabled) { transform: translateY(1px); }
  button:disabled { color: #b1bfd4; opacity: .7; }
  .run-check, .confirm-topup { color: #091b20; background: var(--sentinel-teal); border-color: var(--sentinel-teal); font-weight: 650; }
  .run-check:hover:not(:disabled), .confirm-topup:hover:not(:disabled) { background: #8aefdd; }
  .badge { color: #cad5e5; background: #25344d; }
  .badge.confirmed { color: #7bf0db; background: rgba(82, 223, 200, .12); }
  .notice { padding: .9rem 1rem; border: 1px solid var(--sentinel-line); border-radius: .55rem; font-size: .88rem; }
  .notice.error { color: #ffd2d2; background: rgba(153, 39, 50, .2); border-color: rgba(244, 105, 114, .34); }
  .notice.success { color: #a0f2e3; background: rgba(30, 132, 113, .15); border-color: rgba(82, 223, 200, .28); }
  .notice.idle { padding: 1rem 1.15rem; background: #17243a; border-color: rgba(142, 173, 216, .2); }
  .notice.idle strong { color: #e6edf8; }
  .login-note { padding: 1.2rem 1.4rem; color: #dce6f4; background: #15233a; border-color: var(--sentinel-line); border-radius: .7rem; }
  .topup-modal { padding: 1.5rem; background: #17243a; border-color: rgba(180, 198, 226, .28); border-radius: .8rem; }
  .topup-modal { max-height: calc(100vh - 2rem); overflow-y: auto; }
  .topup-form { display: grid; gap: .55rem; }
  .topup-form label { color: #c5d1e4; font-size: .82rem; font-weight: 550; }
  .topup-form .fine-print { margin: 0 0 .25rem; }
  .topup-modal h2 { font-size: 1.35rem; letter-spacing: -.02em; }
  .topup-modal > p { color: #c7d3e5; line-height: 1.55; }
  .topup-form { display: grid; gap: .75rem; margin-top: 1.1rem; }
  .topup-form label { color: #c4d1e3; font-size: .82rem; font-weight: 600; }
  .topup-form input, .topup-form select { margin-top: .35rem; min-height: 2.7rem; background: #101d31; border-color: rgba(181, 200, 227, .28); }
  .topup-form .fine-print { margin: -.35rem 0 0; }
  .topup-summary { overflow: hidden; border: 1px solid rgba(180, 198, 226, .16); border-radius: .5rem; }
  .topup-summary > div { grid-template-columns: 8rem 1fr; padding: .75rem .8rem; background: rgba(5, 12, 24, .2); border-bottom: 1px solid rgba(180, 198, 226, .13); }
  .topup-summary > div:last-child { border-bottom: 0; }
  .topup-summary dt { color: #b6c5dc; }
  .topup-summary dd { color: #edf2fa; line-height: 1.45; }
  .topup-modal .fine-print { color: #b4c1d6; font-size: .76rem; }
  .modal-actions { padding-top: .8rem; border-top: 1px solid rgba(180, 198, 226, .15); }
  @media (min-width: 1580px) {
    .telemetry-page { width: 1540px; margin-left: calc(50% - 770px); }
  }
  @media (max-width: 900px) {
    .telemetry-page { width: 100%; max-width: none; margin: 0; padding-top: 1rem; }
    .hero { align-items: stretch; }
    .check-controls { flex: 1 1 100%; max-width: none; }
    .funding-wallet { padding: 1rem; }
    .registry-card { padding: 1rem; }
    .registry-heading { flex-wrap: wrap; }
    .registry-bulk-controls { flex: 1 1 100%; order: 3; align-items: start; margin-left: 0; }
    .registry-bulk-actions { justify-content: start; }
    .registry-bulk-controls small { text-align: left; }
    .table-scroll { margin-right: -1rem; margin-left: -1rem; padding-right: 1rem; padding-left: 1rem; }
    .table-scroll table { min-width: 1280px; }
  }
  @media (max-width: 560px) {
    .telemetry-page { width: 100%; }
    .stats > div { padding: .8rem .7rem; }
    .stats strong { font-size: 1.15rem; }
    .registry-bulk-actions { display: grid; grid-template-columns: 1fr; width: 100%; }
    .registry-bulk-actions button { width: 100%; }
    .funding-heading { align-items: flex-start; flex-direction: column; }
    .balance-grid { grid-template-columns: 1fr; }
    .address-row { grid-template-columns: 1fr auto; }
    .address-row code { grid-column: 1; grid-row: 2; }
    .address-row button { grid-column: 2; grid-row: 2; }
    .funding-footer { display: grid; }
    .registry-card { padding: .8rem; }
    .registry-heading h2 { font-size: 1.1rem; }
    .table-scroll { margin-right: -.8rem; margin-left: -.8rem; padding-right: .8rem; padding-left: .8rem; }
    .alarm-popover { right: -.25rem; width: min(27rem, calc(100vw - 1.5rem)); }
  }

  /* The registry is telemetry first; editing expands only for the selected target. */
  .table-scroll { overflow-x: auto; }
  .table-scroll table { min-width: 1060px; table-layout: fixed; }
  .registry-card th:nth-child(1) { width: 19%; }
  .registry-card th:nth-child(2) { width: 14%; }
  .registry-card th:nth-child(3), .registry-card th:nth-child(4), .registry-card th:nth-child(5) { width: 10%; }
  .registry-card th:nth-child(6) { width: 25%; }
  .registry-card th:nth-child(7) { width: 12%; }
  .registry-card td:first-child, .registry-card td:nth-child(2), .registry-card td:nth-child(3) { min-width: 0; }
  .registry-card td { padding-top: 1rem; padding-bottom: 1rem; }
  .target-cell > strong { color: #f4f7fc; }
  .target-cell small { margin-top: .15rem; }
  .principal-copy { max-width: 100%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .state-pill { display: inline-flex; align-items: center; padding: .2rem .55rem; border: 1px solid rgba(181, 200, 227, .18); border-radius: 99px; background: rgba(181, 200, 227, .08); color: #d4dfed; font-size: .75rem; font-weight: 650; line-height: 1.2; }
  .state-pill.state-low { border-color: rgba(238, 178, 89, .32); background: rgba(238, 178, 89, .1); color: #f3c987; }
  .state-pill.state-healthy { border-color: rgba(82, 223, 200, .23); background: rgba(82, 223, 200, .09); color: #8be9d8; }
  .state-cell small { margin-top: .5rem; line-height: 1.3; }
  .target-flags { display: grid; justify-items: start; gap: .35rem; margin-top: .5rem; }
  .target-flag { display: inline-flex; align-items: center; gap: .38rem; padding: .27rem .48rem; border: 1px solid rgba(82, 223, 200, .23); border-radius: .38rem; background: rgba(82, 223, 200, .075); color: #9be9dc; font-size: .7rem; line-height: 1.15; white-space: nowrap; }
  .target-flag i { width: .42rem; height: .42rem; flex: none; border-radius: 50%; background: var(--sentinel-teal); }
  .target-flag strong { color: #c5fff4; font-size: .65rem; letter-spacing: .045em; }
  .target-flag.flag-off { border-color: rgba(174, 195, 224, .17); background: rgba(174, 195, 224, .055); color: #c3cee0; }
  .target-flag.flag-off i { background: #7f8fa9; }
  .target-flag.flag-off strong { color: #d0d9e8; }
  .metric-cell strong { display: block; color: #f0f5fc; font-size: .94rem; font-variant-numeric: tabular-nums; font-weight: 620; }
  .metric-label { display: none; }
  .rule-summary { display: flex; align-items: baseline; gap: .35rem; flex-wrap: wrap; font-variant-numeric: tabular-nums; }
  .rule-summary strong { color: #eaf5f4; font-size: .86rem; }
  .rule-summary span { color: #8ce6d7; font-size: .83rem; }
  .rule-summary span::before { content: '→'; margin-right: .35rem; color: #7e91ab; }
  .rule-cell small { line-height: 1.35; }
  .registry-card td.rule-cell { white-space: normal; }
  .row-disclosure { margin-top: .55rem; }
  .row-disclosure summary { width: max-content; max-width: 100%; color: #92e9d9; cursor: pointer; font-size: .75rem; font-weight: 600; list-style-position: inside; }
  .row-disclosure summary:hover { color: #c5fff3; }
  .row-disclosure summary:focus-visible { outline: 2px solid var(--sentinel-teal); outline-offset: 3px; border-radius: .2rem; }
  .rule-editor, .manage-controls { display: grid; gap: .55rem; margin-top: .7rem; padding-top: .7rem; border-top: 1px solid var(--sentinel-line); }
  .rule-editor { grid-template-columns: repeat(2, minmax(0, 1fr)); min-width: 0; }
  .rule-editor label { display: block; min-width: 0; color: #c5d1e3; font-size: .72rem; }
  .rule-editor label span { display: flex; align-items: center; gap: .3rem; margin-top: .2rem; }
  .rule-editor input { width: 100%; min-width: 0; max-width: 7.5rem; min-height: 2rem; margin: 0; padding: .3rem .45rem; font-size: .78rem; font-variant-numeric: tabular-nums; }
  .rule-editor .row-action { grid-column: 1 / -1; justify-self: start; }
  .registry-card .rule-error { margin-top: .5rem; color: #ffb7b7; line-height: 1.4; white-space: normal; }
  .manage-controls .inline-toggle { margin: 0; }
  .manage-controls .row-action { width: 100%; margin: 0; text-align: left; white-space: normal; }
  .row-actions { min-width: 0; vertical-align: top !important; }
  .row-actions .row-disclosure { margin-top: 0; }

  @media (max-width: 1100px) {
    .table-scroll { overflow: visible; margin: .85rem 0 0; padding: 0; }
    .table-scroll table { display: block; min-width: 0; width: 100%; }
    .table-scroll thead { position: absolute; width: 1px; height: 1px; overflow: hidden; clip-path: inset(50%); white-space: nowrap; }
    .table-scroll tbody { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: .8rem; }
    .registry-card tbody tr { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); grid-template-areas: 'target target state' 'balance burn runway' 'rule rule rule' 'actions actions actions'; align-content: start; gap: .75rem .55rem; padding: 1rem; border: 1px solid var(--sentinel-line); border-radius: .55rem; background: rgba(9, 18, 33, .45); }
    .registry-card tbody tr:hover { background: rgba(20, 37, 59, .6); }
    .registry-card td, .registry-card tbody tr:last-child td { display: block; min-width: 0; padding: 0; border: 0; }
    .target-cell { grid-area: target; }
    .state-cell { grid-area: state; text-align: right; }
    .state-cell small { font-size: .68rem; }
    .metric-cell:nth-child(3) { grid-area: balance; }
    .metric-cell:nth-child(4) { grid-area: burn; }
    .metric-cell:nth-child(5) { grid-area: runway; }
    .metric-cell { padding-top: .65rem !important; border-top: 1px solid var(--sentinel-line) !important; }
    .metric-label { display: block; margin-bottom: .15rem; color: #9fb0c8; font-size: .69rem; }
    .rule-cell { grid-area: rule; padding-top: .65rem !important; border-top: 1px solid var(--sentinel-line) !important; }
    .row-actions { grid-area: actions; }
    .row-actions .row-disclosure { margin-top: 0; }
    .manage-controls { grid-template-columns: repeat(2, minmax(0, 1fr)); }
  }
  @media (max-width: 760px) {
    .table-scroll tbody { grid-template-columns: 1fr; }
    .registry-card tbody tr { padding: .9rem; }
  }
</style>
