<script lang="ts">
  import { onMount } from 'svelte';
  import { fetchCurrentSpDepositors, type CurrentSpDepositor } from '$services/explorer/explorerService';
  import { getTokenSymbol, getTokenDecimals } from '$utils/explorerHelpers';
  import EntityLink from './EntityLink.svelte';

  let depositors: CurrentSpDepositor[] = $state([]);
  let loading = $state(true);
  let error: string | null = $state(null);

  // Distinct token principals across all depositors → column headers
  const tokens = $derived.by(() => {
    const set = new Set<string>();
    for (const d of depositors) {
      for (const [token] of d.position.stablecoin_balances) {
        set.add(token.toText());
      }
    }
    return Array.from(set);
  });

  function balanceOf(d: CurrentSpDepositor, token: string): bigint {
    for (const [t, amt] of d.position.stablecoin_balances) {
      if (t.toText() === token) return amt;
    }
    return 0n;
  }

  function fmt(amt: bigint, token: string): string {
    if (amt === 0n) return '';
    const decimals = getTokenDecimals(token);
    const v = Number(amt) / Math.pow(10, decimals);
    return v.toLocaleString(undefined, { maximumFractionDigits: 2 });
  }

  onMount(async () => {
    try {
      depositors = await fetchCurrentSpDepositors();
    } catch (err: any) {
      console.error('[SpCurrentDepositorsCard] load failed:', err);
      error = err?.message ?? String(err);
    } finally {
      loading = false;
    }
  });
</script>

<div class="explorer-card">
  <h3 class="text-sm font-medium text-gray-300 mb-1">Current depositors</h3>
  <p class="text-xs text-gray-500 mb-3">
    Every principal currently holding a non-zero SP balance. Per-token columns; sorted by total USD value.
  </p>
  {#if loading}
    <div class="flex items-center justify-center py-6">
      <div class="w-5 h-5 border-2 border-gray-600 border-t-teal-400 rounded-full animate-spin"></div>
    </div>
  {:else if error}
    <p class="text-sm text-red-400 py-2">Failed to load: {error}</p>
  {:else if depositors.length === 0}
    <p class="text-sm text-gray-500 py-2">No active depositors.</p>
  {:else}
    <div class="overflow-x-auto">
      <table class="w-full text-sm">
        <thead>
          <tr class="border-b border-white/5">
            <th class="text-left py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">Depositor</th>
            {#each tokens as t (t)}
              <th class="text-right py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">{getTokenSymbol(t)}</th>
            {/each}
            <th class="text-right py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">Total USD</th>
          </tr>
        </thead>
        <tbody>
          {#each depositors as d (d.principal.toText())}
            <tr class="border-b border-white/[0.03] hover:bg-white/[0.02]">
              <td class="py-2 px-2 font-mono text-xs">
                <EntityLink type="address" value={d.principal.toText()} />
              </td>
              {#each tokens as t (t)}
                <td class="py-2 px-2 text-right tabular-nums text-gray-300">{fmt(balanceOf(d, t), t)}</td>
              {/each}
              <td class="py-2 px-2 text-right tabular-nums text-gray-100 font-medium">
                ${(Number(d.position.total_usd_value_e8s) / 1e8).toLocaleString(undefined, { maximumFractionDigits: 2 })}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</div>
