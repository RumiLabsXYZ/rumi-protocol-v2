<script lang="ts">
  import { onMount } from 'svelte';
  import LensHealthStrip from '../LensHealthStrip.svelte';
  import LensActivityPanel from '../LensActivityPanel.svelte';
  import MiniAreaChart from '../MiniAreaChart.svelte';
  import TreasuryHoldingsCard from '../TreasuryHoldingsCard.svelte';
  import {
    fetchFeeSeries, fetchApys, fetchFeeBreakdownWindow, type FeeBreakdown,
  } from '$services/explorer/analyticsService';
  import {
    fetchInterestSplit, fetchStabilityPoolStatus, fetchThreePoolStatus,
  } from '$services/explorer/explorerService';
  import { ProtocolService } from '$services/protocol';
  import { e8sToNumber, formatCompact, CHART_COLORS } from '$utils/explorerChartHelpers';
  import { liveSpApyPct, liveLpApyPct } from '$utils/liveApy';

  let feeRows: any[] = $state([]);
  let apys: any = $state(null);
  let fees24hData = $state<FeeBreakdown | null>(null);
  let fees90dData = $state<FeeBreakdown | null>(null);
  let interestSplit: any[] = $state([]);
  let protocolStatus: any = $state(null);
  let poolStatus: any = $state(null);
  let threePoolStatus: any = $state(null);
  let loading = $state(true);

  onMount(async () => {
    try {
      const [feeR, apR, f24R, f90R, spR, psR, poolR, tpR] = await Promise.allSettled([
        fetchFeeSeries(90),
        fetchApys(),
        fetchFeeBreakdownWindow(1),
        fetchFeeBreakdownWindow(90),
        fetchInterestSplit(),
        ProtocolService.getProtocolStatus(),
        fetchStabilityPoolStatus(),
        fetchThreePoolStatus(),
      ]);
      if (feeR.status === 'fulfilled') feeRows = feeR.value ?? [];
      if (apR.status === 'fulfilled') apys = apR.value;
      if (f24R.status === 'fulfilled') fees24hData = f24R.value;
      if (f90R.status === 'fulfilled') fees90dData = f90R.value;
      if (spR.status === 'fulfilled') interestSplit = spR.value ?? [];
      if (psR.status === 'fulfilled') protocolStatus = psR.value;
      if (poolR.status === 'fulfilled') poolStatus = poolR.value;
      if (tpR.status === 'fulfilled') threePoolStatus = tpR.value;
    } catch (err) {
      console.error('[RevenueLens] onMount error:', err);
    } finally {
      loading = false;
    }
  });

  const totalBorrow = $derived(fees90dData?.borrowIcusd ?? 0);
  const totalRedemption = $derived(fees90dData?.redemptionIcusd ?? 0);
  const totalSwap = $derived(fees90dData?.swapIcusd ?? 0);
  const totalFees = $derived(totalBorrow + totalRedemption + totalSwap);

  const fees24h = $derived(
    (fees24hData?.borrowIcusd ?? 0) + (fees24hData?.redemptionIcusd ?? 0) + (fees24hData?.swapIcusd ?? 0)
  );

  // Live formulas first; analytics 7d rolling as the fallback (the rolling
  // number can sit at zero when the window happens to have no realized fee
  // activity, even though LPs/depositors are still earning from interest_split).
  const analyticsLpApy = $derived.by(() => {
    const v = apys?.lp_apy_pct?.[0];
    return typeof v === 'number' ? v : null;
  });
  const analyticsSpApy = $derived.by(() => {
    const v = apys?.sp_apy_pct?.[0];
    return typeof v === 'number' ? v : null;
  });
  const liveLp = $derived(liveLpApyPct(protocolStatus, threePoolStatus?.balances));
  const liveSp = $derived(liveSpApyPct(protocolStatus, poolStatus));
  const lpApy = $derived(liveLp ?? analyticsLpApy);
  const spApy = $derived(liveSp ?? analyticsSpApy);
  const lpApySub = $derived(liveLp != null ? 'live' : '7d');
  const spApySub = $derived(liveSp != null ? 'live' : '7d');

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
    { label: '3Pool LP APY', value: lpApy != null ? `${Number(lpApy).toFixed(2)}%` : '--', sub: lpApySub },
    { label: 'SP APY', value: spApy != null ? `${Number(spApy).toFixed(2)}%` : '--', sub: spApySub },
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

<TreasuryHoldingsCard />

<!-- Revenue-bearing = redemptions + liquidations + swaps (everything the protocol charges a fee on). -->
<LensActivityPanel
  scope="revenue"
  title="Revenue-bearing activity"
  viewAllHref="/explorer/activity?type=redemption,liquidation,swap"
/>
