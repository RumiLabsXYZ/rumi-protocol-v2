<script lang="ts">
  import { page } from '$app/stores';
  import { onMount } from 'svelte';
  import EntityShell from '$components/explorer/entity/EntityShell.svelte';
  import EntityLink from '$components/explorer/EntityLink.svelte';
  import CopyButton from '$components/explorer/CopyButton.svelte';
  import MiniAreaChart from '$components/explorer/MiniAreaChart.svelte';
  import MixedEventRow from '$components/explorer/MixedEventRow.svelte';
  import StatusBadge from '$components/explorer/StatusBadge.svelte';
  import { shortenPrincipal, getTokenSymbol } from '$utils/explorerHelpers';
  import {
    fetchThreePoolStatus,
    fetchThreePoolStatsWindow,
    fetchThreePoolHealth,
    fetchThreePoolTopLps,
    fetchThreePoolTopSwappers,
    fetchThreePoolSwapEventsV2,
    fetch3PoolLiquidityEvents,
    fetch3PoolLiquidityEventCount,
    fetchSwapEventCount,
    fetchThreePoolVolumeSeries,
    fetchThreePoolBalanceSeries,
    fetchThreePoolFeeSeries,
    fetchThreePoolVirtualPriceSeries,
    fetchAmmPools,
    fetchAmmVolumeSeries,
    fetchAmmBalanceSeries,
    fetchAmmFeeSeries,
    fetchAmmPoolStats,
    fetchAmmTopSwappers,
    fetchAmmTopLps,
    fetchAmmSwapEventsByTimeRange,
    fetchAmmLiquidityEvents,
    fetchAmmLiquidityEventCount,
    fetchCollateralPrices,
  } from '$services/explorer/explorerService';
  import { fetchPoolRoutes } from '$services/explorer/analyticsService';
  import type { PoolRoute } from '$declarations/rumi_analytics/rumi_analytics.did';
  import { AMM_TOKENS } from '$services/ammService';
  import type { DisplayEvent } from '$utils/displayEvent';
  import { CANISTER_IDS } from '$lib/config';
  import { formatCompact } from '$utils/explorerChartHelpers';

  const poolIdParam = $derived($page.params.id);

  // Pool source detection. 3pool is the literal string "3pool" or the 3pool
  // canister id; anything else is looked up as an AMM pool id (e.g.,
  // "fohh4-...cai_ryjl3-...cai").
  const isThreePool = $derived(poolIdParam === '3pool' || poolIdParam === CANISTER_IDS.THREEPOOL);
  const isAmm = $derived(!isThreePool);

  // ── Shared state ─────────────────────────────────────────────────────────
  let loading = $state(true);
  let error = $state<string | null>(null);
  let poolRoutes = $state<PoolRoute[]>([]);
  let priceMap = $state<Map<string, number>>(new Map());

  // ── 3pool state ──────────────────────────────────────────────────────────
  let status = $state<any>(null);
  let stats = $state<any>(null);
  let health = $state<any>(null);
  let topLps = $state<Array<[any, bigint, number]>>([]);
  let topSwappers = $state<Array<[any, bigint, bigint]>>([]);
  let swapEvents = $state<any[]>([]);
  let liqEvents = $state<any[]>([]);
  let volSeries = $state<any[]>([]);
  let balSeries = $state<any[]>([]);
  let feeSeries = $state<any[]>([]);
  let vpSeries = $state<any[]>([]);

  // ── AMM state ────────────────────────────────────────────────────────────
  let ammPool = $state<any>(null);
  let ammStats = $state<any>(null);
  let ammTopLps = $state<Array<[any, bigint, number]>>([]);
  let ammTopSwappers = $state<Array<[any, bigint, bigint]>>([]);
  let ammSwapEvents = $state<any[]>([]);
  let ammLiqEvents = $state<any[]>([]);
  let ammVolSeries = $state<any[]>([]);
  let ammBalSeries = $state<any[]>([]);
  let ammFeeSeries = $state<any[]>([]);

  // ── 3pool derived ────────────────────────────────────────────────────────
  const tokens = $derived(status?.tokens ?? []);
  const balances = $derived((status?.balances ?? []) as bigint[]);
  const totalSupply = $derived(status ? BigInt(status.lp_total_supply ?? 0) : 0n);
  const virtualPrice = $derived(status ? BigInt(status.virtual_price ?? 0) : 0n);
  const currentA = $derived(status ? Number(status.current_a ?? 0) : 0);
  const swapFeeBps = $derived(status ? Number(status.swap_fee_bps ?? 0) : 0);
  const adminFeeBps = $derived(status ? Number(status.admin_fee_bps ?? 0) : 0);
  const imbalance = $derived(health ? Number(health.current_imbalance ?? 0) : 0);

  // ── AMM derived ──────────────────────────────────────────────────────────
  function tokenMetaFromPrincipal(p: any): { symbol: string; decimals: number; ledgerId: string; color: string } {
    const id = p?.toText?.() ?? String(p);
    const match = AMM_TOKENS.find((t) => t.ledgerId === id);
    if (match) {
      return { symbol: match.symbol, decimals: match.decimals, ledgerId: id, color: match.color };
    }
    return { symbol: shortenPrincipal(id), decimals: 8, ledgerId: id, color: '#94a3b8' };
  }

  const ammTokenA = $derived(ammPool ? tokenMetaFromPrincipal(ammPool.token_a) : null);
  const ammTokenB = $derived(ammPool ? tokenMetaFromPrincipal(ammPool.token_b) : null);
  const ammPoolName = $derived(
    ammTokenA && ammTokenB ? `${ammTokenA.symbol} / ${ammTokenB.symbol}` : 'AMM Pool',
  );
  const ammReserveA = $derived(ammPool ? BigInt(ammPool.reserve_a ?? 0n) : 0n);
  const ammReserveB = $derived(ammPool ? BigInt(ammPool.reserve_b ?? 0n) : 0n);
  const ammLpSupply = $derived(ammPool ? BigInt(ammPool.total_lp_shares ?? 0n) : 0n);
  const ammFeeBps = $derived(ammPool ? Number(ammPool.fee_bps ?? 0) : 0);

  // Instantaneous spot price — a per b, derived from reserves.
  const ammSpotPriceAPerB = $derived.by(() => {
    if (!ammTokenA || !ammTokenB || ammReserveB === 0n) return 0;
    const aNorm = Number(ammReserveA) / 10 ** ammTokenA.decimals;
    const bNorm = Number(ammReserveB) / 10 ** ammTokenB.decimals;
    return bNorm === 0 ? 0 : aNorm / bNorm;
  });

  // ── TVL ──────────────────────────────────────────────────────────────────
  // 3pool TVL: sum of stable balances normalized to USD ($1 each).
  const threePoolTvlUsd = $derived.by(() => {
    if (!status?.tokens?.length || !status?.balances) return 0;
    let total = 0;
    for (let i = 0; i < status.tokens.length; i++) {
      const tok = status.tokens[i];
      const bal = status.balances[i] ?? 0n;
      const dec = Number(tok.decimals ?? 8);
      total += Number(bal) / 10 ** dec; // stables ≈ $1
    }
    return total;
  });

  // AMM TVL: needs oracle prices for both legs. If one leg is 3USD we price it
  // by the 3pool virtual_price; ICP via the protocol's last_price; everything
  // else $1 if a stablecoin, else null (unknown — display "--").
  function priceForToken(ledgerId: string): number | null {
    if (ledgerId === CANISTER_IDS.THREEPOOL) {
      // 3USD LP token priced via virtual price (each unit = vp dollars)
      return Number(virtualPrice) / 1e18 || null;
    }
    if (ledgerId === CANISTER_IDS.ICUSD_LEDGER) return 1;
    if (ledgerId === CANISTER_IDS.CKUSDT_LEDGER) return 1;
    if (ledgerId === CANISTER_IDS.CKUSDC_LEDGER) return 1;
    const p = priceMap.get(ledgerId);
    if (typeof p === 'number' && p > 0) return p;
    return null;
  }

  const ammTvlUsd = $derived.by(() => {
    if (!ammPool || !ammTokenA || !ammTokenB) return null;
    const priceA = priceForToken(ammTokenA.ledgerId);
    const priceB = priceForToken(ammTokenB.ledgerId);
    if (priceA == null || priceB == null) return null;
    const aNorm = Number(ammReserveA) / 10 ** ammTokenA.decimals;
    const bNorm = Number(ammReserveB) / 10 ** ammTokenB.decimals;
    return aNorm * priceA + bNorm * priceB;
  });

  // ── Shared formatters ────────────────────────────────────────────────────
  function formatReserve(raw: bigint, decimals: number): string {
    const divisor = 10n ** BigInt(decimals);
    const whole = raw / divisor;
    return Number(whole).toLocaleString();
  }

  function formatLp(raw: bigint, decimals: number = 8): string {
    const divisor = 10n ** BigInt(decimals);
    const whole = raw / divisor;
    return Number(whole).toLocaleString();
  }

  function formatVirtualPrice(raw: bigint): string {
    return (Number(raw) / 1e18).toFixed(6);
  }

  function formatBps(bps: number): string {
    return `${(bps / 100).toFixed(2)}%`;
  }

  function formatImbalance(val: number): string {
    return `${(val / 1e16).toFixed(2)}%`;
  }

  // ── 3pool series points ──────────────────────────────────────────────────
  const volPoints = $derived(
    volSeries.map((p: any) => ({
      t: Number(p.timestamp),
      v: (p.volume_per_token as bigint[]).reduce((a, b) => a + Number(b) / 1e18, 0),
    })),
  );

  const vpPoints = $derived(
    vpSeries.map((p: any) => ({ t: Number(p.timestamp), v: Number(p.virtual_price) / 1e18 })),
  );

  const feePoints = $derived(
    feeSeries.map((p: any) => ({ t: Number(p.timestamp), v: Number(p.avg_fee_bps) / 100 })),
  );

  const balancePointsByToken = $derived.by(() => {
    if (!balSeries.length || !tokens.length) return [] as Array<{ symbol: string; points: { t: number; v: number }[] }>;
    return tokens.map((tok: any, i: number) => ({
      symbol: tok.symbol,
      points: balSeries.map((p: any) => ({
        t: Number(p.timestamp),
        v: Number((p.balances as bigint[])[i] ?? 0n) / 10 ** Number(tok.decimals),
      })),
    }));
  });

  // ── AMM series points ────────────────────────────────────────────────────
  // Volume series sums both tokens in native units. For a stable/volatile pair
  // this is apples-and-oranges in absolute terms but tracks the shape of
  // activity over time, which is the useful signal. We report the stable-side
  // volume separately where it makes sense.
  const ammVolPoints = $derived.by(() => {
    if (!ammVolSeries.length || !ammTokenA || !ammTokenB) return [];
    return ammVolSeries.map((p: any) => ({
      t: Number(p.ts_ns) / 1_000_000, // ns → ms
      v:
        Number(p.volume_a_e8s) / 10 ** ammTokenA.decimals +
        Number(p.volume_b_e8s) / 10 ** ammTokenB.decimals,
    }));
  });

  const ammBalancePointsA = $derived.by(() => {
    if (!ammBalSeries.length || !ammTokenA) return [];
    return ammBalSeries.map((p: any) => ({
      t: Number(p.ts_ns) / 1_000_000,
      v: Number(p.reserve_a_e8s) / 10 ** ammTokenA.decimals,
    }));
  });

  const ammBalancePointsB = $derived.by(() => {
    if (!ammBalSeries.length || !ammTokenB) return [];
    return ammBalSeries.map((p: any) => ({
      t: Number(p.ts_ns) / 1_000_000,
      v: Number(p.reserve_b_e8s) / 10 ** ammTokenB.decimals,
    }));
  });

  const ammFeePoints = $derived.by(() => {
    if (!ammFeeSeries.length || !ammTokenA || !ammTokenB) return [];
    return ammFeeSeries.map((p: any) => ({
      t: Number(p.ts_ns) / 1_000_000,
      v:
        Number(p.fees_a_e8s) / 10 ** ammTokenA.decimals +
        Number(p.fees_b_e8s) / 10 ** ammTokenB.decimals,
    }));
  });

  // ── Merged event feeds ───────────────────────────────────────────────────
  const mergedEvents = $derived.by((): DisplayEvent[] => {
    const out: DisplayEvent[] = [];
    if (isThreePool) {
      for (const evt of swapEvents) {
        out.push({
          event: evt,
          globalIndex: BigInt(evt.id ?? 0),
          source: '3pool_swap',
          timestamp: Number(evt.timestamp ?? 0),
        });
      }
      for (const evt of liqEvents) {
        out.push({
          event: evt,
          globalIndex: BigInt(evt.id ?? 0),
          source: '3pool_liquidity',
          timestamp: Number(evt.timestamp ?? 0),
        });
      }
    } else {
      for (const evt of ammSwapEvents) {
        out.push({
          event: evt,
          globalIndex: BigInt(evt.id ?? 0),
          source: 'amm_swap',
          timestamp: Number(evt.timestamp ?? 0),
        });
      }
      for (const evt of ammLiqEvents) {
        out.push({
          event: evt,
          globalIndex: BigInt(evt.id ?? 0),
          source: 'amm_liquidity',
          timestamp: Number(evt.timestamp ?? 0),
        });
      }
    }
    out.sort((a, b) => b.timestamp - a.timestamp);
    return out.slice(0, 40);
  });

  /** Activity page deep-link for a route: filter by all tokens in the route
   * with a 7d time window matching the routes fetch. */
  function routeActivityHref(tokens: string[]): string {
    const params = new URLSearchParams();
    params.set('type', 'swap');
    if (tokens.length > 0) params.set('token', tokens.join(','));
    params.set('time', '7d');
    return `/explorer/activity?${params.toString()}`;
  }

  // ── Data load ────────────────────────────────────────────────────────────
  async function loadPool() {
    loading = true;
    error = null;
    // Pool-routes fetch runs on both branches; 7d window matches top swappers.
    const ROUTES_WINDOW_NS = 7n * 86_400n * 1_000_000_000n;
    const routesPromise = fetchPoolRoutes(poolIdParam, ROUTES_WINDOW_NS, 10)
      .then((r) => r.routes)
      .catch(() => [] as PoolRoute[]);
    // Pull oracle prices once so the AMM branch can value the volatile leg.
    fetchCollateralPrices().then((m) => { priceMap = m; }).catch(() => {});

    if (isThreePool) {
      try {
        const swapCount = await fetchSwapEventCount().catch(() => 0n);
        const liqCount = await fetch3PoolLiquidityEventCount().catch(() => 0n);

        const [s, st, h, lps, swappers, sev, lev, vol, bal, fees, vp, routes] = await Promise.all([
          fetchThreePoolStatus(),
          fetchThreePoolStatsWindow('Last7d'),
          fetchThreePoolHealth(),
          fetchThreePoolTopLps(10n),
          fetchThreePoolTopSwappers('Last7d', 10n),
          swapCount > 0n
            ? fetchThreePoolSwapEventsV2(swapCount > 50n ? swapCount - 50n : 0n, swapCount > 50n ? 50n : swapCount)
            : Promise.resolve([]),
          liqCount > 0n ? fetch3PoolLiquidityEvents(liqCount > 30n ? 30n : liqCount, 0n) : Promise.resolve([]),
          fetchThreePoolVolumeSeries('Last7d', 3600n),
          fetchThreePoolBalanceSeries('Last7d', 3600n),
          fetchThreePoolFeeSeries('Last7d', 3600n),
          fetchThreePoolVirtualPriceSeries('Last7d', 3600n),
          routesPromise,
        ]);
        status = s;
        stats = st;
        health = h;
        topLps = lps as any;
        topSwappers = swappers as any;
        swapEvents = sev;
        liqEvents = lev;
        volSeries = vol;
        balSeries = bal;
        feeSeries = fees;
        vpSeries = vp;
        poolRoutes = routes;
      } catch (err) {
        console.error('[pool page] 3pool load failed:', err);
        error = 'Failed to load 3pool data. The canister may be briefly unavailable.';
      } finally {
        loading = false;
      }
      return;
    }

    // AMM pool — look up by id in the pools list.
    try {
      const pools = await fetchAmmPools();
      const pool = pools.find((p: any) => p.pool_id === poolIdParam);
      if (!pool) {
        error = `Pool ${poolIdParam} does not exist.`;
        return;
      }
      ammPool = pool;

      // Pull 3pool status so we can value any 3USD leg via virtual_price.
      // Cheap query, runs in parallel with the rest.
      fetchThreePoolStatus().then((s) => { status = s; }).catch(() => {});

      // Seven-day window of swap events for the activity feed.
      const now = BigInt(Date.now()) * 1_000_000n;
      const sevenDaysNs = 7n * 24n * 3_600n * 1_000_000_000n;
      const liqCount = await fetchAmmLiquidityEventCount().catch(() => 0n);

      const [statsResp, topLpsResp, topSwappersResp, swapsResp, liqsAll, volResp, balResp, feeResp, routesResp] =
        await Promise.all([
          fetchAmmPoolStats(poolIdParam, 'Week'),
          fetchAmmTopLps(poolIdParam, 10),
          fetchAmmTopSwappers(poolIdParam, 'Week', 10),
          fetchAmmSwapEventsByTimeRange(poolIdParam, now - sevenDaysNs, now, 100n),
          // Liquidity events aren't per-pool yet in the old endpoint; filter client-side.
          liqCount > 0n
            ? fetchAmmLiquidityEvents(liqCount > 200n ? liqCount - 200n : 0n, liqCount > 200n ? 200n : liqCount)
            : Promise.resolve([]),
          fetchAmmVolumeSeries(poolIdParam, 'Week', 60),
          fetchAmmBalanceSeries(poolIdParam, 'Week', 60),
          fetchAmmFeeSeries(poolIdParam, 'Week', 60),
          routesPromise,
        ]);

      ammStats = statsResp;
      ammTopLps = topLpsResp as any;
      ammTopSwappers = topSwappersResp as any;
      ammSwapEvents = swapsResp;
      ammLiqEvents = (liqsAll as any[]).filter((e: any) => e.pool_id === poolIdParam);
      ammVolSeries = volResp;
      ammBalSeries = balResp;
      poolRoutes = routesResp;
      ammFeeSeries = feeResp;
    } catch (err) {
      console.error('[pool page] AMM load failed:', err);
      error = 'Failed to load pool data. The canister may be briefly unavailable.';
    } finally {
      loading = false;
    }
  }

  onMount(loadPool);
