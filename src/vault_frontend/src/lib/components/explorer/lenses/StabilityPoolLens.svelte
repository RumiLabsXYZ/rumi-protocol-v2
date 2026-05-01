<script lang="ts">
  import { onMount } from 'svelte';
  import LensHealthStrip from '../LensHealthStrip.svelte';
  import LensActivityPanel from '../LensActivityPanel.svelte';
  import MiniAreaChart from '../MiniAreaChart.svelte';
  import SpCurrentDepositorsCard from '../SpCurrentDepositorsCard.svelte';
  import SpCoverageCard from '../SpCoverageCard.svelte';
  import {
    fetchStabilitySeries, fetchApys,
  } from '$services/explorer/analyticsService';
  import { getTokenDecimals } from '$utils/explorerHelpers';
  import {
    fetchStabilityPoolStatus, fetchStabilityPoolLiquidations,
  } from '$services/explorer/explorerService';
  import { QueryOperations } from '$services/protocol/queryOperations';
  import { e8sToNumber, formatCompact, CHART_COLORS, getCollateralSymbol } from '$utils/explorerChartHelpers';
  import { liveSpApyPct } from '$utils/liveApy';

  let poolStatus: any = $state(null);
  let protocolStatus: any = $state(null);
  let series: any[] = $state([]);
  let liquidations: any[] = $state([]);
  let spApy: number | null = $state(null);
  let loading = $state(true);

  onMount(async () => {
    try {
      const [stR, seR, lqR, apR, prR] = await Promise.allSettled([
        fetchStabilityPoolStatus(),
        fetchStabilitySeries(90),
        fetchStabilityPoolLiquidations(50),
        fetchApys(),
        QueryOperations.getProtocolStatus(),
      ]);
      if (stR.status === 'fulfilled' && stR.value) poolStatus = stR.value;
      else console.warn('[StabilityPoolLens] poolStatus unavailable:', stR.status === 'rejected' ? stR.reason : 'null response');
      if (seR.status === 'fulfilled') series = seR.value ?? [];
      if (lqR.status === 'fulfilled') liquidations = lqR.value ?? [];
      if (prR.status === 'fulfilled') protocolStatus = prR.value;
      else console.warn('[StabilityPoolLens] protocolStatus failed:', prR.reason);
      if (apR.status === 'fulfilled' && apR.value) {
        const v = apR.value.sp_apy_pct?.[0];
        if (typeof v === 'number') spApy = v;
      }
    } catch (err) {
      console.error('[StabilityPoolLens] onMount error:', err);
    } finally {
      loading = false;
    }
  });

  // Live SP APY: shared formula in $utils/liveApy. The analytics 7d rolling
  // number lags reality and can sit at zero when the window has no realized
  // fee activity, so prefer the live value with analytics as fallback.
  const liveSpApy = $derived(liveSpApyPct(protocolStatus, poolStatus));

  // Prefer the live computation; fall back to the 7d rolling analytics number.
  const displayedSpApy = $derived(liveSpApy ?? spApy);

  const totalDeposits = $derived(poolStatus ? e8sToNumber(poolStatus.total_deposits_e8s ?? 0n) : 0);
  const depositors = $derived(poolStatus ? Number(poolStatus.total_depositors ?? 0n) : 0);
  const totalLiquidations = $derived(poolStatus ? Number(poolStatus.total_liquidations_executed ?? 0n) : 0);
  // `eligible_icusd_per_collateral` reports, FOR EACH collateral type, the
  // share of the pool opted-in to absorb that specific collateral. With all
  // depositors opted in to every collateral (the default), each entry equals
  // the total pool — so summing across collaterals N-counts the same dollars
  // and inflates the headline. Take the minimum (worst-case coverage across
  // collaterals) — matches what users expect when reading the label.
  const eligibleCoverage = $derived.by(() => {
    const rows = poolStatus?.eligible_icusd_per_collateral;
    if (!rows || rows.length === 0) return 0;
    let min = Infinity;
    for (const [, v] of rows) {
      const n = Number(v);
      if (n < min) min = n;
    }
    return Number.isFinite(min) ? min / 1e8 : 0;
  });

  const depositSeries = $derived(
    series.map((r: any) => ({ t: Number(r.timestamp_ns) / 1_000_000, v: e8sToNumber(r.total_deposits_e8s ?? 0n) }))
  );
  const liquidationSeries = $derived(
    series.map((r: any) => ({ t: Number(r.timestamp_ns) / 1_000_000, v: Number(r.total_liquidations_executed ?? 0n) }))
  );

  const healthMetrics = $derived.by(() => [
    { label: 'Deposits', value: `$${formatCompact(totalDeposits)}`, sub: 'icUSD' },
    { label: 'Depositors', value: depositors.toLocaleString() },
    { label: 'Eligible coverage', value: `$${formatCompact(eligibleCoverage)}`, sub: 'icUSD-equivalent that can absorb liquidations' },
    { label: 'SP APY', value: displayedSpApy != null ? `${displayedSpApy.toFixed(2)}%` : '--', sub: liveSpApy != null ? 'live' : '7d' },
    { label: 'Liquidations absorbed', value: totalLiquidations.toLocaleString() },
  ]);

  // Sum stables_consumed: vec record { principal; nat64 } → total icUSD-equivalent.
  // All SP stablecoins (icUSD, ckUSDC, ckUSDT) are pegged $1 so a raw sum (after
  // decimals normalization) is fine. Decimals are looked up via the central
  // KNOWN_TOKENS registry so onboarding a new stablecoin doesn't silently
  // produce 100x-inflated numbers here.
  function debtClearedFromRecord(rec: any): number {
    const stables: Array<[any, bigint]> = rec?.stables_consumed ?? [];
    let total = 0;
    for (const [tokenPrincipal, amountE8s] of stables) {
      const principal = typeof tokenPrincipal?.toText === 'function'
        ? tokenPrincipal.toText() : String(tokenPrincipal);
      total += Number(amountE8s) / Math.pow(10, getTokenDecimals(principal));
    }
    return total;
  }
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
      yAxisMode="data-fit"
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
      yAxisMode="data-fit"
      loading={loading}
    />
  </div>
</div>

<SpCoverageCard />

<SpCurrentDepositorsCard />

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
                {formatCompact(debtClearedFromRecord(l))} icUSD
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
