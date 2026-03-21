<script lang="ts">
	import { getEventType, getEventBadgeColor, getEventSummary, formatTimestamp, getEventTimestamp, getEventCaller } from '$lib/utils/eventFormatters';
	import { truncatePrincipal } from '$lib/utils/principalHelpers';

	export let event: any;
	export let index: number | null = null;
	export let vaultCollateralMap: Map<number, any> | undefined = undefined;

	$: type = getEventType(event);
	$: badgeColor = getEventBadgeColor(event);
	$: summary = getEventSummary(event, vaultCollateralMap);
	$: timestamp = getEventTimestamp(event);
	$: caller = getEventCaller(event);
</script>

<a class="event-row" href={index !== null ? `/explorer/event/${index}` : undefined}>
	<span class="event-badge" style="background:{badgeColor}20; color:{badgeColor}; border:1px solid {badgeColor}40;">
		{type}
	</span>
	<span class="event-summary">{summary}</span>
	<span class="event-meta">
		{#if caller}
			<a class="caller-link" href="/explorer/address/{caller}" on:click|stopPropagation>{truncatePrincipal(caller)}</a>
		{/if}
		{#if timestamp}
			<span class="event-time">{formatTimestamp(timestamp)}</span>
		{/if}
		{#if index !== null}
			<span class="event-index">#{index}</span>
		{/if}
	</span>
</a>

<style>
	.event-row {
		display:grid; grid-template-columns:auto 1fr auto; gap:0.75rem; align-items:center;
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
