<script lang="ts">
  import { onMount } from 'svelte';
  import {
    fetchFeeSeries, fetchSwapSeries, fetchTradeActivity
  } from '$services/explorer/analyticsService';
  import { fetchTreasuryStats, fetchInterestSplit } from '$services/explorer/explorerService';
  import { e8sToNumber, formatCompact, nsToDate, formatDateShort } from '$utils/explorerChartHelpers';

  let feeData: any[] = $state([]);
  let swapData: any[] = $state([]);
  let tradeActivity: any = $state(null);
  let treasury: any = $state(null);
  let interestSplit: any = $state(null);
  let loading = $state(true);

  onMount(async () => {
    const [feeResult, swapResult, tradeResult, treasuryResult, splitResult] = await Promise.allSettled([
      fetchFeeSeries(90),
      fetchSwapSeries(90),
      fetchTradeActivity(),
      fetchTreasuryStats(),
      fetchInterestSplit(),
    ]);

    if (feeResult.status === 'fulfilled') feeData = feeResult.value ?? [];
    if (swapResult.status === 'fulfilled') swapData = swapResult.value ?? [];
    if (tradeResult.status === 'fulfilled') tradeActivity = tradeResult.value;
    if (treasuryResult.status === 'fulfilled') treasury = treasuryResult.value;
    if (splitResult.status === 'fulfilled') interestSplit = splitResult.value;
    loading = false;
  });

  // Aggregate totals from fee series
  const totalBorrowFees = $derived(
    feeData.reduce((s, d) => s + e8sToNumber(d.borrowing_fees_e8s?.[0] ?? d.borrowing_fees_e8s ?? 0n), 0)
  );
  const totalRedemptionFees = $derived(
    feeData.reduce((s, d) => s + e8sToNumber(d.redemption_fees_e8s?.[0] ?? d.redemption_fees_e8s ?? 0n), 0)
  );
  const totalSwapFees = $derived(
    feeData.reduce((s, d) => s + e8sToNumber(d.swap_fees_e8s ?? 0n), 0)
  );
  const totalFees = $derived(totalBorrowFees + totalRedemptionFees + totalSwapFees);

  // 24h fees from trade activity
  const fees24h = $derived(tradeActivity ? e8sToNumber(tradeActivity.total_fees_e8s) : 0);

  // Treasury balance
  const treasuryBalance = $derived.by(() => {
    if (!treasury) return 0;
    const raw = treasury.total_balance ?? treasury.total_balance_e8s ?? 0n;
    return e8sToNumber(raw);
  });
</script>

<svelte:head>
  <title>Revenue | Rumi Explorer</title>
</svelte:head>

