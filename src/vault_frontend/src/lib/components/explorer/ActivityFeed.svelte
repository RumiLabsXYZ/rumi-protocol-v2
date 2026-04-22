<script lang="ts">
  import { displayEvent, wrapBackendEvent } from '$lib/utils/displayEvent';
  import { timeAgo, formatTimestamp } from '$lib/utils/explorerHelpers';

  interface Props {
    events: Array<{ event: any; globalIndex: number }>;
    loading?: boolean;
    vaultCollateralMap?: Map<number, string>;
  }

  let { events, loading = false, vaultCollateralMap }: Props = $props();
</script>

<div class="bg-gray-800/50 border border-gray-700/50 rounded-xl overflow-hidden">
  <div class="px-5 py-4 border-b border-gray-700/50">
    <div class="flex items-center justify-between">
      <h3 class="text-sm font-semibold text-gray-200">Recent Activity</h3>
      <a href="/explorer/activity" class="text-xs text-blue-400 hover:text-blue-300 transition-colors">View all</a>
    </div>
  </div>

  {#if loading}
    <div class="px-5 py-8 text-center text-gray-500 text-sm">Loading events...</div>
  {:else if events.length === 0}
    <div class="px-5 py-8 text-center text-gray-500 text-sm">No recent events</div>
  {:else}
    <div class="divide-y divide-gray-700/30">
      {#each events as { event, globalIndex }}
        {@const display = displayEvent(wrapBackendEvent(event, globalIndex), { vaultCollateralMap })}
        <a
          href={display.detailHref}
          class="flex items-center gap-3 px-5 py-3 hover:bg-gray-700/30 transition-colors"
        >
          <span class="shrink-0 text-xs font-medium px-2 py-0.5 rounded-full whitespace-nowrap {display.formatted.badgeColor}">
            {display.formatted.typeName}
          </span>
          <span class="flex-1 text-sm text-gray-300 truncate">{display.formatted.summary}</span>
          <span class="shrink-0 text-xs text-gray-500 tabular-nums">
            {#if display.timestamp}<span title={formatTimestamp(display.timestamp)}>{timeAgo(display.timestamp)}</span>{/if}
          </span>
          <span class="shrink-0 text-xs text-gray-600 tabular-nums">#{display.globalIndex}</span>
        </a>
      {/each}
    </div>
  {/if}
</div>
