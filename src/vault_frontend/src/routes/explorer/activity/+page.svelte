<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/stores';
	import EventRow from '$components/explorer/EventRow.svelte';
	import Pagination from '$components/explorer/Pagination.svelte';
	import StatCard from '$components/explorer/StatCard.svelte';
	import {
		fetchEvents, fetchEventCount,
		fetchSwapEvents, fetchSwapEventCount,
		fetchAmmSwapEvents, fetchAmmSwapEventCount,
		fetchStabilityPoolEvents, fetchStabilityPoolEventCount,
		fetchLiquidatableVaults, fetchBotStats
	} from '$services/explorer/explorerService';
	import {
		getEventCategory, formatSwapEvent, formatAmmSwapEvent,
		formatStabilityPoolEvent
	} from '$utils/explorerFormatters';
	import { formatE8s, timeAgo, shortenPrincipal } from '$utils/explorerHelpers';

	const PAGE_SIZE = 100;

	type ActivityFilter = 'all' | 'vault_ops' | 'liquidations' | 'dex' | 'stability_pool' | 'governance';

	const FILTERS: { key: ActivityFilter; label: string }[] = [
		{ key: 'all', label: 'All' },
		{ key: 'vault_ops', label: 'Vault Operations' },
		{ key: 'liquidations', label: 'Liquidations' },
		{ key: 'dex', label: 'DEX' },
		{ key: 'stability_pool', label: 'Stability Pool' },
		{ key: 'governance', label: 'Governance' },
	];

	let events: [bigint, any][] = $state([]);
	let totalCount: number = $state(0);
	let currentPage: number = $state(0);
	let selectedFilter: ActivityFilter = $state('all');
	let loading: boolean = $state(true);
	let error: string | null = $state(null);

	// Liquidation summary (shown when filter === 'liquidations')
	let liquidationEventCount: number = $state(0);
	let liquidatableVaults: any[] = $state([]);
	let botStats: any = $state(null);

	// Data source type for the current filter
	const isDex = $derived(selectedFilter === 'dex');
	const isStabilityPool = $derived(selectedFilter === 'stability_pool');
	const isProtocolFilter = $derived(!isDex && !isStabilityPool);

	const totalPages = $derived(Math.ceil(totalCount / PAGE_SIZE));

	// For protocol events, apply client-side category filter
	const filteredEvents = $derived.by(() => {
		if (!isProtocolFilter || selectedFilter === 'all') return events;
		return events.filter(([_, event]) => {
			const category = getEventCategory(event);
			if (selectedFilter === 'vault_ops') return category === 'vault_ops';
			if (selectedFilter === 'liquidations') return category === 'liquidation' || category === 'redemption';
			if (selectedFilter === 'governance') return category === 'admin' || category === 'system';
			return true;
		});
	});

	// ── Load functions ──

	async function loadProtocolPage(p: number) {
		loading = true;
		error = null;
		try {
			const result = await fetchEvents(BigInt(p), BigInt(PAGE_SIZE));
			totalCount = Number(result.total);
			events = result.events;
			currentPage = p;
		} catch (e) {
			error = 'Failed to load events.';
			console.error('[activity] loadProtocolPage error:', e);
		} finally {
			loading = false;
		}
	}

	async function loadDexPage(p: number) {
		loading = true;
		error = null;
		try {
			// Fetch both 3Pool and AMM swap events and merge
			const [threePoolCount, ammCount] = await Promise.all([
				fetchSwapEventCount(),
				fetchAmmSwapEventCount(),
			]);
			const total3Pool = Number(threePoolCount);
			const totalAmm = Number(ammCount);
			totalCount = total3Pool + totalAmm;

			// Load all from both (they're typically small datasets)
			const [threePoolEvents, ammEvents] = await Promise.all([
				total3Pool > 0 ? fetchSwapEvents(0n, BigInt(total3Pool)) : Promise.resolve([]),
				totalAmm > 0 ? fetchAmmSwapEvents(0n, BigInt(totalAmm)) : Promise.resolve([]),
			]);

			// Tag events with source and merge
			const tagged3Pool = threePoolEvents.map((e: any) => [BigInt(e.id ?? 0), { ...e, _source: '3pool' }] as [bigint, any]);
			const taggedAmm = ammEvents.map((e: any) => [BigInt(e.id ?? 0), { ...e, _source: 'amm' }] as [bigint, any]);
			const merged = [...tagged3Pool, ...taggedAmm];

			// Sort by timestamp descending
			merged.sort((a, b) => {
				const tsA = Number(a[1].timestamp ?? 0);
				const tsB = Number(b[1].timestamp ?? 0);
				return tsB - tsA;
			});

			// Paginate client-side
			const start = p * PAGE_SIZE;
			events = merged.slice(start, start + PAGE_SIZE);
			currentPage = p;
		} catch (e) {
			error = 'Failed to load DEX events.';
			console.error('[activity] loadDexPage error:', e);
		} finally {
			loading = false;
		}
	}

	async function loadStabilityPoolPage(p: number) {
		loading = true;
		error = null;
		try {
			const count = await fetchStabilityPoolEventCount();
			totalCount = Number(count);

			const total = Number(count);
			const reverseStart = Math.max(0, total - (p + 1) * PAGE_SIZE);
			const length = Math.min(PAGE_SIZE, total - p * PAGE_SIZE);

			if (length <= 0) {
				events = [];
			} else {
				const poolEvents = await fetchStabilityPoolEvents(BigInt(reverseStart), BigInt(length));
				events = poolEvents
					.slice()
					.reverse()
					.map((evt: any) => [BigInt(evt.id), evt] as [bigint, any]);
			}
			currentPage = p;
		} catch (e) {
			error = 'Failed to load Stability Pool events.';
			console.error('[activity] loadStabilityPoolPage error:', e);
		} finally {
			loading = false;
		}
	}

	async function loadLiquidationStats() {
		try {
			const [liqVaults, stats] = await Promise.all([
				fetchLiquidatableVaults(),
				fetchBotStats(),
			]);
			liquidatableVaults = liqVaults;
			botStats = stats;
		} catch (e) {
			console.error('[activity] loadLiquidationStats error:', e);
		}
	}

	function loadPage(p: number) {
		if (isDex) loadDexPage(p);
		else if (isStabilityPool) loadStabilityPoolPage(p);
		else loadProtocolPage(p);
	}

	function handleFilterChange(key: ActivityFilter) {
		const prevFilter = selectedFilter;
		selectedFilter = key;

		// Load liquidation stats when switching to liquidations filter
		if (key === 'liquidations') loadLiquidationStats();

		// Reload from page 0 when switching data sources
		const prevSource = prevFilter === 'dex' ? 'dex' : prevFilter === 'stability_pool' ? 'sp' : 'protocol';
		const newSource = key === 'dex' ? 'dex' : key === 'stability_pool' ? 'sp' : 'protocol';
		if (prevSource !== newSource) {
			loadPage(0);
		}
	}

	// Extract principal from a swap event
	function getSwapCaller(event: any): string | null {
		const caller = event.caller;
		if (!caller) return null;
		if (typeof caller === 'object' && typeof caller.toText === 'function') return caller.toText();
		if (typeof caller === 'string') return caller;
		return null;
	}

	// Check URL params for initial filter
	onMount(() => {
		const urlFilter = $page.url.searchParams.get('filter');
		if (urlFilter && FILTERS.some(f => f.key === urlFilter)) {
			selectedFilter = urlFilter as ActivityFilter;
			if (urlFilter === 'liquidations') loadLiquidationStats();
		}
		loadPage(0);
	});
