<script lang="ts">
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	// Absorbed by the Activity facet bar. `?type=liquidation` expands via
	// TYPE_CATEGORY_ALIASES to the full set of liquidation event variants.
	onMount(() => goto('/explorer/activity?type=liquidation', { replaceState: true }));
</script>
