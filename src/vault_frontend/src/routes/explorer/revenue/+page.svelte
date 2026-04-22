<script lang="ts">
  import { goto } from '$app/navigation';
  import { onMount } from 'svelte';
  // /explorer/revenue was absorbed by the Protocol page Revenue lens.
  onMount(() => goto('/explorer?lens=revenue', { replaceState: true }));
</script>
