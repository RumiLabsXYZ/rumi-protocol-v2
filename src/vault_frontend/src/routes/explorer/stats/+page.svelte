<script lang="ts">
  import { goto } from '$app/navigation';
  import { onMount } from 'svelte';
  // /explorer/stats was absorbed by the Protocol page Overview lens.
  onMount(() => goto('/explorer', { replaceState: true }));
</script>
