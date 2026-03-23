<script lang="ts">
  import { getEventType, getEventBadgeColor, getEventSummary, formatTimestamp, getEventTimestamp } from '$lib/utils/eventFormatters';

  interface Props {
    events: Array<{ event: any; globalIndex: number }>;
    loading?: boolean;
    vaultCollateralMap?: Map<number, any>;
  }

  let { events, loading = false, vaultCollateralMap }: Props = $props();
</script>

<div class="bg-gray-800/50 border border-gray-700/50 rounded-xl overflow-hidden">
  <div class="px-5 py-4 border-b border-gray-700/50">
    <div class="flex items-center justify-between">
      <h3 class="text-sm font-semibold text-gray-200">Recent Activity</h3>
      <a href="/explorer/events" class="text-xs text-blue-400 hover:text-blue-300 transition-colors">View all</a>
    </div>
  </div>

  {#if loading}
    <div class="px-5 py-8 text-center text-gray-500 text-sm">Loading events...</div>
  {:else if events.length === 0}
    <div class="px-5 py-8 text-center text-gray-500 text-sm">No recent events</div>
  {:else}
    <div class="divide-y divide-gray-700/30">
      {#each events as { event, globalIndex }}
        {@const eventType = getEventType(event)}
        {@const badgeColor = getEventBadgeColor(event)}
        {@const summary = getEventSummary(event, vaultCollateralMap)}
        {@const ts = getEventTimestamp(event)}
        <a
          href="/explorer/event/{globalIndex}"
          class="flex items-center gap-3 px-5 py-3 hover:bg-gray-700/30 transition-colors"
        >
          <span
            class="shrink-0 text-xs font-medium px-2 py-0.5 rounded-full whitespace-nowrap"
            style="background:{badgeColor}20; color:{badgeColor}; border:1px solid {badgeColor}40;"
          >
            {eventType}
          </span>
          <span class="flex-1 text-sm text-gray-300 truncate">{summary}</span>
          <span class="shrink-0 text-xs text-gray-500 tabular-nums">
            {#if ts}{formatTimestamp(ts)}{/if}
          </span>
          <span class="shrink-0 text-xs text-gray-600 tabular-nums">#{globalIndex}</span>
        </a>
      {/each}
    </div>
  {/if}
</div>
