<script lang="ts">
  import { timeAgo, shortenPrincipal, formatTimestamp } from '$utils/explorerHelpers';
  import {
    formatNonBackendEvent, extractEventPrincipal, dexDetailHref, DEX_SOURCE_LABEL,
  } from '$utils/displayEvent';
  import type { DisplayEvent, NonBackendSource } from '$utils/displayEvent';

  interface Props {
    event: DisplayEvent;
    vaultOwnerMap?: Map<number, string>;
  }

  let { event, vaultOwnerMap }: Props = $props();

  const source = $derived(event.source as NonBackendSource);
  const formatted = $derived(formatNonBackendEvent(event));
  const principal = $derived(extractEventPrincipal(event.event, event.source, vaultOwnerMap));
  const href = $derived(dexDetailHref(event));
  const sourceLabel = $derived(DEX_SOURCE_LABEL[source] ?? source);
  const relativeTime = $derived(event.timestamp ? timeAgo(event.timestamp) : null);
  const absoluteTime = $derived(event.timestamp ? formatTimestamp(event.timestamp) : null);
</script>

<tr class="border-b border-gray-700/50 hover:bg-gray-800/30 transition-colors group">
  <td class="px-4 py-3">
    <a href={href} class="text-xs text-blue-400 hover:text-blue-300 font-mono" title="{sourceLabel} Event #{Number(event.globalIndex)}">
      {sourceLabel} #{Number(event.globalIndex)}
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
    {#if principal}
      <a href="/explorer/address/{principal}" class="hover:text-blue-400 transition-colors font-mono">
        {shortenPrincipal(principal)}
      </a>
    {:else}
      <span class="text-gray-600">&mdash;</span>
    {/if}
  </td>
  <td class="px-4 py-3">
    <span class="inline-block text-xs font-medium px-2.5 py-0.5 rounded-full whitespace-nowrap {formatted.badgeColor}">
      {formatted.typeName}
    </span>
  </td>
  <td class="px-4 py-3 text-sm text-gray-300 truncate max-w-[300px]">
    {formatted.summary}
  </td>
  <td class="px-4 py-3 text-right">
    <a
      href={href}
      class="text-xs text-blue-400 hover:text-blue-300 opacity-0 group-hover:opacity-100 transition-opacity whitespace-nowrap"
    >
      Details &rarr;
    </a>
  </td>
</tr>
