<script lang="ts">
  import { onMount } from 'svelte';
  import { Principal } from '@dfinity/principal';
  import LensHealthStrip from '../LensHealthStrip.svelte';
  import LensActivityPanel from '../LensActivityPanel.svelte';
  import MiniAreaChart from '../MiniAreaChart.svelte';
  import { fetchFeeSeries } from '$services/explorer/analyticsService';
  import {
    fetchRedemptionRate, fetchRedemptionFeeFloor, fetchRedemptionFeeCeiling,
    fetchRedemptionTier, fetchCollateralTotals,
  } from '$services/explorer/explorerService';
  import {
    e8sToNumber, formatCompact, CHART_COLORS, COLLATERAL_SYMBOLS, getCollateralSymbol,
  } from '$utils/explorerChartHelpers';

  let feeRows: any[] = $state([]);
  let rate: number | null = $state(null);
  let feeFloor: number | null = $state(null);
  let feeCeiling: number | null = $state(null);
  let collateralTotals: any[] = $state([]);
  let tierMap: Map<string, number | null> = $state(new Map());
  let loading = $state(true);

  onMount(async () => {
    try {
      const principals = Object.keys(COLLATERAL_SYMBOLS);
      const [feeR, rateR, floorR, ceilR, totalsR, ...tierRs] = await Promise.allSettled([
        fetchFeeSeries(90),
        fetchRedemptionRate(),
        fetchRedemptionFeeFloor(),
        fetchRedemptionFeeCeiling(),
        fetchCollateralTotals(),
        ...principals.map(p => fetchRedemptionTier(Principal.fromText(p))),
      ]);
      if (feeR.status === 'fulfilled') feeRows = feeR.value ?? [];
      if (rateR.status === 'fulfilled') rate = rateR.value;
      if (floorR.status === 'fulfilled') feeFloor = floorR.value;
      if (ceilR.status === 'fulfilled') feeCeiling = ceilR.value;
      if (totalsR.status === 'fulfilled') collateralTotals = totalsR.value ?? [];

      const tm = new Map<string, number | null>();
      for (let i = 0; i < principals.length; i++) {
        const r = tierRs[i];
        if (r.status === 'fulfilled') tm.set(principals[i], r.value);
        else tm.set(principals[i], null);
      }
      tierMap = tm;
    } catch (err) {
      console.error('[RedemptionsLens] onMount error:', err);
    } finally {
      loading = false;
    }
  });

  const redemptions90d = $derived(feeRows.reduce((s, d: any) => s + Number(d.redemption_count ?? 0), 0));
  const redemptionFees90d = $derived(
    feeRows.reduce((s, d: any) => s + e8sToNumber(d.redemption_fees_e8s?.[0] ?? d.redemption_fees_e8s ?? 0n), 0)
  );
  const redemptionCountPoints = $derived(
    feeRows.map((r: any) => ({ t: Number(r.timestamp_ns) / 1_000_000, v: Number(r.redemption_count ?? 0) }))
  );
  const redemptionFeePoints = $derived(
    feeRows.map((r: any) => ({
      t: Number(r.timestamp_ns) / 1_000_000,
      v: e8sToNumber(r.redemption_fees_e8s?.[0] ?? r.redemption_fees_e8s ?? 0n),
    }))
  );

  const formatPct = (v: number | null) =>
    v == null ? '--' : `${(v * 100).toFixed(2)}%`;

  const healthMetrics = $derived.by(() => [
    { label: 'Live rate', value: formatPct(rate), sub: 'current redemption fee' },
    { label: 'Fee floor', value: formatPct(feeFloor), tone: 'muted' as const },
    { label: 'Fee ceiling', value: formatPct(feeCeiling), tone: 'muted' as const },
    { label: 'Redemptions (90d)', value: redemptions90d.toLocaleString() },
    { label: 'Fees collected (90d)', value: `$${formatCompact(redemptionFees90d)}`, sub: 'icUSD' },
  ]);

  const tierRows = $derived.by(() => {
    const totalsByPid = new Map<string, any>();
    for (const t of collateralTotals) {
      const pid = t.collateral_type?.toText?.() ?? String(t.collateral_type ?? '');
      if (pid) totalsByPid.set(pid, t);
    }
    const rows: { principal: string; symbol: string; tier: number | null; debt: number; vaultCount: number }[] = [];
    for (const [pid, sym] of Object.entries(COLLATERAL_SYMBOLS)) {
      const t = totalsByPid.get(pid);
      rows.push({
        principal: pid,
        symbol: sym,
        tier: tierMap.get(pid) ?? null,
        debt: t?.total_debt != null ? e8sToNumber(t.total_debt) : 0,
        vaultCount: t?.vault_count != null ? Number(t.vault_count) : 0,
      });
    }
    rows.sort((a, b) => {
      const at = a.tier ?? 999;
      const bt = b.tier ?? 999;
      if (at !== bt) return at - bt;
      return b.debt - a.debt;
    });
    return rows;
  });

  function tierTone(tier: number | null): string {
    if (tier == null) return 'text-gray-500';
    if (tier === 1) return 'text-teal-400';
    if (tier === 2) return 'text-amber-400';
    return 'text-violet-400';
  }
