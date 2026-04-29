<script lang="ts">
  import { onMount } from 'svelte';
  import LensHealthStrip from '../LensHealthStrip.svelte';
  import LensActivityPanel from '../LensActivityPanel.svelte';
  import MiniAreaChart from '../MiniAreaChart.svelte';
  import {
    fetchSwapSeries, fetchThreePoolSeries, fetchPegStatus, fetchApys,
  } from '$services/explorer/analyticsService';
  import {
    fetchThreePoolState, fetchThreePoolStats, fetchThreePoolHealth,
    fetchThreePoolVolumeSeries, fetchThreePoolVirtualPriceSeries,
    fetchAmmPools, fetchAmmPoolStats, fetchCollateralPrices,
  } from '$services/explorer/explorerService';
  import { CANISTER_IDS } from '$lib/config';
  import { POOL_TOKENS } from '$services/threePoolService';
  import { e8sToNumber, formatCompact, CHART_COLORS } from '$utils/explorerChartHelpers';
  import { ammPoolLabel, ammPoolPair, ammPoolShortLabel, setAmmPoolRegistry } from '$utils/ammNaming';
  import { getTokenSymbol } from '$utils/explorerHelpers';
  import type { PegStatus } from '$declarations/rumi_analytics/rumi_analytics.did';

  let pegStatus: PegStatus | null = $state(null);
  let poolState: any = $state(null);
  let stats: any = $state(null);
  let health: any = $state(null);
  let swapSeries: any[] = $state([]);
  let volumeSeries: any[] = $state([]);
  let vpSeries: any[] = $state([]);
  let ammPools: any[] = $state([]);
  let ammPoolStats: Record<string, any> = $state({});
  let lpApy: number | null = $state(null);
  let spApy: number | null = $state(null);
  let icpUsdPrice: number | null = $state(null);
  let loading = $state(true);

  onMount(async () => {
    try {
      const [pegR, stR, statR, hlR, ssR, vsR, vpR, ammR, apyR, pricesR] = await Promise.allSettled([
        fetchPegStatus(),
        fetchThreePoolState(),
        fetchThreePoolStats('Last24h'),
        fetchThreePoolHealth(),
        fetchSwapSeries(90),
        fetchThreePoolVolumeSeries('Last7d', 3600n),
        fetchThreePoolVirtualPriceSeries('Last30d', 86400n),
        fetchAmmPools(),
        fetchApys(),
        fetchCollateralPrices(),
      ]);
      if (pegR.status === 'fulfilled') pegStatus = pegR.value ?? null;
      if (stR.status === 'fulfilled') poolState = stR.value;
      if (statR.status === 'fulfilled') stats = statR.value;
      if (hlR.status === 'fulfilled') health = hlR.value;
      if (ssR.status === 'fulfilled') swapSeries = ssR.value ?? [];
      if (vsR.status === 'fulfilled') volumeSeries = vsR.value ?? [];
      if (vpR.status === 'fulfilled') vpSeries = vpR.value ?? [];
      if (ammR.status === 'fulfilled') {
        ammPools = ammR.value ?? [];
        // Seed the registry so AMM event labels everywhere can resolve "AMM1 · 3USD/ICP"
        setAmmPoolRegistry(ammPools);
      }
      if (apyR.status === 'fulfilled' && apyR.value) {
        const aLp = apyR.value.lp_apy_pct?.[0];
        const aSp = apyR.value.sp_apy_pct?.[0];
        if (typeof aLp === 'number' && aLp > 0) lpApy = aLp;
        if (typeof aSp === 'number' && aSp > 0) spApy = aSp;
      }
      if (pricesR.status === 'fulfilled') {
        const map = pricesR.value;
        icpUsdPrice = map.get(CANISTER_IDS.ICP_LEDGER) ?? null;
      }

      // Per-pool stats for the AMM Pools card (TVL, 7d volume).
      if (ammPools.length > 0) {
        const statResults = await Promise.allSettled(
          ammPools.map((p: any) => fetchAmmPoolStats(p.pool_id, 'Week')),
        );
        const out: Record<string, any> = {};
        ammPools.forEach((p: any, i: number) => {
          const r = statResults[i];
          if (r.status === 'fulfilled' && r.value) out[p.pool_id] = r.value;
        });
        ammPoolStats = out;
      }
    } catch (err) {
      console.error('[DexsLens] onMount error:', err);
    } finally {
      loading = false;
    }
  });

  // 3pool TVL in $USD (each balance is in token decimals; all 3 stables are ~$1)
  const threePoolTvl = $derived.by(() => {
    if (!poolState?.balances) return 0;
    let total = 0;
    for (let i = 0; i < poolState.balances.length; i++) {
      const tok = POOL_TOKENS[i];
      if (!tok) continue;
      total += Number(poolState.balances[i]) / Math.pow(10, tok.decimals);
    }
    return total;
  });

  const imbalancePct = $derived.by(() => {
    if (!pegStatus) return 0;
    return pegStatus.max_imbalance_pct;
  });

  const virtualPrice = $derived.by(() => {
    if (!poolState?.virtual_price) return 1;
    return Number(poolState.virtual_price) / 1e18;
  });

  const volume24h = $derived.by(() => {
    if (!stats?.swap_volume_per_token) return 0;
    let total = 0;
    for (let i = 0; i < stats.swap_volume_per_token.length; i++) {
      const tok = POOL_TOKENS[i];
      if (!tok) continue;
      total += Number(stats.swap_volume_per_token[i]) / Math.pow(10, tok.decimals);
    }
    return total;
  });

  const volumePoints = $derived(
    volumeSeries.map((p: any) => {
      let total = 0;
      for (let i = 0; i < p.volume_per_token.length; i++) {
        const tok = POOL_TOKENS[i];
        if (!tok) continue;
        total += Number(p.volume_per_token[i]) / Math.pow(10, tok.decimals);
      }
      return { t: Number(p.timestamp) * 1000, v: total };
    })
  );

  const vpPoints = $derived(
    vpSeries.map((p: any) => ({
      t: Number(p.timestamp) * 1000,
      v: Number(p.virtual_price) / 1e18,
    }))
  );

  const swapSeriesPoints = $derived(
    swapSeries.map((r: any) => ({
      t: Number(r.timestamp_ns) / 1_000_000,
      v: e8sToNumber(r.three_pool_volume_e8s ?? 0n) + e8sToNumber(r.amm_volume_e8s ?? 0n),
    }))
  );

  const healthMetrics = $derived.by(() => {
    const metrics = [
      { label: '3pool TVL', value: `$${formatCompact(threePoolTvl)}` },
      { label: '24h volume', value: `$${formatCompact(volume24h)}` },
      { label: 'Virtual price', value: virtualPrice.toFixed(6), sub: 'compounds with fees' },
      {
        label: '3pool balance',
        value: pegStatus ? `${imbalancePct.toFixed(2)}% skew` : '--',
        tone: imbalancePct < 2 ? 'good' as const : imbalancePct < 5 ? 'caution' as const : 'danger' as const,
        sub: 'weight vs 33/33/33',
      },
    ];
    if (health) {
      const arb = Number(health.arb_opportunity_score);
      metrics.push({
        label: 'Arb score',
        value: `${arb}/100`,
        tone: arb > 50 ? 'caution' as const : 'good' as const,
        sub: 'pool imbalance % of saturation; higher = more profit available to a rebalancing trader',
      });
    }
    metrics.push({ label: '3Pool LP APY', value: lpApy != null ? `${lpApy.toFixed(2)}%` : '--', sub: '7d' });
    return metrics;
  });

  // AMM pools: friendly summary with sequential index, token pair, fee, TVL, 7d volume.
  // TVL = (3USD reserve × 3pool virtual_price) + (other token reserve × oracle price).
  // For the 3USD/ICP pool that means: 3USD value @ vp_USD + ICP balance × icpUsdPrice.
  function principalToText(p: any): string {
    if (!p) return '';
    if (typeof p === 'string') return p;
    if (typeof p?.toText === 'function') return p.toText();
    return String(p);
  }

  function tokenUsdPrice(principalText: string): number | null {
    if (principalText === CANISTER_IDS.THREEPOOL) return virtualPrice; // 3USD LP token priced via vp
    if (principalText === CANISTER_IDS.ICUSD_LEDGER) return 1;
    if (principalText === CANISTER_IDS.CKUSDT_LEDGER) return 1;
    if (principalText === CANISTER_IDS.CKUSDC_LEDGER) return 1;
    if (principalText === CANISTER_IDS.ICP_LEDGER) return icpUsdPrice;
    return null;
  }

  function tokenDecimals(principalText: string): number {
    // ckUSDT/ckUSDC are 6; everything else in our universe is 8. Add to this map
    // as we onboard tokens with different decimals.
    if (principalText === CANISTER_IDS.CKUSDT_LEDGER) return 6;
    if (principalText === CANISTER_IDS.CKUSDC_LEDGER) return 6;
    return 8;
  }

  const ammSummary = $derived.by(() => {
    const sorted = [...ammPools].sort((a: any, b: any) => a.pool_id.localeCompare(b.pool_id));
    return sorted.map((p: any, i: number) => {
      const tokenA = principalToText(p.token_a);
      const tokenB = principalToText(p.token_b);
      const symA = getTokenSymbol(tokenA) || '?';
      const symB = getTokenSymbol(tokenB) || '?';

      const reserveA = Number(p.reserve_a ?? 0n) / Math.pow(10, tokenDecimals(tokenA));
      const reserveB = Number(p.reserve_b ?? 0n) / Math.pow(10, tokenDecimals(tokenB));
      const priceA = tokenUsdPrice(tokenA);
      const priceB = tokenUsdPrice(tokenB);
      const tvlUsd = (priceA != null && priceB != null) ? reserveA * priceA + reserveB * priceB : null;

      // 7d volume: sum of both sides' input volume in USD (each swap touches one side
      // as input, so a + b avoids double-counting).
      const stat = ammPoolStats[p.pool_id] ?? null;
      let vol7d: number | null = null;
      if (stat && priceA != null && priceB != null) {
        const vA = Number(stat.volume_a_e8s ?? 0n) / Math.pow(10, tokenDecimals(tokenA));
        const vB = Number(stat.volume_b_e8s ?? 0n) / Math.pow(10, tokenDecimals(tokenB));
        vol7d = vA * priceA + vB * priceB;
      }

      return {
        index: i + 1,
        id: p.pool_id,
        name: ammPoolLabel(p.pool_id, p.token_a, p.token_b),
        shortName: ammPoolShortLabel(p.pool_id),
        pair: `${symA}/${symB}`,
        feeBps: Number(p.fee_bps ?? 0),
        tvlUsd,
        vol7d,
      };
    });
  });
