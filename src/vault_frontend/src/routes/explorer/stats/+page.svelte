<script lang="ts">
  import { onMount } from 'svelte';
  import StatCard from '$lib/components/explorer/StatCard.svelte';
  import {
    fetchProtocolSummary,
    fetchApys,
    fetchPegStatus,
    fetchTradeActivity,
    fetchCollectorHealth,
  } from '$lib/services/explorer/analyticsService';
  import { formatE8s, formatBps, timeAgo } from '$lib/utils/explorerHelpers';
  import type {
    ProtocolSummary,
    ApyResponse,
    PegStatus,
    TradeActivityResponse,
    CollectorHealth,
  } from '$declarations/rumi_analytics/rumi_analytics.did';

  // ── Inline helpers ───────────────────────────────────────────────────────────

  function fmtPct(v: number | null | undefined): string {
    if (v == null) return 'N/A';
    return `${v.toFixed(2)}%`;
  }

  function fmtPrice(v: number | null | undefined): string {
    if (v == null) return 'N/A';
    return `$${v.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 4 })}`;
  }

  // ── State ────────────────────────────────────────────────────────────────────

  let loading = $state(true);
  let error = $state(false);

  let summary = $state<ProtocolSummary | null>(null);
  let apys = $state<ApyResponse | null>(null);
  let peg = $state<PegStatus | null>(null);
  let trade = $state<TradeActivityResponse | null>(null);
  let health = $state<CollectorHealth | null>(null);

  // ── Load ─────────────────────────────────────────────────────────────────────

  onMount(async () => {
    try {
      const results = await Promise.all([
        fetchProtocolSummary(),
        fetchApys(),
        fetchPegStatus(),
        fetchTradeActivity(),
        fetchCollectorHealth(),
      ]);
      summary = results[0];
      apys = results[1];
      peg = results[2];
      trade = results[3];
      health = results[4];
    } catch (err) {
      console.error('[stats] load failed:', err);
      error = true;
    } finally {
      loading = false;
    }
  });

  // ── Derived display values ────────────────────────────────────────────────────

  let tvl = $derived(summary ? `$${formatE8s(summary.total_collateral_usd_e8s)}` : 'N/A');
  let totalDebt = $derived(summary ? formatE8s(summary.total_debt_e8s) + ' icUSD' : 'N/A');
  let systemCR = $derived(summary ? formatBps(summary.system_cr_bps) : 'N/A');
  let vaultCount = $derived(summary ? summary.total_vault_count.toLocaleString() : 'N/A');
  let volume24h = $derived(summary ? `$${formatE8s(summary.volume_24h_e8s)}` : 'N/A');

  let imbalance = $derived(peg ? `${peg.max_imbalance_pct.toFixed(2)}%` : 'N/A');
  let lpApy = $derived(apys?.lp_apy_pct?.[0] != null ? fmtPct(apys.lp_apy_pct[0]) : (summary?.lp_apy_pct?.[0] != null ? fmtPct(summary.lp_apy_pct[0]) : 'N/A'));
  let spApy = $derived(apys?.sp_apy_pct?.[0] != null ? fmtPct(apys.sp_apy_pct[0]) : (summary?.sp_apy_pct?.[0] != null ? fmtPct(summary.sp_apy_pct[0]) : 'N/A'));
  let swaps24h = $derived(summary ? summary.swap_count_24h.toLocaleString() : 'N/A');

  let tradeThreePool = $derived(trade ? trade.three_pool_swaps.toLocaleString() : 'N/A');
  let tradeAmm = $derived(trade ? trade.amm_swaps.toLocaleString() : 'N/A');
  let tradeFees = $derived(trade ? `$${formatE8s(trade.total_fees_e8s)}` : 'N/A');
  let tradeTraders = $derived(trade ? trade.unique_traders.toLocaleString() : 'N/A');
</script>

