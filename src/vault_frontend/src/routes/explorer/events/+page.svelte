<script lang="ts">
	import { onMount } from 'svelte';
	import EventRow from '$components/explorer/EventRow.svelte';
	import Pagination from '$components/explorer/Pagination.svelte';
	import { fetchEvents, fetchSwapEvents, fetchSwapEventCount, fetchStabilityPoolEvents, fetchStabilityPoolEventCount } from '$services/explorer/explorerService';
	import { getEventCategory, formatSwapEvent, formatStabilityPoolEvent, EVENT_CATEGORIES } from '$utils/explorerFormatters';
	import type { EventCategory } from '$utils/explorerFormatters';

	const PAGE_SIZE = 100;

	let events: [bigint, any][] = $state([]);
	let totalCount: number = $state(0);
	let currentPage: number = $state(0);
	let selectedCategory: EventCategory | 'all' = $state('all');
	let loading: boolean = $state(true);
	let error: string | null = $state(null);

	// Track whether we're showing canister-specific events (separate data sources)
	const isThreePool = $derived((selectedCategory as string) === 'threepool');
	const isStabilityPool = $derived((selectedCategory as string) === 'stability_pool');

	const totalPages = $derived(Math.ceil(totalCount / PAGE_SIZE));

	const filteredEvents = $derived(
		selectedCategory === 'all' || isThreePool || isStabilityPool
			? events
			: events.filter(([_, event]) => getEventCategory(event) === selectedCategory)
	);

	const tabs: { key: EventCategory | 'all'; label: string }[] = [
		{ key: 'all', label: 'All' },
		...EVENT_CATEGORIES.map((c) => ({ key: c.key, label: c.label }))
	];

	async function loadProtocolPage(page: number) {
		loading = true;
		error = null;
		try {
			const result = await fetchEvents(BigInt(page), BigInt(PAGE_SIZE));
			totalCount = Number(result.total);
			events = result.events;
			currentPage = page;
		} catch (e) {
			error = 'Failed to load events.';
			console.error('[events page] loadProtocolPage error:', e);
		} finally {
			loading = false;
		}
	}

	async function loadThreePoolPage(page: number) {
		loading = true;
		error = null;
		try {
			const count = await fetchSwapEventCount();
			totalCount = Number(count);

			// 3pool stores events oldest-first; reverse to show newest-first like protocol events
			const total = Number(count);
			const reverseStart = Math.max(0, total - (page + 1) * PAGE_SIZE);
			const length = Math.min(PAGE_SIZE, total - page * PAGE_SIZE);

			if (length <= 0) {
				events = [];
			} else {
				const swapEvents = await fetchSwapEvents(BigInt(reverseStart), BigInt(length));
				// Reverse so newest is first, wrap as [id, event] tuples
				events = swapEvents
					.slice()
					.reverse()
					.map((swap: any) => [BigInt(swap.id), swap] as [bigint, any]);
			}
			currentPage = page;
		} catch (e) {
			error = 'Failed to load 3Pool events.';
			console.error('[events page] loadThreePoolPage error:', e);
		} finally {
			loading = false;
		}
	}

	async function loadStabilityPoolPage(page: number) {
		loading = true;
		error = null;
		try {
			const count = await fetchStabilityPoolEventCount();
			totalCount = Number(count);

			// SP stores events oldest-first; reverse to show newest-first
			const total = Number(count);
			const reverseStart = Math.max(0, total - (page + 1) * PAGE_SIZE);
			const length = Math.min(PAGE_SIZE, total - page * PAGE_SIZE);

			if (length <= 0) {
				events = [];
			} else {
				const poolEvents = await fetchStabilityPoolEvents(BigInt(reverseStart), BigInt(length));
				events = poolEvents
					.slice()
					.reverse()
					.map((evt: any) => [BigInt(evt.id), evt] as [bigint, any]);
			}
			currentPage = page;
		} catch (e) {
			error = 'Failed to load Stability Pool events.';
			console.error('[events page] loadStabilityPoolPage error:', e);
		} finally {
			loading = false;
		}
	}

	function loadPage(page: number) {
		if (isThreePool) {
			loadThreePoolPage(page);
		} else if (isStabilityPool) {
			loadStabilityPoolPage(page);
		} else {
			loadProtocolPage(page);
		}
	}

	function handlePageChange(page: number) {
		loadPage(page);
	}

	function handleTabChange(key: EventCategory | 'all') {
		const wasSeparateSource = selectedCategory === 'threepool' || selectedCategory === 'stability_pool';
		const willBeSeparateSource = key === 'threepool' || key === 'stability_pool';
		const prevCategory = selectedCategory;
		selectedCategory = key;

		// Reload from page 0 when switching between different data sources
		if (wasSeparateSource !== willBeSeparateSource || (wasSeparateSource && prevCategory !== key)) {
			loadPage(0);
		}
	}

	onMount(() => {
		loadPage(0);
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
				onclick={() => handleTabChange(tab.key)}
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
						{#if isThreePool || isStabilityPool}
							{@const formatted = isThreePool ? formatSwapEvent(event) : formatStabilityPoolEvent(event)}
							<tr class="border-b border-gray-700/50 hover:bg-gray-800/30 transition-colors group">
								<td class="px-4 py-3">
									<span class="text-xs text-gray-500 font-mono">{Number(globalIndex)}</span>
								</td>
								<td class="px-4 py-3 text-xs text-gray-500 whitespace-nowrap">
									{#if event.timestamp}
										{@const ts = Number(event.timestamp) > 1e15 ? Number(event.timestamp) : Number(event.timestamp) * 1e9}
										{@const ago = (() => { const s = Math.floor((Date.now() - ts / 1e6) / 1000); if (s < 60) return `${s}s ago`; if (s < 3600) return `${Math.floor(s/60)}m ago`; if (s < 86400) return `${Math.floor(s/3600)}h ago`; return `${Math.floor(s/86400)}d ago`; })()}
										<span title={new Date(ts / 1e6).toISOString()}>{ago}</span>
									{:else}
										<span class="text-gray-600">&mdash;</span>
									{/if}
								</td>
								<td class="px-4 py-3">
									<span class="inline-block text-xs font-medium px-2.5 py-0.5 rounded-full whitespace-nowrap {formatted.badgeColor}">
										{formatted.typeName}
									</span>
								</td>
								<td class="px-4 py-3 text-sm text-gray-300 truncate max-w-[300px]">
									{formatted.summary}
								</td>
								<td class="px-4 py-3 text-right">
									<!-- No detail page for pool events yet -->
								</td>
							</tr>
						{:else}
							<EventRow {event} index={Number(globalIndex)} />
						{/if}
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
