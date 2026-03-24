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
	const EVENTS_PAGE_SIZE = 200n;
	const DISPLAY_PAGE_SIZE = 50;

	// ── Unified liquidation row type ──────────────────────────────────────
	interface LiquidationRow {
		source: 'event' | 'stability_pool';
		sortTimestamp: bigint; // nanosecond timestamp for sorting
		// Event-log fields
		eventId?: bigint;
		formatted?: FormattedEvent;
		// SP fields
		spVaultId?: bigint;
		spTimestamp?: bigint;
		spDebtBurned?: bigint;
		spCollateralGained?: bigint;
		spCollateralType?: string; // token symbol
		spDepositorsCount?: bigint;
	}

	// ── State ──────────────────────────────────────────────────────────────
	let allRows: LiquidationRow[] = $state([]);
	let displayedRows: LiquidationRow[] = $state([]);
	let totalRowCount: number = $state(0);
	let currentPage: number = $state(0);
	let loading: boolean = $state(true);
	let error: string | null = $state(null);

	// Liquidatable vaults
	let liquidatable: any[] = $state([]);
	let liquidatableLoading: boolean = $state(true);
	let liquidatableError: string | null = $state(null);

	// Bot stats
	let botStats: any | null = $state(null);
	let botLoading: boolean = $state(true);
	let botError: string | null = $state(null);

	// ── Derived ────────────────────────────────────────────────────────────
	const totalPages = $derived(Math.ceil(totalRowCount / DISPLAY_PAGE_SIZE));

	const botBudgetRemaining = $derived(
		botStats ? formatE8s(botStats.budget_remaining_e8s) + ' ICP' : '--'
	);

	const botTotalLiquidations = $derived(
		botStats
			? Number(botStats.total_debt_covered_e8s) > 0
				? formatE8s(botStats.total_debt_covered_e8s) + ' icUSD'
				: '0'
			: '--'
	);

	// ── Helpers ─────────────────────────────────────────────────────────────
	function getFieldValue(formatted: FormattedEvent, label: string): string | null {
		const field = formatted.fields.find((f) => f.label === label);
		return field?.value ?? null;
	}

	function getEventTimestampNs(event: any): bigint {
		// Try to extract timestamp from various event variant shapes
		const key = Object.keys(event)[0];
		if (!key) return 0n;
		const d = event[key];
		if (!d) return 0n;
		// Most events have a timestamp field (opt nat64 in Candid → [] | [bigint])
		if (d.timestamp !== undefined) {
			if (Array.isArray(d.timestamp)) {
				return d.timestamp.length > 0 ? BigInt(d.timestamp[0]) : 0n;
			}
			return BigInt(d.timestamp);
		}
		return 0n;
	}

	// ── Data Loading ───────────────────────────────────────────────────────
	async function loadAllLiquidations() {
		loading = true;
		error = null;
		try {
			// Load event-log liquidations and SP liquidations in parallel
			const [eventRows, spRows] = await Promise.all([
				loadEventLogLiquidations(),
				loadSpLiquidations()
			]);

			// Merge and sort newest-first
			const merged = [...eventRows, ...spRows];
			merged.sort((a, b) => (b.sortTimestamp > a.sortTimestamp ? 1 : b.sortTimestamp < a.sortTimestamp ? -1 : 0));

			allRows = merged;
			totalRowCount = merged.length;
			updateDisplayedRows(0);
		} catch (e) {
			error = 'Failed to load liquidation data.';
			console.error('[liquidations] loadAllLiquidations error:', e);
		} finally {
			loading = false;
		}
	}

	async function loadEventLogLiquidations(): Promise<LiquidationRow[]> {
		const rows: LiquidationRow[] = [];
		let page = 0n;
		let hasMore = true;
		const MAX_PAGES_TO_SCAN = 10;
		let pagesScanned = 0;

		while (hasMore && pagesScanned < MAX_PAGES_TO_SCAN) {
			const result = await fetchEvents(page, EVENTS_PAGE_SIZE);
			if (result.events.length === 0) break;

			for (const [id, event] of result.events) {
				if (getEventCategory(event) === 'liquidation') {
					const ts = getEventTimestampNs(event);
					rows.push({
						source: 'event',
						sortTimestamp: ts,
						eventId: id,
						formatted: formatEvent(event)
					});
				}
			}

			if (result.events.length < Number(EVENTS_PAGE_SIZE)) break;
			page += 1n;
			pagesScanned++;
		}
		return rows;
	}

	async function loadSpLiquidations(): Promise<LiquidationRow[]> {
		const records = await fetchStabilityPoolLiquidations(200);
		return records.map((sp: any) => {
			const totalDebt = (sp.stables_consumed ?? []).reduce(
				(sum: bigint, [_p, amt]: [any, bigint]) => sum + amt,
				0n
			);
			const collateralPrincipal = sp.collateral_type?.toText?.() ?? sp.collateral_type?.toString?.() ?? '';
			return {
				source: 'stability_pool' as const,
				sortTimestamp: sp.timestamp ?? 0n,
				spVaultId: sp.vault_id,
				spTimestamp: sp.timestamp,
				spDebtBurned: totalDebt,
				spCollateralGained: sp.collateral_gained,
				spCollateralType: getTokenSymbol(collateralPrincipal),
				spDepositorsCount: sp.depositors_count
			};
		});
	}

	function updateDisplayedRows(page: number) {
		const start = page * DISPLAY_PAGE_SIZE;
		const end = start + DISPLAY_PAGE_SIZE;
		displayedRows = allRows.slice(start, end);
		currentPage = page;
	}

	function handlePageChange(page: number) {
		updateDisplayedRows(page);
	}

	onMount(async () => {
		const mainPromise = loadAllLiquidations();

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

		await Promise.allSettled([mainPromise, liquidatablePromise, botPromise]);
	});
