<script lang="ts">
	import { onMount } from 'svelte';
	import EventRow from '$components/explorer/EventRow.svelte';
	import Pagination from '$components/explorer/Pagination.svelte';
	import { fetchEvents, fetchEventCount } from '$services/explorer/explorerService';
	import { getEventCategory, EVENT_CATEGORIES } from '$utils/explorerFormatters';
	import type { EventCategory } from '$utils/explorerFormatters';

	const PAGE_SIZE = 100;

	let events: [bigint, any][] = $state([]);
	let totalCount: number = $state(0);
	let currentPage: number = $state(0);
	let selectedCategory: EventCategory | 'all' = $state('all');
	let loading: boolean = $state(true);
	let error: string | null = $state(null);

	const totalPages = $derived(Math.ceil(totalCount / PAGE_SIZE));

	const filteredEvents = $derived(
		selectedCategory === 'all'
			? events
			: events.filter(([_, event]) => getEventCategory(event) === selectedCategory)
	);

	const tabs: { key: EventCategory | 'all'; label: string }[] = [
		{ key: 'all', label: 'All' },
		...EVENT_CATEGORIES.map((c) => ({ key: c.key, label: c.label }))
	];

	async function loadPage(page: number) {
		loading = true;
		error = null;
		try {
			const start = BigInt(Math.max(0, totalCount - (page + 1) * PAGE_SIZE));
			const length = BigInt(
				page === totalPages - 1
					? totalCount - (totalPages - 1) * PAGE_SIZE
					: Math.min(PAGE_SIZE, totalCount)
			);
			const result = await fetchEvents(start, length);
			// fetchEvents returns oldest-first; reverse for newest-first display
			events = [...result].reverse();
			currentPage = page;
		} catch (e) {
			error = 'Failed to load events.';
			console.error('[events page] loadPage error:', e);
		} finally {
			loading = false;
		}
	}

	function handlePageChange(page: number) {
		loadPage(page);
	}

	onMount(async () => {
		try {
			const count = await fetchEventCount();
			totalCount = Number(count);
			if (totalCount > 0) {
				await loadPage(0);
			} else {
				loading = false;
			}
		} catch (e) {
			error = 'Failed to load event count.';
			loading = false;
			console.error('[events page] onMount error:', e);
		}
	});
</script>

<div class="max-w-[1100px] mx-auto px-4 py-8">
	<div class="flex items-baseline justify-between mb-6">
		<h1 class="text-2xl font-bold text-white">Protocol Events</h1>
		{#if totalCount > 0}
			<span class="text-lg font-semibold text-gray-400">
				{totalCount.toLocaleString()} events
			</span>
		{/if}
	</div>

	<!-- Category filter tabs -->
	<div class="flex gap-0 border-b border-gray-700/50 mb-6 overflow-x-auto">
		{#each tabs as tab}
			<button
				class="px-4 py-2.5 text-sm font-medium whitespace-nowrap transition-colors
					{selectedCategory === tab.key
					? 'text-blue-400 border-b-2 border-blue-400'
					: 'text-gray-400 border-b-2 border-transparent hover:text-gray-300 hover:border-gray-600'}"
				onclick={() => (selectedCategory = tab.key)}
			>
				{tab.label}
			</button>
		{/each}
	</div>

	<!-- Loading state -->
	{#if loading}
		<div class="flex items-center justify-center py-20">
			<div class="flex flex-col items-center gap-3">
				<div
					class="w-8 h-8 border-2 border-gray-600 border-t-blue-400 rounded-full animate-spin"
				></div>
				<span class="text-sm text-gray-500">Loading events...</span>
			</div>
		</div>

	<!-- Error state -->
	{:else if error}
		<div class="text-center py-16">
			<p class="text-red-400 text-sm">{error}</p>
		</div>

	<!-- Empty state -->
	{:else if filteredEvents.length === 0}
		<div class="text-center py-16">
			<p class="text-gray-500 text-sm">
				{selectedCategory === 'all'
					? 'No events found.'
					: `No ${tabs.find((t) => t.key === selectedCategory)?.label ?? ''} events on this page.`}
			</p>
		</div>

	<!-- Events table -->
	{:else}
		<div class="bg-gray-800/30 border border-gray-700/50 rounded-xl overflow-hidden">
			<table class="w-full">
				<thead>
					<tr class="border-b border-gray-700/50 text-left">
						<th class="px-4 py-3 text-xs font-medium text-gray-500 uppercase tracking-wider w-[5rem]">#</th>
						<th class="px-4 py-3 text-xs font-medium text-gray-500 uppercase tracking-wider w-[7rem]">Time</th>
						<th class="px-4 py-3 text-xs font-medium text-gray-500 uppercase tracking-wider w-[10rem]">Type</th>
						<th class="px-4 py-3 text-xs font-medium text-gray-500 uppercase tracking-wider">Summary</th>
						<th class="px-4 py-3 text-xs font-medium text-gray-500 uppercase tracking-wider w-[5rem] text-right">Details</th>
					</tr>
				</thead>
				<tbody>
					{#each filteredEvents as [globalIndex, event] (globalIndex)}
						<EventRow {event} index={Number(globalIndex)} />
					{/each}
				</tbody>
			</table>
		</div>
	{/if}

	<!-- Pagination -->
	{#if totalPages > 1}
		<Pagination {currentPage} {totalPages} onPageChange={handlePageChange} />
	{/if}
</div>
