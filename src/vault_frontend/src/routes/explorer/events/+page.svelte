<script lang="ts">
	import { onMount } from 'svelte';
	import SearchBar from '$lib/components/explorer/SearchBar.svelte';
	import EventRow from '$lib/components/explorer/EventRow.svelte';
	import Pagination from '$lib/components/explorer/Pagination.svelte';
	import {
		explorerEvents, explorerEventsLoading, explorerEventsPage,
		explorerEventsTotalCount, fetchEvents, fetchAllVaults, PAGE_SIZE,
		poolEvents, poolEventsLoading, fetchPoolEvents
	} from '$lib/stores/explorerStore';
	import type { UnifiedEvent } from '$lib/stores/explorerStore';
	import { getEventCategory, type EventCategory } from '$lib/utils/eventFormatters';
	import { formatAmount } from '$lib/utils/eventFormatters';
	import { stabilityPoolService } from '$lib/services/stabilityPoolService';
	import { threePoolService } from '$lib/services/threePoolService';

	type ExplorerFilter = EventCategory | 'all' | '3pool';

	let selectedFilter: ExplorerFilter = 'all';
	let vaultCollateralMap: Map<number, any> = new Map();
	let spStatus: any = null;
	let tpStatus: any = null;

	$: totalPages = Math.ceil($explorerEventsTotalCount / PAGE_SIZE);

	// Events already come as {event, globalIndex} from the store (server-side filtered, no AccrueInterest)
	$: filteredEvents = selectedFilter === 'all'
		? $explorerEvents
		: selectedFilter === '3pool'
		? []
		: $explorerEvents.filter((e: any) => getEventCategory(e.event) === selectedFilter);

	$: filteredPoolEvents = (() => {
		if (selectedFilter === '3pool') {
			return $poolEvents.filter(e => e.source === '3pool_swap' || e.source === '3pool_lp');
		}
		if (selectedFilter === 'stability') {
			return $poolEvents.filter(e => e.source === 'stability_pool');
		}
		if (selectedFilter === 'all') {
			return $poolEvents;
		}
		return [];
	})();

	function handlePageChange(page: number) {
		fetchEvents(page);
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

	onMount(async () => {
		fetchEvents(0);
		fetchPoolEvents();
		try { spStatus = await stabilityPoolService.getPoolStatus(); } catch {}
		try { tpStatus = await threePoolService.getPoolStatus(); } catch {}
		// Build vault_id -> collateral_type lookup for event summaries
		const vaults = await fetchAllVaults();
		const map = new Map<number, any>();
		for (const v of vaults) {
			map.set(Number(v.vault_id), v.collateral_type);
		}
		vaultCollateralMap = map;
	});
</script>

<div class="explorer-page">
	<h1 class="page-title">Protocol Events</h1>

	<div class="stats-row">
		<div class="stat glass-card">
			<span class="stat-label">Protocol Events</span>
			<span class="stat-value key-number">{$explorerEventsTotalCount.toLocaleString()}</span>
		</div>
		{#if spStatus}
			<div class="stat glass-card">
				<span class="stat-label">Stability Pool</span>
				<span class="stat-value key-number">{formatAmount(spStatus.total_deposits_e8s)} icUSD</span>
			</div>
		{/if}
		{#if tpStatus}
			<div class="stat glass-card">
				<span class="stat-label">3Pool TVL</span>
				<span class="stat-value key-number">
					{formatAmount(
						tpStatus.balances.reduce((sum: bigint, b: bigint, i: number) =>
							sum + (i === 0 ? b : b * 100n), 0n)
					)} USD
				</span>
			</div>
		{/if}
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
		<div class="loading">Loading events...</div>
	{:else if filteredEvents.length === 0 && filteredPoolEvents.length === 0}
		<div class="empty">No events found.</div>
	{:else}
		{#if filteredEvents.length > 0}
			<div class="events-list glass-card">
				{#each filteredEvents as { event, globalIndex }}
					<EventRow {event} index={globalIndex} {vaultCollateralMap} />
				{/each}
			</div>
		{/if}
	{/if}

	{#if filteredPoolEvents.length > 0}
		<h2 class="section-title" style="margin-top:1.5rem;">Pool Activity</h2>
		{#if $poolEventsLoading}
			<div class="loading">Loading pool events...</div>
		{:else}
			<div class="events-list glass-card">
				{#each filteredPoolEvents as unified}
					<EventRow event={unified.event} poolSource={unified.source} index={unified.globalIndex} />
				{/each}
			</div>
		{/if}
	{/if}

	<Pagination currentPage={$explorerEventsPage} {totalPages} onPageChange={handlePageChange} />
</div>

<style>
	.explorer-page { max-width:900px; margin:0 auto; padding:2rem 1rem; }
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
	.section-title { font-size:1rem; font-weight:600; color:var(--rumi-text-primary); }
	.loading, .empty { text-align:center; padding:3rem; color:var(--rumi-text-muted); }
</style>
