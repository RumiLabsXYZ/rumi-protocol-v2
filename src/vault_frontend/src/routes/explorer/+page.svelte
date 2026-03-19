<script lang="ts">
	import { onMount } from 'svelte';
	import SearchBar from '$lib/components/explorer/SearchBar.svelte';
	import EventRow from '$lib/components/explorer/EventRow.svelte';
	import Pagination from '$lib/components/explorer/Pagination.svelte';
	import {
		explorerEvents, explorerEventsLoading, explorerEventsPage,
		explorerEventsTotalCount, fetchEvents, PAGE_SIZE
	} from '$lib/stores/explorerStore';
	import { getEventCategory, type EventCategory } from '$lib/utils/eventFormatters';

	let selectedFilter: EventCategory | 'all' = 'all';

	$: totalPages = Math.ceil($explorerEventsTotalCount / PAGE_SIZE);

	// Events already come as {event, globalIndex} from the store (server-side filtered, no AccrueInterest)
	$: filteredEvents = selectedFilter === 'all'
		? $explorerEvents
		: $explorerEvents.filter((e: any) => getEventCategory(e.event) === selectedFilter);

	function handlePageChange(page: number) {
		fetchEvents(page);
	}

	const filters: { label: string; value: EventCategory | 'all' }[] = [
		{ label: 'All', value: 'all' },
		{ label: 'Vault Ops', value: 'vault' },
		{ label: 'Liquidations', value: 'liquidation' },
		{ label: 'Stability Pool', value: 'stability' },
		{ label: 'Redemptions', value: 'redemption' },
		{ label: 'Admin', value: 'admin' },
	];

	onMount(() => { fetchEvents(0); });
</script>

<div class="explorer-page">
	<h1 class="page-title">Protocol Explorer</h1>

	<div class="search-row">
		<SearchBar />
	</div>

	<div class="stats-row">
		<div class="stat glass-card">
			<span class="stat-label">Total Events</span>
			<span class="stat-value key-number">{$explorerEventsTotalCount.toLocaleString()}</span>
		</div>
	</div>

	<div class="filter-row">
		{#each filters as f}
			<button
				class="filter-btn"
				class:active={selectedFilter === f.value}
				on:click={() => selectedFilter = f.value}
			>{f.label}</button>
		{/each}
	</div>

	{#if $explorerEventsLoading}
		<div class="loading">Loading events…</div>
	{:else if filteredEvents.length === 0}
		<div class="empty">No events found.</div>
	{:else}
		<div class="events-list glass-card">
			{#each filteredEvents as { event, globalIndex }}
				<EventRow {event} index={globalIndex} />
			{/each}
		</div>
	{/if}

	<Pagination currentPage={$explorerEventsPage} {totalPages} onPageChange={handlePageChange} />
</div>

<style>
	.explorer-page { max-width:900px; margin:0 auto; padding:2rem 1rem; }
	.search-row { margin-bottom:1.5rem; display:flex; justify-content:center; }
	.stats-row { display:flex; gap:1rem; margin-bottom:1rem; }
	.stat { padding:0.75rem 1rem; text-align:center; }
	.stat-label { display:block; font-size:0.75rem; color:var(--rumi-text-muted); margin-bottom:0.25rem; }
	.stat-value { font-size:1.25rem; font-weight:600; }
	.filter-row { display:flex; gap:0.375rem; margin-bottom:1rem; flex-wrap:wrap; }
	.filter-btn {
		padding:0.375rem 0.75rem; font-size:0.8125rem; border:1px solid var(--rumi-border);
		border-radius:9999px; background:transparent; color:var(--rumi-text-secondary);
		cursor:pointer; transition:all 0.15s;
	}
	.filter-btn:hover { border-color:var(--rumi-border-hover); }
	.filter-btn.active { background:var(--rumi-purple-accent); color:white; border-color:var(--rumi-purple-accent); }
	.events-list { padding:0; overflow:hidden; }
	.loading, .empty { text-align:center; padding:3rem; color:var(--rumi-text-muted); }
</style>
