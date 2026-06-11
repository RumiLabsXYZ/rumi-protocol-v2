<script lang="ts">
  import { goto } from '$app/navigation';
  import { onMount } from 'svelte';
  // /explorer/liquidations was absorbed by the Protocol page Liquidations lens
  // (same consolidation as /markets, /pools, /revenue, /risk, /stats).
  onMount(() => goto('/explorer?lens=liquidations', { replaceState: true }));
</script>
