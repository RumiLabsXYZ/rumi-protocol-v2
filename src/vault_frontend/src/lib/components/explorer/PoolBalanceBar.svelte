<script lang="ts">
  interface PoolToken {
    symbol: string;
    balance: number;
    color: string;
  }

  interface Props {
    tokens: PoolToken[];
  }

  let { tokens }: Props = $props();

  const total = $derived(tokens.reduce((s, t) => s + t.balance, 0));
  const segments = $derived(
    tokens.map(t => ({
      ...t,
      pct: total > 0 ? (t.balance / total) * 100 : 100 / tokens.length,
    }))
  );
</script>

<div class="space-y-1.5">
  <!-- Bar -->
  <div class="h-3 rounded-full overflow-hidden flex" style="background: var(--rumi-bg-surface2);">
    {#each segments as seg, i}
      <div
        class="h-full transition-all duration-500 {i === 0 ? 'rounded-l-full' : ''} {i === segments.length - 1 ? 'rounded-r-full' : ''}"
        style="width: {seg.pct}%; background: {seg.color};"
        title="{seg.symbol}: {seg.pct.toFixed(1)}%"
      ></div>
    {/each}
  </div>
  <!-- Labels -->
  <div class="flex justify-between text-xs">
    {#each segments as seg}
      <div class="flex items-center gap-1.5">
        <span class="w-2 h-2 rounded-full flex-shrink-0" style="background: {seg.color};"></span>
        <span class="text-gray-400">{seg.symbol}</span>
        <span class="tabular-nums text-gray-500">{seg.pct.toFixed(1)}%</span>
      </div>
    {/each}
  </div>
</div>