</script>

<svelte:head>
  <title>{isThreePool ? '3pool' : ammPoolName || poolIdParam} Pool | Rumi Explorer</title>
</svelte:head>

<EntityShell
  title={isThreePool ? '3pool (3USD)' : ammPoolName}
  subtitle={isThreePool
    ? 'Curve-style stableswap · icUSD / ckUSDC / ckUSDT'
    : ammPool
      ? `Constant-product AMM · ${formatBps(ammFeeBps)} fee`
      : undefined}
  loading={loading}
  {error}
  onRetry={loadPool}
>
  {#snippet identity()}
    {#if isThreePool}
      <div class="flex flex-wrap items-center gap-3">
        <StatusBadge status="Active" size="md" />
        <span class="text-xs text-gray-500">Canister</span>
        <EntityLink type="canister" value={CANISTER_IDS.THREEPOOL} label={shortenPrincipal(CANISTER_IDS.THREEPOOL)} />
        <CopyButton text={CANISTER_IDS.THREEPOOL} />
      </div>

      <div class="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-7 gap-3">
        <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl p-4">
          <div class="text-[10px] uppercase tracking-wider text-gray-500">TVL</div>
          <div class="text-lg font-mono text-white">${formatCompact(threePoolTvlUsd)}</div>
        </div>
        <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl p-4">
          <div class="text-[10px] uppercase tracking-wider text-gray-500">Amplification (A)</div>
          <div class="text-lg font-mono text-white">{currentA || '--'}</div>
        </div>
        <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl p-4">
          <div class="text-[10px] uppercase tracking-wider text-gray-500">Swap Fee</div>
          <div class="text-lg font-mono text-white">{formatBps(swapFeeBps)}</div>
        </div>
        <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl p-4">
          <div class="text-[10px] uppercase tracking-wider text-gray-500">Admin Fee</div>
          <div class="text-lg font-mono text-white">{formatBps(adminFeeBps)}</div>
        </div>
        <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl p-4">
          <div class="text-[10px] uppercase tracking-wider text-gray-500">Virtual Price</div>
          <div class="text-lg font-mono text-white">{formatVirtualPrice(virtualPrice)}</div>
        </div>
        <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl p-4">
          <div class="text-[10px] uppercase tracking-wider text-gray-500">LP Supply</div>
          <div class="text-lg font-mono text-white">{formatLp(totalSupply, 8)}</div>
        </div>
        <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl p-4">
          <div class="text-[10px] uppercase tracking-wider text-gray-500">Imbalance</div>
          <div class="text-lg font-mono text-white">{formatImbalance(imbalance)}</div>
        </div>
      </div>

      <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl overflow-hidden">
        <div class="px-5 py-3 border-b border-gray-700/50 flex items-center justify-between">
          <span class="text-xs font-semibold text-gray-400 uppercase tracking-wider">Reserves</span>
          {#if stats}
            <span class="text-xs text-gray-500">
              {Number(stats.swap_count ?? 0n).toLocaleString()} swaps · {Number(stats.unique_swappers ?? 0n).toLocaleString()} unique swappers (7d)
            </span>
          {/if}
        </div>
        <table class="w-full text-sm">
          <thead class="bg-gray-900/30">
            <tr class="text-[10px] uppercase tracking-wider text-gray-500">
              <th class="px-5 py-2 text-left">Token</th>
              <th class="px-5 py-2 text-right">Balance</th>
              <th class="px-5 py-2 text-right">Decimals</th>
            </tr>
          </thead>
          <tbody>
            {#each tokens as tok, i (tok.symbol)}
              <tr class="border-t border-gray-700/30">
                <td class="px-5 py-3">
                  <EntityLink type="token" value={tok.ledger_id?.toText?.() ?? String(tok.ledger_id)} label={tok.symbol} />
                </td>
                <td class="px-5 py-3 text-right font-mono text-gray-200">
                  {formatReserve(balances[i] ?? 0n, Number(tok.decimals))}
                </td>
                <td class="px-5 py-3 text-right text-gray-500">{Number(tok.decimals)}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {:else if ammPool && ammTokenA && ammTokenB}
      <div class="flex flex-wrap items-center gap-3">
        <StatusBadge status={ammPool.paused ? 'Paused' : 'Active'} size="md" />
        <span class="text-xs text-gray-500">Pool ID</span>
        <span class="text-xs font-mono text-gray-300">{shortenPrincipal(poolIdParam)}</span>
        <CopyButton text={poolIdParam} />
      </div>

      <div class="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-6 gap-3">
        <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl p-4">
          <div class="text-[10px] uppercase tracking-wider text-gray-500">TVL</div>
          <div class="text-lg font-mono text-white">{ammTvlUsd != null ? `$${formatCompact(ammTvlUsd)}` : '--'}</div>
        </div>
        <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl p-4">
          <div class="text-[10px] uppercase tracking-wider text-gray-500">Curve</div>
          <div class="text-lg font-mono text-white">Constant product</div>
        </div>
        <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl p-4">
          <div class="text-[10px] uppercase tracking-wider text-gray-500">Swap Fee</div>
          <div class="text-lg font-mono text-white">{formatBps(ammFeeBps)}</div>
        </div>
        <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl p-4">
          <div class="text-[10px] uppercase tracking-wider text-gray-500">LP Supply</div>
          <div class="text-lg font-mono text-white">{formatLp(ammLpSupply, 8)}</div>
        </div>
        <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl p-4">
          <div class="text-[10px] uppercase tracking-wider text-gray-500">Spot {ammTokenA.symbol}/{ammTokenB.symbol}</div>
          <div class="text-lg font-mono text-white">{ammSpotPriceAPerB > 0 ? ammSpotPriceAPerB.toFixed(6) : '--'}</div>
        </div>
        <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl p-4">
          <div class="text-[10px] uppercase tracking-wider text-gray-500">Status</div>
          <div class="text-lg font-mono text-white">{ammPool.paused ? 'Paused' : 'Active'}</div>
        </div>
      </div>

      <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl overflow-hidden">
        <div class="px-5 py-3 border-b border-gray-700/50 flex items-center justify-between">
          <span class="text-xs font-semibold text-gray-400 uppercase tracking-wider">Reserves</span>
          {#if ammStats}
            <span class="text-xs text-gray-500">
              {Number(ammStats.swap_count ?? 0).toLocaleString()} swaps · {Number(ammStats.unique_swappers ?? 0).toLocaleString()} swappers (7d)
            </span>
          {/if}
        </div>
        <table class="w-full text-sm">
          <thead class="bg-gray-900/30">
            <tr class="text-[10px] uppercase tracking-wider text-gray-500">
              <th class="px-5 py-2 text-left">Token</th>
              <th class="px-5 py-2 text-right">Balance</th>
              <th class="px-5 py-2 text-right">Decimals</th>
            </tr>
          </thead>
          <tbody>
            <tr class="border-t border-gray-700/30">
              <td class="px-5 py-3">
                <EntityLink type="token" value={ammTokenA.ledgerId} label={ammTokenA.symbol} />
              </td>
              <td class="px-5 py-3 text-right font-mono text-gray-200">{formatReserve(ammReserveA, ammTokenA.decimals)}</td>
              <td class="px-5 py-3 text-right text-gray-500">{ammTokenA.decimals}</td>
            </tr>
            <tr class="border-t border-gray-700/30">
              <td class="px-5 py-3">
                <EntityLink type="token" value={ammTokenB.ledgerId} label={ammTokenB.symbol} />
              </td>
              <td class="px-5 py-3 text-right font-mono text-gray-200">{formatReserve(ammReserveB, ammTokenB.decimals)}</td>
              <td class="px-5 py-3 text-right text-gray-500">{ammTokenB.decimals}</td>
            </tr>
          </tbody>
        </table>
      </div>
    {/if}
  {/snippet}

  {#snippet relationships()}
    {#if isThreePool}
      <div class="grid grid-cols-1 md:grid-cols-3 gap-3">
        <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl p-4 space-y-2">
          <div class="text-[10px] uppercase tracking-wider text-gray-500">Tokens</div>
          {#each tokens as tok (tok.symbol)}
            <div class="flex items-center justify-between text-sm">
              <EntityLink type="token" value={tok.ledger_id?.toText?.() ?? String(tok.ledger_id)} label={tok.symbol} />
              <span class="text-xs text-gray-500">{Number(tok.decimals)} dec</span>
            </div>
          {/each}
        </div>

        <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl p-4 space-y-2">
          <div class="text-[10px] uppercase tracking-wider text-gray-500">Top LPs</div>
          {#if topLps.length === 0}
            <div class="text-xs text-gray-500">No LPs yet.</div>
          {:else}
            <ul class="space-y-1">
              {#each topLps as row (row[0]?.toText?.() ?? String(row[0]))}
                {@const principal = row[0]?.toText?.() ?? String(row[0])}
                {@const bal = BigInt(row[1])}
                {@const bps = Number(row[2])}
                <li class="flex items-center justify-between text-xs">
                  <EntityLink type="address" value={principal} />
                  <span class="text-gray-400 font-mono">{formatLp(bal, 8)} LP · {(bps / 100).toFixed(1)}%</span>
                </li>
              {/each}
            </ul>
          {/if}
        </div>

        <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl p-4 space-y-2">
          <div class="text-[10px] uppercase tracking-wider text-gray-500">Top Swappers (7d)</div>
          {#if topSwappers.length === 0}
            <div class="text-xs text-gray-500">No swaps in this window.</div>
          {:else}
            <ul class="space-y-1">
              {#each topSwappers as row (row[0]?.toText?.() ?? String(row[0]))}
                {@const principal = row[0]?.toText?.() ?? String(row[0])}
                {@const count = Number(row[1])}
                {@const volume = Number(row[2]) / 1e18}
                <li class="flex items-center justify-between text-xs">
                  <EntityLink type="address" value={principal} />
                  <span class="text-gray-400 font-mono">{count} · ${volume.toLocaleString(undefined, { maximumFractionDigits: 0 })}</span>
                </li>
              {/each}
            </ul>
          {/if}
        </div>
      </div>
    {:else if ammPool && ammTokenA && ammTokenB}
      <div class="grid grid-cols-1 md:grid-cols-3 gap-3">
        <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl p-4 space-y-2">
          <div class="text-[10px] uppercase tracking-wider text-gray-500">Tokens in pool</div>
          <div class="flex items-center justify-between text-sm">
            <EntityLink type="token" value={ammTokenA.ledgerId} label={ammTokenA.symbol} />
            <span class="text-xs text-gray-500">{ammTokenA.decimals} dec</span>
          </div>
          <div class="flex items-center justify-between text-sm">
            <EntityLink type="token" value={ammTokenB.ledgerId} label={ammTokenB.symbol} />
            <span class="text-xs text-gray-500">{ammTokenB.decimals} dec</span>
          </div>
        </div>

        <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl p-4 space-y-2">
          <div class="text-[10px] uppercase tracking-wider text-gray-500">Top LPs</div>
          {#if ammTopLps.length === 0}
            <div class="text-xs text-gray-500">No LPs in this pool.</div>
          {:else}
            <ul class="space-y-1">
              {#each ammTopLps as row (row[0]?.toText?.() ?? String(row[0]))}
                {@const principal = row[0]?.toText?.() ?? String(row[0])}
                {@const bal = BigInt(row[1])}
                {@const bps = Number(row[2])}
                <li class="flex items-center justify-between text-xs">
                  <EntityLink type="address" value={principal} />
                  <span class="text-gray-400 font-mono">{formatLp(bal, 8)} LP · {(bps / 100).toFixed(2)}%</span>
                </li>
              {/each}
            </ul>
          {/if}
        </div>

        <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl p-4 space-y-2">
          <div class="text-[10px] uppercase tracking-wider text-gray-500">Top Swappers (7d)</div>
          {#if ammTopSwappers.length === 0}
            <div class="text-xs text-gray-500">No swaps in this window.</div>
          {:else}
            <ul class="space-y-1">
              {#each ammTopSwappers as row (row[0]?.toText?.() ?? String(row[0]))}
                {@const principal = row[0]?.toText?.() ?? String(row[0])}
                {@const count = Number(row[1])}
                {@const volumeA = Number(row[2]) / 10 ** ammTokenA.decimals}
                <li class="flex items-center justify-between text-xs">
                  <EntityLink type="address" value={principal} />
                  <span class="text-gray-400 font-mono">{count} · {volumeA.toLocaleString(undefined, { maximumFractionDigits: 2 })} {ammTokenA.symbol}</span>
                </li>
              {/each}
            </ul>
          {/if}
        </div>
      </div>
    {/if}

    <!-- Routes through this pool (single-hop + reconstructed two-hop) -->
    <div class="mt-3 bg-gray-800/40 border border-gray-700/50 rounded-xl p-4 space-y-3">
      <div class="flex items-center justify-between">
        <div class="text-[10px] uppercase tracking-wider text-gray-500">Routes through this pool (7d)</div>
        <span class="text-[10px] text-gray-600">Ordered token sequence · single and multi-hop</span>
      </div>
      {#if poolRoutes.length === 0}
        <div class="text-xs text-gray-500">No routes in this window.</div>
      {:else}
        <ul class="space-y-2">
          {#each poolRoutes as route, i (i)}
            {@const principals = route.route.map((p) => p.toText())}
            {@const usd = Number(route.volume_usd_e8s) / 1e8}
            <li>
              <a
                href={routeActivityHref(principals)}
                class="flex items-center justify-between gap-3 rounded-md px-2 py-2 hover:bg-gray-700/30"
              >
                <div class="flex items-center gap-1 flex-wrap text-xs">
                  {#each principals as pr, j (pr + j)}
                    <span class="font-mono text-gray-200 bg-gray-900/60 border border-gray-700/50 rounded px-1.5 py-0.5">
                      {getTokenSymbol(pr)}
                    </span>
                    {#if j < principals.length - 1}
                      <span class="text-gray-500">→</span>
                    {/if}
                  {/each}
                  <span class="ml-2 text-[10px] text-gray-500 uppercase">
                    {route.avg_hop_count === 1 ? 'single-hop' : `${route.avg_hop_count}-hop`}
                  </span>
                </div>
                <div class="text-right text-xs text-gray-400 whitespace-nowrap tabular-nums">
                  ${usd.toLocaleString(undefined, { maximumFractionDigits: usd >= 1_000 ? 0 : 2 })}
                  <span class="text-gray-600"> · {route.swap_count.toString()} swap{route.swap_count === 1n ? '' : 's'}</span>
                </div>
              </a>
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  {/snippet}

  {#snippet activity()}
    <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl overflow-hidden">
      {#if mergedEvents.length === 0}
        <div class="px-5 py-8 text-center text-gray-500 text-sm">No recent pool activity.</div>
      {:else}
        <table class="w-full">
          <thead class="bg-gray-900/40">
            <tr class="text-[10px] uppercase tracking-wider text-gray-500">
              <th class="px-4 py-2 text-left">Event</th>
              <th class="px-4 py-2 text-left">When</th>
              <th class="px-4 py-2 text-left">By</th>
              <th class="px-4 py-2 text-left">Type</th>
              <th class="px-4 py-2 text-left">Summary</th>
              <th class="px-4 py-2 text-right"></th>
            </tr>
          </thead>
          <tbody>
            {#each mergedEvents as de (de.source + ':' + de.globalIndex)}
              <MixedEventRow event={de} />
            {/each}
          </tbody>
        </table>
      {/if}
      {#if isAmm && ammPool}
        <div class="px-5 py-3 border-t border-gray-700/50 text-right">
          <a href={`/explorer/activity?pool=${encodeURIComponent(poolIdParam)}`} class="text-xs text-indigo-300 hover:text-indigo-200">
            See all activity →
          </a>
        </div>
      {/if}
    </div>
  {/snippet}

  {#snippet analytics()}
    {#if isThreePool}
      <div class="grid grid-cols-1 lg:grid-cols-2 gap-3">
        <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl p-5">
          <MiniAreaChart
            points={volPoints}
            label="Volume (7d)"
            color="#a78bfa"
            fillColor="rgba(167, 139, 250, 0.12)"
            valueFormat={(v) => `$${v.toLocaleString(undefined, { maximumFractionDigits: 0 })}`}
          />
        </div>
        <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl p-5">
          <MiniAreaChart
            points={vpPoints}
            label="Virtual Price (7d)"
            color="#34d399"
            fillColor="rgba(52, 211, 153, 0.12)"
            valueFormat={(v) => v.toFixed(6)}
          />
        </div>
        <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl p-5">
          <MiniAreaChart
            points={feePoints}
            label="Avg Fee bps (7d)"
            color="#fbbf24"
            fillColor="rgba(251, 191, 36, 0.12)"
            valueFormat={(v) => `${v.toFixed(3)}%`}
          />
        </div>
        {#each balancePointsByToken as bp (bp.symbol)}
          <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl p-5">
            <MiniAreaChart
              points={bp.points}
              label={`${bp.symbol} balance (7d)`}
              color="#60a5fa"
              fillColor="rgba(96, 165, 250, 0.12)"
              valueFormat={(v) => v.toLocaleString(undefined, { maximumFractionDigits: 0 })}
            />
          </div>
        {/each}
      </div>
    {:else if ammPool && ammTokenA && ammTokenB}
      <div class="grid grid-cols-1 lg:grid-cols-2 gap-3">
        <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl p-5">
          <MiniAreaChart
            points={ammVolPoints}
            label={`Swap volume (7d, ${ammTokenA.symbol}+${ammTokenB.symbol} combined)`}
            color="#a78bfa"
            fillColor="rgba(167, 139, 250, 0.12)"
            valueFormat={(v) => v.toLocaleString(undefined, { maximumFractionDigits: 2 })}
          />
        </div>
        <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl p-5">
          <MiniAreaChart
            points={ammFeePoints}
            label="Fees collected (7d)"
            color="#fbbf24"
            fillColor="rgba(251, 191, 36, 0.12)"
            valueFormat={(v) => v.toLocaleString(undefined, { maximumFractionDigits: 4 })}
          />
        </div>
        <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl p-5">
          <MiniAreaChart
            points={ammBalancePointsA}
            label={`${ammTokenA.symbol} reserve (7d)`}
            color={ammTokenA.color}
            fillColor={`${ammTokenA.color}22`}
            valueFormat={(v) => v.toLocaleString(undefined, { maximumFractionDigits: 2 })}
          />
        </div>
        <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl p-5">
          <MiniAreaChart
            points={ammBalancePointsB}
            label={`${ammTokenB.symbol} reserve (7d)`}
            color={ammTokenB.color}
            fillColor={`${ammTokenB.color}22`}
            valueFormat={(v) => v.toLocaleString(undefined, { maximumFractionDigits: 2 })}
          />
        </div>
        {#if ammStats}
          <div class="lg:col-span-2 bg-gray-800/40 border border-gray-700/50 rounded-xl p-5">
            <div class="text-[10px] uppercase tracking-wider text-gray-500 mb-3">Window stats · 7d</div>
            <div class="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-6 gap-3 text-center">
              <div>
                <div class="text-lg font-mono text-white">{Number(ammStats.swap_count ?? 0).toLocaleString()}</div>
                <div class="text-[10px] text-gray-500 uppercase tracking-wider">Swaps</div>
              </div>
              <div>
                <div class="text-lg font-mono text-white">{Number(ammStats.unique_swappers ?? 0).toLocaleString()}</div>
                <div class="text-[10px] text-gray-500 uppercase tracking-wider">Unique Swappers</div>
              </div>
              <div>
                <div class="text-lg font-mono text-white">{Number(ammStats.unique_lps ?? 0).toLocaleString()}</div>
                <div class="text-[10px] text-gray-500 uppercase tracking-wider">LP Holders</div>
              </div>
              <div>
                <div class="text-lg font-mono text-white">{(Number(ammStats.volume_a_e8s ?? 0n) / 10 ** ammTokenA.decimals).toLocaleString(undefined, { maximumFractionDigits: 2 })}</div>
                <div class="text-[10px] text-gray-500 uppercase tracking-wider">{ammTokenA.symbol} Volume</div>
              </div>
              <div>
                <div class="text-lg font-mono text-white">{(Number(ammStats.volume_b_e8s ?? 0n) / 10 ** ammTokenB.decimals).toLocaleString(undefined, { maximumFractionDigits: 2 })}</div>
                <div class="text-[10px] text-gray-500 uppercase tracking-wider">{ammTokenB.symbol} Volume</div>
              </div>
              <div>
                <div class="text-lg font-mono text-white">{(Number(ammStats.fees_a_e8s ?? 0n) / 10 ** ammTokenA.decimals).toLocaleString(undefined, { maximumFractionDigits: 4 })}</div>
                <div class="text-[10px] text-gray-500 uppercase tracking-wider">{ammTokenA.symbol} Fees</div>
              </div>
            </div>
          </div>
        {/if}
      </div>
    {/if}
  {/snippet}
</EntityShell>
