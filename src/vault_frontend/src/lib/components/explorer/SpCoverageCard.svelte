<script lang="ts">
  import { onMount } from 'svelte';
  import { fetchCurrentSpDepositors, fetchCollateralConfigs, type CurrentSpDepositor } from '$services/explorer/explorerService';
  import { getTokenSymbol } from '$utils/explorerHelpers';

  let depositors: CurrentSpDepositor[] = $state([]);
  let collaterals: string[] = $state([]); // principal text
  let loading = $state(true);

  function isOptedIn(d: CurrentSpDepositor, collType: string): boolean {
    return !d.position.opted_out_collateral.some((p) => p.toText() === collType);
  }

  function totalStableUsd(d: CurrentSpDepositor): number {
    return Number(d.position.total_usd_value_e8s) / 1e8;
  }

  const rows = $derived.by(() => {
    return collaterals.map((c) => {
      const optedIn = depositors.filter((d) => isOptedIn(d, c));
      const pct = depositors.length > 0 ? (optedIn.length / depositors.length) * 100 : 0;
      const usd = optedIn.reduce((s, d) => s + totalStableUsd(d), 0);
      return { collateral: c, pct, usd, count: optedIn.length, total: depositors.length };
    });
  });

  onMount(async () => {
    try {
      const [d, configs] = await Promise.all([
        fetchCurrentSpDepositors(),
        fetchCollateralConfigs(),
      ]);
      depositors = d;
      // CollateralConfig uses ledger_canister_id as the collateral identifier.
      collaterals = configs
        .map((c: any) => c.ledger_canister_id?.toText?.() ?? '')
        .filter((id: string) => id.length > 0);
    } catch (err) {
      console.error('[SpCoverageCard] load failed:', err);
    } finally {
      loading = false;
    }
  });
</script>

<div class="explorer-card">
  <h3 class="text-sm font-medium text-gray-300 mb-1">Per-collateral opt-in coverage</h3>
  <p class="text-xs text-gray-500 mb-3">
    For each supported collateral, the share of SP depositors who haven't opted out and the total stable backing they provide.
  </p>
  {#if loading}
    <div class="flex items-center justify-center py-6">
      <div class="w-5 h-5 border-2 border-gray-600 border-t-teal-400 rounded-full animate-spin"></div>
    </div>
  {:else if rows.length === 0}
    <p class="text-sm text-gray-500 py-2">No collaterals configured.</p>
  {:else}
    <div class="space-y-2">
      {#each rows as r (r.collateral)}
        <div class="flex items-baseline justify-between text-sm gap-4">
          <span class="text-gray-200 font-medium w-20 shrink-0">{getTokenSymbol(r.collateral)}</span>
          <span class="tabular-nums text-gray-400 flex-1">{r.count}/{r.total} opted in ({r.pct.toFixed(0)}%)</span>
          <span class="tabular-nums text-gray-100 font-medium">
            ${r.usd.toLocaleString(undefined, { maximumFractionDigits: 0 })}
          </span>
        </div>
      {/each}
    </div>
  {/if}
</div>
