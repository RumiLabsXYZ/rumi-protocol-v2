<script lang="ts">
  import { onMount } from 'svelte';
  import LensHealthStrip from '../LensHealthStrip.svelte';
  import LensActivityPanel from '../LensActivityPanel.svelte';
  import MiniAreaChart from '../MiniAreaChart.svelte';
  import {
    fetchFeeSeries, fetchApys, fetchTradeActivity,
  } from '$services/explorer/analyticsService';
  import {
    fetchTreasuryStats, fetchInterestSplit,
  } from '$services/explorer/explorerService';
  import { ProtocolService } from '$services/protocol';
  import { e8sToNumber, formatCompact, CHART_COLORS } from '$utils/explorerChartHelpers';

  let feeRows: any[] = $state([]);
  let apys: any = $state(null);
  let tradeActivity: any = $state(null);
  let treasury: any = $state(null);
  let interestSplit: any[] = $state([]);
  let protocolStatus: any = $state(null);
  let loading = $state(true);

  onMount(async () => {
    try {
      const [feeR, apR, trR, trsR, spR, psR] = await Promise.allSettled([
        fetchFeeSeries(90),
        fetchApys(),
        fetchTradeActivity(),
        fetchTreasuryStats(),
        fetchInterestSplit(),
        ProtocolService.getProtocolStatus(),
      ]);
      if (feeR.status === 'fulfilled') feeRows = feeR.value ?? [];
      if (apR.status === 'fulfilled') apys = apR.value;
      if (trR.status === 'fulfilled') tradeActivity = trR.value;
      if (trsR.status === 'fulfilled') treasury = trsR.value;
      if (spR.status === 'fulfilled') interestSplit = spR.value ?? [];
      if (psR.status === 'fulfilled') protocolStatus = psR.value;
    } catch (err) {
      console.error('[RevenueLens] onMount error:', err);
    } finally {
      loading = false;
    }
  });

  const totalBorrow = $derived(
    feeRows.reduce((s: number, d: any) => s + e8sToNumber(d.borrowing_fees_e8s?.[0] ?? d.borrowing_fees_e8s ?? 0n), 0)
  );
  const totalRedemption = $derived(
    feeRows.reduce((s: number, d: any) => s + e8sToNumber(d.redemption_fees_e8s?.[0] ?? d.redemption_fees_e8s ?? 0n), 0)
  );
  const totalSwap = $derived(
    feeRows.reduce((s: number, d: any) => s + e8sToNumber(d.swap_fees_e8s ?? 0n), 0)
  );
  const totalFees = $derived(totalBorrow + totalRedemption + totalSwap);

  const estimatedDailyBorrow = $derived.by(() => {
    if (!protocolStatus?.perCollateralInterest) return 0;
    const treasuryBps = protocolStatus.interestSplit?.find((e: any) => e.destination === 'treasury')?.bps ?? 0;
    const treasuryShare = treasuryBps / 10000;
    let dailyInterest = 0;
    for (const info of protocolStatus.perCollateralInterest) {
      dailyInterest += info.weightedInterestRate * info.totalDebtE8s;
    }
    return dailyInterest * treasuryShare;
  });

  const fees24h = $derived.by(() => {
    const swapFees24h = tradeActivity ? e8sToNumber(tradeActivity.total_fees_e8s) : 0;
    return swapFees24h + estimatedDailyBorrow;
  });

  // Analytics returns 0 when the window has no data; treat that as "--"
  // rather than confidently displaying 0.00%.
  const lpApy = $derived.by(() => {
    const v = apys?.lp_apy_pct?.[0];
    return typeof v === 'number' && v > 0 ? v : null;
  });
  const spApy = $derived.by(() => {
    const v = apys?.sp_apy_pct?.[0];
    return typeof v === 'number' && v > 0 ? v : null;
  });

  const feePoints = $derived(
    feeRows.map((r: any) => {
      const b = e8sToNumber(r.borrowing_fees_e8s?.[0] ?? r.borrowing_fees_e8s ?? 0n);
      const rd = e8sToNumber(r.redemption_fees_e8s?.[0] ?? r.redemption_fees_e8s ?? 0n);
      const s = e8sToNumber(r.swap_fees_e8s ?? 0n);
      return { t: Number(r.timestamp_ns) / 1_000_000, v: b + rd + s };
    })
  );

  const splitRows = $derived.by(() => {
    // Prefer the dedicated interestSplit endpoint; fallback to protocol status split.
    const raw = (interestSplit?.length ? interestSplit : protocolStatus?.interestSplit) ?? [];
    return raw.map((e: any) => ({
      destination: e.destination ?? String(e.destination ?? ''),
      bps: Number(e.bps ?? 0),
    })).filter((r: any) => r.bps > 0);
  });

  const treasuryInfo = $derived.by(() => {
    if (!treasury) return null;
    return {
      pendingInterest: Number(treasury.pending_treasury_interest ?? 0n),
      liquidationShare: Number(treasury.liquidation_protocol_share ?? 0),
      flushThreshold: e8sToNumber(treasury.interest_flush_threshold_e8s ?? 0n),
      principal: treasury.treasury_principal?.[0]?.toText?.() ?? treasury.treasury_principal?.[0] ?? null,
    };
  });

  // Treasury share of accrued interest (not the liquidation bonus split — that
  // belongs in the protocol docs). Pulled from interest_split's "treasury"
  // entry; this is the slice of borrower interest that gets routed to the
  // treasury canister.
  const treasuryInterestShare = $derived.by(() => {
    const bps = splitRows.find((r: any) => r.destination === 'treasury')?.bps ?? 0;
    return bps / 10000;
  });

  const healthMetrics = $derived.by(() => [
    { label: 'Fees (90d)', value: `$${formatCompact(totalFees)}` },
    { label: '24h fees', value: `$${formatCompact(fees24h)}` },
    { label: '3Pool LP APY', value: lpApy != null ? `${Number(lpApy).toFixed(2)}%` : '--', sub: '7d' },
    { label: 'SP APY', value: spApy != null ? `${Number(spApy).toFixed(2)}%` : '--', sub: '7d' },
    {
      label: 'Treasury interest share',
      value: `${(treasuryInterestShare * 100).toFixed(0)}%`,
      sub: 'of accrued interest',
    },
  ]);

  function destColor(dest: string): string {
    if (dest === 'treasury') return CHART_COLORS.purple;
    if (dest === 'stability_pool') return CHART_COLORS.teal;
    if (dest === 'three_pool') return CHART_COLORS.action;
    return CHART_COLORS.caution;
  }
  function destLabel(dest: string): string {
    if (dest === 'treasury') return 'Treasury';
    if (dest === 'stability_pool') return 'Stability Pool';
    if (dest === 'three_pool') return '3Pool LPs';
    return dest;
  }
