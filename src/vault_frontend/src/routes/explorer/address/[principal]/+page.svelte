<script lang="ts">
  import { page } from '$app/stores';
  import { onMount } from 'svelte';
  import { Principal } from '@dfinity/principal';
  import StatCard from '$components/explorer/StatCard.svelte';
  import EntityLink from '$components/explorer/EntityLink.svelte';
  import CopyButton from '$components/explorer/CopyButton.svelte';
  import StatusBadge from '$components/explorer/StatusBadge.svelte';
  import EventRow from '$components/explorer/EventRow.svelte';
  import VaultHealthBar from '$components/explorer/VaultHealthBar.svelte';
  import {
    fetchVaultsByOwner, fetchEventsByPrincipal,
    fetchCollateralConfigs, fetchCollateralPrices,
    fetchSwapEvents, fetchSwapEventCount,
    fetchAmmSwapEvents, fetchAmmSwapEventCount,
    fetchAmmLiquidityEvents, fetchAmmLiquidityEventCount,
    fetch3PoolLiquidityEvents, fetch3PoolLiquidityEventCount,
    fetchAllVaults,
  } from '$services/explorer/explorerService';
  import {
    formatE8s, formatUsdRaw, formatCR, getTokenSymbol, getCanisterName,
    isKnownCanister, shortenPrincipal, timeAgo, formatTimestamp
  } from '$utils/explorerHelpers';
  import {
    getEventCategory, formatSwapEvent, formatAmmSwapEvent,
    formatAmmLiquidityEvent, format3PoolLiquidityEvent,
  } from '$utils/explorerFormatters';
  import type { EventCategory } from '$utils/explorerFormatters';
  import { extractEventTimestamp } from '$utils/displayEvent';

  // ── State ────────────────────────────────────────────────────────────
  let loading = $state(true);
  let error: string | null = $state(null);
  let vaults: any[] = $state([]);
  let events: [bigint, any][] = $state([]);
  let configMap = $state(new Map<string, any>());
  let priceMap = $state(new Map<string, number>());
  let selectedCategory: EventCategory | 'all' | 'dex' = $state('all');
  // Source tags match DexEventSource route segments so we can link to /explorer/dex/{source}/{id}
  type DexSource = '3pool_swap' | 'amm_swap' | 'amm_liquidity' | '3pool_liquidity';
  let dexEvents: { id: bigint; source: DexSource; event: any; timestamp: number }[] = $state([]);
  let dexLoading: boolean = $state(false);
  let vaultCollateralMap: Map<number, string> = $state(new Map());
  let vaultOwnerMap: Map<number, string> = $state(new Map());

  // ── Derived ──────────────────────────────────────────────────────────
  const principalStr = $derived($page.params.principal);

  const knownCanister = $derived(isKnownCanister(principalStr));
  const canisterName = $derived(getCanisterName(principalStr));

  const activeVaults = $derived(
    vaults.filter((v) => {
      if (!v.status) return true;
      const key = Object.keys(v.status)[0];
      return key !== 'Closed' && key !== 'closed' && key !== 'Liquidated' && key !== 'liquidated';
    })
  );

  // Compute CR, collateral USD, and debt for each vault
  function vaultCollateralUsd(vault: any): number {
    const ct = vault.collateral_type?.toText?.() ?? vault.collateral_type?.toString?.() ?? '';
    const cfg = configMap.get(ct);
    const decimals = cfg?.decimals ? Number(cfg.decimals) : 8;
    const price = priceMap.get(ct) ?? 0;
    return (Number(vault.collateral_amount) / 10 ** decimals) * price;
  }

  function vaultDebt(vault: any): number {
    return (Number(vault.borrowed_icusd_amount) + Number(vault.accrued_interest ?? 0n)) / 1e8;
  }

  function vaultCR(vault: any): number {
    const debt = vaultDebt(vault);
    if (debt <= 0) return Infinity;
    return vaultCollateralUsd(vault) / debt;
  }

  function vaultLiqRatio(vault: any): number {
    const ct = vault.collateral_type?.toText?.() ?? vault.collateral_type?.toString?.() ?? '';
    const cfg = configMap.get(ct);
    return cfg?.liquidation_threshold ? Number(cfg.liquidation_threshold) * 100 : 110;
  }

  function vaultStatus(vault: any): string {
    if (!vault.status) return 'Active';
    const key = Object.keys(vault.status)[0];
    if (key === 'Closed' || key === 'closed') return 'Closed';
    if (key === 'Liquidated' || key === 'liquidated') return 'Liquidated';
    return 'Active';
  }

  function vaultCollateralSymbol(vault: any): string {
    const ct = vault.collateral_type?.toText?.() ?? vault.collateral_type?.toString?.() ?? '';
    return getTokenSymbol(ct);
  }

  function vaultCollateralDecimals(vault: any): number {
    const ct = vault.collateral_type?.toText?.() ?? vault.collateral_type?.toString?.() ?? '';
    const cfg = configMap.get(ct);
    return cfg?.decimals ? Number(cfg.decimals) : 8;
  }

  // Totals
  const totalCollateralUsd = $derived(vaults.reduce((sum, v) => sum + vaultCollateralUsd(v), 0));
  const totalDebt = $derived(vaults.reduce((sum, v) => sum + vaultDebt(v), 0));
  const weightedCR = $derived(totalDebt > 0 ? totalCollateralUsd / totalDebt : 0);

  // Event filtering — only user-relevant categories for address pages
  const addressTabs: { key: string; label: string }[] = [
    { key: 'all', label: 'All' },
    { key: 'vault_ops', label: 'Vault Operations' },
    { key: 'liquidation', label: 'Liquidations' },
    { key: 'stability_pool', label: 'Stability Pool' },
    { key: 'dex', label: 'DEX' },
  ];

  async function loadDexEvents(principalId: string) {
    dexLoading = true;
    try {
      const [threePoolSwapCount, ammSwapCount, ammLiqCount, threePoolLiqCount] = await Promise.all([
        fetchSwapEventCount(),
        fetchAmmSwapEventCount(),
        fetchAmmLiquidityEventCount(),
        fetch3PoolLiquidityEventCount(),
      ]);

      const [threePoolSwaps, ammSwaps, ammLiqEvents, threePoolLiqEvents] = await Promise.all([
        Number(threePoolSwapCount) > 0 ? fetchSwapEvents(0n, threePoolSwapCount) : Promise.resolve([]),
        Number(ammSwapCount) > 0 ? fetchAmmSwapEvents(0n, ammSwapCount) : Promise.resolve([]),
        Number(ammLiqCount) > 0 ? fetchAmmLiquidityEvents(0n, ammLiqCount) : Promise.resolve([]),
        Number(threePoolLiqCount) > 0 ? fetch3PoolLiquidityEvents(threePoolLiqCount, 0n) : Promise.resolve([]),
      ]);

      // Helper to check if an event's caller matches this principal
      const matchesCaller = (e: any) => {
        const caller = e.caller?.toText?.() ?? String(e.caller ?? '');
        return caller === principalId;
      };

      const tag = (arr: any[], source: DexSource) =>
        arr.filter(matchesCaller).map((e: any) => ({
          id: BigInt(e.id ?? 0),
          source,
          event: e,
          timestamp: Number(e.timestamp ?? 0),
        }));

      const merged = [
        ...tag(threePoolSwaps, '3pool_swap'),
        ...tag(ammSwaps, 'amm_swap'),
        ...tag(ammLiqEvents, 'amm_liquidity'),
        ...tag(threePoolLiqEvents, '3pool_liquidity'),
      ];
      merged.sort((a, b) => b.timestamp - a.timestamp);
      dexEvents = merged;
    } catch (e) {
      console.error('[address] loadDexEvents error:', e);
      dexEvents = [];
    } finally {
      dexLoading = false;
    }
  }

  // Backend events (already ordered by globalIndex; newest first)
  const sortedEvents = $derived(
    [...events].sort(([a], [b]) => Number(b) - Number(a))
  );

  // Backend events filtered by tab (used for non-'all' and non-'dex' tabs)
  const filteredBackendEvents = $derived(
    selectedCategory === 'all'
      ? sortedEvents
      : sortedEvents.filter(([_, event]) => getEventCategory(event) === selectedCategory)
  );

  function getDexFormatter(source: DexSource, event: any) {
    switch (source) {
      case 'amm_liquidity': return formatAmmLiquidityEvent(event);
      case '3pool_liquidity': return format3PoolLiquidityEvent(event);
      case 'amm_swap': return formatAmmSwapEvent(event);
      case '3pool_swap': return formatSwapEvent(event);
    }
  }

  const DEX_SOURCE_LABEL: Record<DexSource, string> = {
    '3pool_swap': '3Pool',
    'amm_swap': 'AMM',
    'amm_liquidity': 'AMM',
    '3pool_liquidity': '3Pool',
  };

  // Unified event stream for the "all" tab: backend events + DEX events sorted by timestamp desc.
  // Each entry carries just enough context for the mixed table renderer below.
  type MixedRow =
    | { kind: 'backend'; globalIndex: bigint; event: any; timestamp: number }
    | { kind: 'dex'; id: bigint; source: DexSource; event: any; timestamp: number };

  const allRows = $derived.by((): MixedRow[] => {
    const backendRows: MixedRow[] = sortedEvents.map(([idx, ev]) => ({
      kind: 'backend',
      globalIndex: idx,
      event: ev,
      timestamp: extractEventTimestamp(ev),
    }));
    const dexRows: MixedRow[] = dexEvents.map((d) => ({
      kind: 'dex',
      id: d.id,
      source: d.source,
      event: d.event,
      timestamp: d.timestamp,
    }));
    const all = [...backendRows, ...dexRows];
    // Sort newest first. Fall back to globalIndex for ties (backend) so order stays stable.
    all.sort((a, b) => b.timestamp - a.timestamp);
    return all;
  });

  const totalEventCount = $derived(events.length + dexEvents.length);

  // ── Load ─────────────────────────────────────────────────────────────
  onMount(async () => {
    loading = true;
    error = null;

    let principal: Principal;
    try {
      principal = Principal.fromText($page.params.principal);
    } catch {
      error = `Invalid principal: "${$page.params.principal}"`;
      loading = false;
      return;
    }

    try {
      // Fetch vaults, events, configs, prices, and all vaults (for collateral type lookup) in parallel
      const [vaultResults, eventsResult, configs, prices, allVaults] = await Promise.all([
        fetchVaultsByOwner(principal),
        fetchEventsByPrincipal(principal),
        fetchCollateralConfigs(),
        fetchCollateralPrices(),
        fetchAllVaults(),
      ]);

      events = eventsResult;
      vaults = vaultResults;

      // Build vault collateral type map for event formatting
      const vcMap = new Map<number, string>();
      const voMap = new Map<number, string>();
      for (const v of allVaults) {
        const id = Number(v.vault_id);
        const collType = v.collateral_type?.toText?.() ?? String(v.collateral_type ?? '');
        if (collType) vcMap.set(id, collType);
        const owner = v.owner?.toText?.() ?? (typeof v.owner === 'string' ? v.owner : '');
        if (owner) voMap.set(id, owner);
      }
      vaultCollateralMap = vcMap;
      vaultOwnerMap = voMap;

      // Build config map keyed by principal text
      const cMap = new Map<string, any>();
      for (const cfg of configs) {
        const key = cfg.ledger_id?.toText?.() ?? cfg.ledger_id?.toString?.() ?? '';
        if (key) cMap.set(key, cfg);
      }
      configMap = cMap;

      // prices is already Map<string, number>
      priceMap = prices;

      // Kick off DEX event load in the background so the "All" tab has them on first paint.
      // We don't await — the main view renders immediately with backend events, and DEX rows
      // fold in as soon as this resolves.
      loadDexEvents($page.params.principal);
    } catch (e) {
      console.error('[address page] Failed to load data:', e);
      error = 'Failed to load address data. Please try again.';
    } finally {
      loading = false;
    }
  });