</script>

<LensHealthStrip title="Redemptions" metrics={healthMetrics} loading={loading} />

<div class="grid grid-cols-1 lg:grid-cols-2 gap-3">
  <div class="explorer-card">
    <MiniAreaChart
      points={redemptionCountPoints}
      label="Daily redemption count (90d)"
      color={CHART_COLORS.teal}
      fillColor={CHART_COLORS.tealDim}
      valueFormat={(v) => v.toLocaleString(undefined, { maximumFractionDigits: 0 })}
      loading={loading}
    />
  </div>
  <div class="explorer-card">
    <MiniAreaChart
      points={redemptionFeePoints}
      label="Daily redemption fees (90d)"
      color={CHART_COLORS.purple}
      fillColor={CHART_COLORS.purpleDim}
      valueFormat={(v) => `$${formatCompact(v)}`}
      loading={loading}
    />
  </div>
</div>

<div class="explorer-card">
  <h3 class="text-sm font-medium text-gray-300 mb-3">Redemption tiers</h3>
  {#if loading}
    <div class="flex items-center justify-center py-6">
      <div class="w-5 h-5 border-2 border-gray-600 border-t-teal-400 rounded-full animate-spin"></div>
    </div>
  {:else}
    <div class="overflow-x-auto">
      <table class="w-full text-sm">
        <thead>
          <tr class="border-b border-white/5">
            <th class="text-left py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">Token</th>
            <th class="text-right py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">Tier</th>
            <th class="text-right py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">Vaults</th>
            <th class="text-right py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">Outstanding debt</th>
          </tr>
        </thead>
        <tbody>
          {#each tierRows as r}
            <tr class="border-b border-white/[0.03] hover:bg-white/[0.02]">
              <td class="py-2 px-2 font-medium text-gray-200">{r.symbol}</td>
              <td class="py-2 px-2 text-right tabular-nums {tierTone(r.tier)}">
                {r.tier != null ? `Tier ${r.tier}` : '--'}
              </td>
              <td class="py-2 px-2 text-right tabular-nums text-gray-400">{r.vaultCount}</td>
              <td class="py-2 px-2 text-right tabular-nums text-gray-300">{formatCompact(r.debt)} icUSD</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
    <p class="text-xs text-gray-500 mt-3">
      Lower tiers are redeemed against first. Tier 1 absorbs redemptions before tier 2, which absorbs before tier 3.
    </p>
  {/if}
</div>

<LensActivityPanel scope="redemptions" title="Redemption activity" viewAllHref="/explorer/activity?type=redemption" />
