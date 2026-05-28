<script lang="ts">
  import { onMount } from 'svelte';
  import { fetchProtocolReserves, type ProtocolReserve } from '$services/explorer/explorerService';

  let reserves = $state<ProtocolReserve[]>([]);
  let loading = $state(true);

  onMount(async () => {
    try {
      reserves = await fetchProtocolReserves();
    } catch (err) {
      console.error('[ProtocolReservesCard] load failed:', err);
    } finally {
      loading = false;
    }
  });

  const totalUsd = $derived(reserves.reduce((s, r) => s + r.usd, 0));
</script>

<div class="explorer-card">
  <h3 class="text-sm font-medium text-gray-300 mb-1">Protocol reserves</h3>
  <p class="text-xs text-gray-500 mb-3">
    Stablecoins held by <code class="text-gray-600">rumi_protocol_backend</code> as a redemption
    buffer. Sourced from liquidation bot swap proceeds and ckUSDC / ckUSDT vault repayments;
    drawn down by <code class="text-gray-600">redeem_reserves</code> before redemptions cascade
    to user vaults.
  </p>
  {#if loading}
    <div class="flex items-center justify-center py-6">
      <div class="w-5 h-5 border-2 border-gray-600 border-t-teal-400 rounded-full animate-spin"></div>
    </div>
  {:else if reserves.length === 0}
    <p class="text-sm text-gray-500 py-2">No reserve balances.</p>
  {:else}
    <table class="w-full text-sm">
      <tbody>
        {#each reserves as r (r.ledger)}
          <tr class="border-b border-white/[0.03]">
            <td class="py-1.5 px-2 text-gray-200">{r.symbol}</td>
            <td class="py-1.5 px-2 text-right tabular-nums text-gray-300">
              {(Number(r.balanceE8s) / Math.pow(10, r.decimals)).toLocaleString(undefined, { maximumFractionDigits: 2 })}
            </td>
            <td class="py-1.5 px-2 text-right tabular-nums text-gray-100 font-medium">
              ${r.usd.toLocaleString(undefined, { maximumFractionDigits: 2 })}
            </td>
          </tr>
        {/each}
        <tr class="border-t border-white/10">
          <td class="py-1.5 px-2 text-xs text-gray-500 uppercase">Buffer</td>
          <td></td>
          <td class="py-1.5 px-2 text-right tabular-nums text-teal-300 font-semibold">
            ${totalUsd.toLocaleString(undefined, { maximumFractionDigits: 2 })}
          </td>
        </tr>
      </tbody>
    </table>
  {/if}
</div>
