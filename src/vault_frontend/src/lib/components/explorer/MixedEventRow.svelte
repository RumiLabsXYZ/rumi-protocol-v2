<script lang="ts">
  import { timeAgo, shortenPrincipal, formatTimestamp, getTokenSymbol } from '$utils/explorerHelpers';
  import { displayEvent } from '$utils/displayEvent';
  import type { DisplayEvent } from '$utils/displayEvent';
  import { extractFacets, typeFacetLabel, type Facets } from '$utils/eventFacets';
  import FacetChip from './FacetChip.svelte';

  interface Props {
    event: DisplayEvent;
    vaultOwnerMap?: Map<number, string>;
    onFacetClick?: (next: Facets) => void;
    currentFacets?: Facets;
  }

  let { event, vaultOwnerMap, onFacetClick, currentFacets }: Props = $props();

  const display = $derived(displayEvent(event, { vaultOwnerMap }));
  const relativeTime = $derived(display.timestamp ? timeAgo(display.timestamp) : null);
  const absoluteTime = $derived(display.timestamp ? formatTimestamp(display.timestamp) : null);

  // Lightweight facet extraction for chip affordances in this row.
  // Skipping priceMap/vaultCollateralMap here — we only need the entity lists,
  // not size_usd.
  const facetsFor = $derived(extractFacets(event, undefined, undefined, vaultOwnerMap));

  // Secondary chips we render after the summary when onFacetClick is wired.
  const extraTokens = $derived(facetsFor.tokens.slice(0, 3));
  const extraPools = $derived(facetsFor.pools.slice(0, 2));
  const extraVaults = $derived(facetsFor.vaultIds.slice(0, 2));
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
      <FacetChip
        kind="principal"
        value={display.principal}
        label={shortenPrincipal(display.principal)}
        title={display.principal}
        class="text-xs text-gray-300 hover:text-blue-300 font-mono px-1 py-0.5"
        {onFacetClick}
        {currentFacets}
      />
    {:else}
      <span class="text-gray-600">&mdash;</span>
    {/if}
  </td>
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
  <td class="px-4 py-3 text-sm text-gray-300">
    <div class="truncate max-w-[360px]" title={display.formatted.summary}>
      {display.formatted.summary}
    </div>
    {#if onFacetClick && (extraTokens.length || extraPools.length || extraVaults.length)}
      <div class="mt-1 flex flex-wrap gap-1 text-[10px] text-gray-500">
        {#each extraTokens as p (p)}
          <FacetChip
            kind="token"
            value={p}
            label="+token:{getTokenSymbol(p)}"
            class="px-1.5 py-0.5 rounded-full border border-gray-700 bg-gray-900/60 hover:text-teal-300"
            {onFacetClick}
            {currentFacets}
          />
        {/each}
        {#each extraPools as id (id)}
          <FacetChip
            kind="pool"
            value={id}
            label="+pool:{id === '3pool' ? '3pool' : id}"
            class="px-1.5 py-0.5 rounded-full border border-gray-700 bg-gray-900/60 hover:text-teal-300"
            {onFacetClick}
            {currentFacets}
          />
        {/each}
        {#each extraVaults as v (v)}
          <FacetChip
            kind="vault"
            value={v}
            label="+vault:#{v}"
            class="px-1.5 py-0.5 rounded-full border border-gray-700 bg-gray-900/60 hover:text-teal-300"
            {onFacetClick}
            {currentFacets}
          />
        {/each}
      </div>
    {/if}
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