</script>

<svelte:head>
  <title>{principalStr ? shortenPrincipal(principalStr) : 'Address'} | Rumi Explorer</title>
</svelte:head>

<div class="max-w-[1100px] mx-auto px-4 py-8">

  <!-- Loading -->
  {#if loading}
    <div class="flex items-center justify-center py-20">
      <div class="flex flex-col items-center gap-3">
        <div class="w-8 h-8 border-2 border-gray-600 border-t-blue-400 rounded-full animate-spin"></div>
        <span class="text-sm text-gray-500">Loading address...</span>
      </div>
    </div>

  <!-- Error -->
  {:else if error}
    <div class="text-center py-16">
      <p class="text-red-400 text-sm mb-4">{error}</p>
      <a href="/explorer" class="text-blue-400 hover:underline text-sm">Back to Explorer</a>
    </div>

  {:else}
    <!-- ── Header ──────────────────────────────────────────────────────── -->
    <div class="mb-8">
      <div class="flex items-center gap-3 mb-3">
        <h1 class="text-2xl font-bold text-white">
          {#if knownCanister}
            {canisterName}
          {:else}
            Address
          {/if}
        </h1>
        {#if knownCanister}
          <StatusBadge status="Canister" />
        {:else}
          <span class="text-xs text-gray-500 bg-gray-800/50 border border-gray-700/50 rounded-full px-2.5 py-0.5">
            User Account
          </span>
        {/if}
      </div>
      <div class="flex items-center gap-2">
        <code class="text-sm text-gray-300 font-mono bg-gray-800/50 border border-gray-700/50 rounded-lg px-3 py-2 break-all">
          {principalStr}
        </code>
        <CopyButton text={principalStr} />
      </div>
    </div>

    <!-- ── Summary Cards ───────────────────────────────────────────────── -->
    <div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-4 gap-4 mb-8">
      <StatCard
        label="Total Vaults"
        value={String(vaults.length)}
        subtitle="{activeVaults.length} active / {vaults.length} total"
      />
      <StatCard
        label="Total Collateral Value"
        value={formatUsdRaw(totalCollateralUsd)}
      />
      <StatCard
        label="Total Debt"
        value="{formatE8s(BigInt(Math.round(totalDebt * 1e8)))} icUSD"
      />
      <StatCard
        label="Weighted Avg CR"
        value={totalDebt > 0 ? formatCR(weightedCR) : 'N/A'}
        subtitle={totalDebt > 0 ? 'collateral / debt' : 'No debt'}
      />
    </div>

    <!-- ── Vaults Section ──────────────────────────────────────────────── -->
    <section class="mb-10">
      <h2 class="text-lg font-semibold text-white mb-4">Vaults ({vaults.length})</h2>

      {#if vaults.length === 0}
        <div class="bg-gray-800/30 border border-gray-700/50 rounded-xl p-12 text-center">
          <p class="text-gray-500 text-sm">No vaults found for this address</p>
        </div>
      {:else}
        <div class="bg-gray-800/30 border border-gray-700/50 rounded-xl overflow-hidden">
          <div class="overflow-x-auto">
            <table class="w-full">
              <thead>
                <tr class="border-b border-gray-700/50 text-left">
                  <th class="px-4 py-3 text-xs font-medium text-gray-500 uppercase tracking-wider">Vault</th>
                  <th class="px-4 py-3 text-xs font-medium text-gray-500 uppercase tracking-wider">Collateral</th>
                  <th class="px-4 py-3 text-xs font-medium text-gray-500 uppercase tracking-wider text-right">Amount</th>
                  <th class="px-4 py-3 text-xs font-medium text-gray-500 uppercase tracking-wider text-right">Debt (icUSD)</th>
                  <th class="px-4 py-3 text-xs font-medium text-gray-500 uppercase tracking-wider" style="min-width: 12rem;">CR</th>
                  <th class="px-4 py-3 text-xs font-medium text-gray-500 uppercase tracking-wider text-center">Status</th>
                </tr>
              </thead>
              <tbody>
                {#each vaults as vault (vault.vault_id)}
                  {@const cr = vaultCR(vault)}
                  {@const liqRatio = vaultLiqRatio(vault)}
                  {@const status = vaultStatus(vault)}
                  <tr
                    class="border-b border-gray-700/30 last:border-b-0 hover:bg-gray-700/20 transition-colors cursor-pointer"
                    onclick={() => { window.location.href = `/explorer/vault/${vault.vault_id}`; }}
                  >
                    <td class="px-4 py-3">
                      <EntityLink type="vault" value={String(vault.vault_id)} />
                    </td>
                    <td class="px-4 py-3">
                      <EntityLink
                        type="token"
                        value={vault.collateral_type?.toText?.() ?? vault.collateral_type?.toString?.() ?? ''}
                        label={vaultCollateralSymbol(vault)}
                      />
                    </td>
                    <td class="px-4 py-3 text-right text-gray-200 text-sm font-mono">
                      {formatE8s(vault.collateral_amount, vaultCollateralDecimals(vault))}
                      <span class="text-gray-500 ml-1">{vaultCollateralSymbol(vault)}</span>
                    </td>
                    <td class="px-4 py-3 text-right text-gray-200 text-sm font-mono">
                      {formatE8s(BigInt(Number(vault.borrowed_icusd_amount) + Number(vault.accrued_interest ?? 0n)))}
                    </td>
                    <td class="px-4 py-3" style="min-width: 12rem;">
                      {#if cr === Infinity}
                        <span class="text-gray-500 text-xs">No debt</span>
                      {:else}
                        <VaultHealthBar collateralRatio={cr * 100} liquidationRatio={liqRatio} />
                      {/if}
                    </td>
                    <td class="px-4 py-3 text-center">
                      <StatusBadge
                        status={status === 'Active' ? 'Active'
                          : status === 'Liquidated' ? 'Liquidated'
                          : 'Closed'}
                      />
                    </td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        </div>
      {/if}
    </section>

    <!-- ── Activity Feed ───────────────────────────────────────────────── -->
    <section>
      <h2 class="text-lg font-semibold text-white mb-4">
        Activity ({totalEventCount} event{totalEventCount === 1 ? '' : 's'}{dexLoading ? ', loading DEX…' : ''})
      </h2>

      <!-- Category filter tabs -->
      <div class="flex gap-0 border-b border-gray-700/50 mb-4 overflow-x-auto">
        {#each addressTabs as tab}
          <button
            class="px-4 py-2.5 text-sm font-medium whitespace-nowrap transition-colors
              {selectedCategory === tab.key
              ? 'text-blue-400 border-b-2 border-blue-400'
              : 'text-gray-400 border-b-2 border-transparent hover:text-gray-300 hover:border-gray-600'}"
            onclick={() => {
              selectedCategory = tab.key as EventCategory | 'all' | 'dex';
            }}
          >
            {tab.label}
          </button>
        {/each}
      </div>

      {#if selectedCategory === 'all'}
        {#if allRows.length === 0}
          <div class="bg-gray-800/30 border border-gray-700/50 rounded-xl p-12 text-center">
            <p class="text-gray-500 text-sm">No activity found</p>
          </div>
        {:else}
          <div class="bg-gray-800/30 border border-gray-700/50 rounded-xl overflow-hidden">
            {#each allRows as row (row.kind === 'backend' ? `b:${row.globalIndex}` : `d:${row.source}:${row.id}`)}
              {#if row.kind === 'backend'}
                <EventRow event={row.event} index={Number(row.globalIndex)} {vaultCollateralMap} {vaultOwnerMap} />
              {:else}
                {@const formatted = getDexFormatter(row.source, row.event)}
                <a
                  href="/explorer/dex/{row.source}/{Number(row.id)}"
                  class="flex items-center gap-4 px-4 py-3 border-b border-gray-700/30 last:border-b-0 hover:bg-gray-700/20 transition-colors"
                >
                  <span class="text-xs text-gray-500 font-mono w-24 shrink-0">{DEX_SOURCE_LABEL[row.source]} #{Number(row.id)}</span>
                  <span class="text-xs text-gray-500 w-20 shrink-0" title={row.timestamp ? formatTimestamp(row.timestamp) : ''}>{row.timestamp ? timeAgo(row.timestamp) : '—'}</span>
                  <span class="inline-block text-xs font-medium px-2.5 py-0.5 rounded-full shrink-0 {formatted.badgeColor}">
                    {formatted.typeName}
                  </span>
                  <span class="text-sm text-gray-300 truncate flex-1 min-w-0">{formatted.summary}</span>
                </a>
              {/if}
            {/each}
          </div>
        {/if}
      {:else if selectedCategory === 'dex'}
        {#if dexLoading && dexEvents.length === 0}
          <div class="flex items-center justify-center py-10">
            <div class="w-6 h-6 border-2 border-gray-600 border-t-blue-400 rounded-full animate-spin"></div>
          </div>
        {:else if dexEvents.length === 0}
          <p class="text-gray-500 text-sm text-center py-10">No DEX activity found for this address.</p>
        {:else}
          <div class="bg-gray-800/30 border border-gray-700/50 rounded-xl overflow-hidden">
            <table class="w-full">
              <thead>
                <tr class="border-b border-gray-700/50 text-left">
                  <th class="px-4 py-3 text-xs font-medium text-gray-500 uppercase">#</th>
                  <th class="px-4 py-3 text-xs font-medium text-gray-500 uppercase">Time</th>
                  <th class="px-4 py-3 text-xs font-medium text-gray-500 uppercase">Type</th>
                  <th class="px-4 py-3 text-xs font-medium text-gray-500 uppercase">Summary</th>
                  <th class="px-4 py-3 text-xs font-medium text-gray-500 uppercase w-16 text-right">Details</th>
                </tr>
              </thead>
              <tbody>
                {#each dexEvents as d (d.source + ':' + d.id)}
                  {@const formatted = getDexFormatter(d.source, d.event)}
                  <tr class="border-b border-gray-700/50 hover:bg-gray-800/30 transition-colors">
                    <td class="px-4 py-3 text-xs text-gray-500 font-mono">{DEX_SOURCE_LABEL[d.source]} #{Number(d.id)}</td>
                    <td class="px-4 py-3 text-xs text-gray-500" title={d.timestamp ? formatTimestamp(d.timestamp) : ''}>{d.timestamp ? timeAgo(d.timestamp) : '—'}</td>
                    <td class="px-4 py-3">
                      <span class="inline-block text-xs font-medium px-2.5 py-0.5 rounded-full {formatted.badgeColor}">
                        {formatted.typeName}
                      </span>
                    </td>
                    <td class="px-4 py-3 text-sm text-gray-300 truncate max-w-[300px]">{formatted.summary}</td>
                    <td class="px-4 py-3 text-right">
                      <a
                        href="/explorer/dex/{d.source}/{Number(d.id)}"
                        class="text-xs text-blue-400 hover:text-blue-300 hover:underline"
                      >Details</a>
                    </td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        {/if}
      {:else if filteredBackendEvents.length === 0}
        <div class="bg-gray-800/30 border border-gray-700/50 rounded-xl p-12 text-center">
          <p class="text-gray-500 text-sm">
            No {addressTabs.find((t) => t.key === selectedCategory)?.label ?? ''} events found
          </p>
        </div>
      {:else}
        <div class="bg-gray-800/30 border border-gray-700/50 rounded-xl overflow-hidden">
          {#each filteredBackendEvents as [globalIndex, event] (globalIndex)}
            <EventRow {event} index={Number(globalIndex)} {vaultCollateralMap} {vaultOwnerMap} />
          {/each}
        </div>
      {/if}
    </section>
  {/if}
</div>
