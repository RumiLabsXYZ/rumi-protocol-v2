<script lang="ts">
  import { page } from '$app/stores';
  import { onMount } from 'svelte';
  import StatCard from '$components/explorer/StatCard.svelte';
  import EntityLink from '$components/explorer/EntityLink.svelte';
  import CopyButton from '$components/explorer/CopyButton.svelte';
  import StatusBadge from '$components/explorer/StatusBadge.svelte';
  import EventRow from '$components/explorer/EventRow.svelte';
  import TokenBadge from '$components/explorer/TokenBadge.svelte';
  import {
    fetchCollateralConfigs, fetchAllVaults, fetchCollateralPrices,
    fetchCollateralTotals, fetchEvents
  } from '$services/explorer/explorerService';
  import {
    formatE8s, formatUsdRaw, formatCR, formatPercent,
    getTokenSymbol, getTokenDecimals
  } from '$utils/explorerHelpers';
  import { decodeRustDecimal } from '$utils/decimalUtils';

  // ── State ──────────────────────────────────────────────────────────────
  let loading = $state(true);
  let error = $state<string | null>(null);
  let rawConfig = $state<any>(null);
  let allVaults = $state<any[]>([]);
  let prices = $state<Map<string, number>>(new Map());
  let totals = $state<any[]>([]);
  let events = $state<[bigint, any][]>([]);

  // ── Derived: token identity ────────────────────────────────────────────
  const tokenId = $derived($page.params.id);
  const symbol = $derived(getTokenSymbol(tokenId));
  const decimals = $derived(rawConfig ? Number(rawConfig.decimals) : getTokenDecimals(tokenId));

  // ── Derived: decoded config fields ─────────────────────────────────────
  const configStatus = $derived.by(() => {
    if (!rawConfig?.status) return 'Unknown';
    const s = rawConfig.status;
    if (typeof s === 'object') {
      if ('Active' in s) return 'Active';
      if ('Paused' in s) return 'Paused';
      if ('Frozen' in s) return 'Frozen';
      if ('Sunset' in s) return 'Sunset';
      if ('Deprecated' in s) return 'Deprecated';
    }
    return 'Unknown';
  });

  const liquidationRatio = $derived(rawConfig ? decodeRustDecimal(rawConfig.liquidation_ratio) : 0);
  const borrowThreshold = $derived(rawConfig ? decodeRustDecimal(rawConfig.borrow_threshold_ratio) : 0);
  const borrowingFee = $derived(rawConfig ? decodeRustDecimal(rawConfig.borrowing_fee) : 0);
  const liquidationBonus = $derived(rawConfig ? decodeRustDecimal(rawConfig.liquidation_bonus) : 0);
  const recoveryTargetCr = $derived(rawConfig ? decodeRustDecimal(rawConfig.recovery_target_cr) : 0);
  const interestRateApr = $derived(rawConfig?.interest_rate_apr ? decodeRustDecimal(rawConfig.interest_rate_apr) : 0);
  const debtCeiling = $derived(rawConfig ? Number(rawConfig.debt_ceiling) : 0);
  const minVaultDebt = $derived(rawConfig ? Number(rawConfig.min_vault_debt) : 0);
  const ledgerFee = $derived(rawConfig ? Number(rawConfig.ledger_fee) : 0);
  const ledgerCanisterId = $derived(rawConfig?.ledger_canister_id?.toText?.() ?? '');

  // ── Derived: current price ─────────────────────────────────────────────
  const currentPrice = $derived.by(() => {
    // Try from prices map first
    const price = prices.get(tokenId);
    if (price !== undefined) return price;
    // Fallback to config's last_price
    if (rawConfig?.last_price?.length > 0) return Number(rawConfig.last_price[0]);
    return 0;
  });

  // ── Derived: vaults ────────────────────────────────────────────────────
  const tokenVaults = $derived(
    allVaults.filter((v: any) => {
      const ct = v.collateral_type?.toText?.() ?? v.collateral_type?.toString?.() ?? String(v.collateral_type);
      return ct === tokenId;
    })
  );

  const activeVaults = $derived(
    tokenVaults.filter((v: any) => {
      if (!v.status) return true;
      const key = Object.keys(v.status)[0];
      return key !== 'Closed' && key !== 'closed' && key !== 'Liquidated' && key !== 'liquidated';
    })
  );

  const displayVaults = $derived(tokenVaults.slice(0, 50));
  const hasMoreVaults = $derived(tokenVaults.length > 50);

  // ── Derived: aggregated stats ──────────────────────────────────────────
  const totalCollateralRaw = $derived(
    tokenVaults.reduce((sum: number, v: any) => sum + Number(v.collateral_amount), 0)
  );
  const totalCollateralHuman = $derived(totalCollateralRaw / 10 ** decimals);
  const totalCollateralUsd = $derived(totalCollateralHuman * currentPrice);

  const totalDebtRaw = $derived(
    tokenVaults.reduce((sum: number, v: any) => sum + Number(v.borrowed_icusd_amount), 0)
  );
  const totalDebtHuman = $derived(totalDebtRaw / 1e8);

  const debtCeilingHuman = $derived(debtCeiling / 1e8);
  const debtUtilizationPct = $derived(
    debtCeilingHuman > 0 ? (totalDebtHuman / debtCeilingHuman) * 100 : 0
  );

  // ── Derived: vault CR computation ──────────────────────────────────────
  function computeVaultCR(vault: any): number {
    const col = Number(vault.collateral_amount) / 10 ** decimals;
    const debt = Number(vault.borrowed_icusd_amount) / 1e8;
    if (debt <= 0) return Infinity;
    return (col * currentPrice) / debt;
  }

  function vaultHealthLabel(cr: number): string {
    if (cr <= liquidationRatio) return 'Liquidatable';
    if (cr <= liquidationRatio + 0.25) return 'Danger';
    if (cr <= liquidationRatio + 0.50) return 'Caution';
    return 'Healthy';
  }

  // ── Derived: collateral totals for this token ──────────────────────────
  const tokenTotals = $derived.by(() => {
    return totals.find((t: any) => {
      const ct = t.collateral_type?.toText?.() ?? t.collateral_type?.toString?.() ?? '';
      return ct === tokenId;
    });
  });

  // ── Derived: events filtered to this token ─────────────────────────────
  const tokenEvents = $derived(
    events.filter(([_idx, event]) => {
      const key = Object.keys(event)[0];
      const data = event[key];
      if (!data) return false;
      const ct = data.collateral_type ?? data.vault?.collateral_type;
      if (!ct) return false;
      const ctStr = ct?.toText?.() ?? ct?.toString?.() ?? String(ct);
      return ctStr === tokenId;
    })
  );

  const eventsSorted = $derived(
    [...tokenEvents].sort((a, b) => Number(b[0]) - Number(a[0]))
  );

  // Build vault collateral map for EventRow
  const vaultCollateralMap = $derived.by(() => {
    const map = new Map<number, any>();
    for (const v of tokenVaults) {
      map.set(Number(v.vault_id), v.collateral_type);
    }
    return map;
  });

  // ── Config display rows ────────────────────────────────────────────────
  const configRows = $derived.by(() => {
    if (!rawConfig) return [];
    return [
      { label: 'Liquidation Ratio', value: formatCR(liquidationRatio) },
      { label: 'Borrow Threshold Ratio', value: formatCR(borrowThreshold) },
      { label: 'Liquidation Bonus', value: formatPercent(liquidationBonus) },
      { label: 'Borrowing Fee (one-time)', value: borrowingFee > 0 ? formatPercent(borrowingFee, 3) : '0%' },
      { label: 'Interest Rate APR', value: interestRateApr > 0 ? formatPercent(interestRateApr) : '0%' },
      { label: 'Debt Ceiling', value: debtCeilingHuman > 0 ? `${debtCeilingHuman.toLocaleString('en-US', { minimumFractionDigits: 2 })} icUSD` : 'Unlimited' },
      { label: 'Min Vault Debt', value: minVaultDebt > 0 ? `${(minVaultDebt / 1e8).toLocaleString('en-US', { minimumFractionDigits: 2 })} icUSD` : '--' },
      { label: 'Recovery Target CR', value: recoveryTargetCr > 0 ? formatCR(recoveryTargetCr) : '--' },
      { label: 'Healthy CR', value: borrowThreshold > 0 ? formatCR(borrowThreshold) : '--' },
      { label: 'Ledger Fee', value: ledgerFee > 0 ? `${formatE8s(ledgerFee, decimals)} ${symbol}` : '--' },
      { label: 'Decimals', value: String(decimals) },
      { label: 'Price Source', value: rawConfig.price_source ? describePriceSource(rawConfig.price_source) : 'XRC' },
    ];
  });

  function describePriceSource(src: any): string {
    if (!src || typeof src !== 'object') return 'XRC';
    if ('Xrc' in src) return 'XRC';
    if ('Fixed' in src) return 'Fixed';
    if ('Manual' in src) return 'Manual';
    return Object.keys(src)[0] ?? 'Unknown';
  }

  // ── Data fetch ─────────────────────────────────────────────────────────
  onMount(async () => {
    loading = true;
    error = null;
    try {
      const [configs, vaults, priceData, totalData] = await Promise.all([
        fetchCollateralConfigs(),
        fetchAllVaults(),
        fetchCollateralPrices(),
        fetchCollateralTotals(),
      ]);

      // Find matching config
      const match = configs.find((c: any) => {
        const ct = c.collateral_type?.toText?.() ?? c.collateral_type?.toString?.() ?? '';
        return ct === tokenId;
      });

      if (!match) {
        error = `Token "${tokenId}" is not a recognized collateral type.`;
        loading = false;
        return;
      }

      rawConfig = match;
      allVaults = vaults;
      prices = priceData;
      totals = totalData;

      // Fetch recent events (last 100) for client-side filtering
      // fetchEvents(page, pageSize) is 0-indexed and returns { total, events }
      const result = await fetchEvents(0n, 100n);
      events = result.events;
    } catch (e: any) {
      console.error('Failed to load token page:', e);
      error = 'Failed to load token data. Please try again.';
    } finally {
      loading = false;
    }
  });
