<script lang="ts">
  import { onMount } from 'svelte';
  import DataTable from '$components/explorer/DataTable.svelte';
  import EntityLink from '$components/explorer/EntityLink.svelte';
  import StatCard from '$components/explorer/StatCard.svelte';
  import { fetchIcusdHolders, fetchThreeUsdHolders, type TokenHolder } from '$services/explorer/explorerService';
  import { formatE8s } from '$utils/explorerHelpers';
  import { Principal } from '@dfinity/principal';
  import { CANISTER_IDS } from '$lib/config';
  import { fetchHolderSeries } from '$services/explorer/analyticsService';

  type TokenTab = 'icusd' | '3usd';

  let selectedToken: TokenTab = $state('icusd');
  let holders: TokenHolder[] = $state([]);
  let totalSupply = $state(0n);
  let txCount = $state(0n);
  let loading = $state(true);
  let error: string | null = $state(null);
  let holderTrends: any[] = $state([]);
  let trendsLoading = $state(false);

  const chartW = 600;
  const chartH = 140;
  const pad = 4;

  let trendPoints = $derived(holderTrends.map((r: any) => ({
    x: Number(r.timestamp_ns) / 1e9,
    y: Number(r.total_holders)
  })));
  let trendXMin = $derived(trendPoints.length ? Math.min(...trendPoints.map((p) => p.x)) : 0);
  let trendXMax = $derived(trendPoints.length ? Math.max(...trendPoints.map((p) => p.x)) : 1);
  let trendYMin = $derived(trendPoints.length ? Math.min(...trendPoints.map((p) => p.y)) : 0);
  let trendYMax = $derived(trendPoints.length ? (Math.max(...trendPoints.map((p) => p.y)) || 1) : 1);
  let trendXRange = $derived(trendXMax - trendXMin || 1);
  let trendYRange = $derived(trendYMax - trendYMin || 1);
  let trendPolyline = $derived(trendPoints.map((p) => {
    const x = pad + ((p.x - trendXMin) / trendXRange) * (chartW - pad * 2);
    const y = pad + (chartH - pad * 2) - ((p.y - trendYMin) / trendYRange) * (chartH - pad * 2);
    return `${x},${y}`;
  }).join(' '));

  // Known protocol canister principals for labeling
  const PROTOCOL_LABELS: Record<string, string> = {
    [CANISTER_IDS.PROTOCOL]: 'Rumi Backend',
    [CANISTER_IDS.TREASURY]: 'Treasury',
    [CANISTER_IDS.STABILITY_POOL]: 'Stability Pool',
    [CANISTER_IDS.THREEPOOL]: '3pool',
    [CANISTER_IDS.RUMI_AMM]: 'AMM',
    [CANISTER_IDS.ICUSD_LEDGER]: 'icUSD Ledger',
  };

  const TOKEN_INFO: Record<TokenTab, { symbol: string; name: string }> = {
    icusd: { symbol: 'icUSD', name: 'icUSD Stablecoin' },
    '3usd': { symbol: '3USD', name: '3USD LP Token' },
  };

  let tokenInfo = $derived(TOKEN_INFO[selectedToken]);

  const columns = $derived([
    { key: 'rank', label: '#', align: 'center' as const, width: '60px' },
    { key: 'account', label: 'Address', align: 'left' as const },
    { key: 'label', label: 'Label', align: 'left' as const, width: '140px' },
    { key: 'balanceNumber', label: `Balance (${tokenInfo.symbol})`, align: 'right' as const, sortable: true },
    { key: 'share', label: 'Share', align: 'right' as const, width: '100px' },
  ]);

  function getLabel(principal: string): string {
    return PROTOCOL_LABELS[principal] ?? '';
  }

  function formatShare(balance: bigint, total: bigint): string {
    if (total === 0n) return '0%';
    const pct = (Number(balance) / Number(total)) * 100;
    if (pct < 0.01) return '<0.01%';
    return `${pct.toFixed(2)}%`;
  }

  async function loadHolders(token: TokenTab) {
    loading = true;
    error = null;
    try {
      const result = token === 'icusd'
        ? await fetchIcusdHolders()
        : await fetchThreeUsdHolders();
      holders = result.holders;
      totalSupply = result.totalSupply;
      txCount = result.txCount;
    } catch (err) {
      error = String(err);
      holders = [];
      totalSupply = 0n;
      txCount = 0n;
    } finally {
      loading = false;
    }
    loadTrends(token);
  }

  async function loadTrends(token: TokenTab) {
    trendsLoading = true;
    try {
      const ledgerId = token === 'icusd' ? CANISTER_IDS.ICUSD_LEDGER : CANISTER_IDS.THREEPOOL;
      holderTrends = await fetchHolderSeries(Principal.fromText(ledgerId), 90);
    } catch (e) {
      console.error('[holders] trends load failed:', e);
      holderTrends = [];
    } finally {
      trendsLoading = false;
    }
  }

  function selectToken(token: TokenTab) {
    if (token === selectedToken && !loading) return;
    selectedToken = token;
    loadHolders(token);
  }

  onMount(() => {
    loadHolders(selectedToken);
  });
