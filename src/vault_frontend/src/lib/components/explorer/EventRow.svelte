<script lang="ts">
	import { getEventType, getEventBadgeColor, getEventSummary, formatTimestamp, getEventTimestamp, getEventCaller, getPoolEventType, getPoolEventBadgeColor, getPoolEventSummary, getPoolEventCaller } from '$lib/utils/eventFormatters';
	import { truncatePrincipal } from '$lib/utils/principalHelpers';

	export let event: any;
	export let index: number | null = null;
	export let vaultCollateralMap: Map<number, any> | undefined = undefined;
	export let poolSource: string | null = null;

	$: isPoolEvent = poolSource !== null;
	$: displayType = isPoolEvent ? getPoolEventType(poolSource!) : getEventType(event);
	$: displayColor = isPoolEvent ? getPoolEventBadgeColor(poolSource!) : getEventBadgeColor(event);
	$: displaySummary = isPoolEvent ? getPoolEventSummary(poolSource!, event) : getEventSummary(event, vaultCollateralMap);
	$: displayCaller = isPoolEvent ? getPoolEventCaller(poolSource!, event) : getEventCaller(event);
	$: displayTimestamp = (() => {
		if (isPoolEvent) {
			const ts = event?.timestamp;
			if (ts) return formatTimestamp(ts);
			return '';
		}
		const ts = getEventTimestamp(event);
		return ts ? formatTimestamp(ts) : '';
	})();
	$: href = isPoolEvent ? undefined : (index !== null ? `/explorer/event/${index}` : undefined);
</script>

<a class="event-row" href={href}>
	<span class="event-badge" style="background:{displayColor}20; color:{displayColor}; border:1px solid {displayColor}40;">
		{displayType}
	</span>
	<span class="event-summary">{displaySummary}</span>
	<span class="event-meta">
		{#if displayCaller}
			<a class="caller-link" href="/explorer/address/{displayCaller}" on:click|stopPropagation>{truncatePrincipal(displayCaller)}</a>
		{/if}
		{#if displayTimestamp}
			<span class="event-time">{displayTimestamp}</span>
		{/if}
		{#if index !== null && !isPoolEvent}
			<span class="event-index">#{index}</span>
		{/if}
	</span>
</a>

<style>
	.event-row {
		display:grid; grid-template-columns:11rem 1fr auto; gap:0.75rem; align-items:center;
		padding:0.625rem 0.875rem; border-bottom:1px solid var(--rumi-border);
		text-decoration:none; color:var(--rumi-text-primary);
		transition:background 0.15s;
	}
	.event-row:hover { background:var(--rumi-bg-surface-2); }
	.event-badge {
		font-size:0.75rem; font-weight:500; padding:0.125rem 0.5rem;
		border-radius:9999px; white-space:nowrap;
	}
	.event-summary { font-size:0.875rem; color:var(--rumi-text-secondary); overflow:hidden; text-overflow:ellipsis; white-space:nowrap; }
	.event-meta { display:flex; align-items:center; gap:0.5rem; white-space:nowrap; }
	.caller-link { font-size:0.6875rem; color:var(--rumi-purple-accent); text-decoration:none; font-family:monospace; }
	.caller-link:hover { text-decoration:underline; }
	.event-time { font-size:0.6875rem; color:var(--rumi-text-muted); }
	.event-index { font-size:0.75rem; color:var(--rumi-text-muted); font-variant-numeric:tabular-nums; }
</style>
