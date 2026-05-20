<script lang="ts">
	import { onMount } from 'svelte';
	import {
		fetchBotCanisterStats,
		fetchStuckBotLiquidations,
	} from '$services/explorer/explorerService';
	import type { BotStats, LiquidationRecordV1 } from '$declarations/liquidation_bot/liquidation_bot.did';

	const E8S = 100_000_000;

	let stats: BotStats | null = $state(null);
	let stuck: LiquidationRecordV1[] = $state([]);
	let loading = $state(true);

	function fmtE8s(v: bigint, decimals = 2): string {
		return (Number(v) / E8S).toLocaleString(undefined, {
			minimumFractionDigits: decimals,
			maximumFractionDigits: decimals,
		});
	}

	onMount(async () => {
		try {
			const [s, k] = await Promise.all([
				fetchBotCanisterStats(),
				fetchStuckBotLiquidations(),
			]);
			stats = s;
			stuck = k;
		} finally {
			loading = false;
		}
	});
</script>

<a href="/explorer?lens=liquidations" class="card" aria-label="Open Liquidations lens">
	<div class="head">
		<h3>Bot Liquidations</h3>
		<span class="cta">View history →</span>
	</div>
	{#if loading}
		<div class="stats placeholder">
			<div class="stat"><span class="stat-label">Loading…</span></div>
		</div>
	{:else if stats}
		<div class="stats">
			<div class="stat">
				<span class="stat-label">Debt covered</span>
				<span class="stat-value">{fmtE8s(stats.total_debt_covered_e8s)} icUSD</span>
			</div>
			<div class="stat">
				<span class="stat-label">Collateral received</span>
				<span class="stat-value">{fmtE8s(stats.total_collateral_received_e8s)} ICP</span>
			</div>
			<div class="stat">
				<span class="stat-label">Records</span>
				<span class="stat-value">{Number(stats.events_count).toLocaleString()}</span>
			</div>
			<div class="stat" class:warn={stuck.length > 0}>
				<span class="stat-label">Stuck claims</span>
				<span class="stat-value">{stuck.length}</span>
			</div>
		</div>
	{/if}
</a>

<style>
	.card {
		display: block;
		background: var(--rumi-card-bg, rgba(255, 255, 255, 0.03));
		border: 1px solid var(--rumi-border, rgba(255, 255, 255, 0.06));
		border-radius: 12px;
		padding: 1rem 1.25rem;
		text-decoration: none;
		color: inherit;
		transition: border-color 120ms ease, background 120ms ease;
	}
	.card:hover {
		border-color: var(--rumi-teal, #3b82f6);
		background: var(--rumi-bg-surface2, rgba(255, 255, 255, 0.04));
	}
	.head {
		display: flex;
		align-items: center;
		justify-content: space-between;
		margin-bottom: 0.75rem;
	}
	.head h3 {
		font-size: 0.875rem;
		font-weight: 500;
		color: var(--rumi-text-secondary, #d1d5db);
		margin: 0;
	}
	.cta {
		font-size: 0.75rem;
		color: var(--rumi-action, #60a5fa);
	}
	.stats {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(140px, 1fr));
		gap: 0.75rem;
	}
	.stats.placeholder { min-height: 2.5rem; opacity: 0.6; }
	.stat { display: flex; flex-direction: column; gap: 0.2rem; }
	.stat.warn .stat-value { color: var(--rumi-error, #f87171); }
	.stat-label {
		font-size: 0.6875rem;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		color: var(--rumi-text-muted, #6b7280);
	}
	.stat-value {
		font-size: 1rem;
		font-weight: 500;
		color: var(--rumi-text-primary, #fff);
		font-variant-numeric: tabular-nums;
	}
</style>
