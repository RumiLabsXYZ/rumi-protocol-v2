<script lang="ts">
  import EntityLink from './EntityLink.svelte';
  import { timeAgo, shortenPrincipal, formatTimestamp } from '$utils/explorerHelpers';
  import { displayEvent, wrapBackendEvent } from '$utils/displayEvent';

  interface Props {
    event: any;
    index: number | null;
    showTimestamp?: boolean;
    vaultCollateralMap?: Map<number, string>;
    vaultOwnerMap?: Map<number, string>;
    [key: string]: any;
  }

  let { event, index, showTimestamp = true, vaultCollateralMap, vaultOwnerMap, ...rest }: Props = $props();

  const hasIndex = $derived(index != null && index >= 0);

  const display = $derived(
    displayEvent(wrapBackendEvent(event, index ?? 0), { vaultCollateralMap, vaultOwnerMap }),
  );

  const relativeTime = $derived(display.timestamp ? timeAgo(display.timestamp) : null);
  const absoluteTime = $derived(display.timestamp ? formatTimestamp(display.timestamp) : null);
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
      {#if relativeTime}
        <span title={absoluteTime ?? ''}>{relativeTime}</span>
      {:else}
        <span class="text-gray-600">&mdash;</span>
      {/if}
    </td>
  {/if}

  <!-- Principal -->
  <td class="px-4 py-3 text-xs text-gray-400 whitespace-nowrap">
    {#if display.principal}
      <a href="/explorer/address/{display.principal}" class="hover:text-blue-400 transition-colors font-mono">
        {shortenPrincipal(display.principal)}
      </a>
    {:else}
      <span class="text-gray-600">&mdash;</span>
    {/if}
  </td>

  <!-- Type badge -->
  <td class="px-4 py-3">
    <span
      class="inline-block text-xs font-medium px-2.5 py-0.5 rounded-full whitespace-nowrap {display.formatted.badgeColor}"
    >
      {display.formatted.typeName}
    </span>
  </td>

  <!-- Summary -->
  <td class="px-4 py-3 text-sm text-gray-300 truncate max-w-[300px]">
    {display.formatted.summary}
  </td>

  <!-- Details link -->
  <td class="px-4 py-3 text-right">
    {#if hasIndex}
      <a
        href={display.detailHref}
        class="text-xs text-blue-400 hover:text-blue-300 opacity-0 group-hover:opacity-100 transition-opacity whitespace-nowrap"
      >
        Details &rarr;
      </a>
    {/if}
  </td>
</tr>
