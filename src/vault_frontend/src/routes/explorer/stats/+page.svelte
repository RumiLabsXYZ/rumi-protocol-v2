<script lang="ts">
  import { onMount } from 'svelte';
  import StatCard from '$components/explorer/StatCard.svelte';
  import EntityLink from '$components/explorer/EntityLink.svelte';
  import StatusBadge from '$components/explorer/StatusBadge.svelte';
  import {
    fetchProtocolStatus, fetchCollateralTotals, fetchCollateralPrices,
    fetchCollateralConfigs, fetchAllVaults, fetchAllSnapshots
  } from '$services/explorer/explorerService';
  import {
    formatE8s, formatUsd, formatUsdRaw, formatCR, formatPercent, formatBps,
    getTokenSymbol, nsToDate
  } from '$utils/explorerHelpers';

  // ── State ──────────────────────────────────────────────────────────────
  let loading = $state(true);
  let protocolStatus: any = $state(null);
  let collateralTotals: any[] = $state([]);
  let collateralPrices: Map<string, number> = $state(new Map());
  let collateralConfigs: any[] = $state([]);
  let allVaults: any[] = $state([]);
  let allSnapshots: any[] = $state([]);

  type TimeRange = '24h' | '7d' | '30d' | '90d' | 'all';
  let timeRange: TimeRange = $state('7d');

  const timeRanges: { label: string; value: TimeRange }[] = [
    { label: '24h', value: '24h' },
    { label: '7d',  value: '7d' },
    { label: '30d', value: '30d' },
    { label: '90d', value: '90d' },
    { label: 'All', value: 'all' },
  ];

  // ── Derived: Current Metrics ───────────────────────────────────────────
  const currentTvl = $derived(
    collateralTotals.reduce((sum, t) => {
      const amount = Number(t.total_collateral) / Math.pow(10, Number(t.decimals));
      return sum + amount * Number(t.price);
    }, 0)
  );

  const currentDebt = $derived(
    protocolStatus ? Number(protocolStatus.total_icusd_borrowed) / 1e8 : 0
  );

  const currentCR = $derived(
    protocolStatus ? protocolStatus.total_collateral_ratio : 0
  );

  const currentVaultCount = $derived(allVaults.length);

  const currentMode = $derived(
    protocolStatus ? Object.keys(protocolStatus.mode)[0] : 'Unknown'
  );

  // ── Derived: Per-Collateral Enriched Rows ──────────────────────────────
  const collateralRows = $derived(
    collateralTotals.map((t) => {
      const principal = t.collateral_type?.toText?.() ?? String(t.collateral_type);
      const decimals = Number(t.decimals);
      const amount = Number(t.total_collateral) / Math.pow(10, decimals);
      const price = Number(t.price);
      const debt = Number(t.total_debt) / 1e8;
      const vaultCount = Number(t.vault_count);

      // Find matching config
      const config = collateralConfigs.find((c) => {
        const cp = c.ledger_canister_id?.toText?.() ?? String(c.ledger_canister_id);
        return cp === principal;
      });

      const interestRate = config ? Number(config.interest_rate_apr) : 0;
      const borrowingFee = config ? Number(config.borrowing_fee) : 0;
      const debtCeiling = config ? Number(config.debt_ceiling) / 1e8 : 0;
      const status = config?.status ? Object.keys(config.status)[0] : 'Unknown';

      // Utilization = total_debt / debt_ceiling (if ceiling is not max)
      const utilization = debtCeiling > 0 && debtCeiling < 1e10
        ? debt / debtCeiling
        : 0;

      return {
        principal,
        symbol: t.symbol ?? getTokenSymbol(principal),
        amount,
        price,
        lockedUsd: amount * price,
        debt,
        vaultCount,
        interestRate,
        borrowingFee,
        debtCeiling,
        utilization,
        status,
      };
    })
  );

  // ── Derived: Filtered Snapshots ────────────────────────────────────────
  const filteredSnapshots = $derived(filterByTimeRange(allSnapshots, timeRange));

  function filterByTimeRange(snaps: any[], range: TimeRange): any[] {
    if (!snaps.length || range === 'all') return snaps;
    const nowNs = Date.now() * 1_000_000;
    const ranges: Record<string, number> = {
      '24h': 24 * 3600e9,
      '7d':  7  * 24 * 3600e9,
      '30d': 30 * 24 * 3600e9,
      '90d': 90 * 24 * 3600e9,
    };
    const cutoff = nowNs - (ranges[range] ?? ranges['7d']);
    return snaps.filter((s) => Number(s.timestamp) >= cutoff);
  }

  // ── Chart Data ─────────────────────────────────────────────────────────
  interface ChartPoint { x: number; y: number }

  const tvlData: ChartPoint[] = $derived(
    filteredSnapshots.map((s) => ({
      x: Number(s.timestamp) / 1e6,
      y: Number(s.total_collateral_value_usd) / 1e8,
    }))
  );

  const debtData: ChartPoint[] = $derived(
    filteredSnapshots.map((s) => ({
      x: Number(s.timestamp) / 1e6,
      y: Number(s.total_debt) / 1e8,
    }))
  );

  const crData: ChartPoint[] = $derived(
    filteredSnapshots.map((s) => {
      const collUsd = Number(s.total_collateral_value_usd) / 1e8;
      const debt = Number(s.total_debt) / 1e8;
      return {
        x: Number(s.timestamp) / 1e6,
        y: debt > 0 ? (collUsd / debt) * 100 : 0,
      };
    })
  );

  // ── SVG Chart Helpers ──────────────────────────────────────────────────
  const chartW = 600;
  const chartH = 160;
  const chartPad = 4;

  function buildPolyline(data: ChartPoint[]): string {
    if (data.length < 2) return '';
    const xMin = data[0].x;
    const xMax = data[data.length - 1].x;
    const yMin = 0;
    const yMax = Math.max(...data.map((d) => d.y)) * 1.1 || 1;
    const xRange = xMax - xMin || 1;
    return data
      .map((d) => {
        const x = chartPad + ((d.x - xMin) / xRange) * (chartW - chartPad * 2);
        const y = chartPad + (chartH - chartPad * 2) - ((d.y - yMin) / (yMax - yMin)) * (chartH - chartPad * 2);
        return `${x.toFixed(1)},${y.toFixed(1)}`;
      })
      .join(' ');
  }

  function buildFill(points: string): string {
    if (!points) return '';
    const firstX = points.split(' ')[0]?.split(',')[0] ?? '0';
    return `${firstX},${chartH} ${points} ${chartW - chartPad},${chartH}`;
  }

  function buildYLabels(data: ChartPoint[], count = 4): { y: number; label: string }[] {
    if (!data.length) return [];
    const yMax = Math.max(...data.map((d) => d.y)) * 1.1 || 1;
    return Array.from({ length: count }, (_, i) => {
      const frac = (count - 1 - i) / (count - 1);
      const val = yMax * frac;
      return {
        y: chartPad + (i / (count - 1)) * (chartH - chartPad * 2),
        label:
          val >= 1_000_000
            ? `${(val / 1_000_000).toFixed(1)}M`
            : val >= 1_000
              ? `${(val / 1_000).toFixed(0)}k`
              : val.toFixed(1),
      };
    });
  }

  function formatDateLabel(ms: number): string {
    const d = new Date(ms);
    return `${d.getMonth() + 1}/${d.getDate()}`;
  }

  // Derived polyline strings
  const tvlPoints = $derived(buildPolyline(tvlData));
  const debtPoints = $derived(buildPolyline(debtData));
  const crPoints = $derived(buildPolyline(crData));

  const tvlFill = $derived(buildFill(tvlPoints));
  const debtFill = $derived(buildFill(debtPoints));
  const crFill = $derived(buildFill(crPoints));

  const tvlLabels = $derived(buildYLabels(tvlData));
  const debtLabels = $derived(buildYLabels(debtData));
  const crLabels = $derived(buildYLabels(crData));

  // Latest chart values for display
  const latestTvlChart = $derived(tvlData.length > 0 ? tvlData[tvlData.length - 1].y : 0);
  const latestDebtChart = $derived(debtData.length > 0 ? debtData[debtData.length - 1].y : 0);
  const latestCrChart = $derived(crData.length > 0 ? crData[crData.length - 1].y : 0);

  // ── Fetch ──────────────────────────────────────────────────────────────
  onMount(async () => {
    loading = true;
    try {
      const [status, totals, prices, configs, vaults, snapshots] = await Promise.all([
        fetchProtocolStatus(),
        fetchCollateralTotals(),
        fetchCollateralPrices(),
        fetchCollateralConfigs(),
        fetchAllVaults(),
        fetchAllSnapshots(),
      ]);
      protocolStatus = status;
      collateralTotals = totals ?? [];
      collateralPrices = prices ?? new Map();
      collateralConfigs = configs ?? [];
      allVaults = vaults ?? [];
      allSnapshots = snapshots ?? [];
    } catch (e) {
      console.error('Failed to load stats:', e);
    } finally {
      loading = false;
    }
  });
