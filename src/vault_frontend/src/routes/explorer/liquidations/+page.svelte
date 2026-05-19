<script lang="ts">
	import { onMount } from 'svelte';
	import BotLiquidationsTable from '$components/liquidations/BotLiquidationsTable.svelte';
	import { fetchBotCanisterStats } from '$services/explorer/explorerService';
	import type { BotStats } from '$declarations/liquidation_bot/liquidation_bot.did';

	const E8S = 100_000_000;
	const E6 = 1_000_000;

	let botStats: BotStats | null = $state(null);

	onMount(async () => {
		botStats = await fetchBotCanisterStats();
	});

	function fmtE8s(v: bigint, decimals = 2): string {
		return (Number(v) / E8S).toLocaleString(undefined, {
			minimumFractionDigits: decimals,
			maximumFractionDigits: decimals,
		});
	}

	function fmtE6(v: bigint, decimals = 2): string {
		return (Number(v) / E6).toLocaleString(undefined, {
			minimumFractionDigits: decimals,
			maximumFractionDigits: decimals,
		});
	}
</script>

<svelte:head>
	<title>Liquidations · Rumi Explorer</title>
</svelte:head>

<div class="page">
	<header class="page-head">
		<h1>Liquidations</h1>
		<p class="lede">
			Vault liquidations that ran through the on-chain liquidation bot.
			For the full activity feed of every liquidation source (bot, stability pool, manual),
			open the <a href="/explorer/activity?type=liquidation">activity log</a>.
		</p>
	</header>

	{#if botStats}
		<div class="stats">
			<div class="stat">
				<span class="stat-label">Debt covered</span>
				<span class="stat-value">{fmtE8s(botStats.total_debt_covered_e8s)} icUSD</span>
			</div>
			<div class="stat">
				<span class="stat-label">Collateral received</span>
				<span class="stat-value">{fmtE8s(botStats.total_collateral_received_e8s)} ICP</span>
			</div>
			<div class="stat">
				<span class="stat-label">To treasury</span>
				<span class="stat-value">{fmtE8s(botStats.total_collateral_to_treasury_e8s)} ICP</span>
			</div>
			<div class="stat">
				<span class="stat-label">ckUSDC swapped</span>
				<span class="stat-value">{fmtE6(botStats.total_ckusdc_deposited_e6)}</span>
			</div>
			<div class="stat">
				<span class="stat-label">Records</span>
				<span class="stat-value">{Number(botStats.events_count)}</span>
			</div>
		</div>
	{/if}

	<section>
		<h2 class="section-title">Bot Liquidation History</h2>
		<BotLiquidationsTable pageSize={25} />
	</section>
</div>

<style>
	.page { display: flex; flex-direction: column; gap: 1.5rem; }

	.page-head h1 {
		font-size: 1.75rem;
		font-weight: 700;
		margin: 0 0 0.5rem;
		color: var(--rumi-text-primary, #fff);
	}
	.lede {
		font-size: 0.9375rem;
		color: var(--rumi-text-muted, #9ca3af);
		margin: 0;
		max-width: 60ch;
	}
	.lede a {
		color: var(--rumi-action, #60a5fa);
		text-decoration: none;
	}
	.lede a:hover { text-decoration: underline; }

	.stats {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(160px, 1fr));
		gap: 0.75rem;
	}
	.stat {
		background: var(--rumi-bg-surface1, rgba(255, 255, 255, 0.03));
		border: 1px solid var(--rumi-border, rgba(255, 255, 255, 0.06));
		border-radius: 8px;
		padding: 0.85rem 1rem;
		display: flex;
		flex-direction: column;
		gap: 0.25rem;
	}
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

	.section-title {
		font-size: 0.8125rem;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		font-weight: 600;
		color: var(--rumi-text-secondary, #9ca3af);
		margin: 0 0 0.75rem;
	}
</style>
