<script lang="ts">
  import { onMount } from 'svelte';
  import { Principal } from '@dfinity/principal';
  import CeilingBar from '$components/explorer/CeilingBar.svelte';
  import {
    fetchTwap, fetchVolatility, fetchProtocolSummary
  } from '$services/explorer/analyticsService';
  import {
    fetchCollateralConfigs, fetchCollateralTotals
  } from '$services/explorer/explorerService';
  import {
    e8sToNumber, formatCompact, COLLATERAL_SYMBOLS, COLLATERAL_COLORS
  } from '$utils/explorerChartHelpers';

  interface MarketRow {
    principal: string;
    symbol: string;
    color: string;
    price: number;
    twapPrice: number;
    volatility: number | null;
    vaultCount: number;
    tvlUsd: number;
    totalDebt: number;
    debtCeiling: number;
    unlimited: boolean;
  }

  let rows: MarketRow[] = $state([]);
  let loading = $state(true);

  onMount(async () => {
    const principals = Object.keys(COLLATERAL_SYMBOLS);

    const [twapResult, configsResult, totalsResult, summaryResult, ...volResults] = await Promise.allSettled([
      fetchTwap(),
      fetchCollateralConfigs(),
      fetchCollateralTotals(),
      fetchProtocolSummary(),
      ...principals.map(p => fetchVolatility(Principal.fromText(p))),
    ]);

    const twapData = twapResult.status === 'fulfilled' ? twapResult.value : null;
    const twapEntries = twapData?.entries ?? [];
    const configs = configsResult.status === 'fulfilled' ? configsResult.value ?? [] : [];
    const totals = totalsResult.status === 'fulfilled' ? totalsResult.value ?? [] : [];
    const summary = summaryResult.status === 'fulfilled' ? summaryResult.value : null;

    // Build lookup maps
    const twapMap = new Map<string, { price: number; twap: number }>();
    for (const e of twapEntries) {
      const pid = e.collateral?.toText?.() ?? String(e.collateral);
      twapMap.set(pid, { price: e.latest_price, twap: e.twap_price });
    }

    const summaryPriceMap = new Map<string, number>();
    if (summary?.prices) {
      for (const p of summary.prices) {
        const pid = p.collateral?.toText?.() ?? String(p.collateral);
        summaryPriceMap.set(pid, p.twap_price);
      }
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
      if (r.status === 'fulfilled' && r.value) {
        volMap.set(principals[i], r.value.annualized_vol_pct);
      }
    }

    const built: MarketRow[] = [];
    for (const [principal, symbol] of Object.entries(COLLATERAL_SYMBOLS)) {
      const twap = twapMap.get(principal);
      const cfg = configMap.get(principal);
      const tot = totalsMap.get(principal);

      const price = twap?.price ?? summaryPriceMap.get(principal) ?? 0;
      const twapPrice = twap?.twap ?? price;

      const totalCollateral = tot?.total_collateral_e8s != null
        ? e8sToNumber(tot.total_collateral_e8s) : 0;
      const totalDebt = tot?.total_debt_e8s != null
        ? e8sToNumber(tot.total_debt_e8s) : 0;
      const vaultCount = tot?.vault_count != null ? Number(tot.vault_count) : 0;

      const debtCeilingRaw = cfg?.debt_ceiling ?? 0n;
      const isUnlimited = typeof debtCeilingRaw === 'bigint'
        ? debtCeilingRaw >= 18446744073709551615n
        : Number(debtCeilingRaw) >= Number.MAX_SAFE_INTEGER;

      built.push({
        principal,
        symbol,
        color: COLLATERAL_COLORS[symbol as keyof typeof COLLATERAL_COLORS] ?? '#888',
        price,
        twapPrice,
        volatility: volMap.get(principal) ?? null,
        vaultCount,
        tvlUsd: totalCollateral * price,
        totalDebt,
        debtCeiling: e8sToNumber(Number(debtCeilingRaw)),
        unlimited: isUnlimited,
      });
    }

    built.sort((a, b) => b.tvlUsd - a.tvlUsd);
    rows = built;
    loading = false;
  });

  function formatPrice(price: number): string {
    if (price >= 10000) return `$${price.toLocaleString(undefined, { maximumFractionDigits: 0 })}`;
    if (price >= 100) return `$${price.toFixed(1)}`;
    if (price >= 1) return `$${price.toFixed(2)}`;
    return `$${price.toFixed(6)}`;
  }