</script>

<svelte:head>
  <title>Protocol Stats | Rumi Explorer</title>
</svelte:head>

<div class="max-w-6xl mx-auto px-4 py-8 space-y-8">
  <!-- Header -->
  <div>
    <a href="/explorer" class="text-sm text-blue-400 hover:underline">&larr; Back to Explorer</a>
    <h1 class="text-2xl font-bold text-white mt-3">Protocol Stats</h1>
  </div>

  {#if loading}
    <div class="flex items-center justify-center gap-3 py-16 text-gray-400">
      <div class="w-5 h-5 border-2 border-gray-600 border-t-blue-400 rounded-full animate-spin"></div>
      <span>Loading protocol stats...</span>
    </div>
  {:else}
    <!-- Current Metrics -->
    <section>
      <h2 class="text-sm font-semibold uppercase tracking-wide text-gray-400 mb-3">Current Metrics</h2>
      <div class="grid grid-cols-2 md:grid-cols-3 lg:grid-cols-5 gap-3">
        <StatCard
          label="Total TVL"
          value={formatUsdRaw(currentTvl)}
          subtitle="Collateral value in USD"
        />
        <StatCard
          label="Total Debt"
          value={`${currentDebt.toLocaleString('en-US', { maximumFractionDigits: 0 })} icUSD`}
          subtitle="Outstanding icUSD"
        />
        <StatCard
          label="System CR"
          value={formatCR(currentCR)}
          subtitle={currentCR >= 2 ? 'Healthy' : currentCR >= 1.5 ? 'Caution' : 'Recovery mode'}
          trend={currentCR >= 2 ? 'up' : currentCR >= 1.5 ? 'neutral' : 'down'}
        />
        <StatCard
          label="Vault Count"
          value={String(currentVaultCount)}
          subtitle="Active vaults"
        />
        <StatCard
          label="Protocol Mode"
          value={currentMode}
          subtitle={currentMode === 'Normal' ? 'All systems healthy' : 'Elevated risk'}
          trend={currentMode === 'Normal' ? 'up' : 'down'}
        />
      </div>
    </section>

    <!-- Per-Collateral Breakdown -->
    {#if collateralRows.length > 0}
      <section>
        <h2 class="text-sm font-semibold uppercase tracking-wide text-gray-400 mb-3">Per-Collateral Breakdown</h2>
        <div class="bg-gray-800/50 border border-gray-700/50 rounded-xl overflow-hidden">
          <div class="overflow-x-auto">
            <table class="w-full text-sm">
              <thead>
                <tr class="border-b border-gray-700/50 text-gray-400 text-xs uppercase tracking-wide">
                  <th class="text-left px-4 py-3 font-medium">Token</th>
                  <th class="text-right px-4 py-3 font-medium">Price</th>
                  <th class="text-right px-4 py-3 font-medium">Total Locked</th>
                  <th class="text-right px-4 py-3 font-medium">Locked (USD)</th>
                  <th class="text-right px-4 py-3 font-medium">Total Debt</th>
                  <th class="text-right px-4 py-3 font-medium">Vaults</th>
                  <th class="text-right px-4 py-3 font-medium">Interest</th>
                  <th class="text-right px-4 py-3 font-medium">Borrow Fee</th>
                  <th class="text-right px-4 py-3 font-medium">Debt Ceiling</th>
                  <th class="text-right px-4 py-3 font-medium">Utilization</th>
                  <th class="text-center px-4 py-3 font-medium">Status</th>
                </tr>
              </thead>
              <tbody>
                {#each collateralRows as row}
                  <tr class="border-b border-gray-700/30 hover:bg-gray-700/20 transition-colors">
                    <td class="px-4 py-3">
                      <EntityLink type="token" value={row.principal} label={row.symbol} />
                    </td>
                    <td class="text-right px-4 py-3 font-mono text-gray-200">
                      {formatUsdRaw(row.price)}
                    </td>
                    <td class="text-right px-4 py-3 font-mono text-gray-200">
                      {row.amount.toLocaleString('en-US', { maximumFractionDigits: 4 })}
                    </td>
                    <td class="text-right px-4 py-3 font-mono text-white font-medium">
                      {formatUsdRaw(row.lockedUsd)}
                    </td>
                    <td class="text-right px-4 py-3 font-mono text-gray-200">
                      {row.debt.toLocaleString('en-US', { maximumFractionDigits: 2 })} icUSD
                    </td>
                    <td class="text-right px-4 py-3 font-mono text-gray-200">
                      {row.vaultCount}
                    </td>
                    <td class="text-right px-4 py-3 font-mono text-gray-200">
                      {formatPercent(row.interestRate)}
                    </td>
                    <td class="text-right px-4 py-3 font-mono text-gray-200">
                      {formatPercent(row.borrowingFee)}
                    </td>
                    <td class="text-right px-4 py-3 font-mono text-gray-200">
                      {row.debtCeiling >= 1e10 ? 'Unlimited' : formatUsdRaw(row.debtCeiling)}
                    </td>
                    <td class="text-right px-4 py-3 font-mono">
                      {#if row.debtCeiling >= 1e10}
                        <span class="text-gray-500">--</span>
                      {:else}
                        <span class={row.utilization > 0.9 ? 'text-red-400' : row.utilization > 0.7 ? 'text-yellow-400' : 'text-emerald-400'}>
                          {formatPercent(row.utilization)}
                        </span>
                      {/if}
                    </td>
                    <td class="text-center px-4 py-3">
                      <StatusBadge status={row.status} />
                    </td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        </div>
      </section>
    {/if}

    <!-- Historical Charts -->
    <section>
      <div class="flex items-center justify-between flex-wrap gap-3 mb-4">
        <h2 class="text-sm font-semibold uppercase tracking-wide text-gray-400">Historical Trends</h2>
        <div class="flex gap-1">
          {#each timeRanges as tr}
            <button
              class="px-3 py-1 text-xs rounded-full border transition-all {timeRange === tr.value
                ? 'bg-blue-500 text-white border-blue-500'
                : 'bg-transparent text-gray-400 border-gray-600 hover:border-gray-400 hover:text-gray-200'}"
              onclick={() => timeRange = tr.value}
            >
              {tr.label}
            </button>
          {/each}
        </div>
      </div>

      {#if filteredSnapshots.length < 2}
        <div class="flex items-center justify-center py-16 text-gray-500">
          No historical data available for this range. Snapshots are captured hourly.
        </div>
      {:else}
        <div class="grid grid-cols-1 lg:grid-cols-2 gap-4">
          <!-- TVL Chart -->
          <div class="bg-gray-800/50 border border-gray-700/50 rounded-xl p-5">
            <div class="flex items-baseline justify-between mb-3">
              <h3 class="text-xs font-medium text-gray-400 uppercase tracking-wide">TVL (USD)</h3>
              <span class="text-sm font-mono text-emerald-400">{formatUsdRaw(latestTvlChart)}</span>
            </div>
            <div class="relative" style="padding-left: 2.5rem;">
              <div class="absolute left-0 top-0 bottom-0 w-9 pointer-events-none">
                {#each tvlLabels as lbl}
                  <span
                    class="absolute right-0 text-[10px] text-gray-500 -translate-y-1/2 whitespace-nowrap"
                    style="top: {lbl.y}px"
                  >{lbl.label}</span>
                {/each}
              </div>
              <svg viewBox="0 0 {chartW} {chartH}" class="w-full h-auto block" preserveAspectRatio="none">
                <defs>
                  <linearGradient id="tvl-grad" x1="0" y1="0" x2="0" y2="1">
                    <stop offset="0%" stop-color="#10b981" stop-opacity="0.25" />
                    <stop offset="100%" stop-color="#10b981" stop-opacity="0.02" />
                  </linearGradient>
                </defs>
                {#if tvlFill}<polygon points={tvlFill} fill="url(#tvl-grad)" />{/if}
                {#if tvlPoints}<polyline points={tvlPoints} fill="none" stroke="#10b981" stroke-width="2" stroke-linejoin="round" />{/if}
              </svg>
            </div>
          </div>

          <!-- Debt Chart -->
          <div class="bg-gray-800/50 border border-gray-700/50 rounded-xl p-5">
            <div class="flex items-baseline justify-between mb-3">
              <h3 class="text-xs font-medium text-gray-400 uppercase tracking-wide">Total Debt (icUSD)</h3>
              <span class="text-sm font-mono text-purple-400">{formatUsdRaw(latestDebtChart)}</span>
            </div>
            <div class="relative" style="padding-left: 2.5rem;">
              <div class="absolute left-0 top-0 bottom-0 w-9 pointer-events-none">
                {#each debtLabels as lbl}
                  <span
                    class="absolute right-0 text-[10px] text-gray-500 -translate-y-1/2 whitespace-nowrap"
                    style="top: {lbl.y}px"
                  >{lbl.label}</span>
                {/each}
              </div>
              <svg viewBox="0 0 {chartW} {chartH}" class="w-full h-auto block" preserveAspectRatio="none">
                <defs>
                  <linearGradient id="debt-grad" x1="0" y1="0" x2="0" y2="1">
                    <stop offset="0%" stop-color="#a855f7" stop-opacity="0.25" />
                    <stop offset="100%" stop-color="#a855f7" stop-opacity="0.02" />
                  </linearGradient>
                </defs>
                {#if debtFill}<polygon points={debtFill} fill="url(#debt-grad)" />{/if}
                {#if debtPoints}<polyline points={debtPoints} fill="none" stroke="#a855f7" stroke-width="2" stroke-linejoin="round" />{/if}
              </svg>
            </div>
          </div>

          <!-- CR Chart -->
          <div class="bg-gray-800/50 border border-gray-700/50 rounded-xl p-5 lg:col-span-2">
            <div class="flex items-baseline justify-between mb-3">
              <h3 class="text-xs font-medium text-gray-400 uppercase tracking-wide">System Collateral Ratio (%)</h3>
              <span class="text-sm font-mono text-yellow-400">{latestCrChart.toFixed(1)}%</span>
            </div>
            <div class="relative" style="padding-left: 2.5rem;">
              <div class="absolute left-0 top-0 bottom-0 w-9 pointer-events-none">
                {#each crLabels as lbl}
                  <span
                    class="absolute right-0 text-[10px] text-gray-500 -translate-y-1/2 whitespace-nowrap"
                    style="top: {lbl.y}px"
                  >{lbl.label}</span>
                {/each}
              </div>
              <svg viewBox="0 0 {chartW} {chartH}" class="w-full h-auto block" preserveAspectRatio="none">
                <defs>
                  <linearGradient id="cr-grad" x1="0" y1="0" x2="0" y2="1">
                    <stop offset="0%" stop-color="#eab308" stop-opacity="0.25" />
                    <stop offset="100%" stop-color="#eab308" stop-opacity="0.02" />
                  </linearGradient>
                </defs>
                {#if crFill}<polygon points={crFill} fill="url(#cr-grad)" />{/if}
                {#if crPoints}<polyline points={crPoints} fill="none" stroke="#eab308" stroke-width="2" stroke-linejoin="round" />{/if}
              </svg>
            </div>
          </div>
        </div>
      {/if}
    </section>
  {/if}
</div>
