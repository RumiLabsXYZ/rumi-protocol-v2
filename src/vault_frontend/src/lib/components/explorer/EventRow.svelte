<script lang="ts">
  import { formatEvent } from '$utils/explorerFormatters';
  import EntityLink from './EntityLink.svelte';
  import { timeAgo } from '$utils/explorerHelpers';
  import { getEventTimestamp } from '$utils/eventFormatters';

  interface Props {
    event: any;
    index: number | null;
    showTimestamp?: boolean;
    [key: string]: any;
  }

  let { event, index, showTimestamp = true, ...rest }: Props = $props();

  const hasIndex = $derived(index != null && index >= 0);

  const formatted = $derived(formatEvent(event));

  const timestamp = $derived.by(() => {
    const ts = getEventTimestamp(event);
    return ts ? timeAgo(ts) : null;
  });
</script>

<tr class="border-b border-gray-700/50 hover:bg-gray-800/30 transition-colors group">
  <!-- Event index -->
  <td class="px-4 py-3">
    {#if hasIndex}
      <EntityLink type="event" value={String(index)} />
    {:else}
      <span class="text-gray-600 text-xs">—</span>
    {/if}
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
      class="inline-block text-xs font-medium px-2.5 py-0.5 rounded-full whitespace-nowrap {formatted.badgeColor}"
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
    {#if hasIndex}
      <a
        href="/explorer/event/{index}"
        class="text-xs text-blue-400 hover:text-blue-300 opacity-0 group-hover:opacity-100 transition-opacity whitespace-nowrap"
      >
        Details &rarr;
      </a>
    {/if}
  </td>
</tr>
