<script lang="ts">
  import type { TokenFlowEdge } from '$declarations/rumi_analytics/rumi_analytics.did';
  import { getTokenSymbol } from '$utils/explorerHelpers';
  import { formatUsdE8s, e8sToNumber } from '$utils/explorerChartHelpers';

  interface Props {
    edges: TokenFlowEdge[];
    loading: boolean;
    timePreset: '24h' | '7d' | '30d';
  }

  let { edges, loading, timePreset }: Props = $props();

  // Ranked list of token pairs by USD volume. Two earlier attempts
  // (bipartite Sankey, chord diagram) both broke down on real data —
  // ICP↔3USD swaps run ~100× the volume of every other pair, so any
  // geometric/proportional layout crushed the small flows into invisible
  // slivers. A ranked bar list keeps every pair legible: the bar
  // proportions still show the dominance, but the labels and counts read
  // independently of bar width.

  function activityHref(from: string, to: string): string {
    const params = new URLSearchParams();
    params.set('type', 'swap');
    params.set('token', `${from},${to}`);
    if (timePreset) params.set('time', timePreset);
    return `/explorer/activity?${params.toString()}`;
  }

  const rows = $derived.by(() => {
    if (edges.length === 0) return [];
    const maxVol = edges.reduce(
      (m, e) => (e.volume_usd_e8s > m ? e.volume_usd_e8s : m),
      0n
    );
    const maxNum = Number(maxVol) || 1;
    return edges.map((e) => {
      const fromKey = e.from_token.toText();
      const toKey = e.to_token.toText();
      const vol = Number(e.volume_usd_e8s);
      return {
        key: `${fromKey}->${toKey}`,
        fromSymbol: getTokenSymbol(fromKey),
        toSymbol: getTokenSymbol(toKey),
        volumeUsd: formatUsdE8s(e.volume_usd_e8s),
        volumeNum: e8sToNumber(e.volume_usd_e8s),
        swapCount: Number(e.swap_count),
        widthPct: Math.max(2, (vol / maxNum) * 100),
        href: activityHref(fromKey, toKey),
      };
    });
  });
</script>

<div class="space-y-1">
  {#if loading}
    <div class="text-sm text-gray-500 py-12 text-center">Loading flows…</div>
  {:else if rows.length === 0}
    <div class="text-sm text-gray-500 py-12 text-center">
      No token flow in this window.
    </div>
  {:else}
    <ul class="divide-y divide-white/5">
      {#each rows as row (row.key)}
        <li>
          <a
            href={row.href}
            class="group block px-2 py-2.5 hover:bg-white/[0.03] transition-colors rounded-md"
            aria-label="Activity: {row.fromSymbol} → {row.toSymbol}"
          >
            <div class="flex items-center justify-between text-sm gap-3 mb-1.5">
              <div class="flex items-center gap-1.5 text-gray-200 font-medium">
                <span>{row.fromSymbol}</span>
                <span class="text-gray-500">→</span>
                <span>{row.toSymbol}</span>
              </div>
              <div class="flex items-center gap-3 text-xs">
                <span class="text-gray-400 tabular-nums">
                  {row.swapCount.toLocaleString()} {row.swapCount === 1 ? 'swap' : 'swaps'}
                </span>
                <span class="text-gray-100 tabular-nums font-medium min-w-[60px] text-right">
                  {row.volumeUsd}
                </span>
              </div>
            </div>
            <div class="relative h-1.5 rounded-full bg-white/[0.04] overflow-hidden">
              <div
                class="absolute inset-y-0 left-0 bg-gradient-to-r from-teal-400/60 to-teal-400/30 group-hover:from-teal-400/80 group-hover:to-teal-400/50 transition-colors"
                style="width: {row.widthPct}%"
              ></div>
            </div>
          </a>
        </li>
      {/each}
    </ul>
  {/if}
</div>
