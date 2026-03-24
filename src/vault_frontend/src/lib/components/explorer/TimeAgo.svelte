<script lang="ts">
  import { timeAgo, formatTimestamp } from '$utils/explorerHelpers';

  interface Props {
    timestamp: bigint | number;
    showFull?: boolean;
  }

  let { timestamp, showFull = false }: Props = $props();

  let relative = $derived(timeAgo(timestamp));
  let full = $derived(formatTimestamp(timestamp));
</script>

{#if showFull}
  <span class="text-gray-400 text-sm">{full}</span>
{:else}
  <span class="text-gray-400 text-sm" title={full}>{relative}</span>
{/if}
