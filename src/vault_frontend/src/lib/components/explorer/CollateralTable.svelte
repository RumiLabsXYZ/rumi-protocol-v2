<script lang="ts">
  import CeilingBar from './CeilingBar.svelte';
  import { formatCompact, COLLATERAL_COLORS } from '$utils/explorerChartHelpers';

  export interface CollateralRow {
    principal: string;
    symbol: string;
    price: number;
    vaultCount: number;
    totalCollateralUsd: number;
    totalDebt: number;
    debtCeiling: number;
    unlimited: boolean;
    medianCrBps: number;
    volatility: number | null;
  }

  interface Props {
    rows: CollateralRow[];
    loading?: boolean;
  }
  let { rows, loading = false }: Props = $props();

  function formatPrice(price: number): string {
    if (price >= 10000) return `$${price.toLocaleString(undefined, { maximumFractionDigits: 0 })}`;
    if (price >= 100) return `$${price.toFixed(1)}`;
    if (price >= 1) return `$${price.toFixed(2)}`;
    return `$${price.toFixed(4)}`;
  }
</script>

<div class="explorer-card overflow-x-auto">
  <h3 class="text-sm font-medium text-gray-300 mb-4">Collateral Overview</h3>

  {#if loading}
    <div class="flex items-center justify-center py-8">
      <div class="w-5 h-5 border-2 border-gray-600 border-t-teal-400 rounded-full animate-spin"></div>
    </div>
  {:else if rows.length === 0}
    <p class="text-sm text-gray-500 py-4">No collateral data.</p>
  {:else}
    <table class="w-full text-sm">
      <thead>
        <tr class="border-b border-white/5">
          <th class="text-left py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">Asset</th>
          <th class="text-right py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">Price</th>
          <th class="text-right py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider hidden md:table-cell">Vol</th>
          <th class="text-right py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">Vaults</th>
          <th class="text-right py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider hidden sm:table-cell">Collateral</th>
          <th class="text-right py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">Debt</th>
          <th class="py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider w-28 hidden lg:table-cell">Ceiling</th>
          <th class="text-right py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider hidden md:table-cell">Avg CR</th>
        </tr>
      </thead>
      <tbody>
        {#each rows as row}
          <tr class="border-b border-white/[0.03] hover:bg-white/[0.02] transition-colors">
            <td class="py-2.5 px-2">
              <a href="/explorer/markets/{row.principal}" class="flex items-center gap-2 hover:text-teal-300 transition-colors">
                <span class="w-2 h-2 rounded-full flex-shrink-0" style="background: {COLLATERAL_COLORS[row.symbol] ?? '#666'}"></span>
                <span class="font-medium text-gray-200">{row.symbol}</span>
              </a>
            </td>
            <td class="py-2.5 px-2 text-right tabular-nums text-gray-300">{formatPrice(row.price)}</td>
            <td class="py-2.5 px-2 text-right tabular-nums text-gray-500 hidden md:table-cell">
              {row.volatility != null ? `${row.volatility.toFixed(1)}%` : '--'}
            </td>
            <td class="py-2.5 px-2 text-right tabular-nums text-gray-400">{row.vaultCount}</td>
            <td class="py-2.5 px-2 text-right tabular-nums text-gray-300 hidden sm:table-cell">${formatCompact(row.totalCollateralUsd)}</td>
            <td class="py-2.5 px-2 text-right tabular-nums text-gray-300">{formatCompact(row.totalDebt)}</td>
            <td class="py-2.5 px-2 hidden lg:table-cell">
              <CeilingBar used={row.totalDebt} total={row.debtCeiling} unlimited={row.unlimited} />
            </td>
            <td class="py-2.5 px-2 text-right tabular-nums hidden md:table-cell
              {row.medianCrBps < 15000 ? 'text-violet-400' : row.medianCrBps < 20000 ? 'text-gray-300' : 'text-teal-400'}">
              {row.medianCrBps > 0 ? `${(row.medianCrBps / 100).toFixed(0)}%` : '--'}
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  {/if}
</div>
