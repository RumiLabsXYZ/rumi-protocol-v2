<script lang="ts">
  import { page } from '$app/stores';
  import { onMount } from 'svelte';
  import StatCard from '$components/explorer/StatCard.svelte';
  import EntityLink from '$components/explorer/EntityLink.svelte';
  import CopyButton from '$components/explorer/CopyButton.svelte';
  import StatusBadge from '$components/explorer/StatusBadge.svelte';
  import VaultHealthBar from '$components/explorer/VaultHealthBar.svelte';
  import EventRow from '$components/explorer/EventRow.svelte';
  import AmountDisplay from '$components/explorer/AmountDisplay.svelte';
  import TimeAgo from '$components/explorer/TimeAgo.svelte';
  import {
    fetchVault, fetchVaultInterestRate, fetchCollateralConfigs,
    fetchCollateralPrices, fetchVaultHistory
  } from '$services/explorer/explorerService';
  import {
    formatE8s, formatUsd, formatUsdRaw, formatCR, formatPercent, formatTokenAmount,
    getTokenSymbol, getTokenDecimals, classifyVaultHealth, healthColor, healthBg
  } from '$utils/explorerHelpers';

  // ── Route param ────────────────────────────────────────────────────────────
  const vaultId = $derived(Number($page.params.id));

  // ── State ──────────────────────────────────────────────────────────────────
  let vault = $state<any>(null);
  let interestRate = $state<number | null>(null);
  let collateralConfigs = $state<any[]>([]);
  let collateralPrices = $state<Map<string, number>>(new Map());
  let history = $state<any[]>([]);

  let loadingVault = $state(true);
  let loadingRate = $state(true);
  let loadingConfigs = $state(true);
  let loadingPrices = $state(true);
  let loadingHistory = $state(true);
  let vaultError = $state(false);
  let newestFirst = $state(true);

  // ── Derived: collateral config for this vault ──────────────────────────────
  const collateralPrincipalStr = $derived(
    vault?.collateral_type ? (vault.collateral_type.toString?.() ?? vault.collateral_type.toText?.() ?? String(vault.collateral_type)) : ''
  );

  const config = $derived.by(() => {
    if (!vault || collateralConfigs.length === 0) return null;
    return collateralConfigs.find((c: any) => {
      const cPrincipal = c.collateral_type?.toString?.() ?? c.collateral_type?.toText?.() ?? String(c.collateral_type ?? '');
      return cPrincipal === collateralPrincipalStr;
    }) ?? null;
  });

  const decimals = $derived(
    collateralPrincipalStr ? getTokenDecimals(collateralPrincipalStr) : 8
  );

  const tokenSymbol = $derived(
    collateralPrincipalStr ? getTokenSymbol(collateralPrincipalStr) : 'tokens'
  );

  // ── Derived: price ─────────────────────────────────────────────────────────
  const price = $derived.by(() => {
    if (!vault || collateralPrices.size === 0) return 0;
    return collateralPrices.get(collateralPrincipalStr) ?? 0;
  });

  // ── Derived: vault amounts ─────────────────────────────────────────────────
  const ownerStr = $derived(vault?.owner?.toString?.() ?? vault?.owner?.toText?.() ?? '');

  const collateralAmount = $derived(vault ? BigInt(vault.collateral_amount ?? 0) : 0n);
  const collateralHuman = $derived(Number(collateralAmount) / 10 ** decimals);
  const collateralValueUsd = $derived(collateralHuman * price);

  const borrowedAmount = $derived(vault ? BigInt(vault.borrowed_icusd_amount ?? 0) : 0n);
  const accruedInterest = $derived(vault ? BigInt(vault.accrued_interest ?? 0) : 0n);
  const totalDebt = $derived(borrowedAmount + accruedInterest);
  const totalDebtHuman = $derived(Number(totalDebt) / 1e8);

  const isActive = $derived(collateralAmount > 0n);

  // ── Derived: collateral ratio (as ratio, e.g. 1.5 = 150%) ─────────────────
  const cr = $derived(totalDebtHuman > 0 ? collateralValueUsd / totalDebtHuman : Infinity);
  const crPct = $derived(cr === Infinity ? Infinity : cr * 100);

  // ── Derived: config values ─────────────────────────────────────────────────
  const minCR = $derived(config?.min_collateral_ratio ? Number(config.min_collateral_ratio) : 1.5);
  const liquidationRatio = $derived(config?.liquidation_threshold ? Number(config.liquidation_threshold) : 1.1);
  const liquidationBonus = $derived(config?.liquidation_bonus ? Number(config.liquidation_bonus) : 0.1);
  const borrowingFee = $derived(config?.borrowing_fee ? Number(config.borrowing_fee) : 0);
  const debtCeiling = $derived(config?.debt_ceiling ? BigInt(config.debt_ceiling) : null);
  const minVaultDebt = $derived(config?.min_vault_debt ? BigInt(config.min_vault_debt) : null);
  const ledgerFee = $derived(config?.transfer_fee ? BigInt(config.transfer_fee) : null);

  // ── Derived: liquidation price ─────────────────────────────────────────────
  const liquidationPrice = $derived.by(() => {
    if (collateralHuman <= 0 || totalDebtHuman <= 0) return 0;
    return (totalDebtHuman * liquidationRatio) / collateralHuman;
  });

  // ── Derived: health classification ─────────────────────────────────────────
  const health = $derived(cr !== Infinity ? classifyVaultHealth(cr, liquidationRatio) : 'healthy' as const);

  // ── Derived: vault status ──────────────────────────────────────────────────
  const vaultStatus = $derived.by(() => {
    if (!vault) return 'Active';
    if (collateralAmount === 0n && totalDebt === 0n) return 'Closed';
    return 'Active';
  });

  // ── Derived: vault collateral map for EventRow ─────────────────────────────
  const vaultCollateralMap = $derived(
    vault ? new Map([[Number(vault.vault_id), vault.collateral_type]]) : new Map<number, any>()
  );

  // ── Derived: sorted history ────────────────────────────────────────────────
  const sortedHistory = $derived(
    newestFirst ? [...history].reverse() : [...history]
  );

  // ── Load ───────────────────────────────────────────────────────────────────
  onMount(async () => {
    const id = BigInt(vaultId);

    const vaultPromise = fetchVault(id).then(v => {
      vault = v;
      if (!v) vaultError = true;
      loadingVault = false;
    }).catch(() => { vaultError = true; loadingVault = false; });

    const ratePromise = fetchVaultInterestRate(id).then(r => {
      interestRate = r;
      loadingRate = false;
    }).catch(() => { loadingRate = false; });

    const configsPromise = fetchCollateralConfigs().then(c => {
      collateralConfigs = c;
      loadingConfigs = false;
    }).catch(() => { loadingConfigs = false; });

    const pricesPromise = fetchCollateralPrices().then(p => {
      collateralPrices = p;
      loadingPrices = false;
    }).catch(() => { loadingPrices = false; });

    const historyPromise = fetchVaultHistory(id).then(h => {
      history = Array.isArray(h) ? h : [];
      loadingHistory = false;
    }).catch(() => { loadingHistory = false; });

    await Promise.all([vaultPromise, ratePromise, configsPromise, pricesPromise, historyPromise]);
  });
