<script lang="ts">
  import { onMount } from 'svelte';
  import SearchBar from '$lib/components/explorer/SearchBar.svelte';
  import { publicActor } from '$lib/services/protocol/apiClient';
  import { fetchSnapshots, protocolSnapshots, snapshotsLoading } from '$lib/stores/explorerStore';
  import { formatAmount } from '$lib/utils/eventFormatters';

  let protocolStatus: any = null;
  let collateralTotals: any[] = [];
  let loading = true;
  let timeRange: '24h' | '7d' | '30d' | '90d' | 'all' = '7d';

  $: filteredSnapshots = filterByTimeRange($protocolSnapshots, timeRange);

  function filterByTimeRange(snaps: any[], range: string) {
    if (!snaps.length || range === 'all') return snaps;
    const now = Date.now() * 1_000_000; // to nanos
    const ranges: Record<string, number> = {
      '24h': 24 * 3600e9,
      '7d': 7 * 24 * 3600e9,
      '30d': 30 * 24 * 3600e9,
      '90d': 90 * 24 * 3600e9,
    };
    const cutoff = now - (ranges[range] || ranges['7d']);
    return snaps.filter((s: any) => Number(s.timestamp) >= cutoff);
  }

  // SVG chart helpers
  function buildChartPoints(data: { x: number; y: number }[], width: number, height: number): string {
    if (!data.length) return '';
    const xMin = Math.min(...data.map(d => d.x));
    const xMax = Math.max(...data.map(d => d.x));
    const yMin = 0;
    const yMax = Math.max(...data.map(d => d.y)) * 1.1 || 1;
    return data.map(d => {
      const x = xMax === xMin ? width / 2 : ((d.x - xMin) / (xMax - xMin)) * width;
      const y = height - ((d.y - yMin) / (yMax - yMin)) * height;
      return `${x},${y}`;
    }).join(' ');
  }

  $: tvlData = filteredSnapshots.map((s: any) => ({
    x: Number(s.timestamp) / 1e6,
    y: Number(s.total_collateral_value_usd) / 1e8
  }));
  $: debtData = filteredSnapshots.map((s: any) => ({
    x: Number(s.timestamp) / 1e6,
    y: Number(s.total_debt) / 1e8
  }));
  $: vaultCountData = filteredSnapshots.map((s: any) => ({
    x: Number(s.timestamp) / 1e6,
    y: Number(s.total_vault_count)
  }));

  const chartW = 600;
  const chartH = 200;

  $: tvlPoints = buildChartPoints(tvlData, chartW, chartH);
  $: debtPoints = buildChartPoints(debtData, chartW, chartH);
  $: vaultCountPoints = buildChartPoints(vaultCountData, chartW, chartH);

  const timeRanges: { label: string; value: typeof timeRange }[] = [
    { label: '24h', value: '24h' },
    { label: '7d', value: '7d' },
    { label: '30d', value: '30d' },
    { label: '90d', value: '90d' },
    { label: 'All', value: 'all' },
  ];

  onMount(async () => {
    loading = true;
    try {
      const [status, totals] = await Promise.all([
        publicActor.get_protocol_status(),
        publicActor.get_collateral_totals(),
      ]);
      protocolStatus = status;
      collateralTotals = totals;
      await fetchSnapshots();
    } catch (e) {
      console.error('Failed to load stats:', e);
    } finally {
      loading = false;
    }
  });
</script>

