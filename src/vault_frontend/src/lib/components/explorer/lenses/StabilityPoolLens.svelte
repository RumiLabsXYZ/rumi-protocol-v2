<script lang="ts">
  import { onMount } from 'svelte';
  import LensHealthStrip from '../LensHealthStrip.svelte';
  import LensActivityPanel from '../LensActivityPanel.svelte';
  import MiniAreaChart from '../MiniAreaChart.svelte';
  import {
    fetchStabilitySeries, fetchApys, fetchTopSpDepositors,
  } from '$services/explorer/analyticsService';
  import { shortenPrincipal, getCanisterName } from '$utils/explorerHelpers';
  import type { TopSpDepositorRow } from '$declarations/rumi_analytics/rumi_analytics.did';
  import {
    fetchStabilityPoolStatus, fetchStabilityPoolLiquidations,
  } from '$services/explorer/explorerService';
  import { QueryOperations } from '$services/protocol/queryOperations';
  import { e8sToNumber, formatCompact, CHART_COLORS, getCollateralSymbol } from '$utils/explorerChartHelpers';

  let poolStatus: any = $state(null);
  let protocolStatus: any = $state(null);
  let series: any[] = $state([]);
  let liquidations: any[] = $state([]);
  let spApy: number | null = $state(null);
  let loading = $state(true);

  // Top depositors leaderboard (analytics-backed).
  type DepWindow = '7d' | '30d' | '90d' | 'all';
  let depWindow: DepWindow = $state('30d');
  let depositors_top: TopSpDepositorRow[] = $state([]);
  let depLoading = $state(false);

  const DEP_WINDOW_NS: Record<DepWindow, bigint> = {
    '7d': 7n * 86_400n * 1_000_000_000n,
    '30d': 30n * 86_400n * 1_000_000_000n,
    '90d': 90n * 86_400n * 1_000_000_000n,
    all: (1n << 63n) - 1n,
  };

  async function loadTopDepositors(win: DepWindow) {
    depLoading = true;
    try {
      const resp = await fetchTopSpDepositors(DEP_WINDOW_NS[win], 20);
      depositors_top = resp.rows;
    } catch (err) {
      console.error('[StabilityPoolLens] loadTopDepositors failed:', err);
      depositors_top = [];
    } finally {
      depLoading = false;
    }
  }

  $effect(() => {
    loadTopDepositors(depWindow);
  });

  onMount(async () => {
    try {
      const [stR, seR, lqR, apR, prR] = await Promise.allSettled([
        fetchStabilityPoolStatus(),
        fetchStabilitySeries(90),
        fetchStabilityPoolLiquidations(50),
        fetchApys(),
        QueryOperations.getProtocolStatus(),
      ]);
      if (stR.status === 'fulfilled') poolStatus = stR.value;
      if (seR.status === 'fulfilled') series = seR.value ?? [];
      if (lqR.status === 'fulfilled') liquidations = lqR.value ?? [];
      if (prR.status === 'fulfilled') protocolStatus = prR.value;
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

  // Live SP APY: same formula the Stability Pool tab uses, computed from
  // current protocol/pool status. Fixes the discrepancy between the analytics
  // 7-day rolling number (slow to react) and the live rate users see in /liquidity.
  const liveSpApy = $derived.by(() => {
    if (!protocolStatus || !poolStatus) return null;
    const split = protocolStatus.interestSplit ?? [];
    const poolShare = (split.find((e: any) => e.destination === 'stability_pool')?.bps ?? 0) / 10000;
    const perC = protocolStatus.perCollateralInterest;
    if (!perC || perC.length === 0 || poolShare === 0) return null;

    const eligibleMap = new Map<string, number>(
      (poolStatus.eligible_icusd_per_collateral ?? []).map(([p, v]: [any, bigint]) => [
        typeof p?.toText === 'function' ? p.toText() : String(p),
        Number(v) / 1e8,
      ]),
    );

    let totalApr = 0;
    for (const info of perC) {
      const eligible = eligibleMap.get(info.collateralType) ?? 0;
      if (eligible === 0 || info.totalDebtE8s === 0 || info.weightedInterestRate === 0) continue;
      totalApr += (info.weightedInterestRate * poolShare * info.totalDebtE8s) / eligible;
    }
    if (totalApr === 0) return null;
    const apy = Math.pow(1 + totalApr / 365, 365) - 1;
    return apy * 100;
  });

  // Prefer the live computation; fall back to the 7d rolling analytics number.
  const displayedSpApy = $derived(liveSpApy ?? spApy);

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
    { label: 'Eligible coverage', value: `$${formatCompact(eligibleCoverage)}`, sub: 'icUSD-equivalent that can absorb liquidations' },
    { label: 'SP APY', value: displayedSpApy != null ? `${displayedSpApy.toFixed(2)}%` : '--', sub: liveSpApy != null ? 'live' : '7d' },
    { label: 'Liquidations absorbed', value: totalLiquidations.toLocaleString() },
  ]);

  // Sum stables_consumed: vec record { principal; nat64 } → total icUSD-equivalent.
  // All SP stablecoins (icUSD, ckUSDC, ckUSDT) are pegged $1 so a raw sum (after
  // decimals normalization) is fine. ckUSDC/ckUSDT are 6-decimal; icUSD is 8.
  function debtClearedFromRecord(rec: any): number {
    const stables: Array<[any, bigint]> = rec?.stables_consumed ?? [];
    let total = 0;
    for (const [tokenPrincipal, amountE8s] of stables) {
      const principal = typeof tokenPrincipal?.toText === 'function'
        ? tokenPrincipal.toText() : String(tokenPrincipal);
      // 6 decimals for ckUSDC / ckUSDT, 8 for everything else (icUSD)
      const decimals = principal.includes('xevnm-gaaaa') /* ckUSDC */
        || principal.includes('cngnf-vqaaa') /* ckUSDT */
        ? 6 : 8;
      total += Number(amountE8s) / Math.pow(10, decimals);
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
  <div class="flex items-center justify-between gap-3 flex-wrap mb-3">
    <div>
      <h3 class="text-sm font-medium text-gray-300">Top depositors</h3>
      <p class="text-xs text-gray-500">Ranked by total deposit volume in the window</p>
    </div>
    <div class="inline-flex rounded-lg border border-gray-700/70 overflow-hidden text-[11px]">
      {#each ['7d', '30d', '90d', 'all'] as const as w (w)}
        <button
          type="button"
          class="px-2.5 py-1 border-r border-gray-700/70 last:border-r-0 transition-colors"
          class:bg-teal-500={depWindow === w}
          class:text-white={depWindow === w}
          class:text-gray-400={depWindow !== w}
          class:hover:text-gray-200={depWindow !== w}
          onclick={() => (depWindow = w)}
        >
          {w.toUpperCase()}
        </button>
      {/each}
    </div>
  </div>
  {#if depLoading && depositors_top.length === 0}
    <div class="flex items-center justify-center py-6">
      <div class="w-5 h-5 border-2 border-gray-600 border-t-teal-400 rounded-full animate-spin"></div>
    </div>
  {:else if depositors_top.length === 0}
    <p class="text-sm text-gray-500 py-4">No deposit activity in this window.</p>
  {:else}
    <div class="overflow-x-auto">
      <table class="w-full text-sm">
        <thead>
          <tr class="border-b border-white/5">
            <th class="text-left py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">#</th>
            <th class="text-left py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">Principal</th>
            <th class="text-right py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">Deposited (window)</th>
            <th class="text-right py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">Current balance</th>
            <th class="text-right py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">Net (window)</th>
          </tr>
        </thead>
        <tbody>
          {#each depositors_top as row, i (row.principal.toText())}
            {@const pid = row.principal.toText()}
            {@const label = getCanisterName(pid) ?? shortenPrincipal(pid)}
            <tr class="border-b border-white/[0.03] hover:bg-white/[0.02]">
              <td class="py-2 px-2 text-gray-500 tabular-nums">{i + 1}</td>
              <td class="py-2 px-2">
                <a
                  href="/explorer/e/address/{pid}"
                  class="text-teal-400 hover:text-teal-300 font-mono"
                  title={pid}
                >
                  {label}
                </a>
              </td>
              <td class="py-2 px-2 text-right tabular-nums text-gray-300">
                {formatCompact(e8sToNumber(row.total_deposited_e8s))} icUSD
              </td>
              <td class="py-2 px-2 text-right tabular-nums text-gray-300">
                {formatCompact(e8sToNumber(row.current_balance_e8s))}
              </td>
              <td
                class="py-2 px-2 text-right tabular-nums"
                class:text-emerald-400={row.net_position_e8s > 0n}
                class:text-rose-400={row.net_position_e8s < 0n}
                class:text-gray-400={row.net_position_e8s === 0n}
              >
                {row.net_position_e8s < 0n ? '-' : ''}{formatCompact(Math.abs(Number(row.net_position_e8s)) / 1e8)}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</div>

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
