<script lang="ts">
  import EventRow from './EventRow.svelte';
  import MixedEventRow from './MixedEventRow.svelte';
  import type { DisplayEvent } from '$utils/displayEvent';
  import type { Facets } from '$utils/eventFacets';

  interface Props {
    events: DisplayEvent[];
    vaultCollateralMap?: Map<number, string>;
    vaultOwnerMap?: Map<number, string>;
    /** Header cell classes — defaults to `px-4 py-3`. Landing page uses `px-4 py-2` for a tighter look. */
    headerCellClass?: string;
    /**
     * Optional facet-click handler. When provided, facet chips in rows become
     * "add-to-filter" buttons that call this with the updated `Facets`.
     * When omitted, chips behave as plain entity links (navigate to /e/...).
     */
    onFacetClick?: (next: Facets) => void;
    currentFacets?: Facets;
  }

  let {
    events,
    vaultCollateralMap,
    vaultOwnerMap,
    headerCellClass = 'px-4 py-3',
    onFacetClick,
    currentFacets,
  }: Props = $props();
</script>

<div class="overflow-x-auto">
  <table class="w-full">
    <thead>
      <tr class="border-b border-gray-700/50 text-left">
        <th class="{headerCellClass} text-xs font-medium text-gray-500 uppercase tracking-wider w-[5rem]">#</th>
        <th class="{headerCellClass} text-xs font-medium text-gray-500 uppercase tracking-wider w-[7rem]">Time</th>
        <th class="{headerCellClass} text-xs font-medium text-gray-500 uppercase tracking-wider w-[8rem]">Principal</th>
        <th class="{headerCellClass} text-xs font-medium text-gray-500 uppercase tracking-wider w-[10rem]">Type</th>
        <th class="{headerCellClass} text-xs font-medium text-gray-500 uppercase tracking-wider">Summary</th>
        <th class="{headerCellClass} text-xs font-medium text-gray-500 uppercase tracking-wider w-[5rem] text-right">Details</th>
      </tr>
    </thead>
    <tbody>
      {#each events as de (String(de.globalIndex) + de.source)}
        {#if de.source === 'backend'}
          <EventRow
            event={de.event}
            index={Number(de.globalIndex)}
            {vaultCollateralMap}
            {vaultOwnerMap}
            {onFacetClick}
            {currentFacets}
          />
        {:else}
          <MixedEventRow
            event={de}
            {vaultOwnerMap}
            {onFacetClick}
            {currentFacets}
          />
        {/if}
      {/each}
    </tbody>
  </table>
</div>
