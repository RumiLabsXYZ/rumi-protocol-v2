<script lang="ts">
  import { onMount } from 'svelte';
  import { Principal } from '@dfinity/principal';
  import { walletStore } from '$lib/stores/wallet';
  import { currentWalletType, walletSessionGeneration } from '$lib/services/auth';
  import { targetStateLabel } from '$lib/services/cycleSentinelTelemetry';
  import {
    ageLabel,
    CYCLES_LEDGER_PRINCIPAL,
    formatIcp,
    formatTCycles,
    fundingOwner,
    fundingOverview,
    fundingTarget,
    isStale,
    legacyIcpAccountIdentifier,
    optionalBigInt as fundingOptionalBigInt,
    parseTCycles,
    targetFundingPolicyChanged,
    variantLabel,
  } from '$lib/services/cycleSentinelFunding';
  import {
    createAnonymousSentinelActor,
    createAuthenticatedSentinelActor,
    getPermissions,
    listProposals,
    listUnresolvedFundingOperations,
    loadPublicTelemetry,
    parseNat,
    parseNat32,
    parsePrincipal,
    requireText,
    sentinelManagement,
    isCycleSentinelConfigured,
    type SentinelActor,
    type TelemetrySnapshot,
  } from '$lib/services/cycleSentinelService';
  import type {
    Criticality,
    Environment,
    ObservationMode,
    ProposalRecord,
    PublicAlarm,
    TargetArgs,
    TargetFundingPolicy,
    TargetPatch,
    GlobalPolicyArgs,
    FundingOperation,
  } from '$declarations/rumi_cycle_sentinel/rumi_cycle_sentinel.did';

  let snapshot: TelemetrySnapshot | null = null;
  let proposals: ProposalRecord[] = [];
  let unresolved: FundingOperation[] = [];
  let signer = false;
  let operatorChecked = false;
  let checkingOperatorAccess = false;
  let checkedSession: string | undefined;
  let operatorAccessEpoch = 0;
  let observedWalletSession: string | undefined;
  let latestWalletConnection: { isConnected: boolean; principal: Principal | null } = { isConnected: false, principal: null };
  let latestWalletType: string | null = null;
  let latestWalletSessionGeneration = 0;
  let loading = true;
  let publicError = '';
  let authError = '';
  let actionMessage = '';
  let copyMessage = '';
  let copyError = '';
  let selectedTargetPrincipal = '';
  let actor: SentinelActor | undefined;

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

  const variant = (value: Record<string, unknown>): string => Object.keys(value)[0] ?? 'Unknown';
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

  function invalidateOperatorAccess(): void {
    operatorAccessEpoch += 1;
    signer = false;
    operatorChecked = false;
    checkingOperatorAccess = false;
    checkedSession = undefined;
    actor = undefined;
    proposals = [];
    unresolved = [];
  }

  function assertCurrentSigner(): void {
    const session = currentWalletSession();
    if (!signer || !actor || !checkedSession || checkedSession !== session) {
      invalidateOperatorAccess();
      throw new Error('Operator access changed. Check signer access again before submitting an action.');
    }
  }

  function observeWalletSession(): void {
    const nextSession = walletSession();
    if (nextSession === observedWalletSession) return;
    observedWalletSession = nextSession;
    // Store subscriptions observe every connect, disconnect, and wallet-type
    // transition. This prevents a same-principal reconnection from retaining
    // a previously-created authenticated actor.
    invalidateOperatorAccess();
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
  $: registryIdle = !!snapshot
    && snapshot.overview.target_count > 0n
    && snapshot.overview.unobserved_count === snapshot.overview.target_count;
  $: neverSampled = !!snapshot && optional(snapshot.overview.last_sample_at_secs) === undefined;
  $: funding = snapshot ? fundingOverview(snapshot.overview) : undefined;
  $: if (loadedTarget && targetPrincipal.trim() !== loadedTarget.principal && selectedTargetPrincipal === loadedTarget.principal) {
    loadedTarget = null;
    selectedTargetPrincipal = '';
  }
  $: fundingPrincipal = snapshot ? fundingOwner(snapshot.overview, CANISTER_IDS.CYCLE_SENTINEL) : undefined;
  $: fundingOwnerText = fundingPrincipal?.toText() ?? 'Unavailable';
  $: icpAccountText = fundingPrincipal ? legacyIcpAccountIdentifier(fundingPrincipal) : 'Unavailable';
  $: fundingUnavailable = !!funding
    && fundingOptionalBigInt(funding.cycles_ledger_balance_cycles) === undefined
    && fundingOptionalBigInt(funding.cycles_ledger_available_cycles) === undefined
    && fundingOptionalBigInt(funding.icp_ledger_balance_e8s) === undefined
    && fundingOptionalBigInt(funding.icp_available_e8s) === undefined;
  $: fundingEmpty = !!funding
    && fundingOptionalBigInt(funding.cycles_ledger_available_cycles) === 0n
    && fundingOptionalBigInt(funding.icp_available_e8s) === 0n;

  async function refresh(): Promise<void> {
    loading = true;
    publicError = '';
    if (isCycleSentinelConfigured) {
      try { snapshot = await loadPublicTelemetry(createAnonymousSentinelActor()); }
      catch (error) { publicError = error instanceof Error ? error.message : String(error); }
    } else snapshot = null;
    loading = false;
  }

  async function checkOperatorAccess(): Promise<void> {
    authError = '';
    actionMessage = '';
    operatorChecked = false;
    checkingOperatorAccess = true;
    signer = false;
    actor = undefined;
    proposals = [];
    unresolved = [];
    let session: string | undefined;
    try {
      session = currentWalletSession();
      if (!session) throw new Error('Connect a wallet before checking operator access.');
      const epoch = ++operatorAccessEpoch;
      const authenticated = await createAuthenticatedSentinelActor();
      const permissions = await getPermissions(authenticated);
      if (epoch !== operatorAccessEpoch || session !== currentWalletSession()) return;
      actor = authenticated;
      signer = permissions.is_signer;
      operatorChecked = true;
      checkedSession = session;
      if (signer) {
        [proposals, unresolved] = await Promise.all([listProposals(authenticated), listUnresolvedFundingOperations(authenticated)]);
      }
    } catch (error) {
      if (currentWalletSession() === session) {
        // A partially completed signer check is not authorization. If either
        // signer-only follow-up query fails, clear the actor and controls.
        invalidateOperatorAccess();
        authError = error instanceof Error ? error.message : String(error);
      }
    } finally {
      if (currentWalletSession() === session) checkingOperatorAccess = false;
    }
  }

  async function run(action: () => Promise<unknown>): Promise<void> {
    actionMessage = '';
    authError = '';
    try { assertCurrentSigner(); await action(); actionMessage = 'Action accepted by Cycle Sentinel.'; await refresh(); }
    catch (error) { authError = error instanceof Error ? error.message : String(error); }
  }

  function id(value: string, label: string): bigint { return parseNat(value, label); }
  function target(): Principal { return parsePrincipal(targetPrincipal, 'Target principal'); }
  function operation(): bigint { return id(operationId, 'Operation ID'); }
  function proposal(): bigint { return id(proposalId, 'Proposal ID'); }
  function alarmCanBeAcknowledged(alarm: PublicAlarm): boolean { return variant(alarm.status) === 'Open'; }

  onMount(() => {
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
    void refresh();
    return () => {
      unsubscribeWallet();
      unsubscribeWalletType();
      unsubscribeWalletSessionGeneration();
    };
  });
</script>

<svelte:head><title>Cycle Sentinel Telemetry | Rumi</title></svelte:head>
<section class="telemetry-page">
  <header class="hero"><div><p class="eyebrow">RUMI OPERATIONS</p><h1>Cycle Sentinel</h1><p class="lede">Public runtime health and auditable cycle maintenance. Operator controls appear only after the canister confirms signer permission.</p></div><button on:click={refresh} disabled={loading}>Refresh telemetry</button></header>
  {#if publicError}<div class="notice error" role="alert">Public telemetry unavailable: {publicError}</div>{/if}
  {#if authError}<div class="notice error" role="alert">Operator query/action: {authError}</div>{/if}
  {#if actionMessage}<div class="notice success">{actionMessage}</div>{/if}
  {#if !isCycleSentinelConfigured}<div class="notice">Cycle Sentinel is not configured yet. This page remains fail-closed until the authoritative Task 10 deployment.</div>
  {:else if loading}<p class="muted">Loading public telemetry…</p>
  {:else if snapshot}
    <div class="stats"><div><span>Targets</span><strong>{format(snapshot.overview.target_count)}</strong></div><div><span>Healthy</span><strong>{format(snapshot.overview.healthy_count)}</strong></div><div><span>Runtime fuel</span><strong>{formatTCycles(snapshot.overview.runtime_cycles)} T</strong><small>This is Sentinel's own operating fuel.</small></div><div><span>Open alarms</span><strong>{format(snapshot.overview.alarm_count)}</strong></div></div>
    <section class="funding-wallet" aria-labelledby="funding-wallet-heading">
      <div class="funding-heading">
        <div>
          <p class="eyebrow">AUTOMATIC TOP-UPS</p>
          <h2 id="funding-wallet-heading">Sentinel funding wallet</h2>
          <p class="muted">Deposit here to fund target top-ups. This reserve is separate from the <strong>Runtime fuel</strong> number above.</p>
        </div>
        <span class="badge confirmed">Uses cycles first · ICP fallback</span>
      </div>
      {#if funding}
        <div class="funding-balances">
          <article class="funding-balance">
            <div class="balance-title"><h3>Cycles reserve</h3><span class="network">ICRC Cycles Ledger</span></div>
            <div class="balance-grid"><div><span>Ledger balance</span><strong>{formatTCycles(fundingOptionalBigInt(funding.cycles_ledger_balance_cycles))} T</strong><small class:stale={isStale(fundingOptionalBigInt(funding.cycles_ledger_balance_as_of_secs), undefined, 7200n)}>{isStale(fundingOptionalBigInt(funding.cycles_ledger_balance_as_of_secs), undefined, 7200n) ? 'Stale · ' : ''}{ageLabel(fundingOptionalBigInt(funding.cycles_ledger_balance_as_of_secs))}</small></div><div><span>Spendable</span><strong>{formatTCycles(fundingOptionalBigInt(funding.cycles_ledger_available_cycles))} T</strong><small>After protected reserves</small></div><div><span>Protected</span><strong>{formatTCycles(fundingOptionalBigInt(funding.protected_self_reserve_cycles))} T</strong><small>Kept for Sentinel recovery</small></div></div>
            <p class="deposit-copy">Send cycles to the Cycles Ledger using the Sentinel principal as the owner and the default subaccount.</p>
            <div class="address-row"><span class="address-label">Cycles deposit owner</span><code>{fundingOwnerText}</code><button on:click={() => copyLabel('Cycles deposit owner', fundingOwnerText)}>Copy</button></div>
            <div class="address-row"><span class="address-label">Cycles Ledger canister</span><code>{CYCLES_LEDGER_PRINCIPAL}</code><button on:click={() => copyLabel('Cycles Ledger canister', CYCLES_LEDGER_PRINCIPAL)}>Copy</button></div>
            <p class="fine-print">The default subaccount is empty. This account is the funding pool; sending cycles directly to a target canister only changes that target's runtime balance.</p>
          </article>
          <article class="funding-balance">
            <div class="balance-title"><h3>ICP fallback reserve</h3><span class="network">ICP Ledger · mainnet</span></div>
            <div class="balance-grid"><div><span>Ledger balance</span><strong>{formatIcp(fundingOptionalBigInt(funding.icp_ledger_balance_e8s))} ICP</strong><small class:stale={isStale(fundingOptionalBigInt(funding.icp_ledger_balance_as_of_secs), undefined, 180n)}>{isStale(fundingOptionalBigInt(funding.icp_ledger_balance_as_of_secs), undefined, 180n) ? 'Stale · ' : ''}{ageLabel(fundingOptionalBigInt(funding.icp_ledger_balance_as_of_secs))}</small></div><div><span>Spendable</span><strong>{formatIcp(fundingOptionalBigInt(funding.icp_available_e8s))} ICP</strong><small>After reserve policy</small></div><div><span>Minimum reserve</span><strong>{formatIcp(fundingOptionalBigInt(funding.min_icp_reserve_e8s))} ICP</strong><small>Held back from conversion</small></div></div>
            <p class="deposit-copy">Send ICP to the Sentinel owner on the ICP Ledger using the default subaccount. ICRC wallets can use the principal below; legacy wallets can use the 64-character account identifier. Sentinel converts ICP through the NNS Cycles Minting Canister only when spendable cycles cannot cover a refill.</p>
            <div class="address-row"><span class="address-label">ICRC owner principal</span><code>{fundingOwnerText}</code><button on:click={() => copyLabel('ICRC owner principal', fundingOwnerText)}>Copy</button></div>
            <div class="address-row"><span class="address-label">ICP account identifier</span><code>{icpAccountText}</code><button on:click={() => copyLabel('ICP account identifier', icpAccountText)}>Copy</button></div>
            <div class="address-row"><span class="address-label">ICP Ledger canister</span><code>{ICP_LEDGER_PRINCIPAL}</code><button on:click={() => copyLabel('ICP Ledger canister', ICP_LEDGER_PRINCIPAL)}>Copy</button></div>
            <p class="fine-print">The ICRC owner uses the empty/default subaccount. The legacy identifier is the same owner account encoded for older ICP Ledger send forms.</p>
          </article>
        </div>
        <div class="funding-footer"><span>Conversion status: <strong>{variantLabel(funding.shared_reserve_conversion_status)}</strong></span><span>Funding owner: <code>{fundingOwnerText}</code></span><span>Next scheduled check: <strong>{nextCheckLabel(optional(snapshot.overview.next_sample_at_secs))}</strong></span></div>
        {#if fundingEmpty}<p class="notice idle"><strong>Automation is configured but awaiting funds.</strong> Deposit cycles or ICP above; the next scheduled Sentinel check will recognize the reserve. Refreshing this page reads current public state but does not force a funding run.</p>{/if}
        {#if copyMessage}<p class="notice success" role="status">{copyMessage}</p>{/if}
        {#if copyError}<p class="notice error" role="alert">{copyError}</p>{/if}
      {:else}
        <p class="muted">Funding balances are unavailable until the Sentinel publishes its funding status.</p>
      {/if}
    </section>
    {#if registryIdle}
      <div class="notice idle">
        <strong>Observation has not been switched on yet.</strong>
        <p>All {snapshot.overview.target_count.toString()} registered targets are still disabled or set to <em>Unobserved</em>, so the Sentinel has never sampled them{neverSampled ? '' : ' recently'}. That is why balance, burn rate, and runway read <em>Unavailable</em> rather than zero — the values are genuinely unknown, not missing from the page.</p>
        <p class="muted">Registration is deliberately fail-closed: a target is created disabled with auto-top-up off. Turning observation on is a separate governed change — a signer proposes a target update with <em>Enabled</em> checked, a second signer approves it, and it executes after the target-registry timelock.</p>
        {#if fundingUnavailable}<p class="muted">The Sentinel also reports no funding source yet (no cycles-ledger balance and no ICP), so top-ups would be rejected even for an enabled target.</p>{/if}
      </div>
    {/if}
    <div class="grid"><article><h2>Target registry</h2>{#if snapshot.targets.length}<table><thead><tr><th>Target</th><th>State</th><th>Rule</th><th>Balance</th><th>Burn / day</th><th>Runway</th><th>Environment</th><th></th></tr></thead><tbody>{#each snapshot.targets as row}<tr class:selected-row={selectedTargetPrincipal === row.principal.toText()}><td><strong>{row.display_name}</strong><small>{row.principal.toText()}</small><small>{row.project} · {variant(row.observation_mode)}</small></td><td title={targetStateLabel(row) === 'Awaiting first sample' ? 'Monitoring is enabled. The next scheduled observation has not completed yet.' : undefined}><strong>{fundingTarget(row).paused ? 'Paused' : targetStateLabel(row)}</strong><small>{fundingTarget(row).enabled ? 'Enabled' : 'Disabled'} · {fundingTarget(row).auto_topup ? 'Auto-top-up on' : 'Auto-top-up off'}</small></td><td><span class="rule">At {formatTCycles(row.low_balance_threshold_cycles)}T, add {formatTCycles(row.refill_cycles)}T</span><small>Cap {formatTCycles(fundingTarget(row).daily_cap_cycles ?? undefined)}T · cooldown {fundingTarget(row).cooldown_secs?.toString() ?? 'Unavailable'}s</small>{#if row.recent_topups.length}<small>Last top-up: {variant(row.recent_topups[row.recent_topups.length - 1].outcome)} · {formatTCycles(row.recent_topups[row.recent_topups.length - 1].amount_cycles)}T</small>{/if}</td><td title={format(optional(row.advisory_balance_cycles))}>{row.advisory_balance_overflowed ? 'Overflow' : formatCycles(optional(row.advisory_balance_cycles))}</td><td title={format(optional(row.burn_cycles_per_day))}>{formatCycles(optional(row.burn_cycles_per_day))}</td><td>{formatRunway(optional(row.runway_secs))}</td><td>{variant(row.environment)}</td><td><button class="select-target" on:click={() => selectTarget(row)}>Use settings</button></td></tr>{/each}</tbody></table>{:else}<p class="muted">No targets have been published.</p>{/if}</article>
      <article><h2>Alarms</h2>{#if snapshot.alarms.length}{#each snapshot.alarms as alarm}<div class="alarm"><span class="dot"></span><div><strong>{variant(alarm.kind)}</strong><small>{alarm.target[0]?.toText() ?? 'Sentinel'}</small></div><span>{variant(alarm.status)}</span>{#if signer && actor && alarmCanBeAcknowledged(alarm)}<button on:click={() => run(() => sentinelManagement.acknowledgeAlarm(actor!, alarm.id))}>Acknowledge</button>{/if}</div>{/each}{:else}<p class="muted">No public alarms.</p>{/if}</article></div>
  {/if}

  {#if $walletStore.isConnected && isCycleSentinelConfigured}<section class="operator"><h2>Operator console <span class:confirmed={signer} class="badge">{signer ? 'Signer confirmed on-chain' : operatorChecked ? 'Connected, not a signer' : 'Access not checked'}</span></h2>
    {#if signer && actor}
      <article><h3>Register target</h3><p class="muted">Registration is fail-closed: the canister creates the target disabled with auto-top-up off. Enablement is a separate governed update.</p><div class="form-grid"><label>Target principal<input bind:value={targetPrincipal} /></label><label>Display name<input bind:value={displayName} /></label><label>Project<input bind:value={project} /></label><label>Tags (comma separated)<input bind:value={tags} /></label><label>Low threshold (T-cycles)<input bind:value={lowThreshold} inputmode="decimal" /></label><label>Refill amount (T-cycles)<input bind:value={refill} inputmode="decimal" /></label><label>Daily cap (T-cycles)<input bind:value={dailyCap} inputmode="decimal" /></label><label>Cooldown seconds<input bind:value={cooldown} inputmode="numeric" /></label><label>Optional burn anomaly limit (T-cycles)<input bind:value={burnAnomalyLimit} inputmode="decimal" /></label><label>Environment<select bind:value={environment}>{#each Object.keys(EnvironmentVariant) as value}<option value={value}>{value}</option>{/each}</select></label><label>Criticality<select bind:value={criticality}>{#each Object.keys(CriticalityVariant) as value}<option value={value}>{value}</option>{/each}</select></label><label>Observation mode<select bind:value={observationMode}>{#each Object.keys(ObservationModeVariant) as value}<option value={value}>{value}</option>{/each}</select></label></div><p class="fine-print">For example: threshold 2, refill 6 means “At 2T, add 6T.” Values are converted to exact cycle integers before submission.</p><button on:click={() => run(() => sentinelManagement.proposeRegisterTarget(actor!, targetArgs()))}>Propose register target</button></article>
      <article><h3>Update or remove target</h3><p class="muted">Update includes funding policy, enabled, and auto-top-up choices. Removal is governed and remains fail-closed while unresolved operations exist.</p><div class="form-grid"><label>Target principal<input bind:value={targetPrincipal} /></label><label>Display name<input bind:value={displayName} /></label><label>Project<input bind:value={project} /></label><label>Tags<input bind:value={tags} /></label><label>Low threshold (T-cycles)<input bind:value={lowThreshold} inputmode="decimal" /></label><label>Refill amount (T-cycles)<input bind:value={refill} inputmode="decimal" /></label><label>Daily cap (T-cycles)<input bind:value={dailyCap} inputmode="decimal" /></label><label>Cooldown seconds<input bind:value={cooldown} inputmode="numeric" /></label><label>Burn anomaly limit (T-cycles)<input bind:value={burnAnomalyLimit} inputmode="decimal" /></label><label>Environment<select bind:value={environment}>{#each Object.keys(EnvironmentVariant) as value}<option value={value}>{value}</option>{/each}</select></label><label>Criticality<select bind:value={criticality}>{#each Object.keys(CriticalityVariant) as value}<option value={value}>{value}</option>{/each}</select></label><label>Observation mode<select bind:value={observationMode}>{#each Object.keys(ObservationModeVariant) as value}<option value={value}>{value}</option>{/each}</select></label><label class="check"><input type="checkbox" bind:checked={enabled} /> Enabled</label><label class="check"><input type="checkbox" bind:checked={autoTopup} /> Auto-top-up</label></div><p class="fine-print">Use “Use settings” in the registry to load the selected target’s current threshold, refill, cap, cooldown, and switches before editing.</p><div class="actions"><button on:click={() => run(() => sentinelManagement.proposeUpdateTarget(actor!, target(), targetPatch()))}>Propose target update</button><button on:click={() => run(() => sentinelManagement.proposeRemoveTarget(actor!, target()))}>Propose target removal</button><button on:click={() => run(() => sentinelManagement.pauseTarget(actor!, target()))}>Pause target immediately</button><button on:click={() => run(() => sentinelManagement.proposeUnpauseTarget(actor!, target()))}>Propose governed unpause</button><button on:click={() => run(() => sentinelManagement.manualTopUp(actor!, target()))}>Manual top-up</button></div></article>
      <article><h3>Signer governance</h3><div class="form-grid"><label>Signer principal<input bind:value={signerPrincipal} /></label><label>Signer threshold (nat32)<input bind:value={signerThreshold} inputmode="numeric" /></label></div><div class="actions"><button on:click={() => run(() => sentinelManagement.proposeAddSigner(actor!, parsePrincipal(signerPrincipal, 'Signer principal')))}>Propose add signer</button><button on:click={() => run(() => sentinelManagement.proposeRemoveSigner(actor!, parsePrincipal(signerPrincipal, 'Signer principal')))}>Propose remove signer</button><button on:click={() => run(() => sentinelManagement.proposeSetSignerThreshold(actor!, parseNat32(signerThreshold, 'Signer threshold')))}>Propose signer threshold</button></div></article>
      <article><h3>Global policy governance</h3><div class="form-grid"><label>Global daily cap<input bind:value={globalDailyCap} inputmode="numeric" /></label><label>Sample interval seconds<input bind:value={sampleInterval} inputmode="numeric" /></label><label>Stale-after seconds<input bind:value={staleAfter} inputmode="numeric" /></label><label>Minimum ICP reserve e8s<input bind:value={minIcpReserve} inputmode="numeric" /></label><label>Self-recovery refill<input bind:value={selfRefill} inputmode="numeric" /></label><label>Self-recovery low threshold<input bind:value={selfLowThreshold} inputmode="numeric" /></label><label>Self-recovery daily cap<input bind:value={selfDailyCap} inputmode="numeric" /></label><label>Protected reserve cycles<input bind:value={protectedReserve} inputmode="numeric" /></label><label>Unpause timelock seconds<input bind:value={unpauseTimelock} inputmode="numeric" /></label><label>Spend-policy timelock seconds<input bind:value={spendTimelock} inputmode="numeric" /></label><label>Target-registry timelock seconds<input bind:value={targetTimelock} inputmode="numeric" /></label><label>Signer-change timelock seconds<input bind:value={signerTimelock} inputmode="numeric" /></label></div><button on:click={() => run(() => sentinelManagement.proposeSetGlobalPolicy(actor!, globalPolicy()))}>Propose global policy</button></article>
      <article><h3>Proposal list and actions</h3><label>Proposal ID<input bind:value={proposalId} inputmode="numeric" /></label><div class="actions"><button on:click={() => run(() => sentinelManagement.approveProposal(actor!, proposal()))}>Approve proposal</button><button on:click={() => run(() => sentinelManagement.executeProposal(actor!, proposal()))}>Execute proposal</button><button on:click={() => run(() => sentinelManagement.cancelProposal(actor!, proposal()))}>Cancel proposal</button></div>{#if proposals.length}<ul>{#each proposals as item}<li>#{item.id.toString()} · {variant(item.status)} · {variant(item.payload)} · {item.approvals.length} approval(s)</li>{/each}</ul>{:else}<p class="muted">No proposals returned.</p>{/if}</article>
      <article><h3>Unresolved funding operations</h3><div class="form-grid"><label>Operation ID<input bind:value={operationId} inputmode="numeric" /></label><label>Ledger block index<input bind:value={blockIndex} inputmode="numeric" /></label></div><div class="actions"><button on:click={() => run(() => sentinelManagement.attachBlockProof(actor!, operation(), id(blockIndex, 'Block index')))}>Attach delivery proof</button><button on:click={() => run(() => sentinelManagement.attachRefundBlockProof(actor!, operation(), id(blockIndex, 'Block index')))}>Attach refund proof</button><button on:click={() => run(() => sentinelManagement.resolveUnknownAsSpent(actor!, operation()) )}>Resolve unknown as spent</button></div>{#if unresolved.length}<ul>{#each unresolved as item}<li>#{item.id.toString()} · {variant(item.state)} · target {item.target.toText()} · reserved {item.reserved_amount_cycles.toString()} cycles</li>{/each}</ul>{:else}<p class="muted">No unresolved funding operations returned.</p>{/if}</article>
    {:else}<p class="muted">{operatorChecked ? 'This wallet is authenticated but is not a configured Sentinel signer. Public telemetry remains available.' : 'Checking operator access asks your wallet to approve a read-only signer-permission query. It never changes Sentinel policy or moves cycles.'}</p><button on:click={checkOperatorAccess} disabled={checkingOperatorAccess}>{checkingOperatorAccess ? 'Checking operator access…' : 'Check operator access'}</button>{/if}
  </section>{:else}<div class="login-note">Connect a wallet to check signer permissions and access operator controls.</div>{/if}
</section>

<style>
  .telemetry-page{max-width:1120px;margin:0 auto;color:var(--rumi-text-primary)}.hero{display:flex;align-items:flex-end;justify-content:space-between;gap:1rem;flex-wrap:wrap;margin-bottom:2rem}.hero h1{margin:.1rem 0;font-size:2.5rem}.lede{max-width:700px;color:var(--rumi-text-secondary)}.eyebrow{color:var(--rumi-teal);font-size:.72rem;letter-spacing:.14em}.stats{display:grid;grid-template-columns:repeat(4,1fr);gap:.75rem;margin-bottom:1rem}.stats div,article,.operator,.login-note{padding:1rem;background:var(--rumi-bg-surface-1);border:1px solid var(--rumi-border);border-radius:.5rem}.stats span,.muted,small{display:block;color:var(--rumi-text-muted);font-size:.78rem}.stats strong{font-size:1.25rem}.grid{display:grid;grid-template-columns:2fr 1fr;gap:1rem}h2,h3{margin-top:0}table{width:100%;border-collapse:collapse}th,td{text-align:left;padding:.55rem;border-bottom:1px solid var(--rumi-border);font-size:.82rem;vertical-align:top}.alarm{display:flex;gap:.5rem;align-items:center;border-bottom:1px solid var(--rumi-border);padding:.65rem 0}.alarm>span:nth-last-of-type(1){margin-left:auto;font-size:.75rem}.dot{width:.45rem;height:.45rem;background:#e05252;border-radius:50%}.operator{margin-top:1rem;display:grid;gap:1rem}.operator>h2,.operator>p{margin-bottom:0}.badge{font-size:.7rem;padding:.25rem .5rem;border-radius:99px;color:var(--rumi-text-muted);background:var(--rumi-bg-surface3)}.badge.confirmed{color:var(--rumi-teal);background:rgba(45,212,191,.12)}.form-grid{display:grid;grid-template-columns:repeat(3,1fr);gap:.7rem}.form-grid label{display:block;font-size:.75rem;color:var(--rumi-text-muted)}input,select{display:block;width:100%;box-sizing:border-box;margin-top:.25rem;padding:.5rem;background:var(--rumi-bg-surface3);border:1px solid var(--rumi-border);color:inherit;border-radius:.3rem}.check{display:flex!important;gap:.5rem;align-items:center}.check input{width:auto}.actions{display:flex;gap:.4rem;flex-wrap:wrap;margin-top:.7rem}button{border:1px solid var(--rumi-border-hover);background:var(--rumi-bg-surface3);color:inherit;border-radius:.4rem;padding:.55rem .8rem;cursor:pointer;font-size:.78rem}button:hover{border-color:var(--rumi-action)}button:disabled{opacity:.5}.notice{padding:.7rem;margin-bottom:1rem;border-radius:.4rem}.error{color:#ff9b9b;background:rgba(224,82,82,.12)}.idle{background:var(--rumi-bg-surface-1);border:1px solid var(--rumi-border)}.idle strong{display:block;margin-bottom:.35rem}.idle p{margin:.35rem 0;font-size:.82rem;color:var(--rumi-text-secondary)}.idle p.muted{font-size:.78rem}.success{color:var(--rumi-teal);background:rgba(45,212,191,.1)}.login-note{margin-top:1rem}li{margin:.4rem 0;font-size:.82rem}.funding-wallet{padding:1.2rem;background:linear-gradient(135deg,rgba(31,48,78,.75),rgba(19,29,52,.95));border:1px solid rgba(45,212,191,.35);border-radius:.65rem;margin-bottom:1rem}.funding-heading{display:flex;justify-content:space-between;gap:1rem;align-items:flex-start;margin-bottom:1rem}.funding-heading h2{margin:.1rem 0 .35rem}.funding-heading p{margin:0}.funding-balances{display:grid;grid-template-columns:1fr 1fr;gap:.8rem}.funding-balance{padding:1rem;background:rgba(8,15,30,.35);border-color:rgba(255,255,255,.12)}.balance-title{display:flex;align-items:baseline;justify-content:space-between;gap:.5rem}.balance-title h3{margin:0}.network{font-size:.7rem;color:var(--rumi-teal)}.balance-grid{display:grid;grid-template-columns:repeat(3,1fr);gap:.5rem;margin:1rem 0}.balance-grid>div{padding:.6rem;background:rgba(255,255,255,.04);border-radius:.35rem}.balance-grid strong{display:block;font-size:1.05rem;margin-top:.2rem}.deposit-copy{color:var(--rumi-text-secondary);font-size:.82rem;line-height:1.45}.address-row{display:grid;grid-template-columns:9.5rem minmax(0,1fr) auto;align-items:center;gap:.5rem;margin-top:.5rem}.address-label{font-size:.72rem;color:var(--rumi-text-muted)}code{font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:.73rem;overflow-wrap:anywhere;user-select:all}.address-row code{padding:.45rem;background:rgba(0,0,0,.22);border-radius:.25rem}.address-row button{padding:.4rem .55rem}.fine-print{font-size:.72rem;color:var(--rumi-text-muted);line-height:1.4}.funding-footer{display:flex;justify-content:space-between;gap:1rem;flex-wrap:wrap;margin-top:.8rem;color:var(--rumi-text-muted);font-size:.75rem}.rule{display:block;white-space:nowrap}.selected-row{background:rgba(45,212,191,.07)}.select-target{padding:.4rem .5rem;white-space:nowrap}@media(max-width:768px){.stats{grid-template-columns:repeat(2,1fr)}.grid,.form-grid,.funding-balances{grid-template-columns:1fr}.balance-grid{grid-template-columns:1fr 1fr}.address-row{grid-template-columns:1fr}.address-row button{justify-self:start}table{font-size:.72rem}}
</style>
