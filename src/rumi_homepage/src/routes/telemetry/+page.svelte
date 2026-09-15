<script lang="ts">
	import { onMount } from 'svelte';
	import {
		OPERATOR_TELEMETRY_URL,
		TARGET_STATE_PRESENTATION,
		alarmKind,
		formatCycles,
		formatDuration,
		formatIcpE8s,
		formatInteger,
		formatTimestamp,
		optionalBigInt,
		refreshTelemetry,
		sentinelCanisterId,
		targetState,
		variantName,
		type TelemetrySnapshot
	} from '$lib/cycleSentinel';

	const configured = Boolean(sentinelCanisterId());
	let snapshot: TelemetrySnapshot | undefined;
	let loading = configured;
	let stale = false;
	let error: string | undefined;

	$: overview = snapshot?.overview;
	$: openAlarms = snapshot?.alarms.filter((alarm) => variantName(alarm.status) !== 'Resolved') ?? [];

	async function refresh() {
		if (!configured) return;
		loading = !snapshot;
		const result = await refreshTelemetry(snapshot);
		snapshot = result.snapshot;
		loading = result.loading;
		stale = result.stale;
		error = result.error;
	}

	onMount(() => {
		void refresh();
		const interval = window.setInterval(() => void refresh(), 60_000);
		return () => window.clearInterval(interval);
	});
</script>

<svelte:head>
	<title>Cycle Telemetry | Rumi Protocol</title>
	<meta
		name="description"
		content="Public cycle balances, health, runway, reserves, and alerts for Rumi Protocol canisters."
	/>
</svelte:head>

