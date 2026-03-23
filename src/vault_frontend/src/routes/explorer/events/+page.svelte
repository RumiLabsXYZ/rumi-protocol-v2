<script lang="ts">
	import { onMount } from 'svelte';
	import DataTable from '$lib/components/explorer/DataTable.svelte';
	import DashboardCard from '$lib/components/explorer/DashboardCard.svelte';
	import EntityLink from '$lib/components/explorer/EntityLink.svelte';
	import TokenBadge from '$lib/components/explorer/TokenBadge.svelte';
	import Pagination from '$lib/components/explorer/Pagination.svelte';
	import {
		explorerEvents, explorerEventsLoading, explorerEventsPage,
		explorerEventsTotalCount, fetchEvents, fetchAllVaults, PAGE_SIZE,
		poolEvents, poolEventsLoading, fetchPoolEvents
	} from '$lib/stores/explorerStore';
	import {
		getEventCategory, getEventType, getEventBadgeColor, getEventSummary,
		getEventKey, getEventVaultId, getEventCaller, getEventTimestamp,
		formatTimestamp, resolveCollateralSymbol,
		getPoolEventType, getPoolEventBadgeColor, getPoolEventSummary, getPoolEventCaller,
		type EventCategory
	} from '$lib/utils/eventFormatters';
	import { truncatePrincipal } from '$lib/utils/principalHelpers';

	type ExplorerFilter = EventCategory | 'all' | '3pool';

	let selectedFilter: ExplorerFilter = $state('all');
	let searchQuery = $state('');
	let vaultCollateralMap: Map<number, any> = $state(new Map());

	const totalPages = $derived(Math.ceil($explorerEventsTotalCount / PAGE_SIZE));

	// Category filter (client-side on fetched page)
	const categoryFiltered = $derived(
		selectedFilter === 'all'
			? $explorerEvents
			: selectedFilter === '3pool'
			? []
			: $explorerEvents.filter((e: any) => getEventCategory(e.event) === selectedFilter)
	);

	// Pool events filtered by category
	const filteredPoolEvents = $derived((() => {
		const filter = selectedFilter as string;
		if (filter === '3pool') {
			return $poolEvents.filter(e => e.source === '3pool_swap' || e.source === '3pool_lp');
		}
		if (filter === 'stability') {
			return $poolEvents.filter(e => e.source === 'stability_pool');
		}
		if (filter === 'all') {
			return $poolEvents;
		}
		return [];
	})());

	// Search filter on top of category filter
	const filteredEvents = $derived((() => {
		if (!searchQuery.trim()) return categoryFiltered;
		const q = searchQuery.toLowerCase();
		return categoryFiltered.filter((e: any) => {
			const summary = getEventSummary(e.event, vaultCollateralMap).toLowerCase();
			const caller = getEventCaller(e.event)?.toLowerCase() ?? '';
			const vaultId = getEventVaultId(e.event);
			const vaultStr = vaultId !== null ? `#${vaultId}` : '';
			const type = getEventType(e.event).toLowerCase();
			return summary.includes(q) || caller.includes(q) || vaultStr.includes(q) || type.includes(q);
		});
	})());

	// Category counts for the current page of events
	const categoryCounts = $derived((() => {
		const counts: Record<string, number> = { all: $explorerEvents.length, vault: 0, liquidation: 0, stability: 0, redemption: 0, admin: 0, '3pool': $poolEvents.filter(e => e.source === '3pool_swap' || e.source === '3pool_lp').length };
		for (const e of $explorerEvents) {
			const cat = getEventCategory(e.event);
			counts[cat] = (counts[cat] || 0) + 1;
		}
		counts.stability += $poolEvents.filter(e => e.source === 'stability_pool').length;
		return counts;
	})());

	// Relative time formatter
	function relativeTime(nanos: bigint | number): string {
		const ms = Number(BigInt(nanos) / BigInt(1_000_000));
		const diff = Date.now() - ms;
		if (diff < 60_000) return 'just now';
		if (diff < 3_600_000) return `${Math.floor(diff / 60_000)}m ago`;
		if (diff < 86_400_000) return `${Math.floor(diff / 3_600_000)}h ago`;
		if (diff < 2_592_000_000) return `${Math.floor(diff / 86_400_000)}d ago`;
		const date = new Date(ms);
		return date.toLocaleDateString('en-US', { month: 'short', day: 'numeric' });
	}

	// Extract collateral principal from an event for TokenBadge display
	function getEventCollateral(event: any): { symbol: string; principalId: string } | null {
		const key = getEventKey(event);
		const data = event[key];
		if (!data) return null;
		const ct = data.collateral_type ?? data.vault?.collateral_type;
		if (ct) {
			const text = ct?.toString?.() ?? ct?.toText?.() ?? String(ct);
			return { symbol: resolveCollateralSymbol(ct), principalId: text };
		}
		// Look up from vault map
		const vaultId = getEventVaultId(event);
		if (vaultId !== null && vaultCollateralMap.has(vaultId)) {
			const mapped = vaultCollateralMap.get(vaultId);
			const text = mapped?.toString?.() ?? String(mapped);
			return { symbol: resolveCollateralSymbol(mapped), principalId: text };
		}
		return null;
	}

	const filters: { label: string; value: ExplorerFilter }[] = [
		{ label: 'All', value: 'all' },
		{ label: 'Vault Ops', value: 'vault' },
		{ label: 'Liquidations', value: 'liquidation' },
		{ label: 'Stability Pool', value: 'stability' },
		{ label: '3Pool', value: '3pool' },
		{ label: 'Redemptions', value: 'redemption' },
		{ label: 'Admin', value: 'admin' },
	];

	const columns = [
		{ key: 'index', label: '#', width: '5rem', align: 'left' as const },
		{ key: 'time', label: 'Time', width: '6.5rem', align: 'left' as const },
		{ key: 'type', label: 'Type', width: '10rem', align: 'left' as const },
		{ key: 'summary', label: 'Summary', align: 'left' as const },
		{ key: 'caller', label: 'Caller', width: '8rem', align: 'right' as const },
	];

	function handlePageChange(page: number) {
		fetchEvents(page);
	}

	onMount(async () => {
		fetchEvents(0);
		fetchPoolEvents();
		const vaults = await fetchAllVaults();
		const map = new Map<number, any>();
		for (const v of vaults) {
			map.set(Number(v.vault_id), v.collateral_type);
		}
		vaultCollateralMap = map;
	});