</script>

<svelte:head>
  <title>Markets | Rumi Explorer</title>
</svelte:head>

<div class="space-y-4">
  <div class="explorer-card">
    <div class="flex items-center justify-between mb-4">
      <h2 class="text-sm font-medium text-gray-300">Collateral Markets</h2>
      <span class="text-xs text-gray-500">{rows.length} assets</span>
    </div>

    {#if loading}
      <div class="flex items-center justify-center py-12">
        <div class="w-5 h-5 border-2 border-gray-600 border-t-teal-400 rounded-full animate-spin"></div>
      </div>
    {:else}
      <div class="overflow-x-auto">
        <table class="w-full text-sm">
          <thead>
            <tr class="border-b text-xs text-gray-500 uppercase tracking-wider" style="border-color: var(--rumi-border);">
              <th class="px-3 py-2.5 text-left">Asset</th>
              <th class="px-3 py-2.5 text-right">Price</th>
              <th class="px-3 py-2.5 text-right hidden sm:table-cell">TWAP</th>
              <th class="px-3 py-2.5 text-right hidden md:table-cell">Volatility</th>
              <th class="px-3 py-2.5 text-right hidden lg:table-cell">Vaults</th>
              <th class="px-3 py-2.5 text-right">TVL</th>
              <th class="px-3 py-2.5 text-right hidden sm:table-cell">Debt</th>
              <th class="px-3 py-2.5 text-right hidden md:table-cell">Ceiling</th>
            </tr>
          </thead>
          <tbody>
            {#each rows as row}
              <tr class="border-b hover:bg-white/[0.02] transition-colors" style="border-color: var(--rumi-border);">
                <td class="px-3 py-3">
                  <a href="/explorer/markets/{row.principal}" class="flex items-center gap-2 hover:text-teal-300 transition-colors">
                    <span class="w-2.5 h-2.5 rounded-full flex-shrink-0" style="background: {row.color};"></span>
                    <span class="font-medium text-gray-200">{row.symbol}</span>
                  </a>
                </td>
                <td class="px-3 py-3 text-right tabular-nums text-gray-200">
                  {formatPrice(row.price)}
                </td>
                <td class="px-3 py-3 text-right tabular-nums text-gray-400 hidden sm:table-cell">
                  {formatPrice(row.twapPrice)}
                </td>
                <td class="px-3 py-3 text-right tabular-nums hidden md:table-cell {row.volatility != null && row.volatility > 80 ? 'text-pink-400' : row.volatility != null && row.volatility > 40 ? 'text-violet-400' : 'text-gray-400'}">
                  {row.volatility != null ? `${row.volatility.toFixed(1)}%` : '--'}
                </td>
                <td class="px-3 py-3 text-right tabular-nums text-gray-400 hidden lg:table-cell">
                  {row.vaultCount}
                </td>
                <td class="px-3 py-3 text-right tabular-nums text-gray-200">
                  ${formatCompact(row.tvlUsd)}
                </td>
                <td class="px-3 py-3 text-right tabular-nums text-gray-400 hidden sm:table-cell">
                  {formatCompact(row.totalDebt)} icUSD
                </td>
                <td class="px-3 py-3 hidden md:table-cell">
                  <div class="flex justify-end">
                    <div class="w-20">
                      <CeilingBar used={row.totalDebt} total={row.debtCeiling} unlimited={row.unlimited} />
                    </div>
                  </div>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  </div>
</div>