</script>

<LensHealthStrip title="Revenue" metrics={healthMetrics} loading={loading} />

<div class="explorer-card">
  <MiniAreaChart
    points={feePoints}
    label="Daily protocol fees (90d)"
    color={CHART_COLORS.action}
    fillColor="rgba(52, 211, 153, 0.15)"
    valueFormat={(v) => `$${formatCompact(v)}`}
    height={180}
    loading={loading}
  />
</div>

<div class="grid grid-cols-1 lg:grid-cols-2 gap-3">
  <div class="explorer-card">
    <h3 class="text-sm font-medium text-gray-300 mb-3">Fee breakdown (90d)</h3>
    {#if loading}
      <div class="flex items-center justify-center py-8">
        <div class="w-5 h-5 border-2 border-gray-600 border-t-teal-400 rounded-full animate-spin"></div>
      </div>
    {:else}
      {@const items = [
        { label: 'Borrowing', value: totalBorrow, color: CHART_COLORS.teal },
        { label: 'Redemption', value: totalRedemption, color: CHART_COLORS.purple },
        { label: 'Swap', value: totalSwap, color: CHART_COLORS.action },
      ]}
      {@const max = Math.max(1, totalBorrow, totalRedemption, totalSwap)}
      <div class="space-y-2">
        {#each items as it}
          <div class="flex items-center gap-3 text-sm">
            <span class="w-24 text-gray-400">{it.label}</span>
            <div class="flex-1 h-4 rounded-sm overflow-hidden" style="background: var(--rumi-bg-surface2);">
              <div class="h-full rounded-sm" style="width: {(it.value / max) * 100}%; background: {it.color}; opacity: 0.7;"></div>
            </div>
            <span class="w-20 text-right tabular-nums text-gray-300">${formatCompact(it.value)}</span>
          </div>
        {/each}
      </div>
    {/if}
  </div>

  <div class="explorer-card">
    <h3 class="text-sm font-medium text-gray-300 mb-3">Interest split</h3>
    {#if loading}
      <div class="flex items-center justify-center py-8">
        <div class="w-5 h-5 border-2 border-gray-600 border-t-teal-400 rounded-full animate-spin"></div>
      </div>
    {:else if splitRows.length === 0}
      <p class="text-sm text-gray-500 py-4">Split unavailable.</p>
    {:else}
      <div class="flex h-3 rounded overflow-hidden mb-3">
        {#each splitRows as r}
          <div style="flex: {r.bps}; background: {destColor(r.destination)};" title="{destLabel(r.destination)}: {(r.bps / 100).toFixed(1)}%"></div>
        {/each}
      </div>
      <div class="space-y-1.5">
        {#each splitRows as r}
          <div class="flex items-center gap-3 text-sm">
            <span class="w-2 h-2 rounded-full flex-shrink-0" style="background: {destColor(r.destination)};"></span>
            <span class="flex-1 text-gray-300">{destLabel(r.destination)}</span>
            <span class="tabular-nums text-gray-400">{(r.bps / 100).toFixed(1)}%</span>
          </div>
        {/each}
      </div>
    {/if}
  </div>
</div>

{#if treasuryInfo}
  <div class="explorer-card">
    <h3 class="text-sm font-medium text-gray-300 mb-3">Treasury</h3>
    <div class="grid grid-cols-2 md:grid-cols-2 gap-4">
      <div>
        <div class="text-xs text-gray-500" title="icUSD that has accrued to the treasury but hasn't yet been swept to the treasury canister. Drops to 0 immediately after each periodic flush.">
          Pending interest
        </div>
        <div class="text-base font-semibold tabular-nums text-gray-200 mt-0.5">
          {formatCompact(treasuryInfo.pendingInterest / 1e8)} icUSD
        </div>
        <div class="text-[11px] text-gray-500 mt-0.5">
          Resets to 0 after each automatic flush.
        </div>
      </div>
      <div>
        <div class="text-xs text-gray-500" title="When pending interest crosses this amount the protocol auto-distributes it to recipients per the interest split. Threshold of 0 means flush every tick.">
          Flush threshold
        </div>
        <div class="text-base font-semibold tabular-nums text-gray-200 mt-0.5">
          {treasuryInfo.flushThreshold > 0 ? `${formatCompact(treasuryInfo.flushThreshold)} icUSD` : 'Flush every tick'}
        </div>
        <div class="text-[11px] text-gray-500 mt-0.5">
          Auto-flushes pending interest when crossed.
        </div>
      </div>
    </div>
    {#if treasuryInfo.principal}
      <div class="mt-3 text-xs text-gray-500">
        Treasury canister: <span class="font-mono text-gray-400">{treasuryInfo.principal}</span>
      </div>
    {/if}
  </div>
{/if}

<!-- Revenue-bearing = redemptions + liquidations + swaps (everything the protocol charges a fee on). -->
<LensActivityPanel
  scope="revenue"
  title="Revenue-bearing activity"
  viewAllHref="/explorer/activity?type=redemption,liquidation,swap"
/>
