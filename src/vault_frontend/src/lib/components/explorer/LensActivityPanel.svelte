<script lang="ts">
  import { onMount } from 'svelte';
  import MixedEventsTable from './MixedEventsTable.svelte';
  import {
    fetchEvents, fetchAllVaults,
    fetchThreePoolSwapEventsCombined,
    fetch3PoolLiquidityEventsCombined,
    fetchAmmSwapEvents, fetchAmmSwapEventCount,
    fetchAmmLiquidityEvents, fetchAmmLiquidityEventCount,
    fetchAmmAdminEvents, fetchAmmAdminEventCount,
    fetch3PoolAdminEvents, fetch3PoolAdminEventCount,
    fetchStabilityPoolEvents, fetchStabilityPoolEventCount,
    type BackendEventFilters,
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

  // Backend events serialize as snake_case JSON (via #[serde(rename = ...)]).
  // The previous filters used CamelCase variant names that don't actually exist
  // on the wire — that's why scope='admin' was empty even though setter events
  // were being recorded. Sets below match the actual variant keys.

  // User-visible vault operations (NOT setter / admin / system events).
  const VAULT_OP_KEYS = new Set([
    'open_vault', 'close_vault', 'withdraw_and_close_vault', 'vault_withdrawn_and_closed',
    'borrow_from_vault', 'repay_to_vault', 'add_margin_to_vault',
    'collateral_withdrawn', 'partial_collateral_withdrawn', 'margin_transfer',
    'liquidate_vault', 'partial_liquidate_vault', 'redistribute_vault',
    'redemption_on_vaults', 'redemption_transfered',
    'dust_forgiven',
  ]);

  // Admin / setter / config-change events. Init + Upgrade live here too —
  // upgrades are admin actions even though they don't carry an explicit caller.
  // AccrueInterest is intentionally NOT in this set: it's auto-triggered system
  // bookkeeping that fires very frequently and floods the activity feed.
  const ADMIN_EVENT_KEYS = new Set([
    'init', 'upgrade',
    'set_ckstable_repay_fee', 'set_min_icusd_amount', 'set_global_icusd_mint_cap',
    'set_stable_token_enabled', 'set_stable_ledger_principal',
    'set_treasury_principal', 'set_stability_pool_principal', 'set_liquidation_bot_principal',
    'set_bot_budget', 'set_bot_allowed_collateral_types',
    'set_liquidation_bonus', 'set_borrowing_fee',
    'set_redemption_fee_floor', 'set_redemption_fee_ceiling',
    'set_max_partial_liquidation_ratio', 'set_recovery_target_cr', 'set_recovery_cr_multiplier',
    'set_liquidation_protocol_share',
    'add_collateral_type', 'update_collateral_status', 'update_collateral_config',
    'set_reserve_redemptions_enabled', 'set_icpswap_routing_enabled',
    'set_reserve_redemption_fee', 'reserve_redemption',
    'admin_mint', 'admin_vault_correction', 'admin_debt_correction', 'admin_sweep_to_treasury',
    'set_recovery_parameters', 'set_rate_curve_markers', 'set_recovery_rate_curve',
    'set_healthy_cr', 'set_collateral_borrowing_fee', 'set_interest_rate', 'set_interest_pool_share',
    'set_rmr_floor', 'set_rmr_ceiling', 'set_rmr_floor_cr', 'set_rmr_ceiling_cr',
    'set_borrowing_fee_curve', 'set_interest_split', 'set_three_pool_canister',
    // Per-collateral setters that previously fell through and never made it
    // into the admin-scoped feed.
    'set_collateral_borrow_threshold', 'set_collateral_display_color',
    'set_collateral_ledger_fee', 'set_collateral_liquidation_bonus',
    'set_collateral_liquidation_ratio', 'set_collateral_min_deposit',
    'set_collateral_min_vault_debt',
    'set_collateral_redemption_fee_ceiling', 'set_collateral_redemption_fee_floor',
    // Wave-10 LIQ-008 mass-liquidation breaker tunables.
    'set_breaker_window_ns', 'set_breaker_window_debt_ceiling_e8s',
    // Wave-8e LIQ-005 deficit-routing tunables.
    'set_deficit_readonly_threshold_e8s', 'set_deficit_repayment_fraction',
  ]);

  function backendEventKey(evt: any): string {
    const et = evt?.event_type ?? evt;
    return Object.keys(et ?? {})[0] ?? '';
  }

  function isBackendVaultEvent(evt: any): boolean {
    return VAULT_OP_KEYS.has(backendEventKey(evt));
  }

  function isBackendRedemption(evt: any): boolean {
    return backendEventKey(evt) === 'redemption_on_vaults';
  }

  function isBackendRevenue(evt: any): boolean {
    const key = backendEventKey(evt);
    return key === 'redemption_on_vaults'
      || key === 'liquidate_vault'
      || key === 'partial_liquidate_vault';
  }

  function isBackendAdmin(evt: any): boolean {
    return ADMIN_EVENT_KEYS.has(backendEventKey(evt));
  }

  function isAccrueInterest(evt: any): boolean {
    return backendEventKey(evt) === 'accrue_interest';
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

  // Sparse-scope server-side filtering: redemptions, revenue (liquidations +
  // redemptions), and admin all match a small fraction of the backend log. A
  // tail-fetch of the most recent 72 events rarely contains any of them, so
  // those scopes pass an explicit `types` filter that lets the backend pick
  // matching events from the entire log. The full-log search is paginated by
  // length=50, which is enough for the lens panel (default limit=12) plus
  // headroom after merging with other sources.
  //
  // The remaining scopes (`all`, `vault_ops`) cover the bulk of event kinds,
  // so the unfiltered tail is the better fit there.
  function backendQueryForScope(s: LensScope): { length: bigint; filters: BackendEventFilters | undefined } {
    switch (s) {
      case 'redemptions':
        return { length: 50n, filters: { types: ['Redemption'] } };
      case 'revenue':
        // Mirrors `isBackendRevenue` (liquidate_vault, partial_liquidate_vault,
        // redemption_on_vaults). `Redemption` also yields RedemptionTransfered,
        // which the client-side filter then drops.
        return { length: 50n, filters: { types: ['Redemption', 'Liquidation', 'PartialLiquidation'] } };
      case 'admin':
        // Mirrors `ADMIN_EVENT_KEYS`. Most setter / config variants collapse
        // into the backend's catch-all `Admin` filter, but `admin_mint`,
        // `admin_sweep_to_treasury`, and `reserve_redemption` have their own
        // dedicated `EventTypeFilter` values (see `Event::type_filter()` in
        // `rumi_protocol_backend/src/event.rs`) and would otherwise be
        // excluded by a single `Admin` filter.
        return { length: 50n, filters: { types: ['Admin', 'AdminMint', 'AdminSweepToTreasury', 'ReserveRedemption'] } };
      default:
        return { length: BigInt(limit * 6), filters: undefined };
    }
  }

  async function loadEvents() {
    loading = true;
    const sampleSize = BigInt(limit * 4);  // overfetch then trim after merging

    try {
      // Fetch backend events + vault maps. Always include unless scope is purely DEX/SP/admin.
      const needsBackend = scope === 'all' || scope === 'vault_ops' || scope === 'redemptions' || scope === 'revenue' || scope === 'admin';
      const { length: backendLength, filters: backendFilters } = backendQueryForScope(scope);
      const backendPromise = needsBackend
        ? fetchEvents(0n, backendLength, backendFilters).catch(() => ({ total: 0n, events: [] }))
        : Promise.resolve({ total: 0n, events: [] });

      const vaultsPromise = fetchAllVaults().catch(() => []);

      const needsDexs = scope === 'all' || scope === 'revenue' || scope === 'dexs' || scope === 'admin';
      const needsSp = scope === 'all' || scope === 'stability_pool';

      // 3pool swap & liquidity reads pull combined v1+v2 streams below;
      // there's no count endpoint that covers both logs.
      const ammSwapCountP = needsDexs ? fetchAmmSwapEventCount().catch(() => 0n) : Promise.resolve(0n);
      const ammLiqCountP = needsDexs ? fetchAmmLiquidityEventCount().catch(() => 0n) : Promise.resolve(0n);
      const ammAdminCountP = scope === 'all' || scope === 'admin'
        ? fetchAmmAdminEventCount().catch(() => 0n) : Promise.resolve(0n);
      const threeAdminCountP = scope === 'all' || scope === 'admin'
        ? fetch3PoolAdminEventCount().catch(() => 0n) : Promise.resolve(0n);
      const spCountP = needsSp ? fetchStabilityPoolEventCount().catch(() => 0n) : Promise.resolve(0n);

      const [backend, vaults, ammSwapCount, ammLiqCount, ammAdminCount, threeAdminCount, spCount] = await Promise.all([
        backendPromise, vaultsPromise, ammSwapCountP, ammLiqCountP, ammAdminCountP, threeAdminCountP, spCountP,
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

      // Pull recent event batches from each source. 3pool swap and liquidity
      // come from combined v1+v2 readers so frozen pre-migration entries
      // appear alongside live writes — otherwise the lens panel silently
      // drops 49 swap + 18 liquidity historical events from the stream.
      const [swaps, ammSwaps, ammLiq, ammAdmin, threeLiq, threeAdmin, sp] = await Promise.all([
        needsDexs ? fetchThreePoolSwapEventsCombined(sampleSize).catch(() => []) : Promise.resolve([]),
        fetchRecent(ammSwapCount, sampleSize, fetchAmmSwapEvents),
        fetchRecent(ammLiqCount, sampleSize, fetchAmmLiquidityEvents),
        fetchRecent(ammAdminCount, sampleSize, fetchAmmAdminEvents),
        needsDexs ? fetch3PoolLiquidityEventsCombined().catch(() => []) : Promise.resolve([]),
        fetchRecent(threeAdminCount, sampleSize, fetch3PoolAdminEvents),
        fetchRecent(spCount, sampleSize, fetchStabilityPoolEvents),
      ]);

      // Filter backend events by scope. AccrueInterest is system bookkeeping
      // that fires every interest tick — never useful in any user-facing scope.
      const backendRows = wrapBackend(backend.events ?? []).filter((de) => {
        if (isAccrueInterest(de.event)) return false;
        switch (scope) {
          case 'all':
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
