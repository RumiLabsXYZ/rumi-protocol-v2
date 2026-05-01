import {
	formatEvent,
	formatSwapEvent, formatAmmSwapEvent,
	formatAmmLiquidityEvent, formatAmmAdminEvent,
	format3PoolLiquidityEvent, format3PoolAdminEvent,
	formatStabilityPoolEvent, formatMultiHopSwapEvent,
} from './explorerFormatters';
import type { FormattedEvent } from './explorerFormatters';
import { ammPoolShortLabel } from './ammNaming';

export type NonBackendSource =
	| '3pool_swap'
	| 'amm_swap'
	| 'amm_liquidity'
	| 'amm_admin'
	| '3pool_liquidity'
	| '3pool_admin'
	| 'stability_pool'
	| 'multi_hop_swap';

export type DisplayEventSource = 'backend' | NonBackendSource;

export interface DisplayEvent {
	globalIndex: bigint;
	event: any;
	source: DisplayEventSource;
	timestamp: number;
}

export const DEX_SOURCE_LABEL: Record<NonBackendSource, string> = {
	'3pool_swap': '3Pool',
	'amm_swap': 'AMM',
	'amm_liquidity': 'AMM',
	'amm_admin': 'AMM',
	'3pool_liquidity': '3Pool',
	'3pool_admin': '3Pool',
	'stability_pool': 'SP',
	'multi_hop_swap': 'Swap',
};

/**
 * Canister timestamps are nanoseconds. Backend events nest the timestamp inside
 * the variant data (e.g. `event.event_type.VaultCreated.timestamp`), while
 * non-backend events expose it at the top level.
 */
export function extractEventTimestamp(event: any): number {
	if (event?.timestamp != null) return Number(event.timestamp);
	const eventType = event?.event_type ?? event;
	if (!eventType) return 0;
	// Skip our `__ts_ns` overlay key when picking the variant tag — the
	// shallow clone fetchEvents creates places it alongside the variant key.
	const key = Object.keys(eventType).filter(k => k !== '__ts_ns')[0];
	if (!key) {
		return event?.__ts_ns != null ? Number(event.__ts_ns) : 0;
	}
	const data = eventType[key];
	if (data?.timestamp != null) {
		// Backend events use `opt nat64` for timestamps which the JS bindings
		// surface as either the bare value or a [value]/[]-wrapped option.
		const t = Array.isArray(data.timestamp) ? data.timestamp[0] : data.timestamp;
		if (t != null) return Number(t);
	}
	// Fallback to the parallel EVENT_TIMESTAMPS side log (see fetchEvents):
	// admin / upgrade / set_* variants don't carry an inline timestamp, so
	// the explorer uses the recording-time timestamp from the side log.
	if (event?.__ts_ns != null) return Number(event.__ts_ns);
	return 0;
}

/**
 * Pulls the most relevant principal out of an event for "show whose event this is"
 * purposes. Handles backend variant events, non-backend events with a top-level
 * `caller`, multi-hop swaps (caller nested in the inner AMM event), and optionally
 * a vault_id → owner fallback map for vault-related backend events.
 */
export function extractEventPrincipal(
	event: any,
	source: DisplayEventSource,
	vaultOwnerMap?: Map<number, string>,
): string | null {
	// Multi-hop: caller lives in the nested AMM or liquidity event
	if (source === 'multi_hop_swap') {
		const caller = event?.ammEvent?.caller ?? event?.liqEvent?.caller;
		if (caller?.toText) return caller.toText();
		if (typeof caller === 'string' && caller.length > 10) return caller;
		return null;
	}

	// Top-level caller (swap / SP / AMM liquidity+admin / 3Pool liquidity+admin)
	const caller = event?.caller;
	if (caller) {
		if (typeof caller === 'object' && typeof caller.toText === 'function') return caller.toText();
		if (typeof caller === 'string' && caller.length > 10) return caller;
	}

	// Backend events: peer inside the variant
	const eventType = event?.event_type ?? event;
	if (!eventType) return null;
	const key = Object.keys(eventType)[0];
	if (!key) return null;
	const data = eventType[key];
	if (!data) return null;

	for (const field of ['owner', 'caller', 'from', 'liquidator', 'redeemer', 'developer_principal']) {
		const val = data[field];
		if (val && typeof val === 'object' && typeof val.toText === 'function') return val.toText();
		// Candid opt principal: [Principal] or []
		if (Array.isArray(val) && val.length > 0) {
			const inner = val[0];
			if (inner && typeof inner === 'object' && typeof inner.toText === 'function') return inner.toText();
			if (typeof inner === 'string' && inner.length > 10) return inner;
		}
		if (typeof val === 'string' && val.length > 20) return val;
	}

	// Nested vault owner
	if (data.vault?.owner) {
		const owner = data.vault.owner;
		if (typeof owner === 'object' && typeof owner.toText === 'function') return owner.toText();
	}

	// vault_id → owner fallback
	if (vaultOwnerMap && data.vault_id != null) {
		const owner = vaultOwnerMap.get(Number(data.vault_id));
		if (owner) return owner;
	}

	return null;
}

