<script lang="ts">
  import { formatEvent } from '$utils/explorerFormatters';
  import EntityLink from './EntityLink.svelte';
  import { timeAgo } from '$utils/explorerHelpers';
  import { getEventTimestamp } from '$utils/eventFormatters';

  interface Props {
    event: any;
    index: number;
    showTimestamp?: boolean;
  }

  let { event, index, showTimestamp = true }: Props = $props();

  const formatted = $derived(formatEvent(event));

  const timestamp = $derived.by(() => {
    const ts = getEventTimestamp(event);
    return ts ? timeAgo(ts) : null;
  });
</script>

<tr class="border-b border-gray-700/50 hover:bg-gray-800/30 transition-colors group">
  <!-- Event index -->
  <td class="px-4 py-3">
    <EntityLink type="event" value={String(index)} />
  </td>

  <!-- Timestamp -->
  {#if showTimestamp}
    <td class="px-4 py-3 text-xs text-gray-500 whitespace-nowrap">
      {#if timestamp}
        <span title={timestamp}>{timestamp}</span>
      {:else}
        <span class="text-gray-600">&mdash;</span>
      {/if}
    </td>
  {/if}

  <!-- Type badge -->
  <td class="px-4 py-3">
    <span
      class="inline-block text-xs font-medium px-2.5 py-0.5 rounded-full whitespace-nowrap"
      style="background: {formatted.badgeColor}20; color: {formatted.badgeColor}; border: 1px solid {formatted.badgeColor}40;"
    >
      {formatted.typeName}
    </span>
  </td>

  <!-- Summary -->
  <td class="px-4 py-3 text-sm text-gray-300 truncate max-w-[300px]">
    {formatted.summary}
  </td>

  <!-- Details link -->
  <td class="px-4 py-3 text-right">
    <a
      href="/explorer/event/{index}"
      class="text-xs text-blue-400 hover:text-blue-300 opacity-0 group-hover:opacity-100 transition-opacity whitespace-nowrap"
    >
      Details &rarr;
    </a>
  </td>
</tr>
