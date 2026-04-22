<script lang="ts">
	import { onMount } from 'svelte';
	import { goto } from '$app/navigation';
	import { page } from '$app/stores';
	import MixedEventsTable from '$components/explorer/MixedEventsTable.svelte';
	import FacetBar from '$components/explorer/FacetBar.svelte';
	import ActiveFilterChips from '$components/explorer/ActiveFilterChips.svelte';
	import SavedViewsStrip, { type SavedView } from '$components/explorer/SavedViewsStrip.svelte';
	import {
		fetchEvents,
		fetchSwapEvents, fetchSwapEventCount,
		fetchAmmSwapEvents, fetchAmmSwapEventCount,
		fetchAmmLiquidityEvents, fetchAmmLiquidityEventCount,
		fetchAmmAdminEvents, fetchAmmAdminEventCount,
		fetch3PoolLiquidityEvents, fetch3PoolLiquidityEventCount,
		fetch3PoolAdminEvents, fetch3PoolAdminEventCount,
		fetchStabilityPoolEvents, fetchStabilityPoolEventCount,
		fetchAllVaults,
		fetchAmmPools,
		fetchCollateralPrices,
	} from '$services/explorer/explorerService';
	import { extractEventTimestamp, displayEvent } from '$utils/displayEvent';
	import type { DisplayEvent } from '$utils/displayEvent';
	import { formatE8s, formatTokenAmount, getTokenSymbol, KNOWN_TOKENS } from '$utils/explorerHelpers';
	import { CANISTER_IDS } from '$lib/config';
	import {
		extractFacets,
		matchesFacets,
		parseFacetsFromUrl,
		buildFacetsQueryString,
		emptyFacets,
		hasAnyFacet,
		type Facets,
	} from '$utils/eventFacets';

	const INITIAL_ROWS = 100;
	const PAGE_STEP = 100;

	// ── State ───────────────────────────────────────────────────────────

	let allEvents: DisplayEvent[] = $state([]);
	let eventFacetsByIndex: Map<string, ReturnType<typeof extractFacets>> = $state(new Map());
	let vaultCollateralMap: Map<number, string> = $state(new Map());
	let vaultOwnerMap: Map<number, string> = $state(new Map());
	let priceMap: Map<string, number> = $state(new Map());
	let tokenOptions: { principal: string; label: string }[] = $state([]);
	let poolOptions: { id: string; label: string }[] = $state([]);

	let loading = $state(true);
	let error: string | null = $state(null);
	let visibleRows = $state(INITIAL_ROWS);

	let savedViews: SavedView[] = $state([]);

	// ── URL-derived facets (reactive via $page) ─────────────────────────

	let facets: Facets = $derived(parseFacetsFromUrl($page.url));
	let currentParams = $derived($page.url.search);
	let sortOrder: 'newest' | 'oldest' = $state<'newest' | 'oldest'>('newest');

	// ── Derived: filtered + sorted + sliced for render ──────────────────

	const filteredEvents = $derived.by(() => {
		if (allEvents.length === 0) return [] as DisplayEvent[];
		const noFacets = !hasAnyFacet(facets);
		if (noFacets) return allEvents;
		return allEvents.filter((de) => {
			const key = `${de.source}:${String(de.globalIndex)}`;
			const ef = eventFacetsByIndex.get(key);
			if (!ef) return false;
			return matchesFacets(ef, facets);
		});
	});

	const sortedEvents = $derived(
		sortOrder === 'oldest'
			? [...filteredEvents].sort((a, b) => a.timestamp - b.timestamp)
			: filteredEvents,
	);

	const visibleEvents = $derived(sortedEvents.slice(0, visibleRows));

	// Reset the visible window whenever the underlying filter changes.
	$effect(() => {
		// Track `currentParams` so edits to facets collapse the window back to INITIAL_ROWS
		void currentParams;
		void sortOrder;
		visibleRows = INITIAL_ROWS;
	});

	// ── URL writer ───────────────────────────────────────────────────────

	function applyFacets(next: Facets) {
		const q = buildFacetsQueryString(next);
		const path = `/explorer/activity${q}`;
		goto(path, { keepFocus: true, noScroll: true, replaceState: false });
	}

	function clearAll() {
		applyFacets(emptyFacets());
	}

	// ── Saved views (localStorage) ──────────────────────────────────────

	const SAVED_VIEWS_KEY = 'rumi.explorer.activity.savedViews';

	function loadSavedViews() {
		if (typeof localStorage === 'undefined') return;
		try {
			const raw = localStorage.getItem(SAVED_VIEWS_KEY);
			if (!raw) return;
			const parsed = JSON.parse(raw);
			if (Array.isArray(parsed)) {
				savedViews = parsed.filter((v) => v && typeof v.id === 'string' && typeof v.name === 'string' && typeof v.params === 'string');
			}
		} catch (e) {
			console.warn('[activity] Failed to parse saved views:', e);
		}
	}

	function persistSavedViews() {
		if (typeof localStorage === 'undefined') return;
		try {
			localStorage.setItem(SAVED_VIEWS_KEY, JSON.stringify(savedViews));
		} catch (e) {
			console.warn('[activity] Failed to persist saved views:', e);
		}
	}

	function saveCurrentView() {
		if (typeof window === 'undefined') return;
		if (!hasAnyFacet(facets)) {
			window.alert('Add at least one filter before saving a view.');
			return;
		}
		const name = window.prompt('Name this saved view');
		if (!name || !name.trim()) return;
		const view: SavedView = {
			id: `v_${Date.now().toString(36)}`,
			name: name.trim(),
			params: currentParams || '',
		};
		savedViews = [...savedViews, view];
		persistSavedViews();
	}

	function applySavedView(v: SavedView) {
		goto(`/explorer/activity${v.params}`, { keepFocus: true, noScroll: true, replaceState: false });
	}

	function renameSavedView(id: string, name: string) {
		savedViews = savedViews.map((v) => (v.id === id ? { ...v, name } : v));
		persistSavedViews();
	}

	function deleteSavedView(id: string) {
		savedViews = savedViews.filter((v) => v.id !== id);
		persistSavedViews();
	}

	// ── CSV export ──────────────────────────────────────────────────────

	function csvEscape(val: unknown): string {
		if (val == null) return '';
		const s = typeof val === 'string' ? val : String(val);
		if (s.includes(',') || s.includes('"') || s.includes('\n')) {
			return `"${s.replace(/"/g, '""')}"`;
		}
		return s;
	}

	function bigintReplacer(_key: string, value: unknown): unknown {
		if (typeof value === 'bigint') return value.toString();
		if (value && typeof value === 'object' && typeof (value as any).toText === 'function') {
			return (value as any).toText();
		}
		return value;
	}

	function exportCsv() {
		if (filteredEvents.length === 0) return;
		const rows: string[] = [];
		rows.push(['timestamp', 'source', 'type', 'size_usd', 'principal', 'vault', 'token', 'pool', 'summary', 'raw'].map(csvEscape).join(','));
		for (const de of filteredEvents) {
			const key = `${de.source}:${String(de.globalIndex)}`;
			const ef = eventFacetsByIndex.get(key);
			const display = displayEvent(de, { vaultCollateralMap, vaultOwnerMap });
			const timestamp = de.timestamp > 0 ? new Date(de.timestamp / 1_000_000).toISOString() : '';
			const principal = ef?.principals.join('|') ?? '';
			const vault = ef?.vaultIds.join('|') ?? '';
			const token = (ef?.tokens ?? []).map((p) => getTokenSymbol(p)).join('|');
			const pool = ef?.pools.join('|') ?? '';
			const sizeUsd = ef?.sizeUsd != null ? ef.sizeUsd.toFixed(2) : '';
			let raw = '';
			try {
				raw = JSON.stringify(de.event, bigintReplacer);
			} catch {
				raw = '';
			}
			rows.push([
				csvEscape(timestamp),
				csvEscape(de.source),
				csvEscape(ef?.typeKey ?? ''),
				csvEscape(sizeUsd),
				csvEscape(principal),
				csvEscape(vault),
				csvEscape(token),
				csvEscape(pool),
				csvEscape(display.formatted.summary),
				csvEscape(raw),
			].join(','));
		}
		const blob = new Blob([rows.join('\n')], { type: 'text/csv;charset=utf-8' });
		const url = URL.createObjectURL(blob);
		const a = document.createElement('a');
		const stamp = new Date().toISOString().replace(/[:.]/g, '-');
		a.href = url;
		a.download = `rumi-activity-${stamp}.csv`;
		document.body.appendChild(a);
		a.click();
		document.body.removeChild(a);
		URL.revokeObjectURL(url);
	}

	// ── Multi-hop swap merge (same logic as before) ────────────────────

	const THREEPOOL_TOKENS: { symbol: string; decimals: number }[] = [
		{ symbol: 'icUSD', decimals: 8 },
		{ symbol: 'ckUSDT', decimals: 6 },
		{ symbol: 'ckUSDC', decimals: 6 },
	];
	const THREEPOOL_PRINCIPAL = CANISTER_IDS.THREEPOOL;

	function mergeMultiHopEvents(events: DisplayEvent[]): DisplayEvent[] {
		const MAX_GAP_NS = 10_000_000_000;
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

			for (const amm of ammEvents) {
				if (mergedSet.has(amm)) continue;
				const ammCaller = amm.event.caller?.toText?.() ?? '';
				if (ammCaller !== liqCaller) continue;
				const gap = Math.abs(liq.timestamp - amm.timestamp);
				if (gap > MAX_GAP_NS) continue;

				const ammTokenIn = amm.event.token_in?.toText?.() ?? '';
				const ammTokenOut = amm.event.token_out?.toText?.() ?? '';
				const ammInvolves3USD = ammTokenIn === THREEPOOL_PRINCIPAL || ammTokenOut === THREEPOOL_PRINCIPAL;
				if (!ammInvolves3USD) continue;

				if (isAdd && ammTokenIn === THREEPOOL_PRINCIPAL) {
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
		const result = events.filter((e) => !mergedSet.has(e));
		result.push(...mergedResults);
		result.sort((a, b) => b.timestamp - a.timestamp);
		return result;
	}

	// ── Load everything once on mount ────────────────────────────────────

	async function loadEverything() {
		loading = true;
		error = null;
		try {
			const BACKEND_PAGE_SIZE = 200;
			const [firstBatch, threePoolSwapCount, ammSwapCount, ammLiqCount, threePoolLiqCount, ammAdminCount, threePoolAdminCount, spCount, ammPools, prices] = await Promise.all([
				fetchEvents(0n, BigInt(BACKEND_PAGE_SIZE)),
				fetchSwapEventCount(),
				fetchAmmSwapEventCount(),
				fetchAmmLiquidityEventCount(),
				fetch3PoolLiquidityEventCount(),
				fetchAmmAdminEventCount(),
				fetch3PoolAdminEventCount(),
				fetchStabilityPoolEventCount(),
				fetchAmmPools(),
				fetchCollateralPrices(),
			]);

			priceMap = prices;

			// Remaining backend pages
			const allBackendEvents: [bigint, any][] = [...firstBatch.events];
			const backendTotal = Number(firstBatch.total);
			if (allBackendEvents.length < backendTotal) {
				const remaining: Promise<{ total: bigint; events: [bigint, any][] }>[] = [];
				for (let p = 1; p * BACKEND_PAGE_SIZE < backendTotal; p++) {
					remaining.push(fetchEvents(BigInt(p), BigInt(BACKEND_PAGE_SIZE)));
				}
				const batches = await Promise.all(remaining);
				for (const batch of batches) allBackendEvents.push(...batch.events);
			}

			const [threePoolSwaps, ammSwaps, ammLiqEvents, threePoolLiqEvents, ammAdminEvts, threePoolAdminEvts, spEvents] = await Promise.all([
				Number(threePoolSwapCount) > 0 ? fetchSwapEvents(0n, threePoolSwapCount) : Promise.resolve([]),
				Number(ammSwapCount) > 0 ? fetchAmmSwapEvents(0n, ammSwapCount) : Promise.resolve([]),
				Number(ammLiqCount) > 0 ? fetchAmmLiquidityEvents(0n, ammLiqCount) : Promise.resolve([]),
				Number(threePoolLiqCount) > 0 ? fetch3PoolLiquidityEvents(threePoolLiqCount, 0n) : Promise.resolve([]),
				Number(ammAdminCount) > 0 ? fetchAmmAdminEvents(0n, ammAdminCount) : Promise.resolve([]),
				Number(threePoolAdminCount) > 0 ? fetch3PoolAdminEvents(0n, threePoolAdminCount) : Promise.resolve([]),
				Number(spCount) > 0 ? fetchStabilityPoolEvents(0n, spCount) : Promise.resolve([]),
			]);

			const filteredSp = spEvents.filter((e: any) => {
				const et = e.event_type ?? {};
				return !('InterestReceived' in et);
			});

			const all: DisplayEvent[] = [];
			for (const [idx, evt] of allBackendEvents) {
				all.push({ globalIndex: idx, event: evt, source: 'backend', timestamp: extractEventTimestamp(evt) });
			}
			for (const e of threePoolSwaps) all.push({ globalIndex: BigInt(e.id ?? 0), event: e, source: '3pool_swap', timestamp: extractEventTimestamp(e) });
			for (const e of ammSwaps) all.push({ globalIndex: BigInt(e.id ?? 0), event: e, source: 'amm_swap', timestamp: extractEventTimestamp(e) });
			for (const e of ammLiqEvents) all.push({ globalIndex: BigInt(e.id ?? 0), event: e, source: 'amm_liquidity', timestamp: extractEventTimestamp(e) });
			for (const e of threePoolLiqEvents) all.push({ globalIndex: BigInt(e.id ?? 0), event: e, source: '3pool_liquidity', timestamp: extractEventTimestamp(e) });
			for (const e of ammAdminEvts) all.push({ globalIndex: BigInt(e.id ?? 0), event: e, source: 'amm_admin', timestamp: extractEventTimestamp(e) });
			for (const e of threePoolAdminEvts) all.push({ globalIndex: BigInt(e.id ?? 0), event: e, source: '3pool_admin', timestamp: extractEventTimestamp(e) });
			for (const e of filteredSp) all.push({ globalIndex: BigInt(e.id ?? 0), event: e, source: 'stability_pool', timestamp: extractEventTimestamp(e) });

			const merged = mergeMultiHopEvents(all);
			merged.sort((a, b) => b.timestamp - a.timestamp);
			allEvents = merged;

			// Extract facets once per event
			const fmap = new Map<string, ReturnType<typeof extractFacets>>();
			for (const de of merged) {
				const key = `${de.source}:${String(de.globalIndex)}`;
				fmap.set(key, extractFacets(de, priceMap, vaultCollateralMap, vaultOwnerMap));
			}
			eventFacetsByIndex = fmap;

			// Build token + pool option lists from the observed data + known tokens
			const tokenSet = new Set<string>();
			const poolSet = new Set<string>();
			for (const ef of fmap.values()) {
				for (const t of ef.tokens) tokenSet.add(t);
				for (const p of ef.pools) poolSet.add(p);
			}
			// Always include the canonical Rumi tokens so the dropdown is complete
			for (const known of Object.keys(KNOWN_TOKENS)) tokenSet.add(known);

			tokenOptions = [...tokenSet]
				.map((principal) => ({ principal, label: getTokenSymbol(principal) }))
				.sort((a, b) => a.label.localeCompare(b.label));

			// Pool options: 3pool always first, then AMM pools
			const ammPoolIds: string[] = ammPools.map((p: any) => p.pool_id);
			for (const id of ammPoolIds) poolSet.add(id);
			poolSet.add('3pool');
			poolOptions = [...poolSet]
				.map((id) => {
					if (id === '3pool') return { id, label: 'Rumi 3Pool' };
					const pool = ammPools.find((p: any) => p.pool_id === id);
					if (pool) {
						const a = getTokenSymbol(pool.token_a?.toText?.() ?? '');
						const b = getTokenSymbol(pool.token_b?.toText?.() ?? '');
						return { id, label: `AMM · ${a}/${b} (${id})` };
					}
					return { id, label: id };
				})
				.sort((a, b) => a.label.localeCompare(b.label));
		} catch (e) {
			console.error('[activity] loadEverything error:', e);
			error = 'Failed to load events.';
		} finally {
			loading = false;
		}
	}

	async function loadVaultMaps() {
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
	}

	onMount(async () => {
		loadSavedViews();
		// Load vault maps first so the facet extractor can use them
		await loadVaultMaps();
		await loadEverything();
	});
</script>

<svelte:head>
	<title>Activity | Rumi Explorer</title>
</svelte:head>

<div class="max-w-[1100px] mx-auto px-4 py-8 space-y-4">
	<div class="flex items-baseline justify-between">
		<h1 class="text-2xl font-bold text-white">Activity</h1>
		{#if allEvents.length > 0}
			<span class="text-sm text-gray-500">
				{filteredEvents.length.toLocaleString()} of {allEvents.length.toLocaleString()} events
			</span>
		{/if}
	</div>

	<FacetBar {facets} {tokenOptions} {poolOptions} onChange={applyFacets} />

	<ActiveFilterChips {facets} onChange={applyFacets} onClear={clearAll} onSaveView={saveCurrentView} />

	<SavedViewsStrip
		views={savedViews}
		{currentParams}
		onApply={applySavedView}
		onRename={renameSavedView}
		onDelete={deleteSavedView}
	/>

	<div class="flex items-center justify-between pt-2">
		<div class="text-sm text-gray-400">
			{filteredEvents.length.toLocaleString()} {filteredEvents.length === 1 ? 'event' : 'events'}
			{#if filteredEvents.length > visibleRows}
				<span class="text-gray-500">· showing {visibleRows.toLocaleString()}</span>
			{/if}
		</div>
		<div class="flex items-center gap-2">
			<button
				type="button"
				class="px-3 py-1.5 text-xs rounded-md bg-gray-800/60 text-gray-200 border border-gray-700 hover:border-gray-500 disabled:opacity-50"
				disabled={filteredEvents.length === 0}
				onclick={exportCsv}
			>
				Export CSV
			</button>
			<label class="text-xs text-gray-400 flex items-center gap-1">
				Sort
				<select
					bind:value={sortOrder}
					class="px-2 py-1 bg-gray-800 border border-gray-700 rounded text-gray-200 text-xs"
				>
					<option value="newest">Newest</option>
					<option value="oldest">Oldest</option>
				</select>
			</label>
		</div>
	</div>

	{#if loading}
		<div class="flex items-center justify-center py-20">
			<div class="flex flex-col items-center gap-3">
				<div class="w-8 h-8 border-2 border-gray-600 border-t-blue-400 rounded-full animate-spin"></div>
				<span class="text-sm text-gray-500">Loading activity...</span>
			</div>
		</div>
	{:else if error}
		<div class="text-center py-16">
			<p class="text-red-400 text-sm">{error}</p>
		</div>
	{:else if visibleEvents.length === 0}
		<div class="text-center py-16">
			<p class="text-gray-500 text-sm">
				{hasAnyFacet(facets) ? 'No events match the current filters.' : 'No events found.'}
			</p>
		</div>
	{:else}
		<div class="explorer-card overflow-hidden p-0">
			<MixedEventsTable
				events={visibleEvents}
				{vaultCollateralMap}
				{vaultOwnerMap}
				onFacetClick={applyFacets}
				currentFacets={facets}
			/>
		</div>

		{#if filteredEvents.length > visibleRows}
			<div class="flex justify-center pt-2">
				<button
					type="button"
					class="px-4 py-2 text-sm rounded-md bg-gray-800/60 text-gray-200 border border-gray-700 hover:border-gray-500"
					onclick={() => { visibleRows = Math.min(visibleRows + PAGE_STEP, filteredEvents.length); }}
				>
					Load more
				</button>
			</div>
		{/if}
	{/if}
</div>