</script>

<svelte:head>
  <title>{symbol} Token | Rumi Explorer</title>
</svelte:head>

<div class="mx-auto max-w-5xl px-4 py-8">
  <!-- Back nav -->
  <a href="/explorer" class="inline-flex items-center gap-1 text-sm text-purple-400 hover:underline mb-6">
    <svg class="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2"><path stroke-linecap="round" stroke-linejoin="round" d="M15 19l-7-7 7-7"/></svg>
    Back to Explorer
  </a>

  {#if loading}
    <div class="text-center py-20 text-gray-400">Loading token data...</div>
  {:else if error}
    <div class="text-center py-20">
      <p class="text-red-400 text-lg mb-2">{error}</p>
      <a href="/explorer" class="text-sm text-purple-400 hover:underline">Return to Explorer</a>
    </div>
  {:else if rawConfig}

    <!-- ── Header ─────────────────────────────────────────────────────── -->
    <div class="mb-8">
      <div class="flex items-center gap-3 flex-wrap mb-3">
        <TokenBadge {symbol} principalId={tokenId} size="md" linked={false} />
        <h1 class="text-3xl font-bold text-white">{symbol}</h1>
        <StatusBadge status={configStatus} size="md" />
        {#if currentPrice > 0}
          <span class="ml-auto text-xl font-semibold text-emerald-400">
            {formatUsdRaw(currentPrice)}
          </span>
        {/if}
      </div>
      <div class="flex items-center gap-2">
        <span class="font-mono text-sm text-gray-400">{tokenId}</span>
        <CopyButton text={tokenId} />
      </div>
    </div>

    <!-- ── Stats Cards ────────────────────────────────────────────────── -->
    <section class="mb-8">
      <div class="grid grid-cols-2 md:grid-cols-3 lg:grid-cols-4 gap-4">
        <StatCard
          label="Total Collateral Locked"
          value="{totalCollateralHuman.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 4 })} {symbol}"
          subtitle={totalCollateralUsd > 0 ? `${formatUsdRaw(totalCollateralUsd)}` : undefined}
        />
        <StatCard
          label="Total Collateral Value"
          value={totalCollateralUsd > 0 ? formatUsdRaw(totalCollateralUsd) : '--'}
        />
        <StatCard
          label="Total Debt Minted"
          value="{totalDebtHuman.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 })} icUSD"
        />
        <StatCard
          label="Active Vaults"
          value={String(activeVaults.length)}
          subtitle={tokenVaults.length !== activeVaults.length ? `${tokenVaults.length} total (incl. closed)` : undefined}
        />
        <StatCard
          label="Debt Ceiling Utilization"
          value={debtCeilingHuman > 0 ? `${debtUtilizationPct.toFixed(1)}%` : 'No cap'}
          subtitle={debtCeilingHuman > 0 ? `${totalDebtHuman.toLocaleString('en-US', { maximumFractionDigits: 0 })} / ${debtCeilingHuman.toLocaleString('en-US', { maximumFractionDigits: 0 })} icUSD` : undefined}
        />
        <StatCard
          label="Borrowing Fee"
          value={borrowingFee > 0 ? formatPercent(borrowingFee, 3) : '0%'}
          subtitle="One-time at mint"
        />
        {#if interestRateApr > 0}
          <StatCard
            label="Interest Rate APR"
            value={formatPercent(interestRateApr)}
          />
        {/if}
      </div>

      <!-- Debt ceiling utilization bar -->
      {#if debtCeilingHuman > 0}
        <div class="mt-4 bg-gray-800/50 border border-gray-700/50 rounded-xl p-4">
          <div class="flex items-center justify-between text-xs text-gray-400 mb-2">
            <span>Debt Ceiling Utilization</span>
            <span class="font-mono">{debtUtilizationPct.toFixed(1)}%</span>
          </div>
          <div class="w-full h-3 bg-gray-700/50 rounded-full overflow-hidden">
            <div
              class="h-full rounded-full transition-all duration-500 {debtUtilizationPct > 90 ? 'bg-red-500' : debtUtilizationPct > 70 ? 'bg-yellow-500' : 'bg-emerald-500'}"
              style="width: {Math.min(debtUtilizationPct, 100)}%"
            ></div>
          </div>
          <div class="flex justify-between text-xs text-gray-500 mt-1">
            <span>0</span>
            <span>{debtCeilingHuman.toLocaleString('en-US', { maximumFractionDigits: 0 })} icUSD</span>
          </div>
        </div>
      {/if}
    </section>

    <!-- ── Configuration ──────────────────────────────────────────────── -->
    <section class="mb-8">
      <h2 class="text-lg font-semibold text-gray-200 mb-3">Configuration</h2>
      <div class="bg-gray-800/50 border border-gray-700/50 rounded-xl overflow-hidden">
        <dl class="divide-y divide-gray-700/40">
          {#each configRows as row}
            <div class="grid grid-cols-2 gap-4 px-5 py-3">
              <dt class="text-sm text-gray-400">{row.label}</dt>
              <dd class="text-sm text-white font-mono text-right">{row.value}</dd>
            </div>
          {/each}
          {#if ledgerCanisterId}
            <div class="grid grid-cols-2 gap-4 px-5 py-3">
              <dt class="text-sm text-gray-400">Ledger Canister</dt>
              <dd class="text-sm text-right">
                <EntityLink type="canister" value={ledgerCanisterId} />
              </dd>
            </div>
          {/if}
        </dl>
      </div>
    </section>

    <!-- ── Vaults Using This Token ────────────────────────────────────── -->
    <section class="mb-8">
      <h2 class="text-lg font-semibold text-gray-200 mb-3">
        Vaults Using {symbol}
        {#if tokenVaults.length > 0}
          <span class="ml-2 text-xs font-normal text-gray-500 bg-gray-700/50 px-2 py-0.5 rounded-full">
            {tokenVaults.length}
          </span>
        {/if}
      </h2>

      {#if tokenVaults.length === 0}
        <div class="bg-gray-800/50 border border-gray-700/50 rounded-xl py-12 text-center text-gray-500">
          No vaults are using this token as collateral.
        </div>
      {:else}
        <div class="bg-gray-800/50 border border-gray-700/50 rounded-xl overflow-hidden">
          <div class="overflow-x-auto">
            <table class="w-full text-sm">
              <thead>
                <tr class="border-b border-gray-700/40 text-xs text-gray-400 uppercase tracking-wide">
                  <th class="px-4 py-3 text-left">Vault</th>
                  <th class="px-4 py-3 text-left">Owner</th>
                  <th class="px-4 py-3 text-right">Collateral</th>
                  <th class="px-4 py-3 text-right">Debt</th>
                  <th class="px-4 py-3 text-right">CR</th>
                  <th class="px-4 py-3 text-center">Health</th>
                </tr>
              </thead>
              <tbody class="divide-y divide-gray-700/30">
                {#each displayVaults as vault}
                  {@const cr = computeVaultCR(vault)}
                  {@const health = vaultHealthLabel(cr)}
                  {@const ownerStr = vault.owner?.toText?.() ?? vault.owner?.toString?.() ?? String(vault.owner)}
                  <tr class="hover:bg-white/[0.02] transition-colors">
                    <td class="px-4 py-3">
                      <EntityLink type="vault" value={String(vault.vault_id)} />
                    </td>
                    <td class="px-4 py-3">
                      <EntityLink type="address" value={ownerStr} short={true} />
                    </td>
                    <td class="px-4 py-3 text-right font-mono text-gray-200">
                      {formatE8s(Number(vault.collateral_amount), decimals)}
                    </td>
                    <td class="px-4 py-3 text-right font-mono text-gray-200">
                      {formatE8s(Number(vault.borrowed_icusd_amount), 8)}
                    </td>
                    <td class="px-4 py-3 text-right font-mono text-gray-200">
                      {cr === Infinity ? '--' : formatCR(cr)}
                    </td>
                    <td class="px-4 py-3 text-center">
                      <StatusBadge status={health} size="sm" />
                    </td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
          {#if hasMoreVaults}
            <div class="px-4 py-3 text-center border-t border-gray-700/40">
              <a href="/explorer?search={tokenId}" class="text-sm text-purple-400 hover:underline">
                View all {tokenVaults.length} vaults
              </a>
            </div>
          {/if}
        </div>
      {/if}
    </section>

    <!-- ── Recent Activity ────────────────────────────────────────────── -->
    <section class="mb-8">
      <h2 class="text-lg font-semibold text-gray-200 mb-3">
        Recent Activity
        {#if eventsSorted.length > 0}
          <span class="ml-2 text-xs font-normal text-gray-500 bg-gray-700/50 px-2 py-0.5 rounded-full">
            {eventsSorted.length} events
          </span>
        {/if}
      </h2>

      {#if eventsSorted.length === 0}
        <div class="bg-gray-800/50 border border-gray-700/50 rounded-xl py-12 text-center text-gray-500">
          No recent events found for this token. Events are filtered from the latest batch of 100.
        </div>
      {:else}
        <div class="bg-gray-800/50 border border-gray-700/50 rounded-xl overflow-hidden divide-y divide-gray-700/30">
          {#each eventsSorted as [idx, evt]}
            <EventRow event={evt} index={Number(idx)} {vaultCollateralMap} />
          {/each}
        </div>
        <p class="text-xs text-gray-500 mt-2 text-center">
          Showing events from the latest batch of 100. Only events that include collateral type data are matched.
        </p>
      {/if}
    </section>
  {/if}
</div>
