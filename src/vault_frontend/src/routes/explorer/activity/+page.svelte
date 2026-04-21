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
		fetchAmmLiquidityEvents, fetchAmmLiquidityEventCount,
		fetchAmmAdminEvents, fetchAmmAdminEventCount,
		fetch3PoolLiquidityEvents, fetch3PoolLiquidityEventCount,
		fetch3PoolAdminEvents, fetch3PoolAdminEventCount,
		fetchStabilityPoolEvents, fetchStabilityPoolEventCount,
		fetchLiquidatableVaults, fetchBotStats, fetchAllVaults
	} from '$services/explorer/explorerService';
	import {
		getEventCategory, formatSwapEvent, formatAmmSwapEvent,
		formatAmmLiquidityEvent, formatAmmAdminEvent,
		format3PoolLiquidityEvent, format3PoolAdminEvent,
		formatStabilityPoolEvent, formatMultiHopSwapEvent
	} from '$utils/explorerFormatters';
	import { formatE8s, formatTokenAmount, timeAgo, shortenPrincipal, getTokenSymbol, getTokenDecimals } from '$utils/explorerHelpers';
	import { CANISTER_IDS } from '$lib/config';

	const PAGE_SIZE = 100;

	type ActivityFilter = 'all' | 'vault_ops' | 'liquidations' | 'dex' | 'stability_pool' | 'system';

	const FILTERS: { key: ActivityFilter; label: string }[] = [
		{ key: 'all', label: 'All' },
		{ key: 'vault_ops', label: 'Vault Operations' },
		{ key: 'liquidations', label: 'Liquidations' },
		{ key: 'dex', label: 'DEX' },
		{ key: 'stability_pool', label: 'Stability Pool' },
		{ key: 'system', label: 'System' },
	];

	// Unified event wrapper for display
	interface DisplayEvent {
		globalIndex: bigint;
		event: any;
		source: 'backend' | '3pool_swap' | 'amm_swap' | 'amm_liquidity' | 'amm_admin' | '3pool_liquidity' | '3pool_admin' | 'stability_pool' | 'multi_hop_swap';
		timestamp: number; // nanoseconds
	}

	let displayEvents: DisplayEvent[] = $state([]);
	let totalCount: number = $state(0);
	let currentPage: number = $state(0);
	let selectedFilter: ActivityFilter = $state('all');
	let loading: boolean = $state(true);
	let error: string | null = $state(null);

	// Liquidation summary (shown when filter === 'liquidations')
	let liquidatableVaults: any[] = $state([]);
	let botStats: any = $state(null);

	// Vault collateral type lookup map (vault_id → collateral_type principal string)
	let vaultCollateralMap: Map<number, string> = $state(new Map());
	// Vault owner lookup map (vault_id → owner principal string)
	let vaultOwnerMap: Map<number, string> = $state(new Map());

	const totalPages = $derived(Math.ceil(totalCount / PAGE_SIZE));

	// ── Timestamp extraction ──

	function extractTimestamp(event: any): number {
		if (event.timestamp != null) return Number(event.timestamp);
		// Backend events may have timestamp inside the variant data
		const eventType = event.event_type ?? event;
		const key = Object.keys(eventType)[0];
		if (key) {
			const data = eventType[key];
			if (data?.timestamp != null) return Number(data.timestamp);
		}
		return 0;
	}

	// ── Principal extraction from any event type ──

	function extractPrincipal(event: any, source: string): string | null {
		// Multi-hop swap: extract caller from nested AMM event
		if (source === 'multi_hop_swap') {
			const caller = event.ammEvent?.caller ?? event.liqEvent?.caller;
			if (caller?.toText) return caller.toText();
			if (typeof caller === 'string' && caller.length > 10) return caller;
			return null;
		}
		// Direct caller field (swap events, SP events, AMM liquidity/admin, 3Pool liquidity/admin)
		const caller = event.caller;
		if (caller) {
			if (typeof caller === 'object' && typeof caller.toText === 'function') return caller.toText();
			if (typeof caller === 'string' && caller.length > 10) return caller;
		}

		// Backend protocol events: look inside the variant
		const eventType = event.event_type ?? event;
		const key = Object.keys(eventType)[0];
		if (key) {
			const data = eventType[key];
			if (!data) return null;
			for (const field of ['owner', 'caller', 'from', 'liquidator', 'redeemer', 'developer_principal']) {
				const val = data[field];
				if (val && typeof val === 'object' && typeof val.toText === 'function') return val.toText();
				// Handle Candid opt principal: arrives as [Principal] or []
				if (Array.isArray(val) && val.length > 0) {
					const inner = val[0];
					if (inner && typeof inner === 'object' && typeof inner.toText === 'function') return inner.toText();
					if (typeof inner === 'string' && inner.length > 10) return inner;
				}
				if (typeof val === 'string' && val.length > 20) return val;
			}
			// Check nested vault
			if (data.vault?.owner) {
				const owner = data.vault.owner;
				if (typeof owner === 'object' && typeof owner.toText === 'function') return owner.toText();
			}
			// Fall back to vault owner map
			if (data.vault_id != null) {
				const owner = vaultOwnerMap.get(Number(data.vault_id));
				if (owner) return owner;
			}
		}
		return null;
	}

	// ── Source label for non-backend event IDs ──
	const SOURCE_LABELS: Record<string, string> = {
		'3pool_swap': '3Pool',
		'amm_swap': 'AMM',
		'amm_liquidity': 'AMM',
		'amm_admin': 'AMM',
		'3pool_liquidity': '3Pool',
		'3pool_admin': '3Pool',
		'stability_pool': 'SP',
		'multi_hop_swap': 'Swap',
	};

	// 3Pool token index → symbol/decimals for multi-hop merging
	const THREEPOOL_TOKENS: { symbol: string; decimals: number }[] = [
		{ symbol: 'icUSD', decimals: 8 },
		{ symbol: 'ckUSDT', decimals: 6 },
		{ symbol: 'ckUSDC', decimals: 6 },
	];

	// 3USD LP token principal (the 3pool canister itself)
	const THREEPOOL_PRINCIPAL = CANISTER_IDS.THREEPOOL;

	/**
	 * Merge correlated multi-hop swap events into single display events.
	 *
	 * Detects pairs where a 3pool_liquidity event and an amm_swap event share
	 * the same caller and occur within 10 seconds, indicating a two-leg swap
	 * routed through the swap router.
	 *
	 * stable_to_icp: AddLiquidity (stablecoin → 3USD) + AMM swap (3USD → ICP)
	 * icp_to_stable: AMM swap (ICP → 3USD) + RemoveOneCoin (3USD → stablecoin)
	 */
	function mergeMultiHopEvents(events: DisplayEvent[]): DisplayEvent[] {
		const MAX_GAP_NS = 10_000_000_000; // 10 seconds in nanoseconds

		// Index liquidity and AMM swap events by caller for fast lookup
		const liqEvents: DisplayEvent[] = [];
		const ammEvents: DisplayEvent[] = [];
		for (const de of events) {
			if (de.source === '3pool_liquidity') liqEvents.push(de);
			else if (de.source === 'amm_swap') ammEvents.push(de);
		}

		if (liqEvents.length === 0 || ammEvents.length === 0) return events;

		const mergedSet = new Set<DisplayEvent>();
		const mergedResults: DisplayEvent[] = [];

		for (const liq of liqEvents) {
			const liqCaller = liq.event.caller?.toText?.() ?? '';
			if (!liqCaller) continue;

			const action = liq.event.action ? Object.keys(liq.event.action)[0] : '';
			const isAdd = action === 'AddLiquidity';
			const isRemove = action === 'RemoveOneCoin';
			if (!isAdd && !isRemove) continue;

			// Find matching AMM swap: same caller, close timestamp
			for (const amm of ammEvents) {
				if (mergedSet.has(amm)) continue;
				const ammCaller = amm.event.caller?.toText?.() ?? '';
				if (ammCaller !== liqCaller) continue;

				const gap = Math.abs(liq.timestamp - amm.timestamp);
				if (gap > MAX_GAP_NS) continue;

				// Verify the AMM swap involves 3USD LP token
				const ammTokenIn = amm.event.token_in?.toText?.() ?? '';
				const ammTokenOut = amm.event.token_out?.toText?.() ?? '';
				const ammInvolves3USD = ammTokenIn === THREEPOOL_PRINCIPAL || ammTokenOut === THREEPOOL_PRINCIPAL;
				if (!ammInvolves3USD) continue;

				if (isAdd && ammTokenIn === THREEPOOL_PRINCIPAL) {
					// stable_to_icp: AddLiquidity then AMM swap (3USD → other)
					const amounts = liq.event.amounts ?? [];
					let stableIdx = -1;
					for (let i = 0; i < amounts.length; i++) {
						if (Number(amounts[i]) > 0) { stableIdx = i; break; }
					}
					if (stableIdx < 0) continue;

					const stableToken = THREEPOOL_TOKENS[stableIdx];
					const stableAmount = formatE8s(amounts[stableIdx], stableToken.decimals);
					const threeUsdAmount = formatE8s(liq.event.lp_amount, 8);
					const finalSym = getTokenSymbol(ammTokenOut);
					const finalAmount = amm.event.amount_out != null ? formatTokenAmount(BigInt(amm.event.amount_out), ammTokenOut) : '?';

					mergedSet.add(liq);
					mergedSet.add(amm);
					mergedResults.push({
						globalIndex: amm.globalIndex,
						source: 'multi_hop_swap',
						timestamp: Math.max(liq.timestamp, amm.timestamp),
						event: {
							direction: 'stable_to_icp',
							liqEvent: liq.event,
							ammEvent: amm.event,
							stablecoinSymbol: stableToken.symbol,
							stablecoinAmount: stableAmount,
							threeUsdAmount,
							finalSymbol: finalSym,
							finalAmount,
						},
					});
					break;
				} else if (isRemove && ammTokenOut === THREEPOOL_PRINCIPAL) {
					// icp_to_stable: AMM swap (other → 3USD) then RemoveOneCoin
					const coinIndex = liq.event.coin_index?.[0] ?? 0;
					const stableToken = THREEPOOL_TOKENS[coinIndex] ?? THREEPOOL_TOKENS[0];
					const amounts = liq.event.amounts ?? [];
					const stableAmount = amounts[coinIndex] != null ? formatE8s(amounts[coinIndex], stableToken.decimals) : '?';
					const threeUsdAmount = formatE8s(liq.event.lp_amount, 8);
					const otherToken = amm.event.token_in?.toText?.() ?? '';
					const otherSym = getTokenSymbol(otherToken);
					const otherAmount = amm.event.amount_in != null ? formatTokenAmount(BigInt(amm.event.amount_in), otherToken) : '?';

					mergedSet.add(liq);
					mergedSet.add(amm);
					mergedResults.push({
						globalIndex: amm.globalIndex,
						source: 'multi_hop_swap',
						timestamp: Math.max(liq.timestamp, amm.timestamp),
						event: {
							direction: 'icp_to_stable',
							liqEvent: liq.event,
							ammEvent: amm.event,
							stablecoinSymbol: stableToken.symbol,
							stablecoinAmount: stableAmount,
							threeUsdAmount,
							finalSymbol: otherSym,
							finalAmount: otherAmount,
						},
					});
					break;
				}
			}
		}

		if (mergedResults.length === 0) return events;

		// Rebuild: keep unmerged events, add merged ones
		const result = events.filter(e => !mergedSet.has(e));
		result.push(...mergedResults);
		result.sort((a, b) => b.timestamp - a.timestamp);
		return result;
	}

	// ── Format event for display ──

	function formatDisplayEvent(de: DisplayEvent): { summary: string; typeName: string; badgeColor: string } {
		switch (de.source) {
			case '3pool_swap': return formatSwapEvent(de.event);
			case 'amm_swap': return formatAmmSwapEvent(de.event);
			case 'amm_liquidity': return formatAmmLiquidityEvent(de.event);
			case 'amm_admin': return formatAmmAdminEvent(de.event);
			case '3pool_liquidity': return format3PoolLiquidityEvent(de.event);
			case '3pool_admin': return format3PoolAdminEvent(de.event);
			case 'stability_pool': return formatStabilityPoolEvent(de.event);
			case 'multi_hop_swap': return formatMultiHopSwapEvent(de.event);
			default: return { summary: '', typeName: '', badgeColor: '' }; // handled by EventRow
		}
	}

	// ── Load functions ──

	async function loadAllPage(p: number) {
		loading = true;
		error = null;
		try {
			// Fetch ALL backend events (batched) and all other event source counts in parallel
			// Backend caps page size at 200 in get_events_filtered.
			const BACKEND_PAGE_SIZE = 200;
			const [firstBatch, threePoolSwapCount, ammSwapCount, ammLiqCount, threePoolLiqCount, ammAdminCount, threePoolAdminCount, spCount] = await Promise.all([
				fetchEvents(0n, BigInt(BACKEND_PAGE_SIZE)),
				fetchSwapEventCount(),
				fetchAmmSwapEventCount(),
				fetchAmmLiquidityEventCount(),
				fetch3PoolLiquidityEventCount(),
				fetchAmmAdminEventCount(),
				fetch3PoolAdminEventCount(),
				fetchStabilityPoolEventCount(),
			]);

			// Batch-fetch remaining backend events if any
			const allBackendEvents: [bigint, any][] = [...firstBatch.events];
			const backendTotal = Number(firstBatch.total);
			if (allBackendEvents.length < backendTotal) {
				const remaining: Promise<{ total: bigint; events: [bigint, any][] }>[] = [];
				for (let page = 1; page * BACKEND_PAGE_SIZE < backendTotal; page++) {
					remaining.push(fetchEvents(BigInt(page), BigInt(BACKEND_PAGE_SIZE)));
				}
				const batches = await Promise.all(remaining);
				for (const batch of batches) {
					allBackendEvents.push(...batch.events);
				}
			}

			// Fetch all non-backend events (typically small datasets)
			const [threePoolSwaps, ammSwaps, ammLiqEvents, threePoolLiqEvents, ammAdminEvts, threePoolAdminEvts, spEvents] = await Promise.all([
				Number(threePoolSwapCount) > 0 ? fetchSwapEvents(0n, threePoolSwapCount) : Promise.resolve([]),
				Number(ammSwapCount) > 0 ? fetchAmmSwapEvents(0n, ammSwapCount) : Promise.resolve([]),
				Number(ammLiqCount) > 0 ? fetchAmmLiquidityEvents(0n, ammLiqCount) : Promise.resolve([]),
				Number(threePoolLiqCount) > 0 ? fetch3PoolLiquidityEvents(threePoolLiqCount, 0n) : Promise.resolve([]),
				Number(ammAdminCount) > 0 ? fetchAmmAdminEvents(0n, ammAdminCount) : Promise.resolve([]),
				Number(threePoolAdminCount) > 0 ? fetch3PoolAdminEvents(0n, threePoolAdminCount) : Promise.resolve([]),
				Number(spCount) > 0 ? fetchStabilityPoolEvents(0n, spCount) : Promise.resolve([]),
			]);

			// Filter out InterestReceived from SP events
			const filteredSp = spEvents.filter((e: any) => {
				const et = e.event_type ?? {};
				return !('InterestReceived' in et);
			});

			// Merge all into DisplayEvent[]
			const all: DisplayEvent[] = [];

			for (const [idx, evt] of allBackendEvents) {
				all.push({ globalIndex: idx, event: evt, source: 'backend', timestamp: extractTimestamp(evt) });
			}
			for (const e of threePoolSwaps) {
				all.push({ globalIndex: BigInt(e.id ?? 0), event: e, source: '3pool_swap', timestamp: extractTimestamp(e) });
			}
			for (const e of ammSwaps) {
				all.push({ globalIndex: BigInt(e.id ?? 0), event: e, source: 'amm_swap', timestamp: extractTimestamp(e) });
			}
			for (const e of ammLiqEvents) {
				all.push({ globalIndex: BigInt(e.id ?? 0), event: e, source: 'amm_liquidity', timestamp: extractTimestamp(e) });
			}
			for (const e of threePoolLiqEvents) {
				all.push({ globalIndex: BigInt(e.id ?? 0), event: e, source: '3pool_liquidity', timestamp: extractTimestamp(e) });
			}
			for (const e of ammAdminEvts) {
				all.push({ globalIndex: BigInt(e.id ?? 0), event: e, source: 'amm_admin', timestamp: extractTimestamp(e) });
			}
			for (const e of threePoolAdminEvts) {
				all.push({ globalIndex: BigInt(e.id ?? 0), event: e, source: '3pool_admin', timestamp: extractTimestamp(e) });
			}
			for (const e of filteredSp) {
				all.push({ globalIndex: BigInt(e.id ?? 0), event: e, source: 'stability_pool', timestamp: extractTimestamp(e) });
			}

			// Merge multi-hop swaps, then sort by timestamp descending
			const merged = mergeMultiHopEvents(all);
			merged.sort((a, b) => b.timestamp - a.timestamp);

			totalCount = merged.length;
			const start = p * PAGE_SIZE;
			displayEvents = merged.slice(start, start + PAGE_SIZE);
			currentPage = p;
		} catch (e) {
			error = 'Failed to load events.';
			console.error('[activity] loadAllPage error:', e);
		} finally {
			loading = false;
		}
	}

	async function loadProtocolPage(p: number, filterCategory: string) {
		loading = true;
		error = null;
		try {
			// Fetch ALL backend events so we can filter by category client-side
			// without the old bug of empty pages from filtering paginated data.
			// Backend caps page size at 200 in get_events_filtered.
			const batchSize = 200;
			const allEvents: [bigint, any][] = [];
			let pageNum = 0;
			let filteredTotal = Infinity;
			while (allEvents.length < filteredTotal) {
				const batch = await fetchEvents(BigInt(pageNum), BigInt(batchSize));
				filteredTotal = Number(batch.total);
				allEvents.push(...batch.events);
				pageNum++;
				if (batch.events.length === 0) break; // safety: backend returned nothing
			}

			// Apply category filter
			let filtered: DisplayEvent[];
			if (filterCategory === 'liquidations') {
				filtered = allEvents
					.filter(([_, event]) => {
						const category = getEventCategory(event);
						return category === 'liquidation' || category === 'redemption';
					})
					.map(([idx, event]) => ({
						globalIndex: idx,
						event,
						source: 'backend' as const,
						timestamp: extractTimestamp(event),
					}));
			} else if (filterCategory === 'vault_ops') {
				filtered = allEvents
					.filter(([_, event]) => getEventCategory(event) === 'vault_ops')
					.map(([idx, event]) => ({
						globalIndex: idx,
						event,
						source: 'backend' as const,
						timestamp: extractTimestamp(event),
					}));
			} else if (filterCategory === 'system') {
				// Show backend admin+system events AND AMM/3Pool admin events
				const backendAdminSystem = allEvents
					.filter(([_, event]) => {
						const category = getEventCategory(event);
						return category === 'admin' || category === 'system';
					})
					.map(([idx, event]) => ({
						globalIndex: idx,
						event,
						source: 'backend' as const,
						timestamp: extractTimestamp(event),
					}));

				// Also fetch AMM and 3Pool admin events
				const [ammAdminCount, threePoolAdminCount] = await Promise.all([
					fetchAmmAdminEventCount(),
					fetch3PoolAdminEventCount(),
				]);
				const [ammAdminEvts, threePoolAdminEvts] = await Promise.all([
					Number(ammAdminCount) > 0 ? fetchAmmAdminEvents(0n, ammAdminCount) : Promise.resolve([]),
					Number(threePoolAdminCount) > 0 ? fetch3PoolAdminEvents(0n, threePoolAdminCount) : Promise.resolve([]),
				]);

				const adminEvents: DisplayEvent[] = [
					...backendAdminSystem,
					...ammAdminEvts.map((e: any) => ({
						globalIndex: BigInt(e.id ?? 0), event: e, source: 'amm_admin' as const, timestamp: extractTimestamp(e)
					})),
					...threePoolAdminEvts.map((e: any) => ({
						globalIndex: BigInt(e.id ?? 0), event: e, source: '3pool_admin' as const, timestamp: extractTimestamp(e)
					})),
				];
				adminEvents.sort((a, b) => b.timestamp - a.timestamp);
				filtered = adminEvents;
			} else {
				filtered = allEvents.map(([idx, event]) => ({
					globalIndex: idx,
					event,
					source: 'backend' as const,
					timestamp: extractTimestamp(event),
				}));
			}

			// Sort descending by timestamp
			filtered.sort((a, b) => b.timestamp - a.timestamp);

			totalCount = filtered.length;
			const start = p * PAGE_SIZE;
			displayEvents = filtered.slice(start, start + PAGE_SIZE);
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
			// Fetch ALL dex event counts (swaps + liquidity from both pools)
			const [
				threePoolSwapCount, ammSwapCount,
				ammLiqCount, threePoolLiqCount
			] = await Promise.all([
				fetchSwapEventCount(),
				fetchAmmSwapEventCount(),
				fetchAmmLiquidityEventCount(),
				fetch3PoolLiquidityEventCount(),
			]);

			// Fetch all events
			const [threePoolSwaps, ammSwaps, ammLiqEvents, threePoolLiqEvents] = await Promise.all([
				Number(threePoolSwapCount) > 0 ? fetchSwapEvents(0n, threePoolSwapCount) : Promise.resolve([]),
				Number(ammSwapCount) > 0 ? fetchAmmSwapEvents(0n, ammSwapCount) : Promise.resolve([]),
				Number(ammLiqCount) > 0 ? fetchAmmLiquidityEvents(0n, ammLiqCount) : Promise.resolve([]),
				Number(threePoolLiqCount) > 0 ? fetch3PoolLiquidityEvents(threePoolLiqCount, 0n) : Promise.resolve([]),
			]);

			// Tag all events
			const tagged: DisplayEvent[] = [
				...threePoolSwaps.map((e: any) => ({
					globalIndex: BigInt(e.id ?? 0), event: e, source: '3pool_swap' as const, timestamp: extractTimestamp(e)
				})),
				...ammSwaps.map((e: any) => ({
					globalIndex: BigInt(e.id ?? 0), event: e, source: 'amm_swap' as const, timestamp: extractTimestamp(e)
				})),
				...ammLiqEvents.map((e: any) => ({
					globalIndex: BigInt(e.id ?? 0), event: e, source: 'amm_liquidity' as const, timestamp: extractTimestamp(e)
				})),
				...threePoolLiqEvents.map((e: any) => ({
					globalIndex: BigInt(e.id ?? 0), event: e, source: '3pool_liquidity' as const, timestamp: extractTimestamp(e)
				})),
			];

			// Merge multi-hop swaps, then sort by timestamp descending
			const merged = mergeMultiHopEvents(tagged);
			merged.sort((a, b) => b.timestamp - a.timestamp);

			totalCount = merged.length;
			const start = p * PAGE_SIZE;
			displayEvents = merged.slice(start, start + PAGE_SIZE);
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
			const total = Number(count);

			if (total === 0) {
				displayEvents = [];
				totalCount = 0;
				currentPage = p;
				return;
			}

			// Fetch ALL stability pool events, then filter out InterestReceived
			const allEvents = await fetchStabilityPoolEvents(0n, BigInt(total));
			const filtered = allEvents.filter((evt: any) => {
				const et = evt.event_type ?? {};
				return !('InterestReceived' in et);
			});

			// Sort descending by id (newest first)
			filtered.sort((a: any, b: any) => Number(b.id ?? 0) - Number(a.id ?? 0));

			totalCount = filtered.length;
			const start = p * PAGE_SIZE;
			const pageEvents = filtered.slice(start, start + PAGE_SIZE);
			displayEvents = pageEvents.map((evt: any) => ({
				globalIndex: BigInt(evt.id ?? 0),
				event: evt,
				source: 'stability_pool' as const,
				timestamp: extractTimestamp(evt),
			}));
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
		if (selectedFilter === 'all') loadAllPage(p);
		else if (selectedFilter === 'dex') loadDexPage(p);
		else if (selectedFilter === 'stability_pool') loadStabilityPoolPage(p);
		else loadProtocolPage(p, selectedFilter);
	}

	function handleFilterChange(key: ActivityFilter) {
		selectedFilter = key;
		if (key === 'liquidations') loadLiquidationStats();
		loadPage(0);
	}

	// Format timestamp for display
	function formatTimeAgo(ts: number): string {
		const nsTs = ts > 1e15 ? ts : ts * 1e9;
		const s = Math.floor((Date.now() - nsTs / 1e6) / 1000);
		if (s < 0) return 'just now';
		if (s < 60) return `${s}s ago`;
		if (s < 3600) return `${Math.floor(s / 60)}m ago`;
		if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
		return `${Math.floor(s / 86400)}d ago`;
	}

	function formatTimestampISO(ts: number): string {
		const nsTs = ts > 1e15 ? ts : ts * 1e9;
		return new Date(nsTs / 1e6).toISOString();
	}

	// Check URL params for initial filter
	onMount(async () => {
		// Load vault collateral type map for proper event formatting
		try {
			const vaults = await fetchAllVaults();
			const collMap = new Map<number, string>();
			const ownerMap = new Map<number, string>();
			for (const v of vaults) {
				const id = Number(v.vault_id);
				const collType = v.collateral_type?.toText?.() ?? String(v.collateral_type ?? '');
				if (collType) collMap.set(id, collType);
				const owner = v.owner?.toText?.() ?? (typeof v.owner === 'string' ? v.owner : '');
				if (owner) ownerMap.set(id, owner);
			}
			vaultCollateralMap = collMap;
			vaultOwnerMap = ownerMap;
		} catch (e) {
			console.error('[activity] Failed to load vault maps:', e);
		}

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
				value={displayEvents.length > 0 ? String(totalCount) : '—'}
				subtitle={totalCount > 0 ? 'Total' : ''}
			/>
			<StatCard
				label="Liquidatable Vaults"
				value={String(liquidatableVaults.length)}
				subtitle={liquidatableVaults.length === 0 ? 'None at risk' : ''}
			/>
			<StatCard
				label="Bot Budget Remaining"
				value={botStats ? `${formatE8s(botStats.budget_remaining_e8s)} ICP` : '—'}
				subtitle="Available for bot"
			/>
			<StatCard
				label="Bot Debt Covered"
				value={botStats ? `${formatE8s(botStats.total_debt_covered_e8s)} icUSD` : '—'}
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
	{:else if displayEvents.length === 0}
		<div class="text-center py-16">
			<p class="text-gray-500 text-sm">
				{selectedFilter === 'all' ? 'No events found.' : `No ${FILTERS.find(f => f.key === selectedFilter)?.label ?? ''} events found.`}
			</p>
		</div>

	<!-- Events table -->
	{:else}
		<div class="explorer-card overflow-hidden p-0">
			<table class="w-full">
				<thead>
					<tr class="border-b border-gray-700/50 text-left">
						<th class="px-4 py-3 text-xs font-medium text-gray-500 uppercase tracking-wider w-[5rem]">#</th>
						<th class="px-4 py-3 text-xs font-medium text-gray-500 uppercase tracking-wider w-[7rem]">Time</th>
						<th class="px-4 py-3 text-xs font-medium text-gray-500 uppercase tracking-wider w-[8rem]">Principal</th>
						<th class="px-4 py-3 text-xs font-medium text-gray-500 uppercase tracking-wider w-[10rem]">Type</th>
						<th class="px-4 py-3 text-xs font-medium text-gray-500 uppercase tracking-wider">Summary</th>
						<th class="px-4 py-3 text-xs font-medium text-gray-500 uppercase tracking-wider w-[5rem] text-right">Details</th>
					</tr>
				</thead>
				<tbody>
					{#each displayEvents as de (String(de.globalIndex) + de.source)}
						{#if de.source === 'backend'}
							<!-- Backend events use EventRow which has its own formatting -->
							<EventRow event={de.event} index={Number(de.globalIndex)} {vaultCollateralMap} {vaultOwnerMap} />
						{:else}
							<!-- Non-backend events use the unified rendering -->
							{@const formatted = formatDisplayEvent(de)}
							{@const principal = extractPrincipal(de.event, de.source)}
							{@const sourceLabel = SOURCE_LABELS[de.source] ?? de.source}
							{@const detailHref = de.source === 'multi_hop_swap'
								? `/explorer/dex/amm_swap/${de.event.ammEvent?.id ?? Number(de.globalIndex)}`
								: `/explorer/dex/${de.source}/${Number(de.globalIndex)}`}
							<tr class="border-b border-gray-700/50 hover:bg-gray-800/30 transition-colors group">
								<td class="px-4 py-3">
									<a href={detailHref} class="text-xs text-blue-400 hover:text-blue-300 font-mono" title="{sourceLabel} Event #{Number(de.globalIndex)}">{sourceLabel} #{Number(de.globalIndex)}</a>
								</td>
								<td class="px-4 py-3 text-xs text-gray-500 whitespace-nowrap">
									{#if de.timestamp}
										<span title={formatTimestampISO(de.timestamp)}>{formatTimeAgo(de.timestamp)}</span>
									{:else}
										<span class="text-gray-600">&mdash;</span>
									{/if}
								</td>
								<td class="px-4 py-3 text-xs text-gray-400 whitespace-nowrap">
									{#if principal}
										<a href="/explorer/address/{principal}" class="hover:text-blue-400 transition-colors font-mono">
											{shortenPrincipal(principal)}
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
									<a
										href={detailHref}
										class="text-xs text-blue-400 hover:text-blue-300 opacity-0 group-hover:opacity-100 transition-opacity whitespace-nowrap"
									>
										Details &rarr;
									</a>
								</td>
							</tr>
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
