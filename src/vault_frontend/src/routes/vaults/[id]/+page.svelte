<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';

  // Vault detail page removed — redirect to vault list
  onMount(() => {
    goto('/vaults', { replaceState: true });
  });
</script>

<div style="text-align: center; padding: 3rem;">
  <p style="color: var(--rumi-text-muted);">Redirecting to vaults…</p>
</div>