<div class="space-y-6">
  <h2 class="text-sm font-medium text-gray-400 uppercase tracking-wider">Protocol Revenue</h2>

  {#if loading}
    <div class="explorer-card flex items-center justify-center py-12">
      <div class="w-5 h-5 border-2 border-gray-600 border-t-teal-400 rounded-full animate-spin"></div>
    </div>
  {:else}
    <!-- Revenue Vitals -->
    <div class="grid grid-cols-2 md:grid-cols-4 gap-3">
      <div class="explorer-card">
        <div class="text-xs text-gray-500 mb-1">Total Fees (90d)</div>
        <div class="text-lg font-semibold tabular-nums text-gray-200">${formatCompact(totalFees)}</div>
      </div>
      <div class="explorer-card">
        <div class="text-xs text-gray-500 mb-1">24h Fees</div>
        <div class="text-lg font-semibold tabular-nums text-gray-200">${formatCompact(fees24h)}</div>
      </div>
      <div class="explorer-card">
        <div class="text-xs text-gray-500 mb-1">Treasury</div>
        <div class="text-lg font-semibold tabular-nums text-gray-200">
          {treasuryBalance > 0 ? `$${formatCompact(treasuryBalance)}` : '--'}
        </div>
      </div>
      <div class="explorer-card">
        <div class="text-xs text-gray-500 mb-1">Fee Breakdown (90d)</div>
        <div class="space-y-1 mt-1">
          <div class="flex justify-between text-xs">
            <span class="text-gray-500">Borrowing</span>
            <span class="tabular-nums text-gray-300">${formatCompact(totalBorrowFees)}</span>
          </div>
          <div class="flex justify-between text-xs">
            <span class="text-gray-500">Redemption</span>
            <span class="tabular-nums text-gray-300">${formatCompact(totalRedemptionFees)}</span>
          </div>
          <div class="flex justify-between text-xs">
            <span class="text-gray-500">Swap</span>
            <span class="tabular-nums text-gray-300">${formatCompact(totalSwapFees)}</span>
          </div>
        </div>
      </div>
    </div>

    <!-- Daily Fee History -->
    <div class="explorer-card">
      <h3 class="text-sm font-medium text-gray-300 mb-3">Daily Fee Revenue (last 14 days)</h3>
      {#if feeData.length === 0}
        <div class="flex items-center justify-center text-gray-500 text-sm py-8">No fee data available</div>
      {:else}
        <div class="space-y-1.5">
          {#each feeData.slice(-14) as day}
            {@const borrow = e8sToNumber(day.borrowing_fees_e8s?.[0] ?? day.borrowing_fees_e8s ?? 0n)}
            {@const redemption = e8sToNumber(day.redemption_fees_e8s?.[0] ?? day.redemption_fees_e8s ?? 0n)}
            {@const swap = e8sToNumber(day.swap_fees_e8s ?? 0n)}
            {@const total = borrow + redemption + swap}
            {@const maxFee = Math.max(...feeData.slice(-14).map((d: any) => {
              const b = e8sToNumber(d.borrowing_fees_e8s?.[0] ?? d.borrowing_fees_e8s ?? 0n);
              const r = e8sToNumber(d.redemption_fees_e8s?.[0] ?? d.redemption_fees_e8s ?? 0n);
              const s = e8sToNumber(d.swap_fees_e8s ?? 0n);
              return b + r + s;
            }))}
            {@const pct = maxFee > 0 ? (total / maxFee) * 100 : 0}
            <div class="flex items-center gap-2 text-xs">
              <span class="w-14 text-gray-500 tabular-nums flex-shrink-0">
                {formatDateShort(nsToDate(day.timestamp_ns))}
              </span>
              <div class="flex-1 h-4 rounded-sm overflow-hidden" style="background: var(--rumi-bg-surface2);">
                <div class="h-full rounded-sm bg-teal-400/40" style="width: {pct}%;"></div>
              </div>
              <span class="w-16 text-right text-gray-400 tabular-nums flex-shrink-0">
                ${formatCompact(total)}
              </span>
            </div>
          {/each}
        </div>
      {/if}
    </div>

    <!-- Swap Volume History -->
    <div class="explorer-card">
      <h3 class="text-sm font-medium text-gray-300 mb-3">Daily Swap Volume (last 14 days)</h3>
      {#if swapData.length === 0}
        <div class="flex items-center justify-center text-gray-500 text-sm py-8">No swap data available</div>
      {:else}
        <div class="space-y-1.5">
          {#each swapData.slice(-14) as day}
            {@const vol = e8sToNumber(day.three_pool_volume_e8s) + e8sToNumber(day.amm_volume_e8s)}
            {@const maxVol = Math.max(...swapData.slice(-14).map((d: any) => e8sToNumber(d.three_pool_volume_e8s) + e8sToNumber(d.amm_volume_e8s)))}
            {@const pct = maxVol > 0 ? (vol / maxVol) * 100 : 0}
            <div class="flex items-center gap-2 text-xs">
              <span class="w-14 text-gray-500 tabular-nums flex-shrink-0">
                {formatDateShort(nsToDate(day.timestamp_ns))}
              </span>
              <div class="flex-1 h-4 rounded-sm overflow-hidden" style="background: var(--rumi-bg-surface2);">
                <div class="h-full rounded-sm bg-violet-400/40" style="width: {pct}%;"></div>
              </div>
              <span class="w-16 text-right text-gray-400 tabular-nums flex-shrink-0">
                ${formatCompact(vol)}
              </span>
            </div>
          {/each}
        </div>
      {/if}
    </div>
  {/if}
</div>