</script>

<div class="max-w-[1100px] mx-auto px-4 py-8">
	<h1 class="text-2xl font-bold text-white mb-6">Protocol Events</h1>

	<!-- Stats -->
	<div class="grid grid-cols-2 sm:grid-cols-4 gap-3 mb-6">
		<DashboardCard label="Total Events" value={$explorerEventsTotalCount.toLocaleString()} />
		<DashboardCard label="Vault Ops" value={String(categoryCounts.vault)} subtitle="this page" />
		<DashboardCard label="Liquidations" value={String(categoryCounts.liquidation)} subtitle="this page" />
		<DashboardCard label="Pool Activity" value={String((categoryCounts.stability || 0) + (categoryCounts['3pool'] || 0))} subtitle="this page" />
	</div>

	<!-- Search + Filters -->
	<div class="flex flex-col sm:flex-row gap-3 mb-4">
		<input
			type="text"
			placeholder="Search events by summary, caller, vault ID, or type..."
			bind:value={searchQuery}
			class="flex-1 px-3 py-2 text-sm bg-gray-800/50 border border-gray-700/50 rounded-lg text-gray-200 placeholder-gray-500 focus:outline-none focus:border-blue-500/50"
		/>
	</div>

	<div class="flex gap-1.5 mb-4 flex-wrap">
		{#each filters as f}
			<button
				class="px-3 py-1.5 text-xs font-medium rounded-full border transition-all
					{selectedFilter === f.value
						? 'bg-blue-500/20 text-blue-400 border-blue-500/40'
						: 'bg-transparent text-gray-400 border-gray-700/50 hover:border-gray-600/50 hover:text-gray-300'}"
				onclick={() => selectedFilter = f.value}
			>
				{f.label}
				{#if categoryCounts[f.value] !== undefined}
					<span class="ml-1 opacity-60">({categoryCounts[f.value]})</span>
				{/if}
			</button>
		{/each}
	</div>

	<!-- Events Table -->
	<div class="bg-gray-800/30 border border-gray-700/50 rounded-xl overflow-hidden">
		<DataTable {columns} rows={filteredEvents} loading={$explorerEventsLoading} emptyMessage="No events found.">
			{#snippet row(item, _i)}
				{@const event = item.event}
				{@const globalIndex = item.globalIndex}
				{@const ts = getEventTimestamp(event)}
				{@const badgeColor = getEventBadgeColor(event)}
				{@const vaultId = getEventVaultId(event)}
				{@const caller = getEventCaller(event)}
				{@const collateral = getEventCollateral(event)}
				<tr class="border-b border-gray-700/30 hover:bg-gray-700/20 transition-colors">
					<td class="px-4 py-3 text-sm">
						<EntityLink type="event" id={globalIndex} label="#{globalIndex}" />
					</td>
					<td class="px-4 py-3 text-xs text-gray-400" title={ts ? formatTimestamp(ts) : ''}>
						{ts ? relativeTime(ts) : '--'}
					</td>
					<td class="px-4 py-3">
						<span
							class="inline-block text-xs font-medium px-2.5 py-0.5 rounded-full"
							style="background:{badgeColor}15; color:{badgeColor}; border:1px solid {badgeColor}30;"
						>
							{getEventType(event)}
						</span>
					</td>
					<td class="px-4 py-3 text-sm text-gray-300 max-w-[400px]">
						<span class="flex items-center gap-1.5 flex-wrap">
							{#if vaultId !== null}
								<EntityLink type="vault" id={vaultId} label="Vault #{vaultId}" />
								<span class="text-gray-500">-</span>
							{/if}
							{#if collateral}
								<TokenBadge symbol={collateral.symbol} principalId={collateral.principalId} size="sm" />
							{/if}
							<span class="truncate text-gray-400">{getEventSummary(event, vaultCollateralMap)}</span>
						</span>
					</td>
					<td class="px-4 py-3 text-right">
						{#if caller}
							<EntityLink type="address" id={caller} />
						{:else}
							<span class="text-gray-600 text-xs">--</span>
						{/if}
					</td>
				</tr>
			{/snippet}
		</DataTable>
	</div>

	<!-- Pool Events Section -->
	{#if filteredPoolEvents.length > 0}
		<h2 class="text-lg font-semibold text-white mt-8 mb-3">Pool Activity</h2>
		<div class="bg-gray-800/30 border border-gray-700/50 rounded-xl overflow-hidden">
			<DataTable columns={columns} rows={filteredPoolEvents} loading={$poolEventsLoading} emptyMessage="No pool events found.">
				{#snippet row(item, _i)}
					{@const poolSource = item.source}
					{@const evt = item.event}
					{@const badgeColor = getPoolEventBadgeColor(poolSource)}
					{@const caller = getPoolEventCaller(poolSource, evt)}
					{@const ts = evt?.timestamp}
					<tr class="border-b border-gray-700/30 hover:bg-gray-700/20 transition-colors">
						<td class="px-4 py-3 text-sm text-gray-500">
							{item.globalIndex !== undefined ? `#${item.globalIndex}` : '--'}
						</td>
						<td class="px-4 py-3 text-xs text-gray-400" title={ts ? formatTimestamp(ts) : ''}>
							{ts ? relativeTime(ts) : '--'}
						</td>
						<td class="px-4 py-3">
							<span
								class="inline-block text-xs font-medium px-2.5 py-0.5 rounded-full"
								style="background:{badgeColor}15; color:{badgeColor}; border:1px solid {badgeColor}30;"
							>
								{getPoolEventType(poolSource)}
							</span>
						</td>
						<td class="px-4 py-3 text-sm text-gray-400 max-w-[400px] truncate">
							{getPoolEventSummary(poolSource, evt)}
						</td>
						<td class="px-4 py-3 text-right">
							{#if caller}
								<EntityLink type="address" id={caller} />
							{:else}
								<span class="text-gray-600 text-xs">--</span>
							{/if}
						</td>
					</tr>
				{/snippet}
			</DataTable>
		</div>
	{/if}

	<Pagination currentPage={$explorerEventsPage} {totalPages} onPageChange={handlePageChange} />
</div>
