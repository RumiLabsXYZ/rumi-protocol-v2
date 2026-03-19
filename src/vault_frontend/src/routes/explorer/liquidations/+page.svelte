<script lang="ts">
  import { onMount } from 'svelte';
  import SearchBar from '$lib/components/explorer/SearchBar.svelte';
  import EventRow from '$lib/components/explorer/EventRow.svelte';
  import { publicActor } from '$lib/services/protocol/apiClient';
  import { isLiquidationEvent, getEventKey, formatAmount } from '$lib/utils/eventFormatters';

  let liquidationEvents: any[] = [];
  let loading = true;
  let filter: 'all' | 'liquidate_vault' | 'partial_liquidate_vault' | 'redistribute_vault' = 'all';

  $: filtered = filter === 'all'
    ? liquidationEvents
    : liquidationEvents.filter(e => getEventKey(e) === filter);

  $: totalDebtLiquidated = liquidationEvents.reduce((sum, e) => {
    const key = getEventKey(e);
    const data = e[key];
    if (key === 'partial_liquidate_vault') return sum + Number(data.liquidator_payment || 0);
    return sum;
  }, 0);

  onMount(async () => {
    loading = true;
    try {
      // Walk events from newest to find liquidations
      const totalCount = Number(await publicActor.get_event_count());
      const batchSize = 2000;
      const allLiquidations: any[] = [];

      for (let i = Math.max(0, totalCount - batchSize); i < totalCount; i += batchSize) {
        const batch = await publicActor.get_events({
          start: BigInt(i),
          length: BigInt(Math.min(batchSize, totalCount - i))
        });
        allLiquidations.push(...batch.filter(isLiquidationEvent));
      }

      // If we didn't get enough, fetch more
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
    } finally {
      loading = false;
    }
  });

  const filters = [
    { label: 'All', value: 'all' as const },
    { label: 'Full', value: 'liquidate_vault' as const },
    { label: 'Partial', value: 'partial_liquidate_vault' as const },
    { label: 'Redistribution', value: 'redistribute_vault' as const },
  ];
</script>

<div class="liquidations-page">
  <a href="/explorer" class="back-link">← Back to Explorer</a>

  <h1 class="page-title">Liquidation History</h1>

  <div class="search-row"><SearchBar /></div>

  <div class="stats-row">
    <div class="stat glass-card">
      <span class="stat-label">Total Liquidations</span>
      <span class="stat-value key-number">{liquidationEvents.length}</span>
    </div>
    <div class="stat glass-card">
      <span class="stat-label">Partial Debt Liquidated</span>
      <span class="stat-value key-number">{formatAmount(BigInt(totalDebtLiquidated))} icUSD</span>
    </div>
  </div>

  <div class="filter-row">
    {#each filters as f}
      <button class="filter-btn" class:active={filter === f.value} on:click={() => filter = f.value}>
        {f.label}
      </button>
    {/each}
  </div>

  {#if loading}
    <div class="loading">Loading liquidation events…</div>
  {:else if filtered.length === 0}
    <div class="empty">No liquidation events found.</div>
  {:else}
    <div class="events-list glass-card">
      {#each filtered as event}
        <EventRow {event} />
      {/each}
    </div>
  {/if}
</div>

<style>
  .liquidations-page { max-width:900px; margin:0 auto; padding:2rem 1rem; }
  .back-link { color:var(--rumi-purple-accent); text-decoration:none; font-size:0.875rem; display:inline-block; margin-bottom:1rem; }
  .back-link:hover { text-decoration:underline; }
  .search-row { margin-bottom:1.5rem; display:flex; justify-content:center; }
  .stats-row { display:flex; gap:1rem; margin-bottom:1rem; }
  .stat { padding:0.75rem 1rem; text-align:center; flex:1; }
  .stat-label { display:block; font-size:0.75rem; color:var(--rumi-text-muted); margin-bottom:0.25rem; }
  .stat-value { font-size:1.25rem; font-weight:600; }
  .filter-row { display:flex; gap:0.375rem; margin-bottom:1rem; }
  .filter-btn { padding:0.375rem 0.75rem; font-size:0.8125rem; border:1px solid var(--rumi-border); border-radius:9999px; background:transparent; color:var(--rumi-text-secondary); cursor:pointer; transition:all 0.15s; }
  .filter-btn:hover { border-color:var(--rumi-border-hover); }
  .filter-btn.active { background:var(--rumi-purple-accent); color:white; border-color:var(--rumi-purple-accent); }
  .events-list { padding:0; overflow:hidden; }
  .loading, .empty { text-align:center; padding:3rem; color:var(--rumi-text-muted); }
</style>
