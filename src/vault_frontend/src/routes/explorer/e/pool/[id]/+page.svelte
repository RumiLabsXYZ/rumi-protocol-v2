<script lang="ts">
  import { page } from '$app/stores';
  import { onMount } from 'svelte';
  import EntityShell from '$components/explorer/entity/EntityShell.svelte';
  import EntityLink from '$components/explorer/EntityLink.svelte';
  import CopyButton from '$components/explorer/CopyButton.svelte';
  import MiniAreaChart from '$components/explorer/MiniAreaChart.svelte';
  import MixedEventRow from '$components/explorer/MixedEventRow.svelte';
  import StatusBadge from '$components/explorer/StatusBadge.svelte';
  import { shortenPrincipal } from '$utils/explorerHelpers';
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
  } from '$services/explorer/explorerService';
  import type { DisplayEvent } from '$utils/displayEvent';
  import { CANISTER_IDS } from '$lib/config';

  const poolId = $derived($page.params.id);

  // Only 3pool supported in this step; AMM lands in Step 7.
  const isThreePool = $derived(poolId === '3pool' || poolId === CANISTER_IDS.THREEPOOL);

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
  let loading = $state(true);
  let notFound = $state(false);

  const tokens = $derived(status?.tokens ?? []);
  const balances = $derived((status?.balances ?? []) as bigint[]);
  const totalSupply = $derived(status ? BigInt(status.lp_total_supply ?? 0) : 0n);
  const virtualPrice = $derived(status ? BigInt(status.virtual_price ?? 0) : 0n);
  const currentA = $derived(status ? Number(status.current_a ?? 0) : 0);
  const swapFeeBps = $derived(status ? Number(status.swap_fee_bps ?? 0) : 0);
  const adminFeeBps = $derived(status ? Number(status.admin_fee_bps ?? 0) : 0);
  const imbalance = $derived(health ? Number(health.current_imbalance ?? 0) : 0);

  function formatReserve(raw: bigint, decimals: number): string {
    const divisor = 10n ** BigInt(decimals);
    const whole = raw / divisor;
    return Number(whole).toLocaleString();
  }

  function formatLp(raw: bigint): string {
    // 3USD LP token uses 8 decimals per project config.
    const divisor = 10n ** 8n;
    const whole = raw / divisor;
    return Number(whole).toLocaleString();
  }

  function formatVirtualPrice(raw: bigint): string {
    // Virtual price uses 18 decimals in Curve convention.
    return (Number(raw) / 1e18).toFixed(6);
  }

  function formatBps(bps: number): string {
    return `${(bps / 100).toFixed(2)}%`;
  }

  function formatImbalance(val: number): string {
    // imbalance is scaled by 1e18 in 3pool (normalized 18-dec precision)
    return `${(val / 1e16).toFixed(2)}%`;
  }

  // Volume series: sum across tokens per bucket (each token is 18-dec normalized)
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

  // Balance series per token: pick one chart per token
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

  // ── Build DisplayEvents for the merged activity feed ─────────────────────
  const mergedEvents = $derived.by((): DisplayEvent[] => {
    const out: DisplayEvent[] = [];
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
    out.sort((a, b) => b.timestamp - a.timestamp);
    return out.slice(0, 40);
  });

  onMount(async () => {
    if (!isThreePool) {
      notFound = true;
      loading = false;
      return;
    }
    try {
      const swapCount = await fetchSwapEventCount().catch(() => 0n);
      const liqCount = await fetch3PoolLiquidityEventCount().catch(() => 0n);

      const [s, st, h, lps, swappers, sev, lev, vol, bal, fees, vp] = await Promise.all([
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
    } finally {
      loading = false;
    }
  });
</script>

<svelte:head>
  <title>{isThreePool ? '3pool' : poolId} Pool | Rumi Explorer</title>
</svelte:head>

<EntityShell
  title={isThreePool ? '3pool (3USD)' : `Pool ${poolId}`}
  subtitle={isThreePool ? 'Curve-style stableswap · icUSD / ckUSDC / ckUSDT' : undefined}
  loading={loading}
  error={notFound ? 'Only 3pool is currently supported as an entity page. AMM pool pages land in a later step.' : null}
>
  {#snippet identity()}
    <div class="flex flex-wrap items-center gap-3">
      <StatusBadge status="Active" size="md" />
      <span class="text-xs text-gray-500">Canister</span>
      <EntityLink type="canister" value={CANISTER_IDS.THREEPOOL} label={shortenPrincipal(CANISTER_IDS.THREEPOOL)} />
      <CopyButton text={CANISTER_IDS.THREEPOOL} />
    </div>

    <div class="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-6 gap-3">
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
        <div class="text-lg font-mono text-white">{formatLp(totalSupply)}</div>
      </div>
      <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl p-4">
        <div class="text-[10px] uppercase tracking-wider text-gray-500">Imbalance</div>
        <div class="text-lg font-mono text-white">{formatImbalance(imbalance)}</div>
      </div>
    </div>

    <!-- Reserves table -->
    <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl overflow-hidden">
      <div class="px-5 py-3 border-b border-gray-700/50 flex items-center justify-between">
        <span class="text-xs font-semibold text-gray-400 uppercase tracking-wider">Reserves</span>
        {#if stats}
          <span class="text-xs text-gray-500">{Number(stats.swap_count ?? 0n).toLocaleString()} swaps · {Number(stats.unique_swappers ?? 0n).toLocaleString()} unique swappers (7d)</span>
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
  {/snippet}

  {#snippet relationships()}
    <div class="grid grid-cols-1 md:grid-cols-3 gap-3">
      <!-- Tokens -->
      <div class="bg-gray-800/40 border border-gray-700/50 rounded-xl p-4 space-y-2">
        <div class="text-[10px] uppercase tracking-wider text-gray-500">Tokens</div>
        {#each tokens as tok (tok.symbol)}
          <div class="flex items-center justify-between text-sm">
            <EntityLink type="token" value={tok.ledger_id?.toText?.() ?? String(tok.ledger_id)} label={tok.symbol} />
            <span class="text-xs text-gray-500">{Number(tok.decimals)} dec</span>
          </div>
        {/each}
      </div>

      <!-- Top LPs -->
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
                <span class="text-gray-400 font-mono">{formatLp(bal)} LP · {(bps / 100).toFixed(1)}%</span>
              </li>
            {/each}
          </ul>
        {/if}
      </div>

      <!-- Top swappers -->
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
    </div>
  {/snippet}

  {#snippet analytics()}
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
  {/snippet}
</EntityShell>
