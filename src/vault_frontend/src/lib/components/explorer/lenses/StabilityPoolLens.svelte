<script lang="ts">
  import { onMount } from 'svelte';
  import LensHealthStrip from '../LensHealthStrip.svelte';
  import LensActivityPanel from '../LensActivityPanel.svelte';
  import MiniAreaChart from '../MiniAreaChart.svelte';
  import {
    fetchStabilitySeries, fetchApys,
  } from '$services/explorer/analyticsService';
  import {
    fetchStabilityPoolStatus, fetchStabilityPoolLiquidations,
  } from '$services/explorer/explorerService';
  import { e8sToNumber, formatCompact, CHART_COLORS, getCollateralSymbol } from '$utils/explorerChartHelpers';

  let poolStatus: any = $state(null);
  let series: any[] = $state([]);
  let liquidations: any[] = $state([]);
  let spApy: number | null = $state(null);
  let loading = $state(true);

  onMount(async () => {
    try {
      const [stR, seR, lqR, apR] = await Promise.allSettled([
        fetchStabilityPoolStatus(),
        fetchStabilitySeries(90),
        fetchStabilityPoolLiquidations(50),
        fetchApys(),
      ]);
      if (stR.status === 'fulfilled') poolStatus = stR.value;
      if (seR.status === 'fulfilled') series = seR.value ?? [];
      if (lqR.status === 'fulfilled') liquidations = lqR.value ?? [];
      if (apR.status === 'fulfilled' && apR.value) {
        const v = apR.value.sp_apy_pct?.[0];
        if (typeof v === 'number' && v > 0) spApy = v;
      }
    } catch (err) {
      console.error('[StabilityPoolLens] onMount error:', err);
    } finally {
      loading = false;
    }
  });

  const totalDeposits = $derived(poolStatus ? e8sToNumber(poolStatus.total_deposits_e8s ?? 0n) : 0);
  const depositors = $derived(poolStatus ? Number(poolStatus.total_depositors ?? 0n) : 0);
  const totalLiquidations = $derived(poolStatus ? Number(poolStatus.total_liquidations_executed ?? 0n) : 0);
  const eligibleCoverage = $derived.by(() => {
    if (!poolStatus?.eligible_icusd_per_collateral) return 0;
    let sum = 0;
    for (const [, v] of poolStatus.eligible_icusd_per_collateral) sum += Number(v);
    return sum / 1e8;
  });

  const depositSeries = $derived(
    series.map((r: any) => ({ t: Number(r.timestamp_ns) / 1_000_000, v: e8sToNumber(r.total_deposits_e8s ?? 0n) }))
  );
  const liquidationSeries = $derived(
    series.map((r: any) => ({ t: Number(r.timestamp_ns) / 1_000_000, v: Number(r.total_liquidations_executed ?? 0n) }))
  );

  const collateralGains = $derived.by(() => {
    // PoolStatus.collateral_gains is the aggregate collateral in the pool,
    // each entry a (collateral principal, raw amount) pair.
    const perCol = poolStatus?.collateral_gains ?? [];
    return perCol.map(([p, v]: [any, any]) => {
      const pid = typeof p === 'object' && p.toText ? p.toText() : String(p);
      return {
        principal: pid,
        symbol: getCollateralSymbol(pid),
        amount: Number(v),
      };
    });
  });

  const healthMetrics = $derived.by(() => [
    { label: 'Deposits', value: `$${formatCompact(totalDeposits)}`, sub: 'icUSD' },
    { label: 'Depositors', value: depositors.toLocaleString() },
    { label: 'Eligible coverage', value: `$${formatCompact(eligibleCoverage)}`, sub: 'backs debt' },
    { label: 'SP APY', value: spApy != null ? `${spApy.toFixed(2)}%` : '--', sub: '7d' },
    { label: 'Liquidations absorbed', value: totalLiquidations.toLocaleString() },
  ]);
</script>

<LensHealthStrip title="Stability pool" metrics={healthMetrics} loading={loading} />

<div class="grid grid-cols-1 lg:grid-cols-2 gap-3">
  <div class="explorer-card">
    <MiniAreaChart
      points={depositSeries}
      label="Pool deposits (90d)"
      color={CHART_COLORS.teal}
      fillColor={CHART_COLORS.tealDim}
      valueFormat={(v) => `$${formatCompact(v)}`}
      loading={loading}
    />
  </div>
  <div class="explorer-card">
    <MiniAreaChart
      points={liquidationSeries}
      label="Liquidations absorbed (90d)"
      color={CHART_COLORS.danger}
      fillColor="rgba(224, 107, 159, 0.15)"
      valueFormat={(v) => v.toLocaleString(undefined, { maximumFractionDigits: 0 })}
      loading={loading}
    />
  </div>
</div>

{#if collateralGains.length > 0}
  <div class="explorer-card">
    <h3 class="text-sm font-medium text-gray-300 mb-3">Collateral in pool</h3>
    <div class="grid grid-cols-2 md:grid-cols-4 gap-3">
      {#each collateralGains as g}
        <div class="flex flex-col">
          <span class="text-xs text-gray-500">{g.symbol}</span>
          <span class="text-base font-semibold tabular-nums text-gray-200 mt-0.5">
            {formatCompact(g.amount / 1e8)}
          </span>
        </div>
      {/each}
    </div>
  </div>
{/if}

<div class="explorer-card">
  <h3 class="text-sm font-medium text-gray-300 mb-3">Recent liquidations absorbed</h3>
  {#if loading}
    <div class="flex items-center justify-center py-6">
      <div class="w-5 h-5 border-2 border-gray-600 border-t-teal-400 rounded-full animate-spin"></div>
    </div>
  {:else if liquidations.length === 0}
    <p class="text-sm text-gray-500 py-4">No liquidations on record.</p>
  {:else}
    <div class="overflow-x-auto">
      <table class="w-full text-sm">
        <thead>
          <tr class="border-b border-white/5">
            <th class="text-left py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">Vault</th>
            <th class="text-left py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">Collateral</th>
            <th class="text-right py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">Debt cleared</th>
            <th class="text-right py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">Collateral gained</th>
          </tr>
        </thead>
        <tbody>
          {#each liquidations.slice(0, 10) as l}
            {@const symbol = getCollateralSymbol(
              l.collateral_type?.toText?.() ?? String(l.collateral_type ?? '')
            )}
            <tr class="border-b border-white/[0.03] hover:bg-white/[0.02]">
              <td class="py-2 px-2">
                {#if l.vault_id != null}
                  <a href="/explorer/e/vault/{Number(l.vault_id)}" class="text-teal-400 hover:text-teal-300">#{Number(l.vault_id)}</a>
                {:else}
                  <span class="text-gray-500">--</span>
                {/if}
              </td>
              <td class="py-2 px-2 text-gray-300">{symbol}</td>
              <td class="py-2 px-2 text-right tabular-nums text-gray-300">
                {formatCompact(Number(l.debt_cleared_e8s ?? l.debt_amount ?? 0n) / 1e8)} icUSD
              </td>
              <td class="py-2 px-2 text-right tabular-nums text-gray-300">
                {formatCompact(Number(l.collateral_gained ?? l.collateral_amount ?? 0n) / 1e8)}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</div>

<LensActivityPanel scope="stability_pool" title="Stability pool activity" viewAllHref="/explorer/activity?type=stability_pool" />
