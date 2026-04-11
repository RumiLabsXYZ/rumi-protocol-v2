<script lang="ts">
  import { onMount } from 'svelte';
  import { Principal } from '@dfinity/principal';
  import CeilingBar from '$components/explorer/CeilingBar.svelte';
  import {
    fetchProtocolSummary, fetchVolatility, fetchLiquidationSeries
  } from '$services/explorer/analyticsService';
  import {
    fetchCollateralConfigs, fetchCollateralTotals, fetchLiquidatableVaults, fetchBotStats
  } from '$services/explorer/explorerService';
  import {
    e8sToNumber, formatCompact, bpsToPercent, nsToDate, formatDateShort,
    COLLATERAL_SYMBOLS, COLLATERAL_COLORS
  } from '$utils/explorerChartHelpers';
  import { decodeRustDecimal } from '$utils/decimalUtils';

  let loading = $state(true);
  let systemCrBps = $state(0);
  let totalDebt = $state(0);
  let totalCollateralUsd = $state(0);
  let liquidatableVaults: any[] = $state([]);
  let botStats: any = $state(null);
  let liquidationSeries: any[] = $state([]);

  interface CollateralRisk {
    principal: string;
    symbol: string;
    color: string;
    volatility: number | null;
    liquidationRatio: number;
    vaultCount: number;
    totalDebt: number;
    totalCollateralUsd: number;
    debtCeiling: number;
    unlimited: boolean;
  }

  let collateralRisks: CollateralRisk[] = $state([]);

  onMount(async () => {
    const principals = Object.keys(COLLATERAL_SYMBOLS);

    const [summaryResult, configsResult, totalsResult, liqVaultsResult, botResult, liqSeriesResult, ...volResults] =
      await Promise.allSettled([
        fetchProtocolSummary(),
        fetchCollateralConfigs(),
        fetchCollateralTotals(),
        fetchLiquidatableVaults(),
        fetchBotStats(),
        fetchLiquidationSeries(90),
        ...principals.map(p => fetchVolatility(Principal.fromText(p))),
      ]);

    // System CR
    if (summaryResult.status === 'fulfilled' && summaryResult.value) {
      systemCrBps = summaryResult.value.system_cr_bps;
      totalDebt = e8sToNumber(summaryResult.value.total_debt_e8s);
      totalCollateralUsd = e8sToNumber(summaryResult.value.total_collateral_usd_e8s);
    }

    // Liquidatable vaults
    if (liqVaultsResult.status === 'fulfilled') {
      liquidatableVaults = liqVaultsResult.value ?? [];
    }

    // Bot stats
    if (botResult.status === 'fulfilled') {
      botStats = botResult.value;
    }

    // Liquidation series
    if (liqSeriesResult.status === 'fulfilled') {
      liquidationSeries = liqSeriesResult.value ?? [];
    }

    // Build collateral risk breakdown
    const configs = configsResult.status === 'fulfilled' ? configsResult.value ?? [] : [];
    const totals = totalsResult.status === 'fulfilled' ? totalsResult.value ?? [] : [];
    const summaryPrices = summaryResult.status === 'fulfilled' && summaryResult.value
      ? summaryResult.value.prices : [];

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

    const priceMap = new Map<string, number>();
    for (const p of summaryPrices) {
      const pid = p.collateral?.toText?.() ?? String(p.collateral);
      priceMap.set(pid, p.twap_price);
    }

    const volMap = new Map<string, number>();
    for (let i = 0; i < principals.length; i++) {
      const r = volResults[i];
      if (r.status === 'fulfilled' && r.value) {
        volMap.set(principals[i], r.value.annualized_vol_pct);
      }
    }

    const risks: CollateralRisk[] = [];
    for (const [principal, symbol] of Object.entries(COLLATERAL_SYMBOLS)) {
      const cfg = configMap.get(principal);
      const tot = totalsMap.get(principal);
      const price = priceMap.get(principal) ?? 0;

      const liqRatio = cfg ? decodeRustDecimal(cfg.liquidation_ratio) : 0;
      const totalColl = tot?.total_collateral_e8s != null ? e8sToNumber(tot.total_collateral_e8s) : 0;
      const debt = tot?.total_debt_e8s != null ? e8sToNumber(tot.total_debt_e8s) : 0;
      const vaults = tot?.vault_count != null ? Number(tot.vault_count) : 0;
      const debtCeilingRaw = cfg?.debt_ceiling ?? 0n;
      const isUnlimited = typeof debtCeilingRaw === 'bigint'
        ? debtCeilingRaw >= 18446744073709551615n
        : Number(debtCeilingRaw) >= Number.MAX_SAFE_INTEGER;

      risks.push({
        principal,
        symbol,
        color: COLLATERAL_COLORS[symbol as keyof typeof COLLATERAL_COLORS] ?? '#888',
        volatility: volMap.get(principal) ?? null,
        liquidationRatio: liqRatio,
        vaultCount: vaults,
        totalDebt: debt,
        totalCollateralUsd: totalColl * price,
        debtCeiling: e8sToNumber(Number(debtCeilingRaw)),
        unlimited: isUnlimited,
      });
    }

    risks.sort((a, b) => b.totalCollateralUsd - a.totalCollateralUsd);
    collateralRisks = risks;
    loading = false;
  });

  const systemCrPct = $derived(systemCrBps > 0 ? (systemCrBps / 100).toFixed(0) : '0');
  const systemCrColor = $derived(
    systemCrBps < 15000 ? 'text-pink-400' : systemCrBps < 20000 ? 'text-violet-400' : 'text-teal-400'
  );

  // Recent liquidation count
  const recentLiquidations = $derived(
    liquidationSeries.reduce((s: number, d: any) => s + (d.liquidation_count ?? 0), 0)
  );
