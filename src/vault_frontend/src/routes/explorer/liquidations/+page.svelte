<script lang="ts">
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	// TODO(Step 4): point at /explorer/activity?type=liquidation once the facet
	// bar lands. For now the activity page honours `?filter=liquidations`, so
	// this keeps old URLs working without breaking.
	onMount(() => goto('/explorer/activity?filter=liquidations', { replaceState: true }));
</script>
