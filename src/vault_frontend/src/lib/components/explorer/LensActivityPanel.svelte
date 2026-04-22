<script lang="ts">
  import { onMount } from 'svelte';
  import MixedEventsTable from './MixedEventsTable.svelte';
  import {
    fetchEvents, fetchAllVaults,
    fetchSwapEvents, fetchSwapEventCount,
    fetchAmmSwapEvents, fetchAmmSwapEventCount,
    fetchAmmLiquidityEvents, fetchAmmLiquidityEventCount,
    fetchAmmAdminEvents, fetchAmmAdminEventCount,
    fetch3PoolLiquidityEvents, fetch3PoolLiquidityEventCount,
    fetch3PoolAdminEvents, fetch3PoolAdminEventCount,
    fetchStabilityPoolEvents, fetchStabilityPoolEventCount,
  } from '$services/explorer/explorerService';
  import { extractEventTimestamp } from '$utils/displayEvent';
  import type { DisplayEvent } from '$utils/displayEvent';

  export type LensScope =
    | 'all'                // Overview: everything
    | 'vault_ops'          // Collateral: backend vault events only, no admin
    | 'stability_pool'     // Stability Pool: SP events only
    | 'redemptions'        // Redemptions: backend Redeem events only
    | 'revenue'            // Revenue: fee-bearing (swaps + redemptions + liquidations)
    | 'dexs'               // DEXs: 3pool swaps/liquidity, AMM swaps/liquidity (no admin)
    | 'admin';             // Admin: admin events from all sources

  interface Props {
    scope: LensScope;
    title?: string;
    limit?: number;
    viewAllHref?: string;
  }
  let {
    scope,
    title = 'Activity',
    limit = 12,
    viewAllHref = '/explorer/activity',
  }: Props = $props();

  let events: DisplayEvent[] = $state([]);
  let vaultCollateralMap: Map<number, string> = $state(new Map());
  let vaultOwnerMap: Map<number, string> = $state(new Map());
  let loading = $state(true);

  // Backend admin-ish event variant keys. Kept as a reference filter;
  // add/remove as the backend event schema evolves.
  const ADMIN_EVENT_KEYS = new Set([
    'Init',
    'Upgrade',
    'ModeChanged',
    'ProtocolModeChanged',
    'ParameterChanged',
    'CollateralAdded',
    'CollateralRemoved',
    'CollateralConfigChanged',
    'InterestRateChanged',
    'DebtCeilingChanged',
    'LiquidationThresholdChanged',
    'TreasuryConfigChanged',
    'StabilityPoolConfigured',
    'FeeFloorChanged',
    'FeeCeilingChanged',
    'InterestSplitChanged',
    'RedemptionConfigChanged',
  ]);

  function backendEventKey(evt: any): string {
    const et = evt?.event_type ?? evt;
    return Object.keys(et ?? {})[0] ?? '';
  }

  function isBackendVaultEvent(evt: any): boolean {
    const key = backendEventKey(evt);
    return key === 'VaultCreated' || key === 'VaultUpdated' || key === 'VaultClosed'
      || key === 'VaultAdjusted' || key === 'CollateralDeposited' || key === 'CollateralWithdrawn'
      || key === 'DebtIncreased' || key === 'DebtRepaid' || key === 'Liquidation' || key === 'Redemption';
  }

  function isBackendRedemption(evt: any): boolean {
    return backendEventKey(evt) === 'Redemption';
  }

  function isBackendRevenue(evt: any): boolean {
    const key = backendEventKey(evt);
    return key === 'Redemption' || key === 'Liquidation';
  }

  function isBackendAdmin(evt: any): boolean {
    return ADMIN_EVENT_KEYS.has(backendEventKey(evt));
  }

  function wrapBackend(events: [bigint, any][]): DisplayEvent[] {
    return events.map(([idx, evt]) => ({
      globalIndex: idx,
      event: evt,
      source: 'backend' as const,
      timestamp: extractEventTimestamp(evt),
    }));
  }

  function wrapSource(events: any[], source: DisplayEvent['source']): DisplayEvent[] {
    return events.map((e) => ({
      globalIndex: BigInt(e.id ?? 0),
      event: e,
      source,
      timestamp: extractEventTimestamp(e),
    }));
  }

  async function fetchRecent<T>(count: bigint, sample: bigint, fetcher: (s: bigint, l: bigint) => Promise<T[]>): Promise<T[]> {
    if (count === 0n) return [];
    const start = count > sample ? count - sample : 0n;
    const length = count > sample ? sample : count;
    try {
      return await fetcher(start, length);
    } catch {
      return [];
    }
  }

  async function loadEvents() {
    loading = true;
    const sampleSize = BigInt(limit * 4);  // overfetch then trim after merging

    try {
      // Fetch backend events + vault maps. Always include unless scope is purely DEX/SP/admin.
      const needsBackend = scope === 'all' || scope === 'vault_ops' || scope === 'redemptions' || scope === 'revenue' || scope === 'admin';
      const backendPromise = needsBackend
        ? fetchEvents(0n, BigInt(limit * 6)).catch(() => ({ total: 0n, events: [] }))
        : Promise.resolve({ total: 0n, events: [] });

      const vaultsPromise = fetchAllVaults().catch(() => []);

      const needsDexs = scope === 'all' || scope === 'revenue' || scope === 'dexs' || scope === 'admin';
      const needsSp = scope === 'all' || scope === 'stability_pool';

      const swapCountP = needsDexs ? fetchSwapEventCount().catch(() => 0n) : Promise.resolve(0n);
      const ammSwapCountP = needsDexs ? fetchAmmSwapEventCount().catch(() => 0n) : Promise.resolve(0n);
      const ammLiqCountP = needsDexs ? fetchAmmLiquidityEventCount().catch(() => 0n) : Promise.resolve(0n);
      const ammAdminCountP = scope === 'all' || scope === 'admin'
        ? fetchAmmAdminEventCount().catch(() => 0n) : Promise.resolve(0n);
      const threeLiqCountP = needsDexs ? fetch3PoolLiquidityEventCount().catch(() => 0n) : Promise.resolve(0n);
      const threeAdminCountP = scope === 'all' || scope === 'admin'
        ? fetch3PoolAdminEventCount().catch(() => 0n) : Promise.resolve(0n);
      const spCountP = needsSp ? fetchStabilityPoolEventCount().catch(() => 0n) : Promise.resolve(0n);

      const [backend, vaults, swapCount, ammSwapCount, ammLiqCount, ammAdminCount, threeLiqCount, threeAdminCount, spCount] = await Promise.all([
        backendPromise, vaultsPromise, swapCountP, ammSwapCountP, ammLiqCountP, ammAdminCountP, threeLiqCountP, threeAdminCountP, spCountP,
      ]);

      // Populate vault maps
      const collMap = new Map<number, string>();
      const ownerMap = new Map<number, string>();
      for (const v of vaults) {
        const id = Number(v.vault_id);
        collMap.set(id, v.collateral_type?.toText?.() ?? String(v.collateral_type ?? ''));
        ownerMap.set(id, v.owner?.toText?.() ?? '');
      }
      vaultCollateralMap = collMap;
      vaultOwnerMap = ownerMap;

      // Pull recent event batches from each source
      const [swaps, ammSwaps, ammLiq, ammAdmin, threeLiq, threeAdmin, sp] = await Promise.all([
        fetchRecent(swapCount, sampleSize, fetchSwapEvents),
        fetchRecent(ammSwapCount, sampleSize, fetchAmmSwapEvents),
        fetchRecent(ammLiqCount, sampleSize, fetchAmmLiquidityEvents),
        fetchRecent(ammAdminCount, sampleSize, fetchAmmAdminEvents),
        threeLiqCount === 0n ? Promise.resolve([]) : fetch3PoolLiquidityEvents(sampleSize < threeLiqCount ? sampleSize : threeLiqCount, 0n).catch(() => []),
        fetchRecent(threeAdminCount, sampleSize, fetch3PoolAdminEvents),
        fetchRecent(spCount, sampleSize, fetchStabilityPoolEvents),
      ]);

      // Filter backend events by scope
      const backendRows = wrapBackend(backend.events ?? []).filter((de) => {
        switch (scope) {
          case 'all':
            // exclude admin by default
            return !isBackendAdmin(de.event);
          case 'vault_ops':
            return isBackendVaultEvent(de.event) && !isBackendAdmin(de.event);
          case 'redemptions':
            return isBackendRedemption(de.event);
          case 'revenue':
            return isBackendRevenue(de.event);
          case 'admin':
            return isBackendAdmin(de.event);
          default:
            return false;
        }
      });

      // Filter SP events: hide noisy InterestReceived entries except when that's the whole show
      const spFiltered = sp.filter((e: any) => {
        const et = e.event_type ?? {};
        return !('InterestReceived' in et);
      });

      let merged: DisplayEvent[] = [...backendRows];

      if (scope === 'all') {
        merged = merged.concat(
          wrapSource(swaps, '3pool_swap'),
          wrapSource(ammSwaps, 'amm_swap'),
          wrapSource(ammLiq, 'amm_liquidity'),
          wrapSource(threeLiq, '3pool_liquidity'),
          wrapSource(spFiltered, 'stability_pool'),
        );
      } else if (scope === 'stability_pool') {
        merged = wrapSource(spFiltered, 'stability_pool');
      } else if (scope === 'dexs') {
        merged = merged.concat(
          wrapSource(swaps, '3pool_swap'),
          wrapSource(ammSwaps, 'amm_swap'),
          wrapSource(ammLiq, 'amm_liquidity'),
          wrapSource(threeLiq, '3pool_liquidity'),
        );
      } else if (scope === 'revenue') {
        // swap events are the revenue on the DEX side
        merged = merged.concat(
          wrapSource(swaps, '3pool_swap'),
          wrapSource(ammSwaps, 'amm_swap'),
        );
      } else if (scope === 'admin') {
        merged = merged.concat(
          wrapSource(ammAdmin, 'amm_admin'),
          wrapSource(threeAdmin, '3pool_admin'),
        );
      }

      merged.sort((a, b) => b.timestamp - a.timestamp);
      events = merged.slice(0, limit);
    } catch (err) {
      console.error('[LensActivityPanel] load error:', err);
      events = [];
    } finally {
      loading = false;
    }
  }

  onMount(() => { loadEvents(); });
</script>

<div class="explorer-card">
  <div class="flex items-center justify-between mb-3">
    <h3 class="text-sm font-medium text-gray-300">{title}</h3>
    <a href={viewAllHref} class="text-xs text-teal-400 hover:text-teal-300">View all &rarr;</a>
  </div>
  {#if loading}
    <div class="flex items-center justify-center py-6">
      <div class="w-5 h-5 border-2 border-gray-600 border-t-teal-400 rounded-full animate-spin"></div>
    </div>
  {:else if events.length === 0}
    <p class="text-sm text-gray-500 py-4">No recent events in this scope.</p>
  {:else}
    <MixedEventsTable {events} {vaultCollateralMap} {vaultOwnerMap} headerCellClass="px-4 py-2" />
  {/if}
</div>