</script>

<svelte:head>
	<title>Activity | Rumi Explorer</title>
</svelte:head>

<div class="max-w-[1100px] mx-auto px-4 py-8">
	<div class="flex items-baseline justify-between mb-6">
		<h1 class="text-2xl font-bold text-white">Protocol Activity</h1>
		{#if totalCount > 0}
			<span class="text-lg font-semibold text-gray-400">
				{totalCount.toLocaleString()} events
			</span>
		{/if}
	</div>

	<!-- Filter pills -->
	<div class="flex flex-wrap gap-2 mb-6">
		{#each FILTERS as filter}
			<button
				class="px-4 py-1.5 text-sm font-medium rounded-full border transition-all
					{selectedFilter === filter.key
					? 'bg-indigo-500/20 text-indigo-300 border-indigo-500/40'
					: 'bg-transparent text-gray-400 border-gray-700 hover:border-gray-500 hover:text-gray-300'}"
				onclick={() => handleFilterChange(filter.key)}
			>
				{filter.label}
			</button>
		{/each}
	</div>

	<!-- Liquidation summary cards (shown only when filtering to liquidations) -->
	{#if selectedFilter === 'liquidations'}
		<div class="grid grid-cols-2 md:grid-cols-4 gap-3 mb-6">
			<StatCard
				label="Liquidation Events"
				value={String(liquidationEventCount || '—')}
			/>
			<StatCard
				label="Liquidatable Vaults"
				value={String(liquidatableVaults.length)}
				subtitle={liquidatableVaults.length === 0 ? 'None at risk' : ''}
			/>
			<StatCard
				label="Bot Budget Remaining"
				value={botStats ? `${formatE8s(botStats.budget_remaining)} ICP` : '—'}
				subtitle="Available for bot"
			/>
			<StatCard
				label="Bot Debt Covered"
				value={botStats ? String(Number(botStats.total_debt_covered ?? 0)) : '—'}
				subtitle="Total debt covered by bot"
			/>
		</div>
	{/if}

	<!-- Loading state -->
	{#if loading}
		<div class="flex items-center justify-center py-20">
			<div class="flex flex-col items-center gap-3">
				<div class="w-8 h-8 border-2 border-gray-600 border-t-blue-400 rounded-full animate-spin"></div>
				<span class="text-sm text-gray-500">Loading activity...</span>
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
				{selectedFilter === 'all' ? 'No events found.' : `No ${FILTERS.find(f => f.key === selectedFilter)?.label ?? ''} events found.`}
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
						<th class="px-4 py-3 text-xs font-medium text-gray-500 uppercase tracking-wider w-[8rem]">Who</th>
						<th class="px-4 py-3 text-xs font-medium text-gray-500 uppercase tracking-wider w-[10rem]">Type</th>
						<th class="px-4 py-3 text-xs font-medium text-gray-500 uppercase tracking-wider">Summary</th>
						<th class="px-4 py-3 text-xs font-medium text-gray-500 uppercase tracking-wider w-[5rem] text-right">Details</th>
					</tr>
				</thead>
				<tbody>
					{#each filteredEvents as [globalIndex, event] (String(globalIndex) + (event._source ?? ''))}
						{#if isDex}
							{@const formatted = event._source === 'amm' ? formatAmmSwapEvent(event) : formatSwapEvent(event)}
							{@const caller = getSwapCaller(event)}
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
								<td class="px-4 py-3 text-xs text-gray-400 whitespace-nowrap">
									{#if caller}
										<a href="/explorer/address/{caller}" class="hover:text-blue-400 transition-colors font-mono">
											{shortenPrincipal(caller)}
										</a>
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
						{:else if isStabilityPool}
							{@const formatted = formatStabilityPoolEvent(event)}
							{@const caller = getSwapCaller(event)}
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
								<td class="px-4 py-3 text-xs text-gray-400 whitespace-nowrap">
									{#if caller}
										<a href="/explorer/address/{caller}" class="hover:text-blue-400 transition-colors font-mono">
											{shortenPrincipal(caller)}
										</a>
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
								<td class="px-4 py-3 text-right"></td>
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
		<Pagination {currentPage} {totalPages} onPageChange={(p) => loadPage(p)} />
	{/if}
</div>
