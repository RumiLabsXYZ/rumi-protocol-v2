<script lang="ts">
  import { onMount } from 'svelte';
  import { Principal } from '@dfinity/principal';
  import ProtocolVitals from '$components/explorer/ProtocolVitals.svelte';
  import TvlChart from '$components/explorer/TvlChart.svelte';
  import CollateralTable from '$components/explorer/CollateralTable.svelte';
  import PoolHealthStrip from '$components/explorer/PoolHealthStrip.svelte';
  import EventRow from '$components/explorer/EventRow.svelte';
  import {
    fetchProtocolSummary, fetchTvlSeries, fetchVaultSeries,
    fetchTwap, fetchPegStatus, fetchApys, fetchVolatility
  } from '$services/explorer/analyticsService';
  import { fetchEvents, fetchCollateralConfigs, fetchAllVaults } from '$services/explorer/explorerService';
  import { e8sToNumber, COLLATERAL_SYMBOLS } from '$utils/explorerChartHelpers';
  import type { ProtocolSummary, DailyTvlRow, PegStatus } from '$declarations/rumi_analytics/rumi_analytics.did';
  import type { CollateralRow } from '$components/explorer/CollateralTable.svelte';

  // Section state
  let summary: ProtocolSummary | null = $state(null);
  let summaryLoading = $state(true);

  let tvlData: DailyTvlRow[] = $state([]);
  let tvlLoading = $state(true);

  let collateralRows: CollateralRow[] = $state([]);
  let collateralLoading = $state(true);

  let pegStatus: PegStatus | null = $state(null);
  let lpApy: number | null = $state(null);
  let spApy: number | null = $state(null);
  let poolsLoading = $state(true);

  let recentEvents: [bigint, any][] = $state([]);
  let eventsLoading = $state(true);

  // Vault maps for EventRow
  let vaultCollateralMap: Map<number, string> = $state(new Map());
  let vaultOwnerMap: Map<number, string> = $state(new Map());

  onMount(async () => {
    // Fetch all sections in parallel
    const [
      summaryResult, tvlResult, vaultSeriesResult, twapResult,
      configsResult, pegResult, apyResult, eventsResult, vaultsResult
    ] = await Promise.allSettled([
      fetchProtocolSummary(),
      fetchTvlSeries(365),
      fetchVaultSeries(1),
      fetchTwap(),
      fetchCollateralConfigs(),
      fetchPegStatus(),
      fetchApys(),
      fetchEvents(0n, 10n),
      fetchAllVaults(),
    ]);

    // Protocol summary
    if (summaryResult.status === 'fulfilled' && summaryResult.value) {
      summary = summaryResult.value;
    }
    summaryLoading = false;

    // TVL chart
    if (tvlResult.status === 'fulfilled' && tvlResult.value) {
      tvlData = tvlResult.value;
    }
    tvlLoading = false;

    // Pools
    if (pegResult.status === 'fulfilled') {
      pegStatus = pegResult.value ?? null;
    }
    if (apyResult.status === 'fulfilled' && apyResult.value) {
      lpApy = apyResult.value.lp_apy_pct?.[0] ?? null;
      spApy = apyResult.value.sp_apy_pct?.[0] ?? null;
    }
    poolsLoading = false;

    // Recent events
    if (eventsResult.status === 'fulfilled' && eventsResult.value) {
      recentEvents = eventsResult.value.events ?? [];
    }
    eventsLoading = false;

    // Vault maps for EventRow
    if (vaultsResult.status === 'fulfilled' && vaultsResult.value) {
      const collMap = new Map<number, string>();
      const ownerMap = new Map<number, string>();
      for (const v of vaultsResult.value) {
        const id = Number(v.vault_id);
        collMap.set(id, v.collateral_type?.toText?.() ?? String(v.collateral_type ?? ''));
        ownerMap.set(id, v.owner?.toText?.() ?? '');
      }
      vaultCollateralMap = collMap;
      vaultOwnerMap = ownerMap;
    }

    // Build collateral table rows
    await buildCollateralRows(vaultSeriesResult, twapResult, configsResult);
  });

  async function buildCollateralRows(
    vaultSeriesResult: PromiseSettledResult<any>,
    twapResult: PromiseSettledResult<any>,
    configsResult: PromiseSettledResult<any>
  ) {
    try {
      // Get latest vault snapshot (the series returns rows, we want the last one)
      const vaultRows = vaultSeriesResult.status === 'fulfilled' ? vaultSeriesResult.value : [];
      const latestVaultSnapshot = vaultRows.length > 0 ? vaultRows[vaultRows.length - 1] : null;

      const twapData = twapResult.status === 'fulfilled' ? twapResult.value : null;
      const twapEntries = twapData?.entries ?? [];

      const configs = configsResult.status === 'fulfilled' ? configsResult.value ?? [] : [];

      // Build price map from TWAP
      const priceMap = new Map<string, number>();
      for (const entry of twapEntries) {
        const principal = entry.collateral?.toText?.() ?? String(entry.collateral);
        priceMap.set(principal, entry.twap_price);
      }

      // Build config map
      const configMap = new Map<string, any>();
      for (const cfg of configs) {
        const principal = cfg.ledger_canister_id?.toText?.() ?? String(cfg.ledger_canister_id);
        configMap.set(principal, cfg);
      }

      // Get collateral stats from vault snapshot
      const collaterals = latestVaultSnapshot?.collaterals ?? [];

      // Fetch volatility for each collateral in parallel
      const volResults = await Promise.allSettled(
        Object.keys(COLLATERAL_SYMBOLS).map(async (principal) => {
          const vol = await fetchVolatility(Principal.fromText(principal));
          return { principal, vol };
        })
      );
      const volMap = new Map<string, number>();
      for (const r of volResults) {
        if (r.status === 'fulfilled' && r.value.vol) {
          volMap.set(r.value.principal, r.value.vol.annualized_vol_pct);
        }
      }

      // Also use summary prices as fallback
      const summaryPriceMap = new Map<string, number>();
      if (summary?.prices) {
        for (const p of summary.prices) {
          const principal = p.collateral?.toText?.() ?? String(p.collateral);
          summaryPriceMap.set(principal, p.twap_price);
        }
      }

      const rows: CollateralRow[] = [];
      for (const [principal, symbol] of Object.entries(COLLATERAL_SYMBOLS)) {
        const stats = collaterals.find((c: any) => {
          const p = c.collateral_type?.toText?.() ?? String(c.collateral_type);
          return p === principal;
        });
        const cfg = configMap.get(principal);
        const price = priceMap.get(principal)
          ?? summaryPriceMap.get(principal)
          ?? (stats?.price_usd_e8s ? e8sToNumber(stats.price_usd_e8s) : 0);
        const debtCeiling = cfg ? Number(cfg.debt_ceiling) : 0;
        const isUnlimited = debtCeiling >= Number(18446744073709551615n);

        rows.push({
          principal,
          symbol,
          price,
          vaultCount: stats ? Number(stats.vault_count) : 0,
          totalCollateralUsd: stats ? e8sToNumber(stats.total_collateral_e8s) * price : 0,
          totalDebt: stats ? e8sToNumber(stats.total_debt_e8s) : 0,
          debtCeiling: e8sToNumber(debtCeiling),
          unlimited: isUnlimited,
          medianCrBps: stats ? Number(stats.median_cr_bps) : 0,
          volatility: volMap.get(principal) ?? null,
        });
      }

      // Sort by TVL descending
      rows.sort((a, b) => b.totalCollateralUsd - a.totalCollateralUsd);
      collateralRows = rows;
    } catch (err) {
      console.error('[dashboard] buildCollateralRows error:', err);
    } finally {
      collateralLoading = false;
    }
  }