</script>

<div class="max-w-[1100px] mx-auto px-4 py-8">
	<!-- Header -->
	<div class="flex items-baseline justify-between mb-6">
		<h1 class="text-2xl font-bold text-white">Liquidations</h1>
		{#if totalRowCount > 0}
			<span class="text-lg font-semibold text-gray-400">
				{totalRowCount.toLocaleString()} found
			</span>
		{/if}
	</div>

	<!-- Summary Cards -->
	<div class="grid grid-cols-2 md:grid-cols-4 gap-4 mb-8">
		<StatCard
			label="Liquidation Events"
			value={loading ? '--' : totalRowCount.toLocaleString()}
			subtitle={loading ? 'Loading...' : undefined}
		/>
		<StatCard
			label="Liquidatable Vaults"
			value={liquidatableLoading ? '--' : liquidatable.length.toLocaleString()}
			subtitle={liquidatableLoading
				? 'Loading...'
				: liquidatable.length > 0
					? 'At risk'
					: 'None at risk'}
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

	<!-- Unified Liquidation History Table -->
	<section class="mb-10">
		<h2 class="text-lg font-semibold text-white mb-4">Liquidation History</h2>

		{#if loading}
			<div class="flex items-center justify-center gap-3 py-16 text-gray-400">
				<div
					class="w-5 h-5 border-2 border-gray-600 border-t-purple-500 rounded-full animate-spin"
				></div>
				<span>Loading liquidation events...</span>
			</div>
		{:else if error}
			<div class="text-center py-16 text-red-400">{error}</div>
		{:else if displayedRows.length === 0}
			<div class="text-center py-16 text-gray-500">No liquidation events found.</div>
		{:else}
			<div class="bg-gray-800/50 border border-gray-700/50 rounded-xl overflow-x-auto">
				<table class="w-full text-sm">
					<thead>
						<tr
							class="border-b border-gray-700/50 text-gray-400 text-xs uppercase tracking-wide"
						>
							<th class="text-left px-4 py-3 font-medium">Source</th>
							<th class="text-left px-4 py-3 font-medium">Type</th>
							<th class="text-left px-4 py-3 font-medium">Time</th>
							<th class="text-left px-4 py-3 font-medium">Summary / Details</th>
							<th class="text-left px-4 py-3 font-medium">Vault</th>
						</tr>
					</thead>
					<tbody>
						{#each displayedRows as row}
							<tr
								class="border-b border-gray-700/30 hover:bg-gray-700/20 transition-colors"
							>
								{#if row.source === 'event' && row.formatted}
									<!-- Event-log liquidation row -->
									<td class="px-4 py-3">
										<span
											class="inline-flex items-center px-2 py-0.5 rounded-full text-xs font-medium bg-amber-500/15 text-amber-400 border border-amber-500/30"
										>
											Event #{row.eventId?.toString() ?? '?'}
										</span>
									</td>
									<td class="px-4 py-3">
										<span
											class="inline-flex items-center px-2 py-0.5 rounded-full text-xs font-medium {row.formatted.badgeColor}"
										>
											{row.formatted.typeName}
										</span>
									</td>
									<td class="px-4 py-3 whitespace-nowrap">
										{#if row.sortTimestamp > 0n}
											<TimeAgo timestamp={row.sortTimestamp} />
										{:else}
											<span class="text-gray-500">--</span>
										{/if}
									</td>
									<td class="px-4 py-3 text-gray-300 text-xs max-w-md truncate">
										{row.formatted.summary}
									</td>
									<td class="px-4 py-3">
										{#if getFieldValue(row.formatted, 'Vault')}
											<EntityLink
												type="vault"
												value={getFieldValue(row.formatted, 'Vault') ?? ''}
											/>
										{:else}
											<span class="text-gray-500">--</span>
										{/if}
									</td>
								{:else if row.source === 'stability_pool'}
									<!-- Stability Pool liquidation row -->
									<td class="px-4 py-3">
										<span
											class="inline-flex items-center px-2 py-0.5 rounded-full text-xs font-medium bg-teal-500/15 text-teal-400 border border-teal-500/30"
										>
											Stability Pool
										</span>
									</td>
									<td class="px-4 py-3">
										<span
											class="inline-flex items-center px-2 py-0.5 rounded-full text-xs font-medium bg-red-500/15 text-red-400 border border-red-500/30"
										>
											SP Liquidation
										</span>
									</td>
									<td class="px-4 py-3 whitespace-nowrap">
										{#if row.spTimestamp}
											<TimeAgo timestamp={row.spTimestamp} />
										{:else}
											<span class="text-gray-500">--</span>
										{/if}
									</td>
									<td class="px-4 py-3 text-gray-300 text-xs">
										{#if row.spDebtBurned !== undefined && row.spDebtBurned > 0n}
											Burned {formatE8s(row.spDebtBurned)} icUSD
										{/if}
										{#if row.spCollateralGained !== undefined && row.spCollateralGained > 0n}
											{#if row.spDebtBurned !== undefined && row.spDebtBurned > 0n}, {/if}received {formatE8s(row.spCollateralGained)} {row.spCollateralType ?? '?'}
										{/if}
										{#if row.spDepositorsCount !== undefined}
											<span class="text-gray-500 ml-1">({Number(row.spDepositorsCount)} depositors)</span>
										{/if}
									</td>
									<td class="px-4 py-3">
										{#if row.spVaultId !== undefined}
											<EntityLink
												type="vault"
												value={String(row.spVaultId)}
											/>
										{:else}
											<span class="text-gray-500">--</span>
										{/if}
									</td>
								{/if}
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
							<tr
								class="border-b border-gray-700/30 hover:bg-gray-700/20 transition-colors"
							>
								<td class="px-4 py-3">
									{#if vault.vault_id !== undefined}
										<EntityLink type="vault" value={String(vault.vault_id)} />
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
</div>
