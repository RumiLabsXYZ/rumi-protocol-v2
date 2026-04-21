<script lang="ts">
  import { onMount } from 'svelte';
  import { Principal } from '@dfinity/principal';
  import ProtocolVitals from '$components/explorer/ProtocolVitals.svelte';
  import TvlChart from '$components/explorer/TvlChart.svelte';
  import CollateralTable from '$components/explorer/CollateralTable.svelte';
  import PoolHealthStrip from '$components/explorer/PoolHealthStrip.svelte';
  import MixedEventsTable from '$components/explorer/MixedEventsTable.svelte';
  import {
    fetchProtocolSummary, fetchTvlSeries, fetchVaultSeries,
    fetchTwap, fetchPegStatus, fetchApys, fetchVolatility
  } from '$services/explorer/analyticsService';
  import {
    fetchEvents, fetchCollateralConfigs, fetchCollateralTotals, fetchAllVaults,
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
  import { calculateTheoreticalApy } from '$services/threePoolService';
  import { threePoolService, POOL_TOKENS } from '$services/threePoolService';
  import { ProtocolService } from '$services/protocol';
  import { stabilityPoolService } from '$services/stabilityPoolService';
  import { e8sToNumber, COLLATERAL_SYMBOLS } from '$utils/explorerChartHelpers';
  import { COLLATERAL_DISPLAY_ORDER } from '$stores/collateralStore';
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

  let recentEvents: DisplayEvent[] = $state([]);
  let eventsLoading = $state(true);

  // Vault maps for EventRow
  let vaultCollateralMap: Map<number, string> = $state(new Map());
  let vaultOwnerMap: Map<number, string> = $state(new Map());

  onMount(async () => {
    // Fetch all sections in parallel
    const [
      summaryResult, tvlResult, vaultSeriesResult, twapResult,
      configsResult, pegResult, apyResult, eventsResult, vaultsResult,
      collateralTotalsResult
    ] = await Promise.allSettled([
      fetchProtocolSummary(),
      fetchTvlSeries(365),
      fetchVaultSeries(365),
      fetchTwap(),
      fetchCollateralConfigs(),
      fetchPegStatus(),
      fetchApys(),
      fetchEvents(0n, 10n),
      fetchAllVaults(),
      fetchCollateralTotals(),
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
    // Try analytics APYs first, then compute directly as fallback
    if (apyResult.status === 'fulfilled' && apyResult.value) {
      const analyticsLp = apyResult.value.lp_apy_pct?.[0];
      const analyticsSp = apyResult.value.sp_apy_pct?.[0];
      // Only use analytics values if they're actual non-zero numbers
      if (typeof analyticsLp === 'number' && analyticsLp > 0) lpApy = analyticsLp;
      if (typeof analyticsSp === 'number' && analyticsSp > 0) spApy = analyticsSp;
    }
    // If analytics didn't have meaningful APYs, compute from live data
    // LP APY: same approach as 3USD page
    // SP APY: same approach as StabilityPoolTab (per-collateral eligible deposits)
    if (!lpApy || !spApy) {
      try {
        const [protocolStatus, poolStatus, spStatus] = await Promise.allSettled([
          ProtocolService.getProtocolStatus(),
          threePoolService.getPoolStatus(),
          stabilityPoolService.getPoolStatus(),
        ]);

        const ps = protocolStatus.status === 'fulfilled' ? protocolStatus.value : null;
        const pool = poolStatus.status === 'fulfilled' ? poolStatus.value : null;
        const sp = spStatus.status === 'fulfilled' ? spStatus.value : null;

        if (!lpApy && ps && pool) {
          let poolTvlE8s = 0;
          for (let i = 0; i < pool.balances.length; i++) {
            const token = POOL_TOKENS[i];
            if (token) {
              poolTvlE8s += token.decimals === 8
                ? Number(pool.balances[i])
                : Number(pool.balances[i]) * 100;
            }
          }
          const threePoolBps = ps.interestSplit?.find((e: any) => e.destination === 'three_pool')?.bps ?? 5000;
          const computed = calculateTheoreticalApy(threePoolBps, ps.perCollateralInterest, poolTvlE8s / 1e8);
          if (computed != null) lpApy = computed * 100;
        }

        // SP APY: replicate StabilityPoolTab logic exactly
        // Uses per-collateral eligible deposits as denominator, not total deposits
        if (!spApy && ps && sp) {
          const poolShare = (ps.interestSplit?.find((e: any) => e.destination === 'stability_pool')?.bps ?? 0) / 10000;
          const perC = ps.perCollateralInterest;
          if (poolShare > 0 && perC && perC.length > 0) {
            const eligibleMap = new Map<string, number>(
              (sp.eligible_icusd_per_collateral ?? []).map(([p, v]: [any, bigint]) => [
                typeof p === 'object' && typeof p.toText === 'function' ? p.toText() : String(p),
                Number(v) / 1e8
              ])
            );
            let totalApr = 0;
            for (const info of perC) {
              const eligible = eligibleMap.get(info.collateralType) ?? 0;
              if (eligible === 0 || info.totalDebtE8s === 0 || info.weightedInterestRate === 0) continue;
              totalApr += (info.weightedInterestRate * poolShare * info.totalDebtE8s) / eligible;
            }
            if (totalApr > 0) {
              const apy = Math.pow(1 + totalApr / 365, 365) - 1;
              spApy = apy * 100;
            }
          }
        }
      } catch (e) {
        console.error('[dashboard] APY fallback error:', e);
      }
    }
    poolsLoading = false;

    // Recent events: merge from all sources, sort by timestamp, take top 10
    const SAMPLE_SIZE = 10n;
    try {
      const backendEvents: DisplayEvent[] = [];
      if (eventsResult.status === 'fulfilled' && eventsResult.value) {
        for (const [idx, evt] of eventsResult.value.events ?? []) {
          backendEvents.push({ globalIndex: idx, event: evt, source: 'backend', timestamp: extractEventTimestamp(evt) });
        }
      }

      // Fetch a small sample from each non-backend source in parallel
      const [
        swapCount, ammSwapCount, ammLiqCount, threePoolLiqCount,
        ammAdminCount, threePoolAdminCount, spCount
      ] = await Promise.all([
        fetchSwapEventCount().catch(() => 0n),
        fetchAmmSwapEventCount().catch(() => 0n),
        fetchAmmLiquidityEventCount().catch(() => 0n),
        fetch3PoolLiquidityEventCount().catch(() => 0n),
        fetchAmmAdminEventCount().catch(() => 0n),
        fetch3PoolAdminEventCount().catch(() => 0n),
        fetchStabilityPoolEventCount().catch(() => 0n),
      ]);

      // Fetch recent events from each source (start from end for newest)
      const fetchRecent = (count: bigint, fetcher: (s: bigint, l: bigint) => Promise<any[]>) => {
        if (Number(count) === 0) return Promise.resolve([]);
        const start = count > SAMPLE_SIZE ? count - SAMPLE_SIZE : 0n;
        const length = count > SAMPLE_SIZE ? SAMPLE_SIZE : count;
        return fetcher(start, length).catch(() => []);
      };

      const [swaps, ammSwaps, ammLiq, threePoolLiq, ammAdmin, threePoolAdmin, spEvts] = await Promise.all([
        fetchRecent(swapCount, fetchSwapEvents),
        fetchRecent(ammSwapCount, fetchAmmSwapEvents),
        fetchRecent(ammLiqCount, fetchAmmLiquidityEvents),
        fetchRecent(threePoolLiqCount, fetch3PoolLiquidityEvents),
        fetchRecent(ammAdminCount, fetchAmmAdminEvents),
        fetchRecent(threePoolAdminCount, fetch3PoolAdminEvents),
        fetchRecent(spCount, fetchStabilityPoolEvents),
      ]);

      const filteredSp = spEvts.filter((e: any) => {
        const et = e.event_type ?? {};
        return !('InterestReceived' in et);
      });

      const all: DisplayEvent[] = [...backendEvents];
      const addSource = (events: any[], source: DisplayEvent['source']) => {
        for (const e of events) {
          all.push({ globalIndex: BigInt(e.id ?? 0), event: e, source, timestamp: extractEventTimestamp(e) });
        }
      };

      addSource(swaps, '3pool_swap');
      addSource(ammSwaps, 'amm_swap');
      addSource(ammLiq, 'amm_liquidity');
      addSource(threePoolLiq, '3pool_liquidity');
      addSource(ammAdmin, 'amm_admin');
      addSource(threePoolAdmin, '3pool_admin');
      addSource(filteredSp, 'stability_pool');

      all.sort((a, b) => b.timestamp - a.timestamp);
      recentEvents = all.slice(0, 10);
    } catch (err) {
      console.error('[dashboard] recent events merge error:', err);
      // Fall back to backend-only events
      if (eventsResult.status === 'fulfilled' && eventsResult.value) {
        recentEvents = (eventsResult.value.events ?? []).map(([idx, evt]: [bigint, any]) => ({
          globalIndex: idx, event: evt, source: 'backend' as const, timestamp: extractEventTimestamp(evt)
        }));
      }
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
    await buildCollateralRows(vaultSeriesResult, twapResult, configsResult, collateralTotalsResult);
  });

  async function buildCollateralRows(
    vaultSeriesResult: PromiseSettledResult<any>,
    twapResult: PromiseSettledResult<any>,
    configsResult: PromiseSettledResult<any>,
    collateralTotalsResult: PromiseSettledResult<any>
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

      // Get collateral stats from vault snapshot (analytics daily aggregation)
      const collaterals = latestVaultSnapshot?.collaterals ?? [];

      // Backend collateral totals as fallback (live data from backend canister)
      const backendTotals = collateralTotalsResult.status === 'fulfilled'
        ? collateralTotalsResult.value ?? []
        : [];
      const backendTotalsMap = new Map<string, any>();
      for (const t of backendTotals) {
        const pid = t.collateral_type?.toText?.() ?? String(t.collateral_type ?? '');
        if (pid) backendTotalsMap.set(pid, t);
      }

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
        // Prefer analytics vault snapshot stats, fall back to backend totals
        const stats = collaterals.find((c: any) => {
          const p = c.collateral_type?.toText?.() ?? String(c.collateral_type);
          return p === principal;
        });
        const backendStats = backendTotalsMap.get(principal);

        const cfg = configMap.get(principal);
        const price = priceMap.get(principal)
          ?? summaryPriceMap.get(principal)
          ?? (stats?.price_usd_e8s ? e8sToNumber(stats.price_usd_e8s) : null)
          ?? (backendStats?.price ? Number(backendStats.price) : 0);

        // Use bigint comparison for debt ceiling to avoid Number precision loss
        const debtCeilingRaw = cfg?.debt_ceiling ?? 0n;
        const isUnlimited = typeof debtCeilingRaw === 'bigint'
          ? debtCeilingRaw >= 18446744073709551615n
          : Number(debtCeilingRaw) >= Number.MAX_SAFE_INTEGER;

        // Vault count and debt: prefer analytics, fall back to backend
        const vaultCount = stats
          ? Number(stats.vault_count)
          : (backendStats?.vault_count != null ? Number(backendStats.vault_count) : 0);
        // Use backend decimals for proper normalization (ckETH=18, ckXAUT=6, others=8)
        const decimals = backendStats?.decimals != null ? Number(backendStats.decimals) : 8;
        const totalCollateralRaw = stats
          ? Number(stats.total_collateral_e8s)
          : (backendStats?.total_collateral != null ? Number(backendStats.total_collateral) : 0);
        const totalCollateralUnits = totalCollateralRaw / Math.pow(10, decimals);
        const totalDebtE8s = stats
          ? e8sToNumber(stats.total_debt_e8s)
          : (backendStats?.total_debt != null ? e8sToNumber(backendStats.total_debt) : 0);
        const medianCrBps = stats ? Number(stats.median_cr_bps) : 0;

        rows.push({
          principal,
          symbol,
          price,
          vaultCount,
          totalCollateralUsd: totalCollateralUnits * price,
          totalDebt: totalDebtE8s,
          debtCeiling: e8sToNumber(debtCeilingRaw),
          unlimited: isUnlimited,
          medianCrBps,
          volatility: volMap.get(principal) ?? null,
        });
      }

      // Sort by canonical display order (same as borrow page / protocol stats)
      rows.sort((a, b) => {
        const ai = COLLATERAL_DISPLAY_ORDER.indexOf(a.symbol);
        const bi = COLLATERAL_DISPLAY_ORDER.indexOf(b.symbol);
        return (ai === -1 ? 999 : ai) - (bi === -1 ? 999 : bi);
      });
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
      <MixedEventsTable events={recentEvents} {vaultCollateralMap} {vaultOwnerMap} headerCellClass="px-4 py-2" />
    {/if}
  </div>

  <!-- Link to docs for protocol parameters -->
  <div class="text-center py-2">
    <a href="/docs/parameters" class="text-xs text-gray-500 hover:text-gray-400 transition-colors">
      Protocol parameters are documented in the Docs &rarr;
    </a>
  </div>
</div>