</script>

<svelte:head>
  <title>Vault #{vaultId} | Rumi Explorer</title>
</svelte:head>

<div class="max-w-5xl mx-auto px-4 py-8 space-y-8">
  <!-- Back nav -->
  <a href="/explorer" class="inline-flex items-center gap-1.5 text-sm text-blue-400 hover:text-blue-300 transition-colors">
    <svg class="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2">
      <path stroke-linecap="round" stroke-linejoin="round" d="M15 19l-7-7 7-7" />
    </svg>
    Back to Explorer
  </a>

  <!-- Loading / Error states -->
  {#if loadingVault}
    <div class="text-center py-16 text-gray-400">
      <div class="inline-block w-6 h-6 border-2 border-gray-500 border-t-blue-400 rounded-full animate-spin mb-3"></div>
      <p>Loading Vault #{vaultId}...</p>
    </div>
  {:else if vaultError || !vault}
    <div class="text-center py-16">
      <p class="text-2xl font-bold text-gray-300 mb-2">Vault Not Found</p>
      <p class="text-gray-500">Vault #{vaultId} does not exist or could not be loaded.</p>
      <a href="/explorer" class="inline-block mt-4 text-blue-400 hover:underline text-sm">Return to Explorer</a>
    </div>
  {:else}
    <!-- ── Header ──────────────────────────────────────────────────────── -->
    <div class="space-y-4">
      <div class="flex flex-wrap items-center gap-3">
        <h1 class="text-2xl sm:text-3xl font-bold text-white">Vault #{vaultId}</h1>
        <StatusBadge status={vaultStatus === 'Closed' ? 'paused' : 'active'} size="md" />
        <span class="inline-flex items-center gap-1.5 px-3 py-1 rounded-full bg-gray-800/60 border border-gray-700/50 text-sm">
          <EntityLink type="token" value={collateralPrincipalStr} label={tokenSymbol} />
        </span>
      </div>

      <div class="flex flex-wrap items-center gap-x-6 gap-y-2 text-sm">
        <div class="flex items-center gap-2">
          <span class="text-gray-500">Owner:</span>
          <EntityLink type="address" value={ownerStr} />
          <CopyButton text={ownerStr} />
        </div>
        <div class="flex items-center gap-2">
          <span class="text-gray-500">Created:</span>
          {#if vault.creation_timestamp}
            <TimeAgo timestamp={vault.creation_timestamp} showFull={true} />
          {:else}
            <span class="text-gray-400 text-sm">--</span>
          {/if}
        </div>
      </div>
    </div>

    <!-- ── Stats Cards ─────────────────────────────────────────────────── -->
    <div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-5 gap-4">
      <StatCard
        label="Collateral"
        value="{formatE8s(collateralAmount, decimals)} {tokenSymbol}"
        subtitle={price > 0 ? formatUsdRaw(collateralValueUsd) : undefined}
      />

      <StatCard
        label="Total Debt"
        value="{formatE8s(totalDebt, 8)} icUSD"
        subtitle={accruedInterest > 0n ? `incl. ${formatE8s(accruedInterest, 8)} interest` : undefined}
      />

      <StatCard
        label="Collateral Ratio"
        value={crPct === Infinity ? '--' : `${crPct.toFixed(1)}%`}
        subtitle={`Liq. at ${(liquidationRatio * 100).toFixed(0)}%`}
      />

      {#if !loadingRate}
        <StatCard
          label="Interest Rate"
          value={interestRate !== null ? formatPercent(interestRate) : '--'}
          subtitle="APR"
        />
      {:else}
        <StatCard label="Interest Rate" value="..." subtitle="Loading" />
      {/if}

      <StatCard
        label="Liquidation Price"
        value={liquidationPrice > 0 ? formatUsdRaw(liquidationPrice) : '--'}
        subtitle={liquidationPrice > 0 && price > 0
          ? `Current: ${formatUsdRaw(price)}`
          : undefined}
        trend={liquidationPrice > 0 && price > 0 && price < liquidationPrice * 1.2 ? 'down' : undefined}
      />
    </div>

    <!-- ── Health Bar ──────────────────────────────────────────────────── -->
    <div class="bg-gray-800/50 border border-gray-700/50 rounded-xl p-5 space-y-2">
      <h2 class="text-sm font-semibold text-gray-400 uppercase tracking-wide">Vault Health</h2>
      {#if crPct !== Infinity && crPct > 0}
        <VaultHealthBar collateralRatio={crPct} liquidationRatio={liquidationRatio * 100} />
      {:else}
        <p class="text-gray-500 text-sm">No debt -- vault is fully collateralized.</p>
      {/if}
    </div>

    <!-- ── Collateral Configuration ────────────────────────────────────── -->
    <div class="bg-gray-800/50 border border-gray-700/50 rounded-xl overflow-hidden">
      <div class="px-5 py-4 border-b border-gray-700/50">
        <h2 class="text-sm font-semibold text-gray-400 uppercase tracking-wide">Collateral Configuration</h2>
      </div>
      {#if loadingConfigs}
        <div class="px-5 py-8 text-center text-gray-500">Loading configuration...</div>
      {:else if config}
        <table class="w-full text-sm">
          <tbody>
            <tr class="border-b border-gray-700/30">
              <td class="px-5 py-3 text-gray-400">Min Collateral Ratio</td>
              <td class="px-5 py-3 text-white font-mono text-right">{(minCR * 100).toFixed(0)}%</td>
            </tr>
            <tr class="border-b border-gray-700/30">
              <td class="px-5 py-3 text-gray-400">Liquidation Ratio</td>
              <td class="px-5 py-3 text-white font-mono text-right">{(liquidationRatio * 100).toFixed(0)}%</td>
            </tr>
            <tr class="border-b border-gray-700/30">
              <td class="px-5 py-3 text-gray-400">Liquidation Bonus</td>
              <td class="px-5 py-3 text-white font-mono text-right">{formatPercent(liquidationBonus)}</td>
            </tr>
            <tr class="border-b border-gray-700/30">
              <td class="px-5 py-3 text-gray-400">Borrowing Fee</td>
              <td class="px-5 py-3 text-white font-mono text-right">{formatPercent(borrowingFee)}</td>
            </tr>
            <tr class="border-b border-gray-700/30">
              <td class="px-5 py-3 text-gray-400">Interest Rate APR</td>
              <td class="px-5 py-3 text-white font-mono text-right">
                {interestRate !== null ? formatPercent(interestRate) : '--'}
              </td>
            </tr>
            {#if debtCeiling !== null}
              <tr class="border-b border-gray-700/30">
                <td class="px-5 py-3 text-gray-400">Debt Ceiling</td>
                <td class="px-5 py-3 text-white font-mono text-right">{formatE8s(debtCeiling, 8)} icUSD</td>
              </tr>
            {/if}
            {#if minVaultDebt !== null}
              <tr class="border-b border-gray-700/30">
                <td class="px-5 py-3 text-gray-400">Min Vault Debt</td>
                <td class="px-5 py-3 text-white font-mono text-right">{formatE8s(minVaultDebt, 8)} icUSD</td>
              </tr>
            {/if}
            {#if ledgerFee !== null}
              <tr>
                <td class="px-5 py-3 text-gray-400">Ledger Fee</td>
                <td class="px-5 py-3 text-white font-mono text-right">{formatE8s(ledgerFee, decimals)} {tokenSymbol}</td>
              </tr>
            {/if}
          </tbody>
        </table>
      {:else}
        <div class="px-5 py-8 text-center text-gray-500">Configuration not available.</div>
      {/if}
    </div>

    <!-- ── Vault History ───────────────────────────────────────────────── -->
    <div class="bg-gray-800/50 border border-gray-700/50 rounded-xl overflow-hidden">
      <div class="px-5 py-4 border-b border-gray-700/50 flex items-center justify-between">
        <h2 class="text-sm font-semibold text-gray-400 uppercase tracking-wide">
          Event History ({history.length})
        </h2>
        {#if history.length > 1}
          <button
            onclick={() => newestFirst = !newestFirst}
            class="text-xs text-gray-400 hover:text-gray-200 transition-colors px-2 py-1 rounded border border-gray-700/50 hover:border-gray-600"
          >
            {newestFirst ? 'Oldest first' : 'Newest first'}
          </button>
        {/if}
      </div>
      {#if loadingHistory}
        <div class="px-5 py-8 text-center text-gray-500">Loading history...</div>
      {:else if sortedHistory.length === 0}
        <div class="px-5 py-8 text-center text-gray-500">No events found for this vault.</div>
      {:else}
        <div class="divide-y divide-gray-700/30">
          {#each sortedHistory as evt, i}
            <EventRow
              event={evt.event ?? evt}
              index={evt.globalIndex ?? null}
              vaultCollateralMap={vaultCollateralMap}
            />
          {/each}
        </div>
      {/if}
    </div>
  {/if}
</div>