/** Single dispatch to the right formatter for a non-backend display event. */
export function formatNonBackendEvent(de: DisplayEvent): FormattedEvent {
	switch (de.source) {
		case '3pool_swap': return formatSwapEvent(de.event);
		case 'amm_swap': return formatAmmSwapEvent(de.event);
		case 'amm_liquidity': return formatAmmLiquidityEvent(de.event);
		case 'amm_admin': return formatAmmAdminEvent(de.event);
		case '3pool_liquidity': return format3PoolLiquidityEvent(de.event);
		case '3pool_admin': return format3PoolAdminEvent(de.event);
		case 'stability_pool': return formatStabilityPoolEvent(de.event);
		case 'multi_hop_swap': return formatMultiHopSwapEvent(de.event);
		default: return { summary: '', typeName: de.source, category: 'system', badgeColor: '', fields: [] };
	}
}

/**
 * Build the correct detail-page href for a non-backend event.
 * Multi-hop swaps route to the inner AMM event since that's where the real swap lives.
 */
export function dexDetailHref(de: DisplayEvent): string {
	if (de.source === 'multi_hop_swap') {
		const innerId = de.event?.ammEvent?.id ?? Number(de.globalIndex);
		return `/explorer/e/event/dex:amm_swap:${innerId}`;
	}
	return `/explorer/e/event/dex:${de.source}:${Number(de.globalIndex)}`;
}

/**
 * Unified render-ready shape for any event across every Explorer surface
 * (Activity, entity streams, event detail, Protocol lenses).
 *
 * Pull this from `displayEvent()` rather than calling the individual
 * formatters so every surface renders the same event the same way.
 */
export interface DisplayedEvent {
	globalIndex: number;
	source: DisplayEventSource;
	formatted: FormattedEvent;
	principal: string | null;
	/** Nanoseconds. 0 when the event has no timestamp. */
	timestamp: number;
	detailHref: string;
	/** 'AMM' / '3Pool' / 'SP' / 'Swap' for non-backend events; null for backend. */
	sourceLabel: string | null;
}

export interface DisplayEventMaps {
	vaultCollateralMap?: Map<number, string>;
	vaultOwnerMap?: Map<number, string>;
}

/**
 * Normalize any DisplayEvent (backend or non-backend) into a render-ready
 * shape. This is the single entry point every row/list/tile component
 * should use to format an event.
 */
export function displayEvent(de: DisplayEvent, maps?: DisplayEventMaps): DisplayedEvent {
	const isBackend = de.source === 'backend';
	const globalIndex = Number(de.globalIndex);
	const formatted = isBackend
		? formatEvent(de.event, maps?.vaultCollateralMap)
		: formatNonBackendEvent(de);
	const principal = extractEventPrincipal(de.event, de.source, maps?.vaultOwnerMap);
	const timestamp = de.timestamp || extractEventTimestamp(de.event);
	const detailHref = isBackend ? `/explorer/e/event/${globalIndex}` : dexDetailHref(de);
	// AMM-source events get a friendly label like "🌊 3USD/ICP" once the registry
	// is loaded. Falls back to the generic "AMM" label if the registry isn't
	// populated yet (first paint, or activity page before pools fetched).
	let sourceLabel: string | null;
	if (isBackend) {
		sourceLabel = null;
	} else if (de.source === 'amm_swap' || de.source === 'amm_liquidity' || de.source === 'amm_admin') {
		// Leading-cell label is the source-prefix ("AMM1"), per the round-2 brief.
		// `ammPoolPair` falls back to the raw underscore-joined principal pair
		// when the registry hasn't loaded the pool, which spilled into the cell
		// as `🌊 fohh4-...cai_ryjl3-...cai`. `ammPoolShortLabel` is the
		// registry-indexed name ("AMM1") with a clean "AMM" fallback.
		const poolId = de.event?.pool_id ?? de.event?.ammEvent?.pool_id;
		sourceLabel = ammPoolShortLabel(poolId ? String(poolId) : null);
	} else {
		sourceLabel = DEX_SOURCE_LABEL[de.source as NonBackendSource] ?? null;
	}

	return { globalIndex, source: de.source, formatted, principal, timestamp, detailHref, sourceLabel };
}

/**
 * Wrap a raw backend event (as returned by the protocol backend) into a
 * DisplayEvent so it can be passed through `displayEvent()`. Useful for
 * legacy call sites that have `{ event, globalIndex }` rather than the full
 * DisplayEvent shape.
 */
export function wrapBackendEvent(event: any, globalIndex: number | bigint): DisplayEvent {
	return {
		event,
		globalIndex: BigInt(globalIndex),
		source: 'backend',
		timestamp: extractEventTimestamp(event),
	};
}
