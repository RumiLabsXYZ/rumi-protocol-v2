<script lang="ts">
  import { formatEvent } from '$utils/explorerFormatters';
  import EntityLink from './EntityLink.svelte';
  import { timeAgo, shortenPrincipal } from '$utils/explorerHelpers';
  import { getEventTimestamp } from '$utils/eventFormatters';

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

  const formatted = $derived(formatEvent(event, vaultCollateralMap));

  const timestamp = $derived.by(() => {
    const ts = getEventTimestamp(event);
    return ts ? timeAgo(ts) : null;
  });

  function extractPrincipal(event: any): string | null {
    // Backend events wrap data in event_type variant
    const eventType = event.event_type ?? event;
    const variant = Object.keys(eventType)[0];
    if (!variant) return null;
    const data = eventType[variant];
    if (!data) return null;

    // Check common principal fields
    for (const key of ['owner', 'caller', 'from', 'liquidator', 'redeemer']) {
      const val = data[key];
      if (val && typeof val === 'object' && typeof val.toText === 'function') {
        return val.toText();
      }
      if (typeof val === 'string' && val.length > 20) {
        return val;
      }
    }

    // Check nested vault owner
    if (data.vault?.owner) {
      const owner = data.vault.owner;
      if (typeof owner === 'object' && typeof owner.toText === 'function') return owner.toText();
    }

    // Fall back to vault owner map for events that reference a vault_id
    if (vaultOwnerMap && data.vault_id != null) {
      const owner = vaultOwnerMap.get(Number(data.vault_id));
      if (owner) return owner;
    }

    return null;
  }

  const principal = $derived(extractPrincipal(event));
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

  <!-- Principal -->
  <td class="px-4 py-3 text-xs text-gray-400 whitespace-nowrap">
    {#if principal}
      <a href="/explorer/address/{principal}" class="hover:text-blue-400 transition-colors font-mono">
        {shortenPrincipal(principal)}
      </a>
    {:else}
      <span class="text-gray-600">&mdash;</span>
    {/if}
  </td>

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
