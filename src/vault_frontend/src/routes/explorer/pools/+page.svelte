<script lang="ts">
  import { onMount } from 'svelte';
  import PoolBalanceBar from '$components/explorer/PoolBalanceBar.svelte';
  import {
    fetchPegStatus, fetchApys, fetchSwapSeries,
    fetchStabilitySeries, fetchThreePoolSeries, fetchTradeActivity
  } from '$services/explorer/analyticsService';
  import {
    fetchThreePoolStatus, fetchStabilityPoolStatus
  } from '$services/explorer/explorerService';
  import { e8sToNumber, formatCompact, nsToDate, formatDateShort } from '$utils/explorerChartHelpers';
  import type { PegStatus } from '$declarations/rumi_analytics/rumi_analytics.did';

  // 3Pool state
  let pegStatus: PegStatus | null = $state(null);
  let lpApy: number | null = $state(null);
  let spApy: number | null = $state(null);
  let threePoolStatus: any = $state(null);
  let swapVolume24h = $state(0);
  let swapFees24h = $state(0);
  let swapCount24h = $state(0);
  let poolLoading = $state(true);

  // Stability Pool state
  let spStatus: any = $state(null);
  let spLoading = $state(true);

  // Historical data
  let swapSeries: any[] = $state([]);
  let stabilitySeries: any[] = $state([]);
  let threePoolSeries: any[] = $state([]);
  let seriesLoading = $state(true);

  // 3Pool token colors
  const POOL_TOKENS = [
    { symbol: 'icUSD', color: '#2DD4BF' },
    { symbol: 'ckUSDT', color: '#26A17B' },
    { symbol: 'ckUSDC', color: '#2775CA' },
  ];

  onMount(async () => {
    const [
      pegResult, apyResult, poolStatusResult, spStatusResult,
      tradeResult, swapSeriesResult, stabilitySeriesResult, threePoolSeriesResult
    ] = await Promise.allSettled([
      fetchPegStatus(),
      fetchApys(),
      fetchThreePoolStatus(),
      fetchStabilityPoolStatus(),
      fetchTradeActivity(),
      fetchSwapSeries(90),
      fetchStabilitySeries(90),
      fetchThreePoolSeries(90),
    ]);

    // Peg status
    if (pegResult.status === 'fulfilled') {
      pegStatus = pegResult.value ?? null;
    }

    // APYs
    if (apyResult.status === 'fulfilled' && apyResult.value) {
      lpApy = apyResult.value.lp_apy_pct?.[0] ?? null;
      spApy = apyResult.value.sp_apy_pct?.[0] ?? null;
    }

    // 3Pool status
    if (poolStatusResult.status === 'fulfilled') {
      threePoolStatus = poolStatusResult.value;
    }

    // Trade activity (24h volume)
    if (tradeResult.status === 'fulfilled' && tradeResult.value) {
      swapVolume24h = e8sToNumber(tradeResult.value.total_volume_e8s);
      swapFees24h = e8sToNumber(tradeResult.value.total_fees_e8s);
      swapCount24h = tradeResult.value.total_swaps;
    }
    poolLoading = false;

    // Stability Pool
    if (spStatusResult.status === 'fulfilled') {
      spStatus = spStatusResult.value;
    }
    spLoading = false;

    // Historical series
    if (swapSeriesResult.status === 'fulfilled') {
      swapSeries = swapSeriesResult.value ?? [];
    }
    if (stabilitySeriesResult.status === 'fulfilled') {
      stabilitySeries = stabilitySeriesResult.value ?? [];
    }
    if (threePoolSeriesResult.status === 'fulfilled') {
      threePoolSeries = threePoolSeriesResult.value ?? [];
    }
    seriesLoading = false;
  });

  // Derived: pool balances for bar
  const poolTokenBalances = $derived.by(() => {
    if (!pegStatus?.pool_balances || pegStatus.pool_balances.length < 3) {
      return POOL_TOKENS.map(t => ({ ...t, balance: 0 }));
    }
    return POOL_TOKENS.map((t, i) => ({
      ...t,
      balance: e8sToNumber(pegStatus!.pool_balances[i]),
    }));
  });

  // Derived: virtual price
  const virtualPrice = $derived(
    pegStatus?.virtual_price ? Number(pegStatus.virtual_price) / 1e18 : null
  );

  // Derived: max imbalance
  const maxImbalance = $derived(pegStatus?.max_imbalance_pct ?? null);

  // Derived: peg health color
  const pegHealthColor = $derived.by(() => {
    if (maxImbalance == null) return 'text-gray-500';
    if (maxImbalance < 2) return 'text-teal-400';
    if (maxImbalance < 5) return 'text-violet-400';
    return 'text-pink-400';
  });

  const pegHealthLabel = $derived.by(() => {
    if (maxImbalance == null) return '--';
    if (maxImbalance < 2) return 'Healthy';
    if (maxImbalance < 5) return 'Mild Imbalance';
    return 'Significant Imbalance';
  });

  // Derived: SP total deposits
  const spTotalDeposits = $derived.by(() => {
    if (!spStatus) return 0;
    // Stability pool status may have total_deposits or total_deposits_e8s
    const raw = spStatus.total_deposits ?? spStatus.total_deposits_e8s ?? 0n;
    return e8sToNumber(raw);
  });

  const spDepositorCount = $derived(
    spStatus?.depositor_count ?? spStatus?.total_depositors ?? 0
  );

  const spLiquidationCount = $derived(
    spStatus?.liquidation_count ?? spStatus?.total_liquidations ?? 0
  );

  // Derived: LP supply from 3pool series
  const latestLpSupply = $derived.by(() => {
    if (threePoolSeries.length === 0) return null;
    const latest = threePoolSeries[threePoolSeries.length - 1];
    return latest?.lp_total_supply ? e8sToNumber(latest.lp_total_supply) : null;
  });

  // Derived: total pool TVL
  const poolTvl = $derived(poolTokenBalances.reduce((s, t) => s + t.balance, 0));

  function formatApy(val: number | null): string {
    if (val == null) return '--';
    return `${val.toFixed(2)}%`;
  }
