<script lang="ts">
  import { onMount } from 'svelte';
  import SearchBar from '$lib/components/explorer/SearchBar.svelte';
  import EntityLink from '$lib/components/explorer/EntityLink.svelte';
  import TokenBadge from '$lib/components/explorer/TokenBadge.svelte';
  import DashboardCard from '$lib/components/explorer/DashboardCard.svelte';
  import { publicActor } from '$lib/services/protocol/apiClient';
  import {
    isLiquidationEvent,
    getEventKey,
    formatAmount,
    getEventTimestamp,
    formatTimestamp,
    resolveCollateralSymbol,
  } from '$lib/utils/eventFormatters';

  // ── State (Svelte 5 runes) ────────────────────────────────────────────────
  let liquidationEvents: any[] = $state([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let filter = $state<'all' | 'liquidate_vault' | 'partial_liquidate_vault' | 'redistribute_vault'>('all');

  const filters = [
    { label: 'All',          value: 'all' as const },
    { label: 'Full',         value: 'liquidate_vault' as const },
    { label: 'Partial',      value: 'partial_liquidate_vault' as const },
    { label: 'Redistribution', value: 'redistribute_vault' as const },
  ] as const;

  // ── Derived ───────────────────────────────────────────────────────────────
  const filtered = $derived(
    filter === 'all'
      ? liquidationEvents
      : liquidationEvents.filter(e => getEventKey(e) === filter)
  );

  const totalLiquidations = $derived(liquidationEvents.length);

  const totalCollateralSeized = $derived(
    liquidationEvents.reduce((sum, e) => {
      const key = getEventKey(e);
      const data = e[key];
      if (key === 'partial_liquidate_vault') return sum + Number(data.icp_to_liquidator ?? 0);
      return sum;
    }, 0)
  );

  const totalDebtCleared = $derived(
    liquidationEvents.reduce((sum, e) => {
      const key = getEventKey(e);
      const data = e[key];
      if (key === 'partial_liquidate_vault') return sum + Number(data.liquidator_payment ?? 0);
      return sum;
    }, 0)
  );

  // ── Helpers ───────────────────────────────────────────────────────────────
  function getLiquidationType(key: string): { label: string; color: string } {
    switch (key) {
      case 'liquidate_vault':       return { label: 'Full',          color: 'var(--rumi-danger)' };
      case 'partial_liquidate_vault': return { label: 'Partial',     color: 'var(--rumi-caution)' };
      case 'redistribute_vault':    return { label: 'Redistribution', color: 'var(--rumi-text-muted)' };
      default:                      return { label: key,              color: 'var(--rumi-text-muted)' };
    }
  }

  function getVaultId(e: any): number | null {
    const key = getEventKey(e);
    const data = e[key];
    if (data?.vault_id !== undefined) return Number(data.vault_id);
    return null;
  }

  function getLiquidator(e: any): string | null {
    const key = getEventKey(e);
    const data = e[key];
    const liq = data?.liquidator;
    if (Array.isArray(liq) && liq.length > 0) return liq[0]?.toString?.() ?? null;
    if (liq?.toString) return liq.toString();
    return null;
  }

  function getCollateralSeized(e: any): bigint | null {
    const key = getEventKey(e);
    const data = e[key];
    if (key === 'partial_liquidate_vault') return data?.icp_to_liquidator ?? null;
    return null;
  }

  function getDebtCleared(e: any): bigint | null {
    const key = getEventKey(e);
    const data = e[key];
    if (key === 'partial_liquidate_vault') return data?.liquidator_payment ?? null;
    return null;
  }

  function getCollateralSymbol(e: any): string {
    const key = getEventKey(e);
    const data = e[key];
    if (data?.collateral_type) return resolveCollateralSymbol(data.collateral_type);
    return 'ICP';
  }

  function relativeTime(nanos: bigint | null): string {
    if (!nanos) return '—';
    const ms = Number(nanos / BigInt(1_000_000));
    const diff = Date.now() - ms;
    if (diff < 60_000) return `${Math.floor(diff / 1000)}s ago`;
    if (diff < 3_600_000) return `${Math.floor(diff / 60_000)}m ago`;
    if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)}h ago`;
    return `${Math.floor(diff / 86_400_000)}d ago`;
  }

  // ── Data Fetch ────────────────────────────────────────────────────────────
  onMount(async () => {
    loading = true;
    error = null;
    try {
      const totalCount = Number(await publicActor.get_event_count());
      const batchSize = 2000;
      const allLiquidations: any[] = [];

      // Fetch the most recent batch first
      for (let i = Math.max(0, totalCount - batchSize); i < totalCount; i += batchSize) {
        const batch = await publicActor.get_events({
          start: BigInt(i),
          length: BigInt(Math.min(batchSize, totalCount - i))
        });
        allLiquidations.push(...batch.filter(isLiquidationEvent));
      }

      // Fetch more if we didn't find enough
      if (allLiquidations.length < 50 && totalCount > batchSize) {
        for (let i = Math.max(0, totalCount - batchSize * 3); i < totalCount - batchSize; i += batchSize) {
          const batch = await publicActor.get_events({
            start: BigInt(i),
            length: BigInt(Math.min(batchSize, totalCount - batchSize - i))
          });
          allLiquidations.unshift(...batch.filter(isLiquidationEvent));
        }
      }

      liquidationEvents = allLiquidations.reverse(); // newest first
    } catch (e) {
      console.error('Failed to fetch liquidation events:', e);
      error = 'Failed to load liquidation events. Please try again.';
    } finally {
      loading = false;
    }
  });
</script>

<div class="liq-page">
  <a href="/explorer" class="back-link">← Back to Explorer</a>

  <div class="page-header">
    <h1 class="page-title">Liquidation History</h1>
    <div class="search-row"><SearchBar /></div>
  </div>

  <!-- Stats Header -->
  <div class="stats-grid">
    <DashboardCard
      label="Total Liquidations"
      value={String(totalLiquidations)}
      subtitle={loading ? 'Loading…' : undefined}
    />
    <DashboardCard
      label="Collateral Seized (Partial)"
      value={loading ? '—' : formatAmount(BigInt(Math.round(totalCollateralSeized))) + ' ICP'}
      subtitle="From partial liquidations"
    />
    <DashboardCard
      label="Debt Cleared (Partial)"
      value={loading ? '—' : formatAmount(BigInt(Math.round(totalDebtCleared))) + ' icUSD'}
      subtitle="From partial liquidations"
    />
  </div>

  <!-- Filter Tabs -->
  <div class="filter-row">
    {#each filters as f}
      <button
        class="filter-btn"
        class:active={filter === f.value}
        onclick={() => filter = f.value}
      >
        {f.label}
        {#if f.value === 'all'}
          <span class="count">{liquidationEvents.length}</span>
        {:else}
          <span class="count">{liquidationEvents.filter(e => getEventKey(e) === f.value).length}</span>
        {/if}
      </button>
    {/each}
  </div>

  <!-- Table -->
  {#if loading}
    <div class="state-msg">
      <div class="spinner"></div>
      <span>Loading liquidation events…</span>
    </div>
  {:else if error}
    <div class="state-msg error">{error}</div>
  {:else if filtered.length === 0}
    <div class="state-msg">No liquidation events found.</div>
  {:else}
    <div class="table-wrap glass-card">
      <div class="table-header">
        <span>Time</span>
        <span>Vault</span>
        <span>Type</span>
        <span>Collateral Seized</span>
        <span>Debt Cleared</span>
        <span>Liquidator</span>
      </div>
      {#each filtered as event}
        {@const key = getEventKey(event)}
        {@const data = event[key]}
        {@const ts = getEventTimestamp(event)}
        {@const vaultId = getVaultId(event)}
        {@const liquidator = getLiquidator(event)}
        {@const typeInfo = getLiquidationType(key)}
        {@const collateralSeized = getCollateralSeized(event)}
        {@const debtCleared = getDebtCleared(event)}
        {@const symbol = getCollateralSymbol(event)}
        <div class="table-row">
          <!-- Time -->
          <span class="cell-time" title={ts ? formatTimestamp(ts) : ''}>
            {relativeTime(ts)}
          </span>

          <!-- Vault -->
          <span class="cell-vault">
            {#if vaultId !== null}
              <EntityLink type="vault" id={vaultId} label="#{vaultId}" />
            {:else}
              <span class="dim">—</span>
            {/if}
          </span>

          <!-- Type Badge -->
          <span class="cell-type">
            <span
              class="type-badge"
              style="background:{typeInfo.color}20; color:{typeInfo.color}; border:1px solid {typeInfo.color}40;"
            >
              {typeInfo.label}
            </span>
          </span>

          <!-- Collateral Seized -->
          <span class="cell-amount">
            {#if collateralSeized !== null}
              <span class="key-number">{formatAmount(collateralSeized)}</span>
              <TokenBadge symbol={symbol} linked={false} />
            {:else if key === 'liquidate_vault'}
              <span class="dim">Full seize</span>
            {:else}
              <span class="dim">—</span>
            {/if}
          </span>

          <!-- Debt Cleared -->
          <span class="cell-amount">
            {#if debtCleared !== null}
              <span class="key-number">{formatAmount(debtCleared)}</span>
              <TokenBadge symbol="icUSD" linked={false} />
            {:else if key === 'liquidate_vault'}
              <span class="dim">All debt</span>
            {:else}
              <span class="dim">—</span>
            {/if}
          </span>

          <!-- Liquidator -->
          <span class="cell-addr">
            {#if liquidator}
              <EntityLink type="address" id={liquidator} />
            {:else}
              <span class="dim">—</span>
            {/if}
          </span>
        </div>
      {/each}
    </div>
  {/if}
</div>

<style>
  .liq-page { max-width: 1100px; margin: 0 auto; padding: 2rem 1rem; }

  .back-link {
    color: var(--rumi-purple-accent);
    text-decoration: none;
    font-size: 0.875rem;
    display: inline-block;
    margin-bottom: 1rem;
  }
  .back-link:hover { text-decoration: underline; }

  .page-header { margin-bottom: 1.5rem; }
  .page-title { margin: 0 0 1rem; font-size: 1.5rem; font-weight: 700; }
  .search-row { display: flex; justify-content: center; }

  /* Stats grid */
  .stats-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
    gap: 1rem;
    margin-bottom: 1.5rem;
  }

  /* Filter row */
  .filter-row { display: flex; gap: 0.375rem; margin-bottom: 1rem; flex-wrap: wrap; }
  .filter-btn {
    display: inline-flex;
    align-items: center;
    gap: 0.375rem;
    padding: 0.375rem 0.75rem;
    font-size: 0.8125rem;
    border: 1px solid var(--rumi-border);
    border-radius: 9999px;
    background: transparent;
    color: var(--rumi-text-secondary);
    cursor: pointer;
    transition: all 0.15s;
  }
  .filter-btn:hover { border-color: var(--rumi-border-hover); color: var(--rumi-text-primary); }
  .filter-btn.active {
    background: var(--rumi-purple-accent);
    color: white;
    border-color: var(--rumi-purple-accent);
  }
  .filter-btn .count {
    font-size: 0.6875rem;
    opacity: 0.75;
    background: rgba(255,255,255,0.15);
    padding: 0 0.3rem;
    border-radius: 9999px;
  }

  /* Table */
  .table-wrap { overflow-x: auto; border-radius: 0.75rem; }

  .table-header, .table-row {
    display: grid;
    grid-template-columns: 6.5rem 4.5rem 6.5rem 1fr 1fr 1fr;
    gap: 0.5rem;
    align-items: center;
    padding: 0.625rem 0.875rem;
    font-size: 0.8125rem;
  }

  .table-header {
    color: var(--rumi-text-muted);
    border-bottom: 1px solid var(--rumi-border);
    font-weight: 500;
    position: sticky;
    top: 0;
    background: var(--rumi-bg-surface);
    z-index: 1;
  }

  .table-row {
    border-bottom: 1px solid var(--rumi-border);
    color: var(--rumi-text-secondary);
    transition: background 0.12s;
  }
  .table-row:last-child { border-bottom: none; }
  .table-row:hover { background: var(--rumi-bg-surface-2); }

  .cell-time {
    font-size: 0.75rem;
    color: var(--rumi-text-muted);
    cursor: default;
    white-space: nowrap;
  }

  .cell-vault, .cell-addr { min-width: 0; overflow: hidden; text-overflow: ellipsis; }

  .cell-amount {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    white-space: nowrap;
  }

  .cell-type { white-space: nowrap; }

  .type-badge {
    font-size: 0.6875rem;
    font-weight: 500;
    padding: 0.125rem 0.5rem;
    border-radius: 9999px;
    white-space: nowrap;
  }

  .dim { color: var(--rumi-text-muted); font-size: 0.75rem; }

  /* State messages */
  .state-msg {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0.75rem;
    padding: 4rem 2rem;
    color: var(--rumi-text-muted);
    text-align: center;
  }
  .state-msg.error { color: var(--rumi-danger); }

  .spinner {
    width: 1.25rem;
    height: 1.25rem;
    border: 2px solid var(--rumi-border);
    border-top-color: var(--rumi-purple-accent);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
    flex-shrink: 0;
  }
  @keyframes spin { to { transform: rotate(360deg); } }
</style>
