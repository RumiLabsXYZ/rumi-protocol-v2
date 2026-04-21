<script lang="ts">
  import EventRow from './EventRow.svelte';
  import MixedEventRow from './MixedEventRow.svelte';
  import type { DisplayEvent } from '$utils/displayEvent';

  interface Props {
    events: DisplayEvent[];
    vaultCollateralMap?: Map<number, string>;
    vaultOwnerMap?: Map<number, string>;
    /** Header cell classes — defaults to `px-4 py-3`. Landing page uses `px-4 py-2` for a tighter look. */
    headerCellClass?: string;
  }

  let { events, vaultCollateralMap, vaultOwnerMap, headerCellClass = 'px-4 py-3' }: Props = $props();
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
          <EventRow event={de.event} index={Number(de.globalIndex)} {vaultCollateralMap} {vaultOwnerMap} />
        {:else}
          <MixedEventRow event={de} {vaultOwnerMap} />
        {/if}
      {/each}
    </tbody>
  </table>
</div>
