<script lang="ts">
  import { goto } from '$app/navigation';
  import { onMount } from 'svelte';
  // /explorer/risk was absorbed by the Protocol page Collateral lens.
  onMount(() => goto('/explorer?lens=collateral', { replaceState: true }));
</script>
