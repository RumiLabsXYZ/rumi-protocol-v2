<script lang="ts">
  import { onMount, onDestroy } from 'svelte';
  import LensHealthStrip from '../LensHealthStrip.svelte';
  import LensActivityPanel from '../LensActivityPanel.svelte';
  import MiniAreaChart from '../MiniAreaChart.svelte';
  import TreasuryHoldingsCard from '../TreasuryHoldingsCard.svelte';
  import ProtocolReservesCard from '../ProtocolReservesCard.svelte';
  import {
    fetchFeeSeries, fetchApys, fetchFeeBreakdownWindow, type FeeBreakdown,
  } from '$services/explorer/analyticsService';
  import {
    fetchInterestSplit, fetchStabilityPoolStatus, fetchThreePoolStatus,
    createProtocolFeeScan,
  } from '$services/explorer/explorerService';
  import { completeFee, formatFeeE8s, type FeeScanState } from '$services/explorer/feeScan';
  import { ProtocolService } from '$services/protocol';
  import { e8sToNumber, formatCompact, CHART_COLORS } from '$utils/explorerChartHelpers';
  import { liveSpApyPct, liveLpApyPct } from '$utils/liveApy';

  let feeRows: any[] = $state([]);
  let feeSeriesState = $state<'loading' | 'success' | 'error'>('loading');
  let apys: any = $state(null);
  let fees24hData = $state<FeeBreakdown | null>(null);
  let fees90dData = $state<FeeBreakdown | null>(null);
  let feeScan = $state.raw<FeeScanState | null>(null);
  let scan: ReturnType<typeof createProtocolFeeScan> | undefined;
  let disposed = false;
  function runFees(refresh = false) {
    if (refresh) { scan?.cancel(); scan = undefined; }
    scan ??= createProtocolFeeScan({ refresh });
    const active = scan;
    feeScan = active.snapshot();
    void active.run(next => { if (!disposed && scan === active) feeScan = next; });
  }
  onDestroy(() => { disposed = true; scan?.cancel(); });
  let interestSplit: any[] = $state([]);
  let protocolStatus: any = $state(null);
  let poolStatus: any = $state(null);
  let threePoolStatus: any = $state(null);
  let loading = $state(true);

  onMount(async () => {
    runFees();
    try {
      // Borrow / redemption fees come from the backend event log directly.
      // The analytics rollup (`fetchFeeBreakdownWindow`) reads `evt_vaults`
      // whose historical rows have `fee_amount = None` (round-1 added the
      // capture; pre-round-1 entries are stuck at null), which is why the
      // 90d totals previously showed only swap fees. Swap fees stay on
      // analytics (`evt_swaps` was always populated correctly).
      const [feeR, apR, f24R, f90R, spR, psR, poolR, tpR] = await Promise.allSettled([
        fetchFeeSeries(90, { throwOnError: true }),
        fetchApys(),
        fetchFeeBreakdownWindow(1),
        fetchFeeBreakdownWindow(90),
        fetchInterestSplit(),
        ProtocolService.getProtocolStatus(),
        fetchStabilityPoolStatus(),
        fetchThreePoolStatus(),
      ]);
      if (feeR.status === 'fulfilled') {
        feeRows = feeR.value ?? [];
        feeSeriesState = 'success';
      } else {
        feeSeriesState = 'error';
      }
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

  // Borrow / redemption: backend event log (authoritative). Swap: analytics
  // (`evt_swaps` was always populated correctly by the 3pool / AMM tailers).
  const totalBorrow = $derived(completeFee(feeScan, 1, 'borrow'));
  const totalRedemption = $derived(completeFee(feeScan, 1, 'redemption'));
  const totalSwap = $derived(fees90dData?.swapIcusd ?? null);
  const totalFees = $derived(
    totalBorrow != null && totalRedemption != null && totalSwap != null
      ? Number(totalBorrow + totalRedemption) / 1e8 + totalSwap
      : null
  );

  const borrow24h = $derived(completeFee(feeScan, 0, 'borrow'));
  const redemption24h = $derived(completeFee(feeScan, 0, 'redemption'));
  // These combined headlines include approximate analytics swap values. Exact
  // backend amounts are kept as bigint and rendered separately below.
  const fees24h = $derived(
    borrow24h != null && redemption24h != null && fees24hData != null
      ? Number(borrow24h + redemption24h) / 1e8 + fees24hData.swapIcusd : null
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

  // The analytics borrowing/redemption fields include historical rows that
  // predate fee capture; only the independently reliable swap series belongs
  // in this chart.
  const feePoints = $derived(
    feeRows.map((r: any) => {
      const s = e8sToNumber(r.swap_fees_e8s ?? 0n);
      return { t: Number(r.timestamp_ns) / 1_000_000, v: s };
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
    { label: 'Fees (90d)', value: totalFees == null ? 'Unavailable' : `≈$${formatCompact(totalFees)}` },
    { label: '24h fees', value: fees24h == null ? 'Unavailable' : `≈$${formatCompact(fees24h)}` },
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

<section class="explorer-card text-sm space-y-3" aria-label="Borrowing and redemption fee coverage">
  <h3 class="font-medium">Borrowing and redemption fees</h3>
  <p aria-live="polite">
    {#if !feeScan || feeScan.phase === 'idle'}Loading fee history…
    {:else if feeScan.phase === 'error'}Unavailable — scan failed.
    {:else if feeScan.phase === 'scanning'}Scanning fee history…
    {:else if feeScan.phase === 'incomplete'}Incomplete — more history remains.
    {:else if feeScan.undated.borrow.count > 0n || feeScan.undated.redemption.count > 0n}Incomplete — historical fee records have no timestamp.
    {:else}Fee history complete for this snapshot.{/if}
    {#if feeScan?.end != null} Scanned {feeScan.cursor.toString()} of {feeScan.end.toString()} records.{/if}
  </p>
  {#if feeScan}
    <p class="text-gray-400">As of {new Date(Number(feeScan.asOfNs / 1_000_000n)).toISOString()}.
      {#if feeScan.end === 0n}Empty event-log snapshot.
      {:else if feeScan.end != null}Recorded through event {(feeScan.end - 1n).toString()}.{/if}
      Later records are included on refresh. Large histories may need several Continue steps.
    </p>
    {#each feeScan.windows as window, index}
      <dl aria-label="{index === 0 ? '24h' : '90d'} exact backend fees" class="space-y-1">
        {#each ['borrow', 'redemption'] as category}
          {@const kind = category as 'borrow' | 'redemption'}
          {@const total = completeFee(feeScan, index, kind)}
          <div class="flex flex-wrap justify-between gap-2">
            <dt>{index === 0 ? '24h' : '90d'} {kind === 'borrow' ? 'borrowing' : 'redemption'}</dt>
            <dd class="tabular-nums break-all">{#if total != null}{formatFeeE8s(total)} icUSD{:else}Unavailable — partial known-dated subtotal: {formatFeeE8s(window[kind])} icUSD{/if}</dd>
          </div>
        {/each}
      </dl>
    {/each}
    {#each ['borrow', 'redemption'] as category}
      {@const kind = category as 'borrow' | 'redemption'}
      {#if feeScan.undated[kind].count > 0n}
        <p class="text-amber-300">{feeScan.undated[kind].count.toString()} undated {kind === 'borrow' ? 'borrowing' : 'redemption'} fee records ({formatFeeE8s(feeScan.undated[kind].amount)} icUSD) cannot be assigned to a time window. Retrying does not recover missing timestamps.</p>
      {/if}
    {/each}
  {/if}
  <div class="flex gap-3">
    {#if feeScan?.phase === 'incomplete'}<button class="text-teal-400" onclick={() => runFees()}>Continue</button>{/if}
    {#if feeScan?.phase === 'error'}<button class="text-teal-400" onclick={() => runFees()}>Retry fee scan</button>{/if}
    <button class="text-teal-400 disabled:opacity-50" disabled={!feeScan || feeScan.phase === 'scanning'} onclick={() => runFees(true)}>Refresh fee snapshot</button>
  </div>
</section>

<div class="explorer-card" role="region" aria-label="Daily swap fees over 90 days">
  {#if feeSeriesState === 'loading'}
    <div class="flex items-center justify-center py-8">
      <div class="w-5 h-5 border-2 border-gray-600 border-t-teal-400 rounded-full animate-spin"></div>
    </div>
  {:else if feeSeriesState === 'error'}
    <p class="text-sm text-gray-500 py-4">Swap fee history unavailable.</p>
  {:else}
    <MiniAreaChart
      points={feePoints}
      label="Daily swap fees (90d)"
      color={CHART_COLORS.action}
      valueFormat={(v) => `$${formatCompact(v)}`}
      headlineValue={feePoints.reduce((s, p) => s + p.v, 0)}
      height={180}
      kind="bar"
    />
  {/if}
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
        { label: 'Borrowing', value: totalBorrow == null ? null : Number(totalBorrow) / 1e8, exact: totalBorrow == null ? null : `${formatFeeE8s(totalBorrow)} icUSD`, color: CHART_COLORS.teal },
        { label: 'Redemption', value: totalRedemption == null ? null : Number(totalRedemption) / 1e8, exact: totalRedemption == null ? null : `${formatFeeE8s(totalRedemption)} icUSD`, color: CHART_COLORS.purple },
        { label: 'Swap', value: totalSwap, exact: null, color: CHART_COLORS.action },
      ]}
      {@const max = Math.max(1, ...items.map((item) => item.value ?? 0))}
      <div class="space-y-2">
        {#each items as it}
          <div class="flex items-center gap-3 text-sm">
            <span class="w-24 text-gray-400">{it.label}</span>
            <div class="flex-1 h-4 rounded-sm overflow-hidden" style="background: var(--rumi-bg-surface2);">
              <div class="h-full rounded-sm" style="width: {((it.value ?? 0) / max) * 100}%; background: {it.color}; opacity: 0.7;"></div>
            </div>
            <span class="min-w-20 text-right tabular-nums text-gray-300">{it.value == null ? 'Unavailable' : it.exact ?? `≈$${formatCompact(it.value)}`}</span>
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

<ProtocolReservesCard />

<TreasuryHoldingsCard />

<!-- Revenue-bearing = borrow fees + redemptions + liquidations + swaps. -->
<LensActivityPanel
  scope="revenue"
  title="Revenue-bearing activity"
  viewAllHref="/explorer/activity?type=borrow,redemption,liquidation,swap"
/>
