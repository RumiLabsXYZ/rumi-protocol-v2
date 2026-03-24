<script lang="ts">
	import { onMount } from 'svelte';
	import StatCard from '$components/explorer/StatCard.svelte';
	import EntityLink from '$components/explorer/EntityLink.svelte';
	import Pagination from '$components/explorer/Pagination.svelte';
	import TimeAgo from '$components/explorer/TimeAgo.svelte';
	import StatusBadge from '$components/explorer/StatusBadge.svelte';
	import {
		fetchLiquidationRecords,
		fetchLiquidationCount,
		fetchPendingLiquidations,
		fetchBotStats,
		fetchStabilityPoolLiquidations
	} from '$services/explorer/explorerService';
	import { formatE8s, formatUsdRaw, getTokenSymbol } from '$utils/explorerHelpers';

	// ── Constants ──────────────────────────────────────────────────────────
	const PAGE_SIZE = 50;

	// ── State ──────────────────────────────────────────────────────────────
	// Liquidation records (paginated)
	let totalCount: number = $state(0);
	let records: any[] = $state([]);
	let currentPage: number = $state(0);
	let recordsLoading: boolean = $state(true);
	let recordsError: string | null = $state(null);

	// Pending liquidations
	let pending: any[] = $state([]);
	let pendingLoading: boolean = $state(true);
	let pendingError: string | null = $state(null);

	// Bot stats
	let botStats: any | null = $state(null);
	let botLoading: boolean = $state(true);
	let botError: string | null = $state(null);

	// Stability pool liquidations
	let spLiquidations: any[] = $state([]);
	let spLoading: boolean = $state(true);
	let spError: string | null = $state(null);

	// ── Derived ────────────────────────────────────────────────────────────
	const totalPages = $derived(Math.ceil(totalCount / PAGE_SIZE));

	const botBudgetRemaining = $derived(
		botStats ? formatE8s(botStats.budget_remaining_e8s) + ' ICP' : '--'
	);

	const botTotalLiquidations = $derived(
		botStats ? Number(botStats.total_debt_covered_e8s) > 0 ? formatE8s(botStats.total_debt_covered_e8s) + ' icUSD' : '0' : '--'
	);

	// ── Data Loading ───────────────────────────────────────────────────────
	async function loadRecordsPage(page: number) {
		recordsLoading = true;
		recordsError = null;
		try {
			// Fetch newest first: calculate start from end
			const start = BigInt(Math.max(0, totalCount - (page + 1) * PAGE_SIZE));
			const length = BigInt(
				page === totalPages - 1
					? totalCount - (totalPages - 1) * PAGE_SIZE
					: Math.min(PAGE_SIZE, totalCount)
			);
			const result = await fetchLiquidationRecords(start, length);
			// Reverse so newest appear first
			records = [...result].reverse();
			currentPage = page;
		} catch (e) {
			recordsError = 'Failed to load liquidation records.';
			console.error('[liquidations] loadRecordsPage error:', e);
		} finally {
			recordsLoading = false;
		}
	}

	function handlePageChange(page: number) {
		loadRecordsPage(page);
	}

	onMount(async () => {
		// Load all sections in parallel
		const countPromise = fetchLiquidationCount()
			.then(async (count) => {
				totalCount = Number(count);
				if (totalCount > 0) {
					await loadRecordsPage(0);
				} else {
					recordsLoading = false;
				}
			})
			.catch((e) => {
				recordsError = 'Failed to load liquidation count.';
				recordsLoading = false;
				console.error('[liquidations] count error:', e);
			});

		const pendingPromise = fetchPendingLiquidations()
			.then((result) => {
				pending = result;
			})
			.catch((e) => {
				pendingError = 'Failed to load pending liquidations.';
				console.error('[liquidations] pending error:', e);
			})
			.finally(() => {
				pendingLoading = false;
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

		await Promise.allSettled([countPromise, pendingPromise, botPromise, spPromise]);
	});

	// ── Helpers ─────────────────────────────────────────────────────────────
	function getLiquidationType(record: any): { label: string; classes: string } {
		// Detect bot vs partial vs full based on available fields
		if (record.is_bot_liquidation) {
			return { label: 'Bot', classes: 'bg-purple-500/20 text-purple-300 border-purple-500/30' };
		}
		if (record.is_partial) {
			return { label: 'Partial', classes: 'bg-yellow-500/20 text-yellow-300 border-yellow-500/30' };
		}
		return { label: 'Full', classes: 'bg-red-500/20 text-red-300 border-red-500/30' };
	}

	function getCollateralTypeStr(record: any): string {
		if (record.collateral_type) {
			// Could be a principal or variant
			if (typeof record.collateral_type === 'string') return record.collateral_type;
			if (record.collateral_type?.toString) {
				const str = record.collateral_type.toString();
				return getTokenSymbol(str);
			}
		}
		return 'ICP';
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
		{#if totalCount > 0}
			<span class="text-lg font-semibold text-gray-400">
				{totalCount.toLocaleString()} total
			</span>
		{/if}
	</div>

	<!-- Summary Cards -->
	<div class="grid grid-cols-2 md:grid-cols-4 gap-4 mb-8">
		<StatCard
			label="Total Liquidations"
			value={recordsLoading ? '--' : totalCount.toLocaleString()}
			subtitle={recordsLoading ? 'Loading...' : undefined}
		/>
		<StatCard
			label="Pending Liquidations"
			value={pendingLoading ? '--' : pending.length.toLocaleString()}
			subtitle={pendingLoading ? 'Loading...' : pending.length > 0 ? 'In-flight' : 'None active'}
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
				<span>Loading liquidation records...</span>
			</div>
		{:else if recordsError}
			<div class="text-center py-16 text-red-400">{recordsError}</div>
		{:else if records.length === 0}
			<div class="text-center py-16 text-gray-500">No liquidation records found.</div>
		{:else}
			<div class="bg-gray-800/50 border border-gray-700/50 rounded-xl overflow-x-auto">
				<table class="w-full text-sm">
					<thead>
						<tr class="border-b border-gray-700/50 text-gray-400 text-xs uppercase tracking-wide">
							<th class="text-left px-4 py-3 font-medium">Time</th>
							<th class="text-left px-4 py-3 font-medium">Vault</th>
							<th class="text-left px-4 py-3 font-medium">Type</th>
							<th class="text-right px-4 py-3 font-medium">Debt Covered</th>
							<th class="text-right px-4 py-3 font-medium">Collateral Seized</th>
							<th class="text-left px-4 py-3 font-medium">Collateral</th>
							<th class="text-left px-4 py-3 font-medium">Liquidator</th>
						</tr>
					</thead>
					<tbody>
						{#each records as record}
							{@const typeInfo = getLiquidationType(record)}
							<tr
								class="border-b border-gray-700/30 hover:bg-gray-700/20 transition-colors"
							>
								<td class="px-4 py-3 whitespace-nowrap">
									{#if record.timestamp}
										<TimeAgo timestamp={record.timestamp} />
									{:else}
										<span class="text-gray-500">--</span>
									{/if}
								</td>
								<td class="px-4 py-3">
									{#if record.vault_id !== undefined}
										<EntityLink
											type="vault"
											value={String(record.vault_id)}
										/>
									{:else}
										<span class="text-gray-500">--</span>
									{/if}
								</td>
								<td class="px-4 py-3">
									<span
										class="inline-flex items-center px-2 py-0.5 rounded-full border text-xs font-medium {typeInfo.classes}"
									>
										{typeInfo.label}
									</span>
								</td>
								<td class="px-4 py-3 text-right font-mono text-gray-300">
									{#if record.debt_covered_e8s !== undefined}
										{formatE8s(record.debt_covered_e8s)}
									{:else if record.liquidator_payment !== undefined}
										{formatE8s(record.liquidator_payment)}
									{:else}
										<span class="text-gray-500">--</span>
									{/if}
								</td>
								<td class="px-4 py-3 text-right font-mono text-gray-300">
									{#if record.collateral_seized_e8s !== undefined}
										{formatE8s(record.collateral_seized_e8s)}
									{:else if record.icp_to_liquidator !== undefined}
										{formatE8s(record.icp_to_liquidator)}
									{:else}
										<span class="text-gray-500">--</span>
									{/if}
								</td>
								<td class="px-4 py-3 text-gray-300">
									{getCollateralTypeStr(record)}
								</td>
								<td class="px-4 py-3">
									{#if record.liquidator}
										<EntityLink
											type="address"
											value={typeof record.liquidator === 'string'
												? record.liquidator
												: record.liquidator?.toString?.() ?? '--'}
											short={true}
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

	<!-- Pending Liquidations -->
	{#if pendingLoading}
		<!-- Don't show section while loading -->
	{:else if pendingError}
		<section class="mb-10">
			<h2 class="text-lg font-semibold text-white mb-4">Pending Liquidations</h2>
			<div class="text-red-400 text-sm">{pendingError}</div>
		</section>
	{:else if pending.length > 0}
		<section class="mb-10">
			<h2 class="text-lg font-semibold text-white mb-4">
				Pending Liquidations
				<span class="text-sm font-normal text-gray-400 ml-2">({pending.length})</span>
			</h2>
			<div class="bg-gray-800/50 border border-gray-700/50 rounded-xl overflow-x-auto">
				<table class="w-full text-sm">
					<thead>
						<tr
							class="border-b border-gray-700/50 text-gray-400 text-xs uppercase tracking-wide"
						>
							<th class="text-left px-4 py-3 font-medium">Vault</th>
							<th class="text-left px-4 py-3 font-medium">Status</th>
							<th class="text-left px-4 py-3 font-medium">Claimant</th>
						</tr>
					</thead>
					<tbody>
						{#each pending as item}
							<tr class="border-b border-gray-700/30 hover:bg-gray-700/20 transition-colors">
								<td class="px-4 py-3">
									{#if item.vault_id !== undefined}
										<EntityLink
											type="vault"
											value={String(item.vault_id)}
										/>
									{:else}
										<span class="text-gray-500">--</span>
									{/if}
								</td>
								<td class="px-4 py-3">
									<StatusBadge status={item.status ?? 'pending'} />
								</td>
								<td class="px-4 py-3">
									{#if item.claimant}
										<EntityLink
											type="address"
											value={typeof item.claimant === 'string'
												? item.claimant
												: item.claimant?.toString?.() ?? '--'}
											short={true}
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