</script>

<svelte:head>
  <title>Pools | Rumi Explorer</title>
</svelte:head>

<div class="space-y-6">
  <!-- ── 3Pool Section ──────────────────────────────────────────────── -->
  <div>
    <h2 class="text-sm font-medium text-gray-400 uppercase tracking-wider mb-3">3Pool (icUSD / ckUSDT / ckUSDC)</h2>

    {#if poolLoading}
      <div class="explorer-card flex items-center justify-center py-12">
        <div class="w-5 h-5 border-2 border-gray-600 border-t-teal-400 rounded-full animate-spin"></div>
      </div>
    {:else}
      <!-- Vitals Grid -->
      <div class="grid grid-cols-2 md:grid-cols-4 gap-3 mb-4">
        <div class="explorer-card">
          <div class="text-xs text-gray-500 mb-1">Peg Status</div>
          <div class="text-lg font-semibold {pegHealthColor}">{pegHealthLabel}</div>
          {#if maxImbalance != null}
            <div class="text-xs text-gray-500 mt-0.5">Max imbalance: {maxImbalance.toFixed(2)}%</div>
          {/if}
        </div>
        <div class="explorer-card">
          <div class="text-xs text-gray-500 mb-1">Virtual Price</div>
          <div class="text-lg font-semibold tabular-nums text-gray-200">
            {virtualPrice != null ? virtualPrice.toFixed(6) : '--'}
          </div>
          <div class="text-xs text-gray-500 mt-0.5">1 3USD = {virtualPrice != null ? `$${virtualPrice.toFixed(4)}` : '--'}</div>
        </div>
        <div class="explorer-card">
          <div class="text-xs text-gray-500 mb-1">Pool TVL</div>
          <div class="text-lg font-semibold tabular-nums text-gray-200">
            ${formatCompact(poolTvl)}
          </div>
          {#if latestLpSupply != null}
            <div class="text-xs text-gray-500 mt-0.5">{formatCompact(latestLpSupply)} 3USD LP</div>
          {/if}
        </div>
        <div class="explorer-card">
          <div class="text-xs text-gray-500 mb-1">LP APY (7d)</div>
          <div class="text-lg font-semibold tabular-nums {lpApy != null && lpApy > 0 ? 'text-teal-400' : 'text-gray-400'}">
            {formatApy(lpApy)}
          </div>
        </div>
      </div>

      <!-- Pool Balance Distribution -->
      <div class="explorer-card mb-4">
        <h3 class="text-sm font-medium text-gray-300 mb-3">Pool Composition</h3>
        <PoolBalanceBar tokens={poolTokenBalances} />
        <div class="grid grid-cols-3 gap-3 mt-3">
          {#each poolTokenBalances as token}
            <div class="text-center">
              <div class="text-sm font-medium tabular-nums text-gray-200">{formatCompact(token.balance)}</div>
              <div class="text-xs text-gray-500">{token.symbol}</div>
            </div>
          {/each}
        </div>
      </div>

      <!-- 24h Trading Activity -->
      <div class="explorer-card">
        <h3 class="text-sm font-medium text-gray-300 mb-3">24h Trading Activity</h3>
        <div class="grid grid-cols-3 gap-4">
          <div>
            <div class="text-xs text-gray-500 mb-1">Volume</div>
            <div class="text-lg font-semibold tabular-nums text-gray-200">${formatCompact(swapVolume24h)}</div>
          </div>
          <div>
            <div class="text-xs text-gray-500 mb-1">Fees</div>
            <div class="text-lg font-semibold tabular-nums text-gray-200">${formatCompact(swapFees24h)}</div>
          </div>
          <div>
            <div class="text-xs text-gray-500 mb-1">Swaps</div>
            <div class="text-lg font-semibold tabular-nums text-gray-200">{swapCount24h}</div>
          </div>
        </div>
      </div>
    {/if}
  </div>

  <!-- ── Stability Pool Section ──────────────────────────────────────── -->
  <div>
    <h2 class="text-sm font-medium text-gray-400 uppercase tracking-wider mb-3">Stability Pool</h2>

    {#if spLoading}
      <div class="explorer-card flex items-center justify-center py-12">
        <div class="w-5 h-5 border-2 border-gray-600 border-t-teal-400 rounded-full animate-spin"></div>
      </div>
    {:else}
      <div class="grid grid-cols-2 md:grid-cols-4 gap-3 mb-4">
        <div class="explorer-card">
          <div class="text-xs text-gray-500 mb-1">Total Deposits</div>
          <div class="text-lg font-semibold tabular-nums text-gray-200">
            {formatCompact(spTotalDeposits)} icUSD
          </div>
        </div>
        <div class="explorer-card">
          <div class="text-xs text-gray-500 mb-1">SP APY (7d)</div>
          <div class="text-lg font-semibold tabular-nums {spApy != null && spApy > 0 ? 'text-teal-400' : 'text-gray-400'}">
            {formatApy(spApy)}
          </div>
        </div>
        <div class="explorer-card">
          <div class="text-xs text-gray-500 mb-1">Depositors</div>
          <div class="text-lg font-semibold tabular-nums text-gray-200">
            {Number(spDepositorCount)}
          </div>
        </div>
        <div class="explorer-card">
          <div class="text-xs text-gray-500 mb-1">Liquidations</div>
          <div class="text-lg font-semibold tabular-nums text-gray-200">
            {Number(spLiquidationCount)}
          </div>
        </div>
      </div>
    {/if}
  </div>

  <!-- ── Historical Charts ──────────────────────────────────────────── -->
  <div>
    <h2 class="text-sm font-medium text-gray-400 uppercase tracking-wider mb-3">Historical Data</h2>

    {#if seriesLoading}
      <div class="explorer-card flex items-center justify-center py-12">
        <div class="w-5 h-5 border-2 border-gray-600 border-t-teal-400 rounded-full animate-spin"></div>
      </div>
    {:else}
      <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
        <!-- Swap Volume History -->
        <div class="explorer-card">
          <h3 class="text-sm font-medium text-gray-300 mb-3">Daily Swap Volume (90d)</h3>
          {#if swapSeries.length === 0}
            <div class="flex items-center justify-center text-gray-500 text-sm py-8">No data</div>
          {:else}
            <div class="space-y-1.5">
              {#each swapSeries.slice(-14) as day}
                {@const vol = e8sToNumber(day.three_pool_volume_e8s) + e8sToNumber(day.amm_volume_e8s)}
                {@const maxVol = Math.max(...swapSeries.slice(-14).map((d: any) => e8sToNumber(d.three_pool_volume_e8s) + e8sToNumber(d.amm_volume_e8s)))}
                {@const pct = maxVol > 0 ? (vol / maxVol) * 100 : 0}
                <div class="flex items-center gap-2 text-xs">
                  <span class="w-14 text-gray-500 tabular-nums flex-shrink-0">
                    {formatDateShort(nsToDate(day.timestamp_ns))}
                  </span>
                  <div class="flex-1 h-4 rounded-sm overflow-hidden" style="background: var(--rumi-bg-surface2);">
                    <div class="h-full rounded-sm bg-teal-400/40" style="width: {pct}%;"></div>
                  </div>
                  <span class="w-16 text-right text-gray-400 tabular-nums flex-shrink-0">
                    ${formatCompact(vol)}
                  </span>
                </div>
              {/each}
            </div>
          {/if}
        </div>

        <!-- Stability Pool Deposits History -->
        <div class="explorer-card">
          <h3 class="text-sm font-medium text-gray-300 mb-3">SP Deposits (90d)</h3>
          {#if stabilitySeries.length === 0}
            <div class="flex items-center justify-center text-gray-500 text-sm py-8">No data</div>
          {:else}
            <div class="space-y-1.5">
              {#each stabilitySeries.slice(-14) as day}
                {@const deps = e8sToNumber(day.total_deposits_e8s)}
                {@const maxDeps = Math.max(...stabilitySeries.slice(-14).map((d: any) => e8sToNumber(d.total_deposits_e8s)))}
                {@const pct = maxDeps > 0 ? (deps / maxDeps) * 100 : 0}
                <div class="flex items-center gap-2 text-xs">
                  <span class="w-14 text-gray-500 tabular-nums flex-shrink-0">
                    {formatDateShort(nsToDate(day.timestamp_ns))}
                  </span>
                  <div class="flex-1 h-4 rounded-sm overflow-hidden" style="background: var(--rumi-bg-surface2);">
                    <div class="h-full rounded-sm bg-violet-400/40" style="width: {pct}%;"></div>
                  </div>
                  <span class="w-16 text-right text-gray-400 tabular-nums flex-shrink-0">
                    {formatCompact(deps)}
                  </span>
                </div>
              {/each}
            </div>
          {/if}
        </div>
      </div>
    {/if}
  </div>
</div>
