<script lang="ts">
  import { POINTS_ENABLED } from '$lib/config';
  import CurrentSeasonStatus from './CurrentSeasonStatus.svelte';
</script>

{#if POINTS_ENABLED}<CurrentSeasonStatus />{/if}
