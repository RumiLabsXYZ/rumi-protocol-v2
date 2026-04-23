<script lang="ts">
  import TokenBadge from './TokenBadge.svelte';
  import type { CollateralInfo } from '$lib/services/types';

  interface CollateralRow {
    config: CollateralInfo;
    vaultCount: number;
    totalCollateral: number;
    totalDebt: number;
  }

  interface Props {
    rows: CollateralRow[];
    loading?: boolean;
  }

  let { rows, loading = false }: Props = $props();

  const E8S = 100_000_000;

  function formatUsd(value: number): string {
    if (value >= 1_000_000) return `$${(value / 1_000_000).toFixed(2)}M`;
    if (value >= 1_000) return `$${(value / 1_000).toFixed(1)}K`;
    return `$${value.toFixed(2)}`;
  }

  function formatPrice(value: number): string {
    if (value >= 1000) return `$${value.toLocaleString(undefined, { maximumFractionDigits: 2 })}`;
    if (value >= 1) return `$${value.toFixed(2)}`;
    return `$${value.toFixed(4)}`;
  }

  function formatAmount(value: number, decimals: number): string {
    const human = value / Math.pow(10, decimals);
    if (human >= 1_000_000) return `${(human / 1_000_000).toFixed(2)}M`;
    if (human >= 1_000) return `${(human / 1_000).toFixed(1)}K`;
    if (human >= 1) return human.toFixed(2);
    return human.toFixed(4);
  }

  function debtCeilingPct(row: CollateralRow): number {
    if (!row.config.debtCeiling || row.config.debtCeiling === 0) return 0;
    return (row.totalDebt / row.config.debtCeiling) * 100;
  }

  function statusColor(status: string): string {
    switch (status) {
      case 'Active': return 'text-green-400 bg-green-400/10 border-green-400/30';
      case 'Paused': return 'text-yellow-400 bg-yellow-400/10 border-yellow-400/30';
      case 'Frozen': return 'text-red-400 bg-red-400/10 border-red-400/30';
      case 'Sunset': return 'text-orange-400 bg-orange-400/10 border-orange-400/30';
      default: return 'text-gray-400 bg-gray-400/10 border-gray-400/30';
    }
  }
</script>

<div class="bg-gray-800/50 border border-gray-700/50 rounded-xl overflow-hidden">
  <div class="px-5 py-4 border-b border-gray-700/50">
    <h3 class="text-sm font-semibold text-gray-200">Collateral Breakdown</h3>
  </div>

  <div class="overflow-x-auto">
    <table class="w-full text-sm">
      <thead>
        <tr class="border-b border-gray-700/50">
          <th class="px-4 py-3 text-xs font-medium text-gray-400 uppercase tracking-wider text-left">Token</th>
          <th class="px-4 py-3 text-xs font-medium text-gray-400 uppercase tracking-wider text-right">Price</th>
          <th class="px-4 py-3 text-xs font-medium text-gray-400 uppercase tracking-wider text-right">Collateral Locked</th>
          <th class="px-4 py-3 text-xs font-medium text-gray-400 uppercase tracking-wider text-right">Debt Minted</th>
          <th class="px-4 py-3 text-xs font-medium text-gray-400 uppercase tracking-wider text-right">Vaults</th>
          <th class="px-4 py-3 text-xs font-medium text-gray-400 uppercase tracking-wider text-right">Debt Ceiling</th>
          <th class="px-4 py-3 text-xs font-medium text-gray-400 uppercase tracking-wider text-right">Borrow Fee</th>
          <th class="px-4 py-3 text-xs font-medium text-gray-400 uppercase tracking-wider text-center">Status</th>
        </tr>
      </thead>
      <tbody>
        {#if loading}
          <tr>
            <td colspan="8" class="px-4 py-12 text-center text-gray-500">Loading...</td>
          </tr>
        {:else if rows.length === 0}
          <tr>
            <td colspan="8" class="px-4 py-12 text-center text-gray-500">No collateral types configured</td>
          </tr>
        {:else}
          {#each rows as row}
            {@const collateralHuman = row.totalCollateral / Math.pow(10, row.config.decimals)}
            {@const collateralUsd = collateralHuman * row.config.price}
            {@const debtHuman = row.totalDebt / E8S}
            {@const ceilPct = debtCeilingPct(row)}
            <tr class="border-b border-gray-700/30 hover:bg-gray-700/20 transition-colors">
              <td class="px-4 py-3">
                <a href="/explorer/e/token/{row.config.principal}" class="inline-block hover:opacity-80 transition-opacity">
                  <TokenBadge symbol={row.config.symbol} principalId={row.config.principal} size="md" linked={false} />
                </a>
              </td>
              <td class="px-4 py-3 text-right text-gray-200 tabular-nums">{formatPrice(row.config.price)}</td>
              <td class="px-4 py-3 text-right">
                <div class="text-gray-200 tabular-nums">{formatAmount(row.totalCollateral, row.config.decimals)}</div>
                <div class="text-xs text-gray-500">{formatUsd(collateralUsd)}</div>
              </td>
              <td class="px-4 py-3 text-right text-gray-200 tabular-nums">{formatUsd(debtHuman)}</td>
              <td class="px-4 py-3 text-right text-gray-300 tabular-nums">{row.vaultCount}</td>
              <td class="px-4 py-3 text-right">
                <div class="flex items-center justify-end gap-2">
                  <div class="w-16 h-1.5 bg-gray-700 rounded-full overflow-hidden">
                    <div
                      class="h-full rounded-full transition-all duration-300 {ceilPct > 80 ? 'bg-red-400' : ceilPct > 50 ? 'bg-yellow-400' : 'bg-green-400'}"
                      style="width: {Math.min(100, ceilPct)}%"
                    ></div>
                  </div>
                  <span class="text-xs text-gray-400 tabular-nums w-10 text-right">{ceilPct.toFixed(0)}%</span>
                </div>
              </td>
              <td class="px-4 py-3 text-right text-gray-300 tabular-nums">{(row.config.borrowingFee * 100).toFixed(1)}%</td>
              <td class="px-4 py-3 text-center">
                <span class="text-xs font-medium px-2 py-0.5 rounded-full border {statusColor(row.config.status)}">
                  {row.config.status}
                </span>
              </td>
            </tr>
          {/each}
        {/if}
      </tbody>
    </table>
  </div>
</div>
