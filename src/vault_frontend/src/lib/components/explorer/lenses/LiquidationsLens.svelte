<script lang="ts">
	import { onMount } from 'svelte';
	import BotLiquidationsTable from '$components/liquidations/BotLiquidationsTable.svelte';
	import MiniAreaChart from '../MiniAreaChart.svelte';
	import { fetchBotCanisterStats, fetchProtocolStatus } from '$services/explorer/explorerService';
	import { fetchLiquidationSeries } from '$services/explorer/analyticsService';
	import { formatCompact, CHART_COLORS } from '$utils/explorerChartHelpers';
	import type { BotStats } from '$declarations/liquidation_bot/liquidation_bot.did';

	const E8S = 100_000_000;
	const NINETY_DAYS_MS = 90 * 24 * 60 * 60 * 1000;

	let botStats: BotStats | null = $state(null);
	let protocolStatus: any = $state(null);
	let liqRows: any[] = $state([]);
	let seriesLoading = $state(true);

	onMount(async () => {
		const [botR, statusR, seriesR] = await Promise.allSettled([
			fetchBotCanisterStats(),
			fetchProtocolStatus(),
			fetchLiquidationSeries(90),
		]);
		if (botR.status === 'fulfilled') botStats = botR.value;
		if (statusR.status === 'fulfilled') protocolStatus = statusR.value;
		if (seriesR.status === 'fulfilled') liqRows = seriesR.value ?? [];
		seriesLoading = false;
	});

	function fmtE8s(v: bigint, decimals = 2): string {
		return (Number(v) / E8S).toLocaleString(undefined, {
			minimumFractionDigits: decimals,
			maximumFractionDigits: decimals,
		});
	}

	// Daily liquidated debt (bars) over the trailing 90d window.
	const liqPoints = $derived.by(() => {
		const cutoff = Date.now() - NINETY_DAYS_MS;
		return liqRows
			.map((r: any) => ({
				t: Number(r.timestamp_ns) / 1_000_000,
				v: Number(r.total_debt_covered_e8s ?? 0n) / E8S,
				count: Number(r.full_count ?? 0) + Number(r.partial_count ?? 0) + Number(r.redistribution_count ?? 0),
			}))
			.filter((p) => p.t >= cutoff);
	});
	const liq90dVolume = $derived(liqPoints.reduce((s, p) => s + p.v, 0));
	const liq90dCount = $derived(liqPoints.reduce((s, p) => s + (p as any).count, 0));

	const badDebt = $derived(
		protocolStatus ? Number(protocolStatus.protocol_deficit_icusd ?? 0) / E8S : null
	);
	const badDebtRepaid = $derived(
		protocolStatus ? Number(protocolStatus.total_deficit_repaid_icusd ?? 0) / E8S : null
	);
	const breakerTripped = $derived(Boolean(protocolStatus?.liquidation_breaker_tripped));
</script>

<div class="lens">
	<header class="head">
		<h1>Liquidations</h1>
		<p class="lede">
			Liquidation outcomes across every source — the on-chain bot, the stability pool,
			and manual liquidators — plus the protocol's bad-debt accounting.
			For the raw event feed, open the
			<a href="/explorer/activity?type=liquidation">activity log</a>.
		</p>
	</header>

	<div class="stats">
		<div class="stat">
			<span class="stat-label">Liquidated (90d)</span>
			<span class="stat-value">{liq90dVolume.toLocaleString(undefined, { maximumFractionDigits: 2 })} icUSD</span>
			<span class="stat-sub">{liq90dCount} event{liq90dCount === 1 ? '' : 's'}</span>
		</div>
		{#if botStats}
			<div class="stat">
				<span class="stat-label">Bot debt covered</span>
				<span class="stat-value">{fmtE8s(botStats.total_debt_covered_e8s)} icUSD</span>
				<span class="stat-sub">all time</span>
			</div>
			<div class="stat">
				<span class="stat-label">Bot collateral received</span>
				<span class="stat-value">{fmtE8s(botStats.total_collateral_received_e8s)} ICP</span>
			</div>
			<div class="stat">
				<span class="stat-label">To treasury</span>
				<span class="stat-value">{fmtE8s(botStats.total_collateral_to_treasury_e8s)} ICP</span>
			</div>
		{/if}
		{#if badDebt != null}
			<div class="stat">
				<span class="stat-label">Bad debt</span>
				<span class="stat-value" class:val-danger={badDebt > 0} class:val-good={badDebt === 0}>
					{badDebt === 0 ? '$0' : `${badDebt.toLocaleString(undefined, { maximumFractionDigits: 2 })} icUSD`}
				</span>
				<span class="stat-sub">{badDebtRepaid && badDebtRepaid > 0 ? `${formatCompact(badDebtRepaid)} repaid via fees` : 'shortfall from underwater liquidations'}</span>
			</div>
			<div class="stat">
				<span class="stat-label">Surge breaker</span>
				<span class="stat-value" class:val-danger={breakerTripped} class:val-good={!breakerTripped}>
					{breakerTripped ? 'Tripped' : 'Normal'}
				</span>
				<span class="stat-sub">{breakerTripped ? 'auto-routing paused' : 'auto-routing active'}</span>
			</div>
		{/if}
	</div>

	<div class="explorer-card">
		<MiniAreaChart
			points={liqPoints}
			label="Daily liquidated debt (90d)"
			color={CHART_COLORS.danger}
			valueFormat={(v) => `${formatCompact(v)} icUSD`}
			headlineValue={liq90dVolume}
			height={150}
			kind="bar"
			loading={seriesLoading}
		/>
	</div>

	<section>
		<h2 class="section-title">Bot Liquidation History</h2>
		<BotLiquidationsTable pageSize={25} />
	</section>

	<p class="cross-link">
		Stability-pool absorptions (which vaults the pool covered, stables consumed,
		collateral gained) live on the
		<a href="/explorer?lens=stability">Stability Pool lens</a>.
	</p>
</div>

<style>
	.lens { display: flex; flex-direction: column; gap: 1.5rem; }

	.head h1 {
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
	.stat-sub {
		font-size: 0.6875rem;
		color: var(--rumi-text-muted, #6b7280);
	}
	.val-danger { color: var(--rumi-danger, #f87171); }
	.val-good { color: var(--rumi-teal, #2dd4bf); }

	.cross-link {
		font-size: 0.8125rem;
		color: var(--rumi-text-muted, #9ca3af);
		margin: 0;
	}
	.cross-link a {
		color: var(--rumi-action, #60a5fa);
		text-decoration: none;
	}
	.cross-link a:hover { text-decoration: underline; }

	.section-title {
		font-size: 0.8125rem;
		text-transform: uppercase;
		letter-spacing: 0.05em;
		font-weight: 600;
		color: var(--rumi-text-secondary, #9ca3af);
		margin: 0 0 0.75rem;
	}
</style>
