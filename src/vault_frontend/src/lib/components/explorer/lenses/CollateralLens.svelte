<script lang="ts">
  import { onMount } from 'svelte';
  import { Principal } from '@dfinity/principal';
  import CollateralTable, { type CollateralRow } from '../CollateralTable.svelte';
  import LiquidationRiskTable from '../LiquidationRiskTable.svelte';
  import LensHealthStrip from '../LensHealthStrip.svelte';
  import LensActivityPanel from '../LensActivityPanel.svelte';
  import {
    fetchProtocolSummary, fetchTwap, fetchVolatility, fetchLiquidationSeries,
  } from '$services/explorer/analyticsService';
  import {
    fetchCollateralConfigs, fetchCollateralTotals, fetchAllVaults, fetchLiquidatableVaults,
  } from '$services/explorer/explorerService';
  import {
    e8sToNumber, formatCompact, bpsToPercent, COLLATERAL_SYMBOLS, COLLATERAL_COLORS,
  } from '$utils/explorerChartHelpers';
  import { COLLATERAL_DISPLAY_ORDER } from '$stores/collateralStore';

  let systemCrBps = $state(0);
  let loading = $state(true);
  let collateralRows: CollateralRow[] = $state([]);
  let atRiskVaults: any[] = $state([]);
  let allVaults: any[] = $state([]);
  let liq7dCount = $state(0);

  interface CrBucket { label: string; count: number; lo: number; hi: number }
  let crBuckets: CrBucket[] = $state([]);

  let mixTotal = $state(0);

  onMount(async () => {
    try {
      const principals = Object.keys(COLLATERAL_SYMBOLS);
      const [
        summaryR, twapR, configsR, totalsR, allVaultsR, liqVaultsR, liqSeriesR, ...volResults
      ] = await Promise.allSettled([
        fetchProtocolSummary(),
        fetchTwap(),
        fetchCollateralConfigs(),
        fetchCollateralTotals(),
        fetchAllVaults(),
        fetchLiquidatableVaults(),
        fetchLiquidationSeries(7),
        ...principals.map(p => fetchVolatility(Principal.fromText(p))),
      ]);

      const summary = summaryR.status === 'fulfilled' ? summaryR.value : null;
      systemCrBps = summary?.system_cr_bps ? Number(summary.system_cr_bps) : 0;

      const configs = configsR.status === 'fulfilled' ? configsR.value ?? [] : [];
      const totals = totalsR.status === 'fulfilled' ? totalsR.value ?? [] : [];
      allVaults = allVaultsR.status === 'fulfilled' ? allVaultsR.value ?? [] : [];
      atRiskVaults = liqVaultsR.status === 'fulfilled' ? liqVaultsR.value ?? [] : [];

      const liqSeries = liqSeriesR.status === 'fulfilled' ? liqSeriesR.value ?? [] : [];
      liq7dCount = liqSeries.reduce((s: number, d: any) => s + Number(d.liquidation_count ?? 0), 0);

      const twapEntries = twapR.status === 'fulfilled' ? twapR.value?.entries ?? [] : [];
      const priceMap = new Map<string, number>();
      for (const e of twapEntries) {
        const p = e.collateral?.toText?.() ?? String(e.collateral);
        priceMap.set(p, e.twap_price);
      }

      const configMap = new Map<string, any>();
      for (const c of configs) {
        const pid = c.ledger_canister_id?.toText?.() ?? String(c.ledger_canister_id);
        configMap.set(pid, c);
      }
      const totalsMap = new Map<string, any>();
      for (const t of totals) {
        const pid = t.collateral_type?.toText?.() ?? String(t.collateral_type ?? '');
        if (pid) totalsMap.set(pid, t);
      }

      const volMap = new Map<string, number>();
      for (let i = 0; i < principals.length; i++) {
        const r = volResults[i];
        if (r.status === 'fulfilled' && r.value) volMap.set(principals[i], r.value.annualized_vol_pct);
      }

      const rows: CollateralRow[] = [];
      for (const [principal, symbol] of Object.entries(COLLATERAL_SYMBOLS)) {
        const cfg = configMap.get(principal);
        const tot = totalsMap.get(principal);
        const decimals = tot?.decimals != null ? Number(tot.decimals) : 8;
        const price = priceMap.get(principal) ?? (tot?.price ? Number(tot.price) : 0);
        const totalColl = tot?.total_collateral != null ? Number(tot.total_collateral) / Math.pow(10, decimals) : 0;
        const debt = tot?.total_debt != null ? e8sToNumber(tot.total_debt) : 0;
        const vaults = tot?.vault_count != null ? Number(tot.vault_count) : 0;
        const ceilingRaw = cfg?.debt_ceiling ?? 0n;
        const unlimited = typeof ceilingRaw === 'bigint'
          ? ceilingRaw >= 18446744073709551615n
          : Number(ceilingRaw) >= Number.MAX_SAFE_INTEGER;

        rows.push({
          principal, symbol, price, vaultCount: vaults,
          totalCollateralUsd: totalColl * price,
          totalDebt: debt,
          debtCeiling: e8sToNumber(ceilingRaw),
          unlimited,
          medianCrBps: 0,
          volatility: volMap.get(principal) ?? null,
        });
      }
      rows.sort((a, b) => {
        const ai = COLLATERAL_DISPLAY_ORDER.indexOf(a.symbol);
        const bi = COLLATERAL_DISPLAY_ORDER.indexOf(b.symbol);
        return (ai === -1 ? 999 : ai) - (bi === -1 ? 999 : bi);
      });
      collateralRows = rows;

      mixTotal = rows.reduce((s, r) => s + r.totalCollateralUsd, 0);

      // CR distribution histogram from all active vaults
      const bucketDefs: [number, number, string][] = [
        [0, 110, '<110%'],
        [110, 150, '110-150%'],
        [150, 200, '150-200%'],
        [200, 300, '200-300%'],
        [300, 500, '300-500%'],
        [500, Infinity, '500%+'],
      ];
      const buckets: CrBucket[] = bucketDefs.map(([lo, hi, label]) => ({ lo, hi, label, count: 0 }));
      for (const v of allVaults) {
        const cr = Number(v.collateral_ratio ?? 0);
        if (cr === 0) continue;
        const pct = cr; // backend uses percent (e.g. 243 for 243%)
        for (const b of buckets) {
          if (pct >= b.lo && pct < b.hi) { b.count += 1; break; }
        }
      }
      crBuckets = buckets;
    } catch (err) {
      console.error('[CollateralLens] onMount error:', err);
    } finally {
      loading = false;
    }
  });

  const systemCrTone = $derived.by(() => {
    if (systemCrBps === 0) return 'muted' as const;
    if (systemCrBps < 15000) return 'danger' as const;
    if (systemCrBps < 20000) return 'caution' as const;
    return 'good' as const;
  });

  const tierSpread = $derived(() => {
    // Tier 1/2/3 are seeded in project memory; for v1 we hardcode the display
    return '1-3';
  });

  const healthMetrics = $derived.by(() => [
    { label: 'Aggregate CR', value: systemCrBps > 0 ? bpsToPercent(systemCrBps) : '--', tone: systemCrTone },
    { label: 'At-risk vaults', value: String(atRiskVaults.length), tone: atRiskVaults.length > 0 ? 'caution' as const : 'good' as const },
    { label: 'Liquidated (7d)', value: String(liq7dCount), sub: liq7dCount > 0 ? 'events' : 'none' },
    { label: 'Global liq threshold', value: '110%', sub: 'min CR' },
    { label: 'Redeem tier spread', value: tierSpread() },
  ]);

  const maxBucket = $derived(Math.max(1, ...crBuckets.map(b => b.count)));