<section class="telemetry-shell">
	<div class="hero">
		<div>
			<p class="eyebrow">Public infrastructure status</p>
			<h1>Cycle Telemetry</h1>
			<p class="lede">
				Live, read-only visibility into the cycle health of Rumi Protocol canisters. A low signal
				is an early refill warning, not proof that a canister is frozen.
			</p>
		</div>
		<div class="hero-actions">
			<button class="refresh" on:click={refresh} disabled={!configured || loading}>Refresh</button>
			<a class="operator-link" href={OPERATOR_TELEMETRY_URL}>Open operator console</a>
		</div>
	</div>

	{#if !configured}
		<div class="notice neutral" role="status">
			<h2>Cycle Sentinel is not configured</h2>
			<p>This environment has no Cycle Sentinel canister ID. No live values are being shown.</p>
		</div>
	{:else if loading && !snapshot}
		<div class="notice neutral" aria-live="polite">Loading live cycle telemetry...</div>
	{:else if error && !snapshot}
		<div class="notice bad" role="alert">
			<h2>Telemetry is unavailable</h2>
			<p>{error}</p>
			<button class="refresh" on:click={refresh}>Try again</button>
		</div>
	{:else if snapshot && overview}
		{#if stale}
			<div class="notice warn" role="status">
				Showing the last successful snapshot from {snapshot.refreshedAt.toLocaleString()}. Refresh
				failed: {error}
			</div>
		{/if}

		<div class="summary-grid" aria-label="Cycle health overview">
			<article class="summary-card">
				<span>Healthy</span>
				<strong>{formatInteger(overview.healthy_count)} / {formatInteger(overview.target_count)}</strong>
			</article>
			<article class="summary-card warn-card">
				<span>Needs attention</span>
				<strong>{formatInteger(overview.low_count + overview.unreachable_count + overview.stopped_count + overview.uninstalled_count)}</strong>
			</article>
			<article class="summary-card">
				<span>Total observed</span>
				<strong>{formatCycles(overview.total_observed_cycles)}</strong>
			</article>
			<article class="summary-card">
				<span>Sentinel runtime</span>
				<strong>{formatCycles(overview.runtime_cycles)}</strong>
			</article>
		</div>

		<div class="reserve-grid">
			<div><span>Cycles Ledger available</span><strong>{formatCycles(optionalBigInt(overview.cycles_ledger_available_cycles))}</strong></div>
			<div><span>Protected self reserve</span><strong>{formatCycles(optionalBigInt(overview.protected_self_reserve_cycles))}</strong></div>
			<div><span>ICP fallback available</span><strong>{formatIcpE8s(optionalBigInt(overview.icp_available_e8s))}</strong></div>
			<div><span>Open alarms</span><strong>{formatInteger(overview.alarm_count)}</strong></div>
		</div>

		<div class="sample-meta">
			<span>Last sample: {formatTimestamp(optionalBigInt(overview.last_sample_at_secs))}</span>
			<span>Next sample: {formatTimestamp(optionalBigInt(overview.next_sample_at_secs))}</span>
			<span>Page refreshed: {snapshot.refreshedAt.toLocaleString()}</span>
		</div>

		{#if openAlarms.length > 0}
			<section class="alarms" aria-labelledby="alarm-heading">
				<div class="section-heading">
					<div><p class="eyebrow">Current signals</p><h2 id="alarm-heading">Alarms</h2></div>
					<span>{openAlarms.length} visible</span>
				</div>
				<div class="alarm-list">
					{#each openAlarms as alarm (alarm.id.toString())}
						<div class="alarm-row">
							<strong>{alarmKind(alarm)}</strong>
							<span>{alarm.target.length ? alarm.target[0].toText() : 'Cycle Sentinel'}</span>
							<time>{formatTimestamp(alarm.opened_at_secs)}</time>
						</div>
					{/each}
				</div>
			</section>
		{/if}

		<section class="targets" aria-labelledby="target-heading">
			<div class="section-heading">
				<div><p class="eyebrow">Monitored fleet</p><h2 id="target-heading">Canisters</h2></div>
				<span>{snapshot.targets.length} listed</span>
			</div>

			{#if snapshot.targets.length === 0}
				<div class="notice neutral">No monitored canisters are configured yet.</div>
			{:else}
				<div class="table-wrap">
					<table>
						<thead>
							<tr>
								<th>Canister</th><th>Status</th><th>Balance</th><th>Threshold</th><th>Refill</th><th>Burn / day</th><th>Runway</th><th>Recent top-ups</th><th>Observed</th>
							</tr>
						</thead>
						<tbody>
							{#each snapshot.targets as row (row.principal.toText())}
								{@const state = targetState(row)}
								{@const presentation = TARGET_STATE_PRESENTATION[state]}
								<tr data-state={state}>
									<td>
										<strong>{row.display_name}</strong>
										<code>{row.principal.toText()}</code>
										<small>{variantName(row.environment)} · {variantName(row.criticality)} · {variantName(row.observation_mode)}</small>
									</td>
									<td><span class="state {presentation.tone}">{presentation.label}</span></td>
									<td>{row.advisory_balance_overflowed ? 'Above display limit' : formatCycles(optionalBigInt(row.advisory_balance_cycles))}</td>
									<td>{formatCycles(row.low_balance_threshold_cycles)}</td>
									<td>{formatCycles(row.refill_cycles)}</td>
									<td>{formatCycles(optionalBigInt(row.burn_cycles_per_day))}</td>
									<td>{formatDuration(optionalBigInt(row.runway_secs))}</td>
									<td>
										{#if row.recent_topups.length === 0}
											<span>None recorded</span>
										{:else}
											<div class="topup-list">
												{#each row.recent_topups.slice(0, 3) as topup}
													<span>{formatCycles(topup.amount_cycles)} · {variantName(topup.rail)} · {variantName(topup.outcome)}</span>
												{/each}
											</div>
										{/if}
									</td>
									<td>{row.as_of_secs === 0n ? 'Not yet observed' : formatTimestamp(row.as_of_secs)}</td>
								</tr>
							{/each}
						</tbody>
					</table>
				</div>
			{/if}
		</section>
	{/if}
</section>

<style>
	.telemetry-shell { width: min(1180px, calc(100% - 2rem)); margin: 0 auto; padding: 5rem 0 6rem; }
	.hero { display: flex; align-items: flex-end; justify-content: space-between; gap: 2rem; margin-bottom: 2.5rem; }
	.hero h1 { margin: .25rem 0 .75rem; font-size: clamp(2.5rem, 6vw, 4.5rem); line-height: .95; }
	.eyebrow { margin: 0; color: var(--rumi-action); font-size: .75rem; font-weight: 700; letter-spacing: .14em; text-transform: uppercase; }
	.lede { max-width: 680px; margin: 0; color: var(--rumi-text-secondary); font-size: 1.05rem; line-height: 1.7; }
	.hero-actions { display: flex; flex-wrap: wrap; gap: .75rem; }
	.refresh, .operator-link { border-radius: .55rem; padding: .7rem 1rem; font: inherit; font-weight: 600; text-decoration: none; cursor: pointer; }
	.refresh { border: 1px solid var(--rumi-border-hover); background: var(--rumi-bg-surface2); color: var(--rumi-text-primary); }
	.refresh:disabled { opacity: .5; cursor: default; }
	.operator-link { background: var(--rumi-action); color: var(--rumi-bg-primary); }
	.notice { margin: 1.5rem 0; padding: 1.25rem; border: 1px solid var(--rumi-border); border-radius: .75rem; background: var(--rumi-bg-surface1); }
	.notice h2, .notice p { margin: 0 0 .5rem; }
	.notice p:last-child { margin-bottom: 0; }
	.notice.warn { border-color: rgba(245, 158, 11, .4); color: #fcd34d; }
	.notice.bad { border-color: rgba(248, 113, 113, .45); }
	.summary-grid { display: grid; grid-template-columns: repeat(4, 1fr); gap: .75rem; }
	.summary-card, .reserve-grid > div { padding: 1.15rem; border: 1px solid var(--rumi-border); border-radius: .75rem; background: var(--rumi-bg-surface1); }
	.summary-card span, .reserve-grid span { display: block; color: var(--rumi-text-muted); font-size: .78rem; text-transform: uppercase; letter-spacing: .08em; }
	.summary-card strong { display: block; margin-top: .55rem; font-size: 1.55rem; }
	.warn-card strong { color: #fcd34d; }
	.reserve-grid { display: grid; grid-template-columns: repeat(4, 1fr); gap: .75rem; margin-top: .75rem; }
	.reserve-grid strong { display: block; margin-top: .45rem; font-size: .95rem; }
	.sample-meta { display: flex; flex-wrap: wrap; gap: .5rem 1.5rem; margin: 1rem 0 3rem; color: var(--rumi-text-muted); font-size: .8rem; }
	.alarms, .targets { margin-top: 2.5rem; }
	.section-heading { display: flex; align-items: flex-end; justify-content: space-between; margin-bottom: 1rem; }
	.section-heading h2 { margin: .2rem 0 0; font-size: 1.75rem; }
	.section-heading > span { color: var(--rumi-text-muted); font-size: .85rem; }
	.alarm-list { display: grid; gap: .5rem; }
	.alarm-row { display: grid; grid-template-columns: minmax(160px, .8fr) 1.5fr auto; gap: 1rem; padding: .85rem 1rem; border: 1px solid rgba(245,158,11,.25); border-radius: .6rem; background: rgba(245,158,11,.06); }
	.alarm-row span, .alarm-row time { color: var(--rumi-text-secondary); }
	.table-wrap { overflow-x: auto; border: 1px solid var(--rumi-border); border-radius: .75rem; background: var(--rumi-bg-surface1); }
	table { width: 100%; min-width: 1050px; border-collapse: collapse; }
	th, td { padding: .9rem .85rem; text-align: left; border-bottom: 1px solid var(--rumi-border); vertical-align: middle; }
	th { color: var(--rumi-text-muted); font-size: .72rem; text-transform: uppercase; letter-spacing: .08em; }
	td { color: var(--rumi-text-secondary); font-size: .86rem; }
	td:first-child strong, td:first-child code, td:first-child small { display: block; }
	td:first-child strong { color: var(--rumi-text-primary); font-size: .95rem; }
	td:first-child code { max-width: 230px; overflow: hidden; text-overflow: ellipsis; color: var(--rumi-text-secondary); font-size: .74rem; }
	td:first-child small { margin-top: .25rem; color: var(--rumi-text-muted); }
	tbody tr:last-child td { border-bottom: 0; }
	.state { display: inline-flex; border-radius: 999px; padding: .25rem .55rem; font-size: .75rem; font-weight: 700; }
	.state.good { color: #6ee7b7; background: rgba(52,211,153,.1); }
	.state.warn { color: #fcd34d; background: rgba(245,158,11,.1); }
	.state.bad { color: #fca5a5; background: rgba(248,113,113,.1); }
	.state.neutral { color: var(--rumi-text-secondary); background: var(--rumi-bg-surface3); }
	.topup-list { display: grid; gap: .25rem; min-width: 190px; font-size: .74rem; }
	@media (max-width: 900px) { .summary-grid, .reserve-grid { grid-template-columns: repeat(2, 1fr); } .hero { align-items: flex-start; flex-direction: column; } }
	@media (max-width: 540px) { .telemetry-shell { padding-top: 3rem; } .summary-grid, .reserve-grid { grid-template-columns: 1fr; } .alarm-row { grid-template-columns: 1fr; gap: .25rem; } }
</style>