</script>

<svelte:head>
  <title>Risk | Rumi Explorer</title>
</svelte:head>

<div class="space-y-6">
  <h2 class="text-sm font-medium text-gray-400 uppercase tracking-wider">Protocol Risk</h2>

  {#if loading}
    <div class="explorer-card flex items-center justify-center py-12">
      <div class="w-5 h-5 border-2 border-gray-600 border-t-teal-400 rounded-full animate-spin"></div>
    </div>
  {:else}
    <!-- System Health -->
    <div class="grid grid-cols-2 md:grid-cols-4 gap-3">
      <div class="explorer-card">
        <div class="text-xs text-gray-500 mb-1">System CR</div>
        <div class="text-2xl font-semibold tabular-nums {systemCrColor}">{systemCrPct}%</div>
      </div>
      <div class="explorer-card">
        <div class="text-xs text-gray-500 mb-1">Total Collateral</div>
        <div class="text-lg font-semibold tabular-nums text-gray-200">${formatCompact(totalCollateralUsd)}</div>
      </div>
      <div class="explorer-card">
        <div class="text-xs text-gray-500 mb-1">Total Debt</div>
        <div class="text-lg font-semibold tabular-nums text-gray-200">{formatCompact(totalDebt)} icUSD</div>
      </div>
      <div class="explorer-card">
        <div class="text-xs text-gray-500 mb-1">Liquidatable Vaults</div>
        <div class="text-lg font-semibold tabular-nums {liquidatableVaults.length > 0 ? 'text-pink-400' : 'text-teal-400'}">
          {liquidatableVaults.length}
        </div>
      </div>
    </div>

    <!-- Collateral Risk Table -->
    <div class="explorer-card">
      <h3 class="text-sm font-medium text-gray-300 mb-3">Collateral Risk Breakdown</h3>
      <div class="overflow-x-auto">
        <table class="w-full text-sm">
          <thead>
            <tr class="border-b text-xs text-gray-500 uppercase tracking-wider" style="border-color: var(--rumi-border);">
              <th class="px-3 py-2 text-left">Asset</th>
              <th class="px-3 py-2 text-right">Volatility</th>
              <th class="px-3 py-2 text-right hidden sm:table-cell">Liq. Ratio</th>
              <th class="px-3 py-2 text-right hidden md:table-cell">Vaults</th>
              <th class="px-3 py-2 text-right">Exposure</th>
              <th class="px-3 py-2 text-right hidden sm:table-cell">Debt</th>
              <th class="px-3 py-2 text-right hidden md:table-cell">Ceiling</th>
            </tr>
          </thead>
          <tbody>
            {#each collateralRisks as risk}
              <tr class="border-b hover:bg-white/[0.02] transition-colors" style="border-color: var(--rumi-border);">
                <td class="px-3 py-2.5">
                  <div class="flex items-center gap-2">
                    <span class="w-2.5 h-2.5 rounded-full flex-shrink-0" style="background: {risk.color};"></span>
                    <span class="font-medium text-gray-200">{risk.symbol}</span>
                  </div>
                </td>
                <td class="px-3 py-2.5 text-right tabular-nums {risk.volatility != null && risk.volatility > 80 ? 'text-pink-400' : risk.volatility != null && risk.volatility > 40 ? 'text-violet-400' : 'text-gray-400'}">
                  {risk.volatility != null ? `${risk.volatility.toFixed(1)}%` : '--'}
                </td>
                <td class="px-3 py-2.5 text-right tabular-nums text-gray-400 hidden sm:table-cell">
                  {risk.liquidationRatio > 0 ? `${(risk.liquidationRatio * 100).toFixed(0)}%` : '--'}
                </td>
                <td class="px-3 py-2.5 text-right tabular-nums text-gray-400 hidden md:table-cell">
                  {risk.vaultCount}
                </td>
                <td class="px-3 py-2.5 text-right tabular-nums text-gray-200">
                  ${formatCompact(risk.totalCollateralUsd)}
                </td>
                <td class="px-3 py-2.5 text-right tabular-nums text-gray-400 hidden sm:table-cell">
                  {formatCompact(risk.totalDebt)}
                </td>
                <td class="px-3 py-2.5 hidden md:table-cell">
                  <div class="flex justify-end">
                    <div class="w-20">
                      <CeilingBar used={risk.totalDebt} total={risk.debtCeiling} unlimited={risk.unlimited} />
                    </div>
                  </div>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    </div>

    <!-- Liquidation History -->
    <div class="explorer-card">
      <h3 class="text-sm font-medium text-gray-300 mb-3">Liquidation History (90d)</h3>
      {#if liquidationSeries.length === 0}
        <div class="flex items-center justify-center text-gray-500 text-sm py-8">No liquidation data</div>
      {:else}
        <div class="text-xs text-gray-500 mb-3">Total: {recentLiquidations} liquidations</div>
        <div class="space-y-1.5">
          {#each liquidationSeries.slice(-14) as day}
            {@const count = day.liquidation_count ?? 0}
            {@const maxCount = Math.max(...liquidationSeries.slice(-14).map((d: any) => d.liquidation_count ?? 0))}
            {@const pct = maxCount > 0 ? (count / maxCount) * 100 : 0}
            <div class="flex items-center gap-2 text-xs">
              <span class="w-14 text-gray-500 tabular-nums flex-shrink-0">
                {formatDateShort(nsToDate(day.timestamp_ns))}
              </span>
              <div class="flex-1 h-4 rounded-sm overflow-hidden" style="background: var(--rumi-bg-surface2);">
                <div class="h-full rounded-sm bg-pink-400/40" style="width: {pct}%;"></div>
              </div>
              <span class="w-8 text-right text-gray-400 tabular-nums flex-shrink-0">
                {count}
              </span>
            </div>
          {/each}
        </div>
      {/if}
    </div>

    <!-- Liquidatable Vaults (if any) -->
    {#if liquidatableVaults.length > 0}
      <div class="explorer-card">
        <h3 class="text-sm font-medium text-pink-400 mb-3">Currently Liquidatable Vaults</h3>
        <div class="overflow-x-auto">
          <table class="w-full text-sm">
            <thead>
              <tr class="border-b text-xs text-gray-500 uppercase tracking-wider" style="border-color: var(--rumi-border);">
                <th class="px-3 py-2 text-left">Vault</th>
                <th class="px-3 py-2 text-right">Collateral</th>
                <th class="px-3 py-2 text-right">Debt</th>
              </tr>
            </thead>
            <tbody>
              {#each liquidatableVaults.slice(0, 20) as vault}
                <tr class="border-b hover:bg-white/[0.02]" style="border-color: var(--rumi-border);">
                  <td class="px-3 py-2">
                    <a href="/explorer/vault/{vault.vault_id}" class="text-teal-400 hover:text-teal-300">
                      #{String(vault.vault_id)}
                    </a>
                  </td>
                  <td class="px-3 py-2 text-right tabular-nums text-gray-300">
                    {e8sToNumber(Number(vault.collateral_amount)).toFixed(4)}
                  </td>
                  <td class="px-3 py-2 text-right tabular-nums text-gray-300">
                    {e8sToNumber(Number(vault.borrowed_icusd_amount)).toFixed(2)} icUSD
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
        {#if liquidatableVaults.length > 20}
          <div class="text-center py-2 text-xs text-gray-500">
            Showing 20 of {liquidatableVaults.length} liquidatable vaults
          </div>
        {/if}
      </div>
    {/if}
  {/if}
</div>
