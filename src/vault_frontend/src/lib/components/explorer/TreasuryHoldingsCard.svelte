<script lang="ts">
  import { onMount } from 'svelte';
  import { ProtocolService } from '$services/protocol';
  import { fetchTreasuryHoldings, type TreasuryHolding } from '$services/explorer/explorerService';

  let holdings = $state<TreasuryHolding[]>([]);
  let loading = $state(true);

  onMount(async () => {
    try {
      const status = await ProtocolService.getProtocolStatus();
      const icpPrice: number = (status?.lastIcpRate ?? 0) as number;
      holdings = await fetchTreasuryHoldings(icpPrice);
    } catch (err) {
      console.error('[TreasuryHoldingsCard] load failed:', err);
    } finally {
      loading = false;
    }
  });

  const totalUsd = $derived(holdings.reduce((s, h) => s + h.usd, 0));
</script>

<div class="explorer-card">
  <h3 class="text-sm font-medium text-gray-300 mb-1">Treasury holdings</h3>
  <p class="text-xs text-gray-500 mb-3">
    Live token balances of the rumi_treasury canister
    (<code class="text-gray-600">tlg74-oiaaa-aaaap-qrd6a-cai</code>).
  </p>
  {#if loading}
    <div class="flex items-center justify-center py-6">
      <div class="w-5 h-5 border-2 border-gray-600 border-t-teal-400 rounded-full animate-spin"></div>
    </div>
  {:else if holdings.length === 0}
    <p class="text-sm text-gray-500 py-2">No tracked balances.</p>
  {:else}
    <table class="w-full text-sm">
      <tbody>
        {#each holdings as h (h.ledger)}
          <tr class="border-b border-white/[0.03]">
            <td class="py-1.5 px-2 text-gray-200">{h.symbol}</td>
            <td class="py-1.5 px-2 text-right tabular-nums text-gray-300">
              {(Number(h.balanceE8s) / Math.pow(10, h.decimals)).toLocaleString(undefined, { maximumFractionDigits: 2 })}
            </td>
            <td class="py-1.5 px-2 text-right tabular-nums text-gray-100 font-medium">
              ${h.usd.toLocaleString(undefined, { maximumFractionDigits: 2 })}
            </td>
          </tr>
        {/each}
        <tr class="border-t border-white/10">
          <td class="py-1.5 px-2 text-xs text-gray-500 uppercase">Total</td>
          <td></td>
          <td class="py-1.5 px-2 text-right tabular-nums text-teal-300 font-semibold">
            ${totalUsd.toLocaleString(undefined, { maximumFractionDigits: 2 })}
          </td>
        </tr>
      </tbody>
    </table>
  {/if}
</div>
