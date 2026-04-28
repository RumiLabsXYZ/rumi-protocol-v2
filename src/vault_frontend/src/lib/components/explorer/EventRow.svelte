<script lang="ts">
  import EntityLink from './EntityLink.svelte';
  import FacetChip from './FacetChip.svelte';
  import { timeAgo, formatTimestamp } from '$utils/explorerHelpers';
  import { displayEvent, wrapBackendEvent } from '$utils/displayEvent';
  import { extractFacets, typeFacetLabel, type Facets } from '$utils/eventFacets';

  interface Props {
    event: any;
    index: number | null;
    showTimestamp?: boolean;
    vaultCollateralMap?: Map<number, string>;
    vaultOwnerMap?: Map<number, string>;
    onFacetClick?: (next: Facets) => void;
    currentFacets?: Facets;
    [key: string]: any;
  }

  let {
    event, index, showTimestamp = true,
    vaultCollateralMap, vaultOwnerMap,
    onFacetClick, currentFacets,
    ...rest
  }: Props = $props();

  const hasIndex = $derived(index != null && index >= 0);

  const wrappedEvent = $derived(wrapBackendEvent(event, index ?? 0));
  const display = $derived(
    displayEvent(wrappedEvent, { vaultCollateralMap, vaultOwnerMap }),
  );

  const relativeTime = $derived(display.timestamp ? timeAgo(display.timestamp) : null);
  const absoluteTime = $derived(display.timestamp ? formatTimestamp(display.timestamp) : null);

  const facetsFor = $derived(extractFacets(wrappedEvent, undefined, vaultCollateralMap, vaultOwnerMap));

  const extraTokens = $derived(facetsFor.tokens.slice(0, 3));
  const extraVaults = $derived(facetsFor.vaultIds.slice(0, 2));
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
      <EntityLink
        type="address"
        value={display.principal}
        class="inline-flex items-center gap-1 text-xs text-gray-300 hover:text-blue-300 font-mono px-1 py-0.5"
      />
    {:else}
      <span class="text-gray-600">&mdash;</span>
    {/if}
  </td>

  <!-- Type badge -->
  <td class="px-4 py-3">
    <FacetChip
      kind="type"
      value={facetsFor.typeKey}
      label={display.formatted.typeName}
      title="Filter by type: {typeFacetLabel(facetsFor.typeKey)}"
      class="inline-block text-xs font-medium px-2.5 py-0.5 rounded-full whitespace-nowrap {display.formatted.badgeColor}"
      {onFacetClick}
      {currentFacets}
    />
  </td>

  <!-- Summary -->
  <td class="px-4 py-3 text-sm text-gray-300">
    <div class="truncate max-w-[360px]" title={display.formatted.summary}>
      {display.formatted.summary}
    </div>
    {#if extraTokens.length || extraVaults.length}
      <div class="mt-1 flex flex-wrap gap-1 text-[10px] text-gray-500">
        {#each extraTokens as p (p)}
          <EntityLink
            type="token"
            value={p}
            class="inline-flex items-center gap-1 px-1.5 py-0.5 rounded-full border border-gray-700 bg-gray-900/60 text-[10px] text-gray-400 hover:text-teal-300 font-mono"
          />
        {/each}
        {#each extraVaults as v (v)}
          <EntityLink
            type="vault"
            value={String(v)}
            label={`#${v}`}
            class="inline-flex items-center gap-1 px-1.5 py-0.5 rounded-full border border-gray-700 bg-gray-900/60 text-[10px] text-gray-400 hover:text-teal-300 font-mono"
          />
        {/each}
      </div>
    {/if}
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