</script>

<LensHealthStrip title="Collateral health" metrics={healthMetrics} loading={loading} />

<div class="grid grid-cols-1 lg:grid-cols-2 gap-3">
  <!-- Collateral mix -->
  <div class="explorer-card">
    <h3 class="text-sm font-medium text-gray-300 mb-3">Collateral mix</h3>
    {#if loading}
      <div class="flex items-center justify-center py-8">
        <div class="w-5 h-5 border-2 border-gray-600 border-t-teal-400 rounded-full animate-spin"></div>
      </div>
    {:else if mixTotal === 0}
      <p class="text-sm text-gray-500 py-4">No collateral deposited.</p>
    {:else}
      <!-- stacked mix bar -->
      <div class="flex h-3 rounded overflow-hidden mb-3">
        {#each collateralRows as r}
          {#if r.totalCollateralUsd > 0}
            <div
              style="flex: {r.totalCollateralUsd / mixTotal}; background: {COLLATERAL_COLORS[r.symbol] ?? '#444'};"
              title="{r.symbol}: ${formatCompact(r.totalCollateralUsd)}"
            ></div>
          {/if}
        {/each}
      </div>
      <div class="space-y-1.5">
        {#each collateralRows as r}
          {#if r.totalCollateralUsd > 0}
            {@const pct = (r.totalCollateralUsd / mixTotal) * 100}
            <div class="flex items-center gap-2 text-xs">
              <span class="w-2 h-2 rounded-full flex-shrink-0" style="background: {COLLATERAL_COLORS[r.symbol] ?? '#666'};"></span>
              <span class="w-12 font-medium text-gray-300">{r.symbol}</span>
              <div class="flex-1 h-1.5 rounded-sm overflow-hidden" style="background: var(--rumi-bg-surface2);">
                <div class="h-full rounded-sm" style="width: {pct}%; background: {COLLATERAL_COLORS[r.symbol] ?? '#666'}; opacity: 0.6;"></div>
              </div>
              <span class="w-20 text-right tabular-nums text-gray-400">${formatCompact(r.totalCollateralUsd)}</span>
              <span class="w-12 text-right tabular-nums text-gray-500">{pct.toFixed(1)}%</span>
            </div>
          {/if}
        {/each}
      </div>
    {/if}
  </div>

  <!-- CR distribution histogram -->
  <div class="explorer-card">
    <h3 class="text-sm font-medium text-gray-300 mb-3">CR distribution</h3>
    {#if loading}
      <div class="flex items-center justify-center py-8">
        <div class="w-5 h-5 border-2 border-gray-600 border-t-teal-400 rounded-full animate-spin"></div>
      </div>
    {:else if allVaults.length === 0}
      <p class="text-sm text-gray-500 py-4">No active vaults.</p>
    {:else}
      <div class="flex items-end gap-2 h-40 border-b border-white/5 mt-4">
        {#each crBuckets as b}
          {@const pct = (b.count / maxBucket) * 100}
          {@const tone = b.lo < 150 ? '#f472b6' : b.lo < 200 ? '#a78bfa' : '#2DD4BF'}
          <div class="flex-1 flex flex-col items-center justify-end">
            <span class="text-[11px] font-medium text-gray-400 mb-1">{b.count}</span>
            <div
              class="w-full rounded-t"
              style="height: {pct}%; min-height: {b.count > 0 ? '2px' : '0'}; background: {tone}; opacity: 0.75;"
            ></div>
          </div>
        {/each}
      </div>
      <div class="flex gap-2 mt-2">
        {#each crBuckets as b}
          <span class="flex-1 text-center text-[11px] text-gray-500 tabular-nums">{b.label}</span>
        {/each}
      </div>
    {/if}
  </div>
</div>

<CollateralTable rows={collateralRows} loading={loading} />

<LiquidationRiskTable
  vaults={atRiskVaults}
  totalAtRisk={atRiskVaults.length}
  loading={loading}
/>

<LensActivityPanel scope="vault_ops" title="Vault activity" />
