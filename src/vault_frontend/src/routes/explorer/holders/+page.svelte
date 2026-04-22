<script lang="ts">
  import { goto } from '$app/navigation';
  import { onMount } from 'svelte';
  // /explorer/holders will live on per-token entity pages (Step 3) plus
  // Activity 'holders' view (Step 4). For now redirect to Protocol Overview.
  onMount(() => goto('/explorer', { replaceState: true }));
</script>
