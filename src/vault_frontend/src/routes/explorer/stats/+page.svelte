<script lang="ts">
  import { onMount } from 'svelte';
  import SearchBar from '$lib/components/explorer/SearchBar.svelte';
  import DashboardCard from '$lib/components/explorer/DashboardCard.svelte';
  import TokenBadge from '$lib/components/explorer/TokenBadge.svelte';
  import { publicActor } from '$lib/services/protocol/apiClient';
  import { fetchSnapshots, protocolSnapshots, snapshotsLoading } from '$lib/stores/explorerStore';
  import { collateralStore } from '$lib/stores/collateralStore';
  import { get } from 'svelte/store';
  import { formatAmount } from '$lib/utils/eventFormatters';

  // ── State ────────────────────────────────────────────────────────────────
  let protocolStatus: any = $state(null);
  let collateralTotals: { symbol: string; amount: number; debt: number; vaultCount: number; price: number; color: string }[] = $state([]);
  let loading = $state(true);
  type TimeRange = '24h' | '7d' | '30d' | '90d' | 'all';
  let timeRange: TimeRange = $state('7d');

  const timeRanges: { label: string; value: TimeRange }[] = [
    { label: '24h', value: '24h' },
    { label: '7d',  value: '7d' },
    { label: '30d', value: '30d' },
    { label: '90d', value: '90d' },
    { label: 'All', value: 'all' },
  ];

  // ── Derived ───────────────────────────────────────────────────────────────
  const filteredSnapshots = $derived(filterByTimeRange($protocolSnapshots, timeRange));

  function filterByTimeRange(snaps: any[], range: string) {
    if (!snaps.length || range === 'all') return snaps;
    const now = Date.now() * 1_000_000; // to nanos
    const ranges: Record<string, number> = {
      '24h': 24 * 3600e9,
      '7d':  7  * 24 * 3600e9,
      '30d': 30 * 24 * 3600e9,
      '90d': 90 * 24 * 3600e9,
    };
    const cutoff = now - (ranges[range] ?? ranges['7d']);
    return snaps.filter((s: any) => Number(s.timestamp) >= cutoff);
  }

  // ── Current summary values ────────────────────────────────────────────────
  const currentTvl = $derived(
    collateralTotals.reduce((sum, ct) => sum + ct.amount * ct.price, 0)
  );
  const currentDebt = $derived(
    protocolStatus ? Number(protocolStatus.total_icusd_borrowed) / 1e8 : 0
  );
  const currentCR = $derived(
    protocolStatus ? (protocolStatus.total_collateral_ratio * 100) : 0
  );
  const currentVaultCount = $derived(
    collateralTotals.reduce((sum, ct) => sum + ct.vaultCount, 0)
  );
  const currentMode = $derived(
    protocolStatus ? Object.keys(protocolStatus.mode)[0] : '—'
  );

  // ── SVG Chart Helpers ─────────────────────────────────────────────────────
  const chartW = 600;
  const chartH = 160;

  interface ChartPoint { x: number; y: number }

  function buildPolyline(data: ChartPoint[], w: number, h: number): string {
    if (data.length < 2) return '';
    const xMin = data[0].x;
    const xMax = data[data.length - 1].x;
    const yMin = 0;
    const yMax = Math.max(...data.map(d => d.y)) * 1.1 || 1;
    const xRange = xMax - xMin || 1;
    return data.map(d => {
      const x = ((d.x - xMin) / xRange) * w;
      const y = h - ((d.y - yMin) / (yMax - yMin)) * h;
      return `${x.toFixed(1)},${y.toFixed(1)}`;
    }).join(' ');
  }

  function buildFill(points: string, w: number, h: number): string {
    if (!points) return '';
    const firstX = points.split(' ')[0]?.split(',')[0] ?? '0';
    return `${firstX},${h} ${points} ${w},${h}`;
  }

  function buildYLabels(data: ChartPoint[], count = 4): { y: number; label: string }[] {
    if (!data.length) return [];
    const yMax = Math.max(...data.map(d => d.y)) * 1.1 || 1;
    return Array.from({ length: count }, (_, i) => {
      const frac = (count - 1 - i) / (count - 1);
      const val = yMax * frac;
      return {
        y: (i / (count - 1)) * chartH,
        label: val >= 1_000_000
          ? `${(val / 1_000_000).toFixed(1)}M`
          : val >= 1_000
          ? `${(val / 1_000).toFixed(0)}k`
          : val.toFixed(1),
      };
    });
  }

  // Chart data derived from filtered snapshots
  const tvlData = $derived(filteredSnapshots.map((s: any) => ({
    x: Number(s.timestamp) / 1e6,
    y: Number(s.total_collateral_value_usd) / 1e8
  })));

  const debtData = $derived(filteredSnapshots.map((s: any) => ({
    x: Number(s.timestamp) / 1e6,
    y: Number(s.total_debt) / 1e8
  })));

  const crData = $derived(filteredSnapshots.map((s: any) => ({
    x: Number(s.timestamp) / 1e6,
    y: (s.total_collateral_ratio ?? 0) * 100
  })));

  const vaultCountData = $derived(filteredSnapshots.map((s: any) => ({
    x: Number(s.timestamp) / 1e6,
    y: Number(s.total_vault_count)
  })));

  const tvlPoints     = $derived(buildPolyline(tvlData, chartW, chartH));
  const debtPoints    = $derived(buildPolyline(debtData, chartW, chartH));
  const crPoints      = $derived(buildPolyline(crData, chartW, chartH));
  const vaultPoints   = $derived(buildPolyline(vaultCountData, chartW, chartH));

  const tvlFill       = $derived(buildFill(tvlPoints, chartW, chartH));
  const debtFill      = $derived(buildFill(debtPoints, chartW, chartH));
  const crFill        = $derived(buildFill(crPoints, chartW, chartH));
  const vaultFill     = $derived(buildFill(vaultPoints, chartW, chartH));

  const tvlLabels     = $derived(buildYLabels(tvlData));
  const debtLabels    = $derived(buildYLabels(debtData));
  const crLabels      = $derived(buildYLabels(crData));
  const vaultLabels   = $derived(buildYLabels(vaultCountData));

  // ── Fetch ─────────────────────────────────────────────────────────────────
  onMount(async () => {
    loading = true;
    try {
      await collateralStore.fetchSupportedCollateral();
      const [status, totals] = await Promise.all([
        publicActor.get_protocol_status(),
        publicActor.get_collateral_totals(),
      ]);
      protocolStatus = status;

      const collaterals = get(collateralStore).collaterals;
      collateralTotals = (totals as any[]).map((t: any) => {
        const ct = t.collateral_type?.toText?.() || '';
        const info = collaterals.find((c: any) => c.principal === ct);
        const decimals = info?.decimals ?? Number(t.decimals);
        return {
          symbol:     info?.symbol ?? ct.substring(0, 5),
          amount:     Number(t.total_collateral) / Math.pow(10, decimals),
          debt:       Number(t.total_debt) / 1e8,
          vaultCount: Number(t.vault_count),
          price:      Number(t.price),
          color:      info?.color ?? '#94A3B8',
        };
      }).filter((t: any) => t.amount > 0);

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

  <div class="page-header">
    <h1 class="page-title">Protocol Stats</h1>
    <div class="search-row"><SearchBar /></div>
  </div>

  <!-- Current Summary Cards -->
  {#if loading}
    <div class="state-msg">
      <div class="spinner"></div>
      <span>Loading protocol stats…</span>
    </div>
  {:else if protocolStatus}
    <div class="summary-grid">
      <DashboardCard
        label="Total TVL"
        value={'$' + currentTvl.toLocaleString('en-US', { maximumFractionDigits: 0 })}
        subtitle="Collateral value in USD"
      />
      <DashboardCard
        label="Total Debt"
        value={currentDebt.toLocaleString('en-US', { maximumFractionDigits: 0 }) + ' icUSD'}
        subtitle="Outstanding icUSD borrowed"
      />
      <DashboardCard
        label="Collateral Ratio"
        value={currentCR.toFixed(0) + '%'}
        subtitle={currentCR >= 200 ? 'Healthy' : currentCR >= 150 ? 'Caution' : 'Recovery mode'}
        trend={currentCR >= 200 ? 'up' : currentCR >= 150 ? 'neutral' : 'down'}
      />
      <DashboardCard
        label="Active Vaults"
        value={String(currentVaultCount)}
        subtitle={currentMode !== 'Normal' ? `Mode: ${currentMode}` : undefined}
      />
    </div>

    <!-- Per-Collateral Breakdown -->
    {#if collateralTotals.length > 0}
      <h2 class="section-title">Per-Collateral Breakdown</h2>
      <div class="collateral-table glass-card">
        <div class="ct-header">
          <span>Collateral</span>
          <span>Amount</span>
          <span>TVL (USD)</span>
          <span>Debt</span>
          <span>Vaults</span>
          <span>Price</span>
        </div>
        {#each collateralTotals as ct}
          <div class="ct-row">
            <span class="ct-symbol">
              <span class="ct-dot" style="background:{ct.color};"></span>
              {ct.symbol}
            </span>
            <span class="key-number">{ct.amount.toLocaleString('en-US', { maximumFractionDigits: 4 })}</span>
            <span class="key-number">${(ct.amount * ct.price).toLocaleString('en-US', { maximumFractionDigits: 0 })}</span>
            <span class="key-number">{ct.debt.toLocaleString('en-US', { maximumFractionDigits: 2 })} icUSD</span>
            <span class="key-number">{ct.vaultCount}</span>
            <span class="key-number">${ct.price.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 })}</span>
          </div>
        {/each}
      </div>
    {/if}

    <!-- Historical Charts -->
    <div class="historical-header">
      <h2 class="section-title" style="margin:0;">Historical Trends</h2>
      <div class="time-range-row">
        {#each timeRanges as tr}
          <button
            class="filter-btn"
            class:active={timeRange === tr.value}
            onclick={() => timeRange = tr.value}
          >
            {tr.label}
          </button>
        {/each}
      </div>
    </div>

    {#if $snapshotsLoading}
      <div class="state-msg">
        <div class="spinner"></div>
        <span>Loading historical data…</span>
      </div>
    {:else if filteredSnapshots.length < 2}
      <div class="state-msg">No snapshot data available for this range. Snapshots are captured hourly.</div>
    {:else}
      <div class="charts-grid">
        <!-- TVL -->
        <div class="chart-card glass-card">
          <h3 class="chart-title">TVL (USD)</h3>
          <div class="chart-wrap">
            <div class="y-labels">
              {#each tvlLabels as lbl}
                <span style="top:{lbl.y}px">{lbl.label}</span>
              {/each}
            </div>
            <svg viewBox="0 0 {chartW} {chartH}" class="chart-svg" preserveAspectRatio="none">
              <defs>
                <linearGradient id="tvl-grad" x1="0" y1="0" x2="0" y2="1">
                  <stop offset="0%"   stop-color="var(--rumi-safe)" stop-opacity="0.25" />
                  <stop offset="100%" stop-color="var(--rumi-safe)" stop-opacity="0.02" />
                </linearGradient>
              </defs>
              {#if tvlFill}<polygon points={tvlFill} fill="url(#tvl-grad)" />{/if}
              {#if tvlPoints}<polyline points={tvlPoints} fill="none" stroke="var(--rumi-safe)" stroke-width="2" stroke-linejoin="round" />{/if}
            </svg>
          </div>
        </div>

        <!-- Debt -->
        <div class="chart-card glass-card">
          <h3 class="chart-title">Total Debt (icUSD)</h3>
          <div class="chart-wrap">
            <div class="y-labels">
              {#each debtLabels as lbl}
                <span style="top:{lbl.y}px">{lbl.label}</span>
              {/each}
            </div>
            <svg viewBox="0 0 {chartW} {chartH}" class="chart-svg" preserveAspectRatio="none">
              <defs>
                <linearGradient id="debt-grad" x1="0" y1="0" x2="0" y2="1">
                  <stop offset="0%"   stop-color="var(--rumi-purple-accent)" stop-opacity="0.25" />
                  <stop offset="100%" stop-color="var(--rumi-purple-accent)" stop-opacity="0.02" />
                </linearGradient>
              </defs>
              {#if debtFill}<polygon points={debtFill} fill="url(#debt-grad)" />{/if}
              {#if debtPoints}<polyline points={debtPoints} fill="none" stroke="var(--rumi-purple-accent)" stroke-width="2" stroke-linejoin="round" />{/if}
            </svg>
          </div>
        </div>

        <!-- Collateral Ratio -->
        <div class="chart-card glass-card">
          <h3 class="chart-title">Collateral Ratio (%)</h3>
          <div class="chart-wrap">
            <div class="y-labels">
              {#each crLabels as lbl}
                <span style="top:{lbl.y}px">{lbl.label}</span>
              {/each}
            </div>
            <svg viewBox="0 0 {chartW} {chartH}" class="chart-svg" preserveAspectRatio="none">
              <defs>
                <linearGradient id="cr-grad" x1="0" y1="0" x2="0" y2="1">
                  <stop offset="0%"   stop-color="var(--rumi-caution)" stop-opacity="0.25" />
                  <stop offset="100%" stop-color="var(--rumi-caution)" stop-opacity="0.02" />
                </linearGradient>
              </defs>
              {#if crFill}<polygon points={crFill} fill="url(#cr-grad)" />{/if}
              {#if crPoints}<polyline points={crPoints} fill="none" stroke="var(--rumi-caution)" stroke-width="2" stroke-linejoin="round" />{/if}
            </svg>
          </div>
        </div>

        <!-- Vault Count -->
        <div class="chart-card glass-card">
          <h3 class="chart-title">Vault Count</h3>
          <div class="chart-wrap">
            <div class="y-labels">
              {#each vaultLabels as lbl}
                <span style="top:{lbl.y}px">{lbl.label}</span>
              {/each}
            </div>
            <svg viewBox="0 0 {chartW} {chartH}" class="chart-svg" preserveAspectRatio="none">
              <defs>
                <linearGradient id="vault-grad" x1="0" y1="0" x2="0" y2="1">
                  <stop offset="0%"   stop-color="#60a5fa" stop-opacity="0.25" />
                  <stop offset="100%" stop-color="#60a5fa" stop-opacity="0.02" />
                </linearGradient>
              </defs>
              {#if vaultFill}<polygon points={vaultFill} fill="url(#vault-grad)" />{/if}
              {#if vaultPoints}<polyline points={vaultPoints} fill="none" stroke="#60a5fa" stroke-width="2" stroke-linejoin="round" />{/if}
            </svg>
          </div>
        </div>
      </div>
    {/if}
  {/if}
</div>

<style>
  .stats-page { max-width: 1100px; margin: 0 auto; padding: 2rem 1rem; }

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

  /* Summary cards */
  .summary-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
    gap: 1rem;
    margin-bottom: 2rem;
  }

  /* Section title */
  .section-title { font-size: 1rem; font-weight: 600; margin: 0 0 0.75rem; color: var(--rumi-text-primary); }

  /* Per-collateral table */
  .collateral-table { padding: 0; overflow: hidden; margin-bottom: 2rem; border-radius: 0.75rem; }
  .ct-header, .ct-row {
    display: grid;
    grid-template-columns: 7rem 1fr 1fr 1fr 4rem 5rem;
    gap: 0.5rem;
    padding: 0.625rem 0.875rem;
    font-size: 0.8125rem;
    align-items: center;
  }
  .ct-header {
    color: var(--rumi-text-muted);
    border-bottom: 1px solid var(--rumi-border);
    font-weight: 500;
  }
  .ct-row {
    border-bottom: 1px solid var(--rumi-border);
    color: var(--rumi-text-secondary);
  }
  .ct-row:last-child { border-bottom: none; }
  .ct-row:hover { background: var(--rumi-bg-surface-2); }
  .ct-symbol { display: flex; align-items: center; gap: 0.5rem; font-weight: 500; color: var(--rumi-text-primary); }
  .ct-dot { width: 8px; height: 8px; border-radius: 50%; flex-shrink: 0; }

  /* Historical header */
  .historical-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    flex-wrap: wrap;
    gap: 0.75rem;
    margin-bottom: 1rem;
  }

  /* Time-range buttons */
  .time-range-row { display: flex; gap: 0.375rem; }
  .filter-btn {
    padding: 0.25rem 0.625rem;
    font-size: 0.8125rem;
    border: 1px solid var(--rumi-border);
    border-radius: 9999px;
    background: transparent;
    color: var(--rumi-text-secondary);
    cursor: pointer;
    transition: all 0.15s;
  }
  .filter-btn:hover { border-color: var(--rumi-border-hover); color: var(--rumi-text-primary); }
  .filter-btn.active { background: var(--rumi-purple-accent); color: white; border-color: var(--rumi-purple-accent); }

  /* Charts grid: 2-column on wider screens */
  .charts-grid {
    display: grid;
    grid-template-columns: 1fr;
    gap: 1rem;
  }
  @media (min-width: 768px) {
    .charts-grid { grid-template-columns: 1fr 1fr; }
  }

  .chart-card { padding: 1rem; border-radius: 0.75rem; }
  .chart-title { margin: 0 0 0.5rem; font-size: 0.8125rem; color: var(--rumi-text-muted); font-weight: 500; }

  .chart-wrap {
    position: relative;
    padding-left: 2.5rem; /* room for y-labels */
  }

  .y-labels {
    position: absolute;
    left: 0;
    top: 0;
    bottom: 0;
    width: 2.25rem;
    pointer-events: none;
  }
  .y-labels span {
    position: absolute;
    right: 0;
    transform: translateY(-50%);
    font-size: 0.625rem;
    color: var(--rumi-text-muted);
    white-space: nowrap;
  }

  .chart-svg { width: 100%; height: auto; display: block; }

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
