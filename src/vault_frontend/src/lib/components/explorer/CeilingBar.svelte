<script lang="ts">
  import { formatCompact } from '$utils/explorerChartHelpers';

  interface Props {
    used: number;
    total: number;
    /** If true, total is unlimited (u64::MAX). Show "Unlimited" instead of bar. */
    unlimited?: boolean;
  }
  let { used, total, unlimited = false }: Props = $props();

  const pct = $derived(unlimited ? 0 : total > 0 ? Math.min((used / total) * 100, 100) : 0);
  // Show absolute ceiling next to bar — easier to reason about than just "7%".
  const ceilingLabel = $derived(unlimited ? 'No limit' : formatCompact(total));
  const barColor = $derived(
    pct > 90 ? 'bg-pink-400' : pct > 70 ? 'bg-violet-400' : 'bg-teal-400'
  );
</script>

{#if unlimited}
  <span class="text-xs text-gray-500">Unlimited</span>
{:else}
  <div class="flex items-center gap-2" title="{used.toLocaleString(undefined, { maximumFractionDigits: 0 })} of {total.toLocaleString(undefined, { maximumFractionDigits: 0 })} icUSD ({pct.toFixed(0)}%)">
    <div class="fill-bar flex-1 min-w-[3rem]">
      <div class="fill-bar-inner {barColor}" style="width: {pct}%"></div>
    </div>
    <span class="text-xs tabular-nums text-gray-400 w-12 text-right">{ceilingLabel}</span>
  </div>
{/if}
