<script lang="ts">
	import { onMount } from 'svelte';
	import StatCard from '$components/explorer/StatCard.svelte';
	import EntityLink from '$components/explorer/EntityLink.svelte';
	import Pagination from '$components/explorer/Pagination.svelte';
	import TimeAgo from '$components/explorer/TimeAgo.svelte';
	import StatusBadge from '$components/explorer/StatusBadge.svelte';
	import {
		fetchEvents,
		fetchLiquidatableVaults,
		fetchBotStats,
		fetchStabilityPoolLiquidations
	} from '$services/explorer/explorerService';
	import { formatE8s, formatUsdRaw, getTokenSymbol } from '$utils/explorerHelpers';
	import { getEventCategory, formatEvent } from '$utils/explorerFormatters';
	import type { FormattedEvent } from '$utils/explorerFormatters';

	// ── Constants ──────────────────────────────────────────────────────────
	const EVENTS_PAGE_SIZE = 200n; // Fetch larger pages since we filter client-side
	const DISPLAY_PAGE_SIZE = 50;

	// ── State ──────────────────────────────────────────────────────────────
	// Liquidation events (from event log, filtered client-side)
	let liquidationEvents: { id: bigint; event: any; formatted: FormattedEvent }[] = $state([]);
	let displayedEvents: { id: bigint; event: any; formatted: FormattedEvent }[] = $state([]);
	let totalLiquidationCount: number = $state(0);
	let currentPage: number = $state(0);
	let recordsLoading: boolean = $state(true);
	let recordsError: string | null = $state(null);

	// Liquidatable vaults
	let liquidatable: any[] = $state([]);
	let liquidatableLoading: boolean = $state(true);
	let liquidatableError: string | null = $state(null);

	// Bot stats
	let botStats: any | null = $state(null);
	let botLoading: boolean = $state(true);
	let botError: string | null = $state(null);

	// Stability pool liquidations
	let spLiquidations: any[] = $state([]);
	let spLoading: boolean = $state(true);
	let spError: string | null = $state(null);

	// ── Derived ────────────────────────────────────────────────────────────
	const totalPages = $derived(Math.ceil(totalLiquidationCount / DISPLAY_PAGE_SIZE));

	const botBudgetRemaining = $derived(
		botStats ? formatE8s(botStats.budget_remaining_e8s) + ' ICP' : '--'
	);

	const botTotalLiquidations = $derived(
		botStats ? Number(botStats.total_debt_covered_e8s) > 0 ? formatE8s(botStats.total_debt_covered_e8s) + ' icUSD' : '0' : '--'
	);

	// ── Data Loading ───────────────────────────────────────────────────────
	async function loadLiquidationEvents() {
		recordsLoading = true;
		recordsError = null;
		try {
			// Fetch multiple pages of events and filter for liquidation category
			const allLiquidationEvents: { id: bigint; event: any; formatted: FormattedEvent }[] = [];
			let page = 0n;
			let hasMore = true;

			// Scan through event pages to find liquidation events
			// We fetch up to a few pages to get a reasonable amount of liquidation history
			const MAX_PAGES_TO_SCAN = 10;
			let pagesScanned = 0;

			while (hasMore && pagesScanned < MAX_PAGES_TO_SCAN) {
				const result = await fetchEvents(page, EVENTS_PAGE_SIZE);
				if (result.events.length === 0) {
					hasMore = false;
					break;
				}

				for (const [id, event] of result.events) {
					if (getEventCategory(event) === 'liquidation') {
						allLiquidationEvents.push({
							id,
							event,
							formatted: formatEvent(event)
						});
					}
				}

				// Check if we've reached the end
				if (result.events.length < Number(EVENTS_PAGE_SIZE)) {
					hasMore = false;
				}
				page += 1n;
				pagesScanned++;
			}

			// Sort by event ID descending (newest first)
			allLiquidationEvents.sort((a, b) => (b.id > a.id ? 1 : b.id < a.id ? -1 : 0));

			liquidationEvents = allLiquidationEvents;
			totalLiquidationCount = allLiquidationEvents.length;
			updateDisplayedEvents(0);
		} catch (e) {
			recordsError = 'Failed to load liquidation events.';
			console.error('[liquidations] loadLiquidationEvents error:', e);
		} finally {
			recordsLoading = false;
		}
	}

	function updateDisplayedEvents(page: number) {
		const start = page * DISPLAY_PAGE_SIZE;
		const end = start + DISPLAY_PAGE_SIZE;
		displayedEvents = liquidationEvents.slice(start, end);
		currentPage = page;
	}

	function handlePageChange(page: number) {
		updateDisplayedEvents(page);
	}

	onMount(async () => {
		// Load all sections in parallel
		const eventsPromise = loadLiquidationEvents();

		const liquidatablePromise = fetchLiquidatableVaults()
			.then((result) => {
				liquidatable = result;
			})
			.catch((e) => {
				liquidatableError = 'Failed to load liquidatable vaults.';
				console.error('[liquidations] liquidatable error:', e);
			})
			.finally(() => {
				liquidatableLoading = false;
			});

		const botPromise = fetchBotStats()
			.then((result) => {
				botStats = result;
			})
			.catch((e) => {
				botError = 'Failed to load bot stats.';
				console.error('[liquidations] bot stats error:', e);
			})
			.finally(() => {
				botLoading = false;
			});

		const spPromise = fetchStabilityPoolLiquidations(50)
			.then((result) => {
				spLiquidations = result;
			})
			.catch((e) => {
				spError = 'Failed to load stability pool liquidations.';
				console.error('[liquidations] SP liquidations error:', e);
			})
			.finally(() => {
				spLoading = false;
			});

		await Promise.allSettled([eventsPromise, liquidatablePromise, botPromise, spPromise]);
	});

	// ── Helpers ─────────────────────────────────────────────────────────────
	function getFieldValue(formatted: FormattedEvent, label: string): string | null {
		const field = formatted.fields.find((f) => f.label === label);
		return field?.value ?? null;
	}

	function getSpCollateralType(record: any): string {
		// SP records use CollateralType variant: { ICP: null } or { CkBTC: null }
		if (record.collateral_type) {
			if ('ICP' in record.collateral_type) return 'ICP';
			if ('CkBTC' in record.collateral_type) return 'ckBTC';
		}
		return 'Unknown';
	}