</script>

<svelte:head>
  <title>Dashboard | Rumi Explorer</title>
</svelte:head>

<div class="space-y-4">
  <!-- Protocol Vitals -->
  <ProtocolVitals {summary} loading={summaryLoading} />

  <!-- TVL Chart -->
  <TvlChart data={tvlData} loading={tvlLoading} />

  <!-- Collateral Overview -->
  <CollateralTable rows={collateralRows} loading={collateralLoading} />

  <!-- Pool Health -->
  <PoolHealthStrip {pegStatus} {lpApy} {spApy} loading={poolsLoading} />

  <!-- Recent Activity -->
  <div class="explorer-card">
    <div class="flex items-center justify-between mb-3">
      <h3 class="text-sm font-medium text-gray-300">Recent Activity</h3>
      <a href="/explorer/activity" class="text-xs text-teal-400 hover:text-teal-300">View all &rarr;</a>
    </div>
    {#if eventsLoading}
      <div class="flex items-center justify-center py-6">
        <div class="w-5 h-5 border-2 border-gray-600 border-t-teal-400 rounded-full animate-spin"></div>
      </div>
    {:else if recentEvents.length === 0}
      <p class="text-sm text-gray-500 py-4">No recent events.</p>
    {:else}
      <div class="overflow-x-auto">
        <table class="w-full">
          <tbody>
            {#each recentEvents as [index, event]}
              <EventRow {event} index={Number(index)} {vaultCollateralMap} {vaultOwnerMap} />
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  </div>

  <!-- Link to docs for protocol parameters -->
  <div class="text-center py-2">
    <a href="/docs/parameters" class="text-xs text-gray-500 hover:text-gray-400 transition-colors">
      Protocol parameters are documented in the Docs &rarr;
    </a>
  </div>
</div>