</script>

<svelte:head>
  <title>{tokenInfo.symbol} Holders | Rumi Explorer</title>
</svelte:head>

<!-- Header -->
<div class="mb-6">
  <h1 class="text-2xl font-bold text-white">Token Holders</h1>
  <p class="text-sm text-gray-400 mt-1">
    All accounts holding {tokenInfo.symbol}, ranked by balance. Derived from on-chain transaction history.
  </p>
</div>

<!-- Token Selector -->
<div class="flex gap-2 mb-6">
  <button
    onclick={() => selectToken('icusd')}
    class="px-4 py-2 rounded-lg text-sm font-medium transition-colors
           {selectedToken === 'icusd'
             ? 'bg-indigo-500/20 text-indigo-300 border border-indigo-500/40'
             : 'bg-gray-800/40 text-gray-400 border border-gray-700/50 hover:text-gray-200 hover:border-gray-600'}"
  >
    icUSD
  </button>
  <button
    onclick={() => selectToken('3usd')}
    class="px-4 py-2 rounded-lg text-sm font-medium transition-colors
           {selectedToken === '3usd'
             ? 'bg-indigo-500/20 text-indigo-300 border border-indigo-500/40'
             : 'bg-gray-800/40 text-gray-400 border border-gray-700/50 hover:text-gray-200 hover:border-gray-600'}"
  >
    3USD
  </button>
</div>

{#if error}
  <div class="rounded-xl bg-red-500/10 border border-red-500/30 p-4 text-red-300 text-sm mb-6">
    Failed to load holder data: {error}
  </div>
{/if}

<!-- Summary Stats -->
<div class="grid grid-cols-2 md:grid-cols-4 gap-4 mb-6">
  <StatCard
    label="Holders"
    value={loading ? '...' : holders.length.toLocaleString()}
  />
  <StatCard
    label="Total Supply"
    value={loading ? '...' : `${formatE8s(totalSupply)} ${tokenInfo.symbol}`}
  />
  <StatCard
    label="Transactions"
    value={loading ? '...' : txCount.toLocaleString()}
  />
  <StatCard
    label="Top Holder Share"
    value={loading || holders.length === 0 ? '...' : formatShare(holders[0].balance, totalSupply)}
  />
</div>

<!-- Holders Table -->
<DataTable {columns} data={holders} {loading} emptyMessage="No {tokenInfo.symbol} holders found">
  {#snippet row(holder: TokenHolder, i: number)}
    <tr class="border-b border-gray-700/30 hover:bg-white/[0.02] transition-colors">
      <td class="px-4 py-3 text-center text-gray-500 text-sm">{i + 1}</td>
      <td class="px-4 py-3">
        <EntityLink type="address" value={holder.principal} short={true} />
        {#if holder.subaccount}
          <span class="ml-1 text-xs text-gray-500 font-mono" title="Subaccount: {holder.subaccount}">
            (sub)
          </span>
        {/if}
      </td>
      <td class="px-4 py-3">
        {#if getLabel(holder.principal)}
          <span class="inline-flex items-center px-2 py-0.5 rounded-md text-xs font-medium bg-indigo-500/20 text-indigo-300 border border-indigo-500/30">
            {getLabel(holder.principal)}
          </span>
        {/if}
      </td>
      <td class="px-4 py-3 text-right font-mono text-sm text-gray-200">
        {formatE8s(holder.balance)}
      </td>
      <td class="px-4 py-3 text-right text-sm text-gray-400">
        {formatShare(holder.balance, totalSupply)}
      </td>
    </tr>
  {/snippet}
</DataTable>

<!-- Holder Trends -->
{#if holderTrends.length > 1}
<section class="mt-8">
  <h3 class="mb-3 text-base font-semibold text-white">Holder Count Over Time</h3>
  <div class="explorer-card">
    <svg viewBox="0 0 {chartW} {chartH}" class="w-full h-auto" preserveAspectRatio="none">
      <polyline
        fill="none"
        stroke="#6366f1"
        stroke-width="2"
        points={trendPolyline}
      />
    </svg>
    <div class="mt-1 flex justify-between text-xs text-white/30">
      <span>{new Date(trendXMin * 1000).toLocaleDateString()}</span>
      <span>Latest: {trendPoints[trendPoints.length - 1]?.y ?? 0} holders</span>
      <span>{new Date(trendXMax * 1000).toLocaleDateString()}</span>
    </div>
  </div>
</section>
{/if}