</script>

<div class="max-w-[1100px] mx-auto px-4 py-8">
	<!-- Header -->
	<div class="flex items-baseline justify-between mb-6">
		<h1 class="text-2xl font-bold text-white">Liquidations</h1>
		{#if totalLiquidationCount > 0}
			<span class="text-lg font-semibold text-gray-400">
				{totalLiquidationCount.toLocaleString()} found
			</span>
		{/if}
	</div>

	<!-- Summary Cards -->
	<div class="grid grid-cols-2 md:grid-cols-4 gap-4 mb-8">
		<StatCard
			label="Liquidation Events"
			value={recordsLoading ? '--' : totalLiquidationCount.toLocaleString()}
			subtitle={recordsLoading ? 'Loading...' : undefined}
		/>
		<StatCard
			label="Liquidatable Vaults"
			value={liquidatableLoading ? '--' : liquidatable.length.toLocaleString()}
			subtitle={liquidatableLoading ? 'Loading...' : liquidatable.length > 0 ? 'At risk' : 'None at risk'}
		/>
		<StatCard
			label="Bot Budget Remaining"
			value={botLoading ? '--' : botBudgetRemaining}
			subtitle={botLoading ? 'Loading...' : 'Available for bot'}
		/>
		<StatCard
			label="Bot Debt Covered"
			value={botLoading ? '--' : botTotalLiquidations}
			subtitle={botLoading ? 'Loading...' : 'Total debt covered by bot'}
		/>
	</div>

	<!-- Liquidation History Table -->
	<section class="mb-10">
		<h2 class="text-lg font-semibold text-white mb-4">Liquidation History</h2>

		{#if recordsLoading}
			<div class="flex items-center justify-center gap-3 py-16 text-gray-400">
				<div
					class="w-5 h-5 border-2 border-gray-600 border-t-purple-500 rounded-full animate-spin"
				></div>
				<span>Loading liquidation events...</span>
			</div>
		{:else if recordsError}
			<div class="text-center py-16 text-red-400">{recordsError}</div>
		{:else if displayedEvents.length === 0}
			<div class="text-center py-16 text-gray-500">No liquidation events found.</div>
		{:else}
			<div class="bg-gray-800/50 border border-gray-700/50 rounded-xl overflow-x-auto">
				<table class="w-full text-sm">
					<thead>
						<tr class="border-b border-gray-700/50 text-gray-400 text-xs uppercase tracking-wide">
							<th class="text-left px-4 py-3 font-medium">ID</th>
							<th class="text-left px-4 py-3 font-medium">Type</th>
							<th class="text-left px-4 py-3 font-medium">Summary</th>
							<th class="text-left px-4 py-3 font-medium">Vault</th>
						</tr>
					</thead>
					<tbody>
						{#each displayedEvents as { id, formatted }}
							<tr
								class="border-b border-gray-700/30 hover:bg-gray-700/20 transition-colors"
							>
								<td class="px-4 py-3 font-mono text-gray-400 text-xs">
									#{id.toString()}
								</td>
								<td class="px-4 py-3">
									<span
										class="inline-flex items-center px-2 py-0.5 rounded-full border text-xs font-medium {formatted.badgeColor}"
									>
										{formatted.typeName}
									</span>
								</td>
								<td class="px-4 py-3 text-gray-300 text-xs max-w-md truncate">
									{formatted.summary}
								</td>
								<td class="px-4 py-3">
									{#if getFieldValue(formatted, 'Vault')}
										<EntityLink
											type="vault"
											value={getFieldValue(formatted, 'Vault')}
										/>
									{:else}
										<span class="text-gray-500">--</span>
									{/if}
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>

			{#if totalPages > 1}
				<div class="mt-4">
					<Pagination
						{currentPage}
						{totalPages}
						onPageChange={handlePageChange}
					/>
				</div>
			{/if}
		{/if}
	</section>

	<!-- Liquidatable Vaults -->
	{#if liquidatableLoading}
		<!-- Don't show section while loading -->
	{:else if liquidatableError}
		<section class="mb-10">
			<h2 class="text-lg font-semibold text-white mb-4">Liquidatable Vaults</h2>
			<div class="text-red-400 text-sm">{liquidatableError}</div>
		</section>
	{:else if liquidatable.length > 0}
		<section class="mb-10">
			<h2 class="text-lg font-semibold text-white mb-4">
				Liquidatable Vaults
				<span class="text-sm font-normal text-gray-400 ml-2">({liquidatable.length})</span>
			</h2>
			<div class="bg-gray-800/50 border border-gray-700/50 rounded-xl overflow-x-auto">
				<table class="w-full text-sm">
					<thead>
						<tr
							class="border-b border-gray-700/50 text-gray-400 text-xs uppercase tracking-wide"
						>
							<th class="text-left px-4 py-3 font-medium">Vault</th>
							<th class="text-left px-4 py-3 font-medium">Owner</th>
							<th class="text-right px-4 py-3 font-medium">Debt</th>
							<th class="text-right px-4 py-3 font-medium">Collateral</th>
							<th class="text-left px-4 py-3 font-medium">Status</th>
						</tr>
					</thead>
					<tbody>
						{#each liquidatable as vault}
							<tr class="border-b border-gray-700/30 hover:bg-gray-700/20 transition-colors">
								<td class="px-4 py-3">
									{#if vault.vault_id !== undefined}
										<EntityLink
											type="vault"
											value={String(vault.vault_id)}
										/>
									{:else}
										<span class="text-gray-500">--</span>
									{/if}
								</td>
								<td class="px-4 py-3">
									{#if vault.owner}
										<EntityLink
											type="address"
											value={typeof vault.owner === 'string'
												? vault.owner
												: vault.owner?.toString?.() ?? '--'}
											short={true}
										/>
									{:else}
										<span class="text-gray-500">--</span>
									{/if}
								</td>
								<td class="px-4 py-3 text-right font-mono text-gray-300">
									{#if vault.borrowed_icusd_e8s !== undefined}
										{formatE8s(vault.borrowed_icusd_e8s)} icUSD
									{:else}
										<span class="text-gray-500">--</span>
									{/if}
								</td>
								<td class="px-4 py-3 text-right font-mono text-gray-300">
									{#if vault.collateral_amount_e8s !== undefined}
										{formatE8s(vault.collateral_amount_e8s)}
									{:else}
										<span class="text-gray-500">--</span>
									{/if}
								</td>
								<td class="px-4 py-3">
									<StatusBadge status="liquidatable" />
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		</section>
	{/if}

	<!-- Bot Stats -->
	<section class="mb-10">
		<h2 class="text-lg font-semibold text-white mb-4">Liquidation Bot</h2>

		{#if botLoading}
			<div class="flex items-center gap-3 py-8 text-gray-400 justify-center">
				<div
					class="w-4 h-4 border-2 border-gray-600 border-t-purple-500 rounded-full animate-spin"
				></div>
				<span>Loading bot stats...</span>
			</div>
		{:else if botError}
			<div class="text-red-400 text-sm">{botError}</div>
		{:else if botStats}
			<div class="grid grid-cols-2 md:grid-cols-4 gap-4">
				<StatCard
					label="Budget Total"
					value={formatE8s(botStats.budget_total_e8s) + ' ICP'}
					size="sm"
				/>
				<StatCard
					label="Budget Remaining"
					value={formatE8s(botStats.budget_remaining_e8s) + ' ICP'}
					size="sm"
				/>
				<StatCard
					label="Total Debt Covered"
					value={formatE8s(botStats.total_debt_covered_e8s) + ' icUSD'}
					size="sm"
				/>
				<StatCard
					label="Total icUSD Deposited"
					value={formatE8s(botStats.total_icusd_deposited_e8s) + ' icUSD'}
					size="sm"
				/>
			</div>
			{#if botStats.liquidation_bot_principal && botStats.liquidation_bot_principal.length > 0}
				<div class="mt-3 text-sm text-gray-400">
					Bot Principal:
					<EntityLink
						type="address"
						value={botStats.liquidation_bot_principal[0].toString()}
					/>
				</div>
			{/if}
		{:else}
			<div class="text-gray-500 text-sm">No bot stats available.</div>
		{/if}
	</section>

	<!-- Stability Pool Liquidations -->
	<section class="mb-10">
		<h2 class="text-lg font-semibold text-white mb-4">Stability Pool Liquidations</h2>

		{#if spLoading}
			<div class="flex items-center gap-3 py-8 text-gray-400 justify-center">
				<div
					class="w-4 h-4 border-2 border-gray-600 border-t-purple-500 rounded-full animate-spin"
				></div>
				<span>Loading stability pool liquidations...</span>
			</div>
		{:else if spError}
			<div class="text-red-400 text-sm">{spError}</div>
		{:else if spLiquidations.length === 0}
			<div class="text-center py-8 text-gray-500">No stability pool liquidations recorded.</div>
		{:else}
			<div class="bg-gray-800/50 border border-gray-700/50 rounded-xl overflow-x-auto">
				<table class="w-full text-sm">
					<thead>
						<tr
							class="border-b border-gray-700/50 text-gray-400 text-xs uppercase tracking-wide"
						>
							<th class="text-left px-4 py-3 font-medium">ID</th>
							<th class="text-left px-4 py-3 font-medium">Vault</th>
							<th class="text-left px-4 py-3 font-medium">Time</th>
							<th class="text-right px-4 py-3 font-medium">Debt Burned</th>
							<th class="text-right px-4 py-3 font-medium">Collateral Received</th>
							<th class="text-left px-4 py-3 font-medium">Type</th>
							<th class="text-right px-4 py-3 font-medium">Depositors</th>
						</tr>
					</thead>
					<tbody>
						{#each spLiquidations as sp}
							<tr
								class="border-b border-gray-700/30 hover:bg-gray-700/20 transition-colors"
							>
								<td class="px-4 py-3 font-mono text-gray-300">
									{sp.liquidation_id !== undefined
										? `#${sp.liquidation_id}`
										: '--'}
								</td>
								<td class="px-4 py-3">
									{#if sp.vault_id !== undefined}
										<EntityLink
											type="vault"
											value={String(sp.vault_id)}
										/>
									{:else}
										<span class="text-gray-500">--</span>
									{/if}
								</td>
								<td class="px-4 py-3 whitespace-nowrap">
									{#if sp.liquidation_time}
										<TimeAgo timestamp={sp.liquidation_time} />
									{:else}
										<span class="text-gray-500">--</span>
									{/if}
								</td>
								<td class="px-4 py-3 text-right font-mono text-gray-300">
									{#if sp.liquidated_debt !== undefined}
										{formatE8s(sp.liquidated_debt)}
									{:else if sp.stables_consumed}
										{@const total = sp.stables_consumed.reduce(
											(sum: number, [_, amt]: [any, bigint]) =>
												sum + Number(amt),
											0
										)}
										{formatE8s(BigInt(total))}
									{:else}
										<span class="text-gray-500">--</span>
									{/if}
								</td>
								<td class="px-4 py-3 text-right font-mono text-gray-300">
									{#if sp.collateral_received !== undefined}
										{formatE8s(sp.collateral_received)}
									{:else}
										<span class="text-gray-500">--</span>
									{/if}
								</td>
								<td class="px-4 py-3 text-gray-300">
									{getSpCollateralType(sp)}
								</td>
								<td class="px-4 py-3 text-right font-mono text-gray-300">
									{sp.depositors_count !== undefined
										? Number(sp.depositors_count)
										: '--'}
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/if}
	</section>
</div>
