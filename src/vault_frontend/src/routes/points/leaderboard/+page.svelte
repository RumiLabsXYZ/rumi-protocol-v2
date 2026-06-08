<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { POINTS_ENABLED } from '$lib/config';
  import { principal } from '$lib/stores/wallet';
  import { getLeaderboard, getPointsConfig } from '$lib/services/pointsService';
  import { formatPoints, hasNextPage } from '$lib/utils/points';
  import { truncatePrincipal, copyToClipboard } from '$lib/utils/principalHelpers';
  import { toastStore } from '$lib/stores/toast';
  import type { LeaderboardEntry, PointsConfig } from '$declarations/rumi_points/rumi_points.did';
  import DataTable from '$lib/components/explorer/DataTable.svelte';
  import EmptyState from '$lib/components/explorer/EmptyState.svelte';

  const PAGE = 50;
  let rows = $state<LeaderboardEntry[]>([]);
  let config = $state<PointsConfig | null>(null);
  let offset = $state(0);
  let loading = $state(true);
  let error = $state(false);

  const columns = [
    { key: 'rank', label: 'Rank', align: 'left' as const, width: '15%' },
    { key: 'principal', label: 'Address', align: 'left' as const },
    { key: 'total_points', label: 'Points (USD-days)', align: 'right' as const },
  ];

  async function loadPage(o: number) {
    loading = true;
    error = false;
    try {
      const [page, cfg] = await Promise.all([
        getLeaderboard(o, PAGE),
        config ? Promise.resolve(config) : getPointsConfig(),
      ]);
      rows = page;
      config = cfg;
      offset = o;
    } catch (e) {
      console.error('[points] leaderboard load failed', e);
      error = true;
      toastStore.error('Could not load the leaderboard.');
    } finally {
      loading = false;
    }
  }
  onMount(() => {
    if (!POINTS_ENABLED) {
      goto('/');
      return;
    }
    loadPage(0);
  });

  async function copyAddress(text: string) {
    if (await copyToClipboard(text)) toastStore.success('Address copied');
  }

  const myText = $derived($principal ? $principal.toText() : null);
  const participants = $derived(config ? Number(config.registered_count) : null);
  const canNext = $derived(!loading && hasNextPage(offset, rows.length, PAGE, participants));
</script>

<svelte:head><title>Leaderboard · Rumi Points</title></svelte:head>

<div class="max-w-4xl mx-auto px-4 py-6 flex flex-col gap-4">
  <div class="flex items-center justify-between">
    <h1 class="text-xl font-semibold text-gray-100">Points Leaderboard</h1>
    {#if participants !== null}
      <span class="text-xs text-gray-400">{participants.toLocaleString()} participants</span>
    {/if}
  </div>

  {#if error && rows.length === 0}
    <div class="rounded-xl bg-gray-800/30 border border-gray-700/50 p-4 flex items-center justify-between gap-3">
      <span class="text-sm text-gray-300">Couldn't load the leaderboard.</span>
      <button class="px-3 py-1.5 rounded-lg text-sm border border-gray-700/50 text-teal-400 hover:border-teal-500/40" onclick={() => loadPage(offset)}>
        Retry
      </button>
    </div>
  {:else if !loading && offset === 0 && rows.length === 0}
    <EmptyState title="No points yet" message="The leaderboard fills in once accrual begins." icon="chart" />
  {:else}
    <DataTable {columns} data={rows} {loading} rowKey={(r) => r.principal.toText()}>
      {#snippet row(entry: LeaderboardEntry)}
        <tr class="border-b border-gray-700/30 {myText && entry.principal.toText() === myText ? 'bg-teal-500/10' : ''}">
          <td class="px-4 py-3 text-gray-300">#{entry.rank}</td>
          <td class="px-4 py-3">
            <span class="inline-flex items-center gap-2">
              <a href={`/explorer/e/address/${entry.principal.toText()}`} class="text-teal-400 hover:underline font-mono text-xs">
                {truncatePrincipal(entry.principal.toText())}
              </a>
              <button class="text-gray-500 hover:text-teal-400 text-xs" title="Copy address" aria-label="Copy address" onclick={() => copyAddress(entry.principal.toText())}>⧉</button>
              {#if myText && entry.principal.toText() === myText}<span class="text-xs text-teal-400">you</span>{/if}
            </span>
          </td>
          <td class="px-4 py-3 text-right text-gray-100">{formatPoints(entry.total_points)}</td>
        </tr>
      {/snippet}
    </DataTable>

    <div class="flex items-center justify-between">
      <button
        class="px-3 py-1.5 rounded-lg text-sm border border-gray-700/50 text-gray-300 disabled:opacity-40"
        disabled={offset === 0 || loading}
        onclick={() => loadPage(Math.max(0, offset - PAGE))}
      >Previous</button>
      <span class="text-xs text-gray-500">Showing {rows.length === 0 ? 0 : offset + 1}–{offset + rows.length}</span>
      <button
        class="px-3 py-1.5 rounded-lg text-sm border border-gray-700/50 text-gray-300 disabled:opacity-40"
        disabled={!canNext}
        onclick={() => loadPage(offset + PAGE)}
      >Next</button>
    </div>
  {/if}
</div>
