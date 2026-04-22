<script lang="ts">
  import { goto } from '$app/navigation';
  import { onMount } from 'svelte';
  // /explorer/pools was absorbed by the Protocol page DEXs lens.
  onMount(() => goto('/explorer?lens=dexs', { replaceState: true }));
</script>
