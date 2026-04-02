<script lang="ts">
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	onMount(() => goto('/explorer/activity?filter=liquidations', { replaceState: true }));
</script>