</script>

<LensHealthStrip title="DEXs" metrics={healthMetrics} loading={loading} />

<div class="grid grid-cols-1 lg:grid-cols-2 gap-3">
  <div class="explorer-card">
    <h3 class="text-sm font-medium text-gray-300 mb-3">3Pool balances</h3>
    {#if loading}
      <div class="flex items-center justify-center py-8">
        <div class="w-5 h-5 border-2 border-gray-600 border-t-teal-400 rounded-full animate-spin"></div>
      </div>
    {:else if !poolState}
      <p class="text-sm text-gray-500 py-4">3pool state unavailable.</p>
    {:else}
      {@const bal = poolState.balances ?? []}
      {@const normTotal = bal.reduce((s: number, b: any, i: number) => {
        const tok = POOL_TOKENS[i];
        return tok ? s + Number(b) / Math.pow(10, tok.decimals) : s;
      }, 0)}
      <div class="flex h-3 rounded overflow-hidden mb-3">
        {#each bal as b, i}
          {@const tok = POOL_TOKENS[i]}
          {#if tok}
            {@const amount = Number(b) / Math.pow(10, tok.decimals)}
            <div
              style="flex: {amount}; background: {tok.color};"
              title="{tok.symbol}: ${formatCompact(amount)}"
            ></div>
          {/if}
        {/each}
      </div>
      <div class="space-y-1.5">
        {#each bal as b, i}
          {@const tok = POOL_TOKENS[i]}
          {#if tok}
            {@const amount = Number(b) / Math.pow(10, tok.decimals)}
            {@const pct = normTotal > 0 ? (amount / normTotal) * 100 : 0}
            <div class="flex items-center gap-2 text-sm">
              <span class="w-2 h-2 rounded-full flex-shrink-0" style="background: {tok.color};"></span>
              <span class="w-20 font-medium text-gray-300">{tok.symbol}</span>
              <div class="flex-1 h-1.5 rounded-sm overflow-hidden" style="background: var(--rumi-bg-surface2);">
                <div class="h-full rounded-sm" style="width: {pct}%; background: {tok.color}; opacity: 0.6;"></div>
              </div>
              <span class="w-20 text-right tabular-nums text-gray-400">${formatCompact(amount)}</span>
              <span class="w-12 text-right tabular-nums text-gray-500">{pct.toFixed(1)}%</span>
            </div>
          {/if}
        {/each}
      </div>
    {/if}
  </div>

  <div class="explorer-card">
    <MiniAreaChart
      points={volumePoints}
      label="3Pool swap volume (7d / hourly)"
      color={CHART_COLORS.teal}
      fillColor={CHART_COLORS.tealDim}
      valueFormat={(v) => `$${formatCompact(v)}`}
      loading={loading}
    />
  </div>
</div>

<div class="grid grid-cols-1 lg:grid-cols-2 gap-3">
  <div class="explorer-card">
    <MiniAreaChart
      points={swapSeriesPoints}
      label="Daily swap volume (all DEXs, 90d)"
      color={CHART_COLORS.purple}
      fillColor={CHART_COLORS.purpleDim}
      valueFormat={(v) => `$${formatCompact(v)}`}
      loading={loading}
    />
  </div>
  <div class="explorer-card">
    <MiniAreaChart
      points={vpPoints}
      label="3Pool virtual price (30d / daily)"
      color={CHART_COLORS.action}
      fillColor="rgba(52, 211, 153, 0.15)"
      valueFormat={(v) => v.toFixed(6)}
      loading={loading}
      yAxisMode="data-fit"
    />
  </div>
</div>

{#if ammSummary.length > 0}
  <div class="explorer-card">
    <h3 class="text-sm font-medium text-gray-300 mb-3">AMM pools</h3>
    <div class="overflow-x-auto">
      <table class="w-full text-sm">
        <thead>
          <tr class="border-b border-white/5">
            <th class="text-left py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">Pool</th>
            <th class="text-left py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">Pair</th>
            <th class="text-right py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">Fee</th>
            <th class="text-right py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">TVL</th>
            <th class="text-right py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">7d volume</th>
          </tr>
        </thead>
        <tbody>
          {#each ammSummary as p (p.id)}
            <tr class="border-b border-white/[0.03] hover:bg-white/[0.02]">
              <td class="py-2 px-2 font-medium">
                <a href={`/explorer/e/pool/${encodeURIComponent(p.id)}`} class="text-indigo-300 hover:text-indigo-200">{p.shortName}</a>
              </td>
              <td class="py-2 px-2 text-gray-300">
                <a href={`/explorer/e/pool/${encodeURIComponent(p.id)}`} class="hover:text-indigo-200">{p.pair}</a>
              </td>
              <td class="py-2 px-2 text-right tabular-nums text-gray-400">{(p.feeBps / 100).toFixed(2)}%</td>
              <td class="py-2 px-2 text-right tabular-nums text-gray-300">
                {p.tvlUsd != null ? `$${formatCompact(p.tvlUsd)}` : '--'}
              </td>
              <td class="py-2 px-2 text-right tabular-nums text-gray-400">
                {p.vol7d != null ? `$${formatCompact(p.vol7d)}` : '--'}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  </div>
{/if}

<LensActivityPanel scope="dexs" title="DEX activity" viewAllHref="/explorer/activity?type=dex" />
