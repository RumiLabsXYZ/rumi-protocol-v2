<script lang="ts">
  import { timeAgo, shortenPrincipal, formatTimestamp } from '$utils/explorerHelpers';
  import { displayEvent } from '$utils/displayEvent';
  import type { DisplayEvent } from '$utils/displayEvent';

  interface Props {
    event: DisplayEvent;
    vaultOwnerMap?: Map<number, string>;
  }

  let { event, vaultOwnerMap }: Props = $props();

  const display = $derived(displayEvent(event, { vaultOwnerMap }));
  const relativeTime = $derived(display.timestamp ? timeAgo(display.timestamp) : null);
  const absoluteTime = $derived(display.timestamp ? formatTimestamp(display.timestamp) : null);
</script>

<tr class="border-b border-gray-700/50 hover:bg-gray-800/30 transition-colors group">
  <td class="px-4 py-3">
    <a href={display.detailHref} class="text-xs text-blue-400 hover:text-blue-300 font-mono" title="{display.sourceLabel} Event #{display.globalIndex}">
      {display.sourceLabel} #{display.globalIndex}
    </a>
  </td>
  <td class="px-4 py-3 text-xs text-gray-500 whitespace-nowrap">
    {#if relativeTime}
      <span title={absoluteTime ?? ''}>{relativeTime}</span>
    {:else}
      <span class="text-gray-600">&mdash;</span>
    {/if}
  </td>
  <td class="px-4 py-3 text-xs text-gray-400 whitespace-nowrap">
    {#if display.principal}
      <a href="/explorer/address/{display.principal}" class="hover:text-blue-400 transition-colors font-mono">
        {shortenPrincipal(display.principal)}
      </a>
    {:else}
      <span class="text-gray-600">&mdash;</span>
    {/if}
  </td>
  <td class="px-4 py-3">
    <span class="inline-block text-xs font-medium px-2.5 py-0.5 rounded-full whitespace-nowrap {display.formatted.badgeColor}">
      {display.formatted.typeName}
    </span>
  </td>
  <td class="px-4 py-3 text-sm text-gray-300 truncate max-w-[300px]">
    {display.formatted.summary}
  </td>
  <td class="px-4 py-3 text-right">
    <a
      href={display.detailHref}
      class="text-xs text-blue-400 hover:text-blue-300 opacity-0 group-hover:opacity-100 transition-opacity whitespace-nowrap"
    >
      Details &rarr;
    </a>
  </td>
</tr>