<div class="stats-page">
  <a href="/explorer" class="back-link">← Back to Explorer</a>

  <h1 class="page-title">Protocol Stats</h1>

  <div class="search-row"><SearchBar /></div>

  {#if loading}
    <div class="loading">Loading protocol stats…</div>
  {:else if protocolStatus}
    <div class="metrics-row">
      <div class="metric glass-card">
        <span class="metric-label">Total TVL</span>
        <span class="metric-value key-number">${collateralTotals.reduce((sum, ct) => sum + Number(ct.total_collateral) / Math.pow(10, Number(ct.decimals)) * Number(ct.price), 0).toLocaleString('en-US', {maximumFractionDigits: 0})}</span>
      </div>
      <div class="metric glass-card">
        <span class="metric-label">Total Debt</span>
        <span class="metric-value key-number">{formatAmount(BigInt(protocolStatus.total_icusd_borrowed))} icUSD</span>
      </div>
      <div class="metric glass-card">
        <span class="metric-label">Collateral Ratio</span>
        <span class="metric-value key-number">{(protocolStatus.total_collateral_ratio * 100).toFixed(0)}%</span>
      </div>
      <div class="metric glass-card">
        <span class="metric-label">Mode</span>
        <span class="metric-value">{Object.keys(protocolStatus.mode)[0]}</span>
      </div>
    </div>

    {#if collateralTotals.length > 0}
      <h2 class="section-title">Per-Collateral Breakdown</h2>
      <div class="collateral-table glass-card">
        <div class="table-header">
          <span>Collateral</span>
          <span>TVL</span>
          <span>Debt</span>
          <span>Vaults</span>
          <span>Price</span>
        </div>
        {#each collateralTotals as ct}
          <div class="table-row">
            <span>{ct.collateral_type.toString().slice(0, 5)}…</span>
            <span class="key-number">{formatAmount(BigInt(ct.total_collateral), Number(ct.decimals))}</span>
            <span class="key-number">{formatAmount(BigInt(ct.total_debt))} icUSD</span>
            <span class="key-number">{Number(ct.vault_count)}</span>
            <span class="key-number">${Number(ct.price).toFixed(2)}</span>
          </div>
        {/each}
      </div>
    {/if}

    <h2 class="section-title">Historical</h2>

    <div class="time-range-row">
      {#each timeRanges as tr}
        <button class="filter-btn" class:active={timeRange === tr.value} on:click={() => timeRange = tr.value}>
          {tr.label}
        </button>
      {/each}
    </div>

    {#if $snapshotsLoading}
      <div class="loading">Loading historical data…</div>
    {:else if filteredSnapshots.length === 0}
      <div class="empty">No snapshot data available yet. Snapshots are captured hourly.</div>
    {:else}
      <div class="charts-grid">
        <div class="chart-card glass-card">
          <h3 class="chart-title">TVL (USD)</h3>
          <svg viewBox="0 0 {chartW} {chartH}" class="chart-svg">
            <polyline points={tvlPoints} fill="none" stroke="var(--rumi-safe)" stroke-width="2" />
          </svg>
        </div>
        <div class="chart-card glass-card">
          <h3 class="chart-title">Total Debt (icUSD)</h3>
          <svg viewBox="0 0 {chartW} {chartH}" class="chart-svg">
            <polyline points={debtPoints} fill="none" stroke="var(--rumi-purple-accent)" stroke-width="2" />
          </svg>
        </div>
        <div class="chart-card glass-card">
          <h3 class="chart-title">Vault Count</h3>
          <svg viewBox="0 0 {chartW} {chartH}" class="chart-svg">
            <polyline points={vaultCountPoints} fill="none" stroke="var(--rumi-caution)" stroke-width="2" />
          </svg>
        </div>
      </div>
    {/if}
  {/if}
</div>

<style>
  .stats-page { max-width:900px; margin:0 auto; padding:2rem 1rem; }
  .back-link { color:var(--rumi-purple-accent); text-decoration:none; font-size:0.875rem; display:inline-block; margin-bottom:1rem; }
  .back-link:hover { text-decoration:underline; }
  .search-row { margin-bottom:1.5rem; display:flex; justify-content:center; }
  .metrics-row { display:grid; grid-template-columns:repeat(auto-fit, minmax(180px, 1fr)); gap:1rem; margin-bottom:2rem; }
  .metric { padding:1rem; text-align:center; }
  .metric-label { display:block; font-size:0.75rem; color:var(--rumi-text-muted); margin-bottom:0.25rem; }
  .metric-value { font-size:1.5rem; font-weight:600; }
  .section-title { margin-bottom:0.75rem; }
  .collateral-table { padding:0; overflow:hidden; margin-bottom:2rem; }
  .table-header, .table-row { display:grid; grid-template-columns:1fr 1fr 1fr 0.5fr 0.75fr; gap:0.5rem; padding:0.625rem 0.875rem; font-size:0.8125rem; }
  .table-header { color:var(--rumi-text-muted); border-bottom:1px solid var(--rumi-border); font-weight:500; }
  .table-row { border-bottom:1px solid var(--rumi-border); color:var(--rumi-text-secondary); }
  .table-row:last-child { border-bottom:none; }
  .time-range-row { display:flex; gap:0.375rem; margin-bottom:1.5rem; }
  .filter-btn { padding:0.375rem 0.75rem; font-size:0.8125rem; border:1px solid var(--rumi-border); border-radius:9999px; background:transparent; color:var(--rumi-text-secondary); cursor:pointer; transition:all 0.15s; }
  .filter-btn:hover { border-color:var(--rumi-border-hover); }
  .filter-btn.active { background:var(--rumi-purple-accent); color:white; border-color:var(--rumi-purple-accent); }
  .charts-grid { display:flex; flex-direction:column; gap:1.5rem; }
  .chart-card { padding:1rem; }
  .chart-title { margin:0 0 0.5rem; font-size:0.875rem; color:var(--rumi-text-secondary); }
  .chart-svg { width:100%; height:auto; }
  .loading, .empty { text-align:center; padding:3rem; color:var(--rumi-text-muted); }
</style>