<div class="space-y-8 {error ? 'border border-red-500/40 rounded-xl p-4' : ''}">

  <!-- Page title -->
  <div class="flex items-center justify-between">
    <h1 class="text-xl font-semibold text-white">Protocol Stats</h1>
    {#if summary}
      <span class="text-xs text-gray-500">Updated {timeAgo(summary.timestamp_ns)}</span>
    {/if}
  </div>

  {#if loading}
    <!-- Loading spinner -->
    <div class="flex items-center justify-center py-20">
      <svg class="animate-spin h-8 w-8 text-indigo-400" viewBox="0 0 24 24" fill="none">
        <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4" />
        <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8v8H4z" />
      </svg>
    </div>

  {:else}

    <!-- ── Section 1: Protocol Overview ── -->
    <section>
      <h2 class="text-sm font-medium text-gray-400 uppercase tracking-wide mb-3">Protocol Overview</h2>
      <div class="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-5 gap-3">
        <StatCard label="Total TVL" value={tvl} />
        <StatCard label="Total Debt" value={totalDebt} />
        <StatCard label="System CR" value={systemCR} />
        <StatCard label="Vaults" value={vaultCount} />
        <StatCard label="24h Volume" value={volume24h} />
      </div>
    </section>

    <!-- ── Section 2: Collateral Prices ── -->
    {#if summary && summary.prices.length > 0}
      <section>
        <h2 class="text-sm font-medium text-gray-400 uppercase tracking-wide mb-3">Collateral Prices</h2>
        <div class="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-4 gap-3">
          {#each summary.prices as entry}
            <StatCard
              label={entry.symbol}
              value={fmtPrice(entry.twap_price)}
              subtitle={`Latest: ${fmtPrice(entry.latest_price)} · ${entry.sample_count} samples`}
            />
          {/each}
        </div>
      </section>
    {/if}

    <!-- ── Section 3: Pool Health ── -->
    <section>
      <h2 class="text-sm font-medium text-gray-400 uppercase tracking-wide mb-3">Pool Health</h2>
      <div class="grid grid-cols-2 sm:grid-cols-4 gap-3">
        <StatCard label="3pool Imbalance" value={imbalance} />
        <StatCard label="LP APY" value={lpApy} />
        <StatCard label="SP APY" value={spApy} />
        <StatCard label="24h Swaps" value={swaps24h} />
      </div>
    </section>

    <!-- ── Section 4: Trade Activity (24h window) ── -->
    <section>
      <h2 class="text-sm font-medium text-gray-400 uppercase tracking-wide mb-3">Trade Activity (24h)</h2>
      <div class="grid grid-cols-2 sm:grid-cols-4 gap-3">
        <StatCard label="3pool Swaps" value={tradeThreePool} />
        <StatCard label="AMM Swaps" value={tradeAmm} />
        <StatCard label="Total Fees" value={tradeFees} />
        <StatCard label="Unique Traders" value={tradeTraders} />
      </div>
    </section>

    <!-- ── Section 5: Collector Health ── -->
    {#if health}
      <section>
        <h2 class="text-sm font-medium text-gray-400 uppercase tracking-wide mb-3">Collector Health</h2>
        <div class="overflow-x-auto rounded-xl border border-gray-700/50">
          <table class="w-full text-sm">
            <thead>
              <tr class="text-left text-xs text-gray-400 uppercase tracking-wide border-b border-gray-700/50">
                <th class="px-4 py-3">Cursor</th>
                <th class="px-4 py-3 text-right">Position</th>
                <th class="px-4 py-3 text-right">Sources</th>
                <th class="px-4 py-3 text-right">Last Success</th>
                <th class="px-4 py-3 text-right">Status</th>
              </tr>
            </thead>
            <tbody>
              {#each health.cursors as cursor}
                <tr class="border-b border-gray-800/50 hover:bg-gray-800/30 transition-colors">
                  <td class="px-4 py-3 font-mono text-white">{cursor.name}</td>
                  <td class="px-4 py-3 text-right font-mono text-gray-300">{cursor.cursor_position.toLocaleString()}</td>
                  <td class="px-4 py-3 text-right text-gray-300">{cursor.source_count.toLocaleString()}</td>
                  <td class="px-4 py-3 text-right text-gray-400">
                    {cursor.last_success_ns > 0n ? timeAgo(cursor.last_success_ns) : 'never'}
                  </td>
                  <td class="px-4 py-3 text-right">
                    {#if cursor.last_error.length > 0}
                      <span class="inline-flex items-center gap-1 text-red-400 font-medium" title={cursor.last_error[0]}>
                        <span class="h-1.5 w-1.5 rounded-full bg-red-400 inline-block"></span>
                        Error
                      </span>
                    {:else}
                      <span class="inline-flex items-center gap-1 text-emerald-400 font-medium">
                        <span class="h-1.5 w-1.5 rounded-full bg-emerald-400 inline-block"></span>
                        OK
                      </span>
                    {/if}
                  </td>
                </tr>
              {/each}
              {#if health.cursors.length === 0}
                <tr>
                  <td colspan="5" class="px-4 py-6 text-center text-gray-500">No cursor data available</td>
                </tr>
              {/if}
            </tbody>
          </table>
        </div>

        <!-- Error counters summary -->
        <div class="mt-3 grid grid-cols-3 sm:grid-cols-5 gap-2">
          {#each Object.entries(health.error_counters) as [source, count]}
            <div class="bg-gray-800/40 border border-gray-700/40 rounded-lg px-3 py-2 text-center">
              <p class="text-xs text-gray-400 capitalize">{source}</p>
              <p class="text-sm font-mono font-bold {Number(count) > 0 ? 'text-red-400' : 'text-emerald-400'}">{Number(count)}</p>
            </div>
          {/each}
        </div>
      </section>
    {/if}

  {/if}
</div>
