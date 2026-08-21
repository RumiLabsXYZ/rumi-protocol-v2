<script lang="ts">
  /**
   * PointsSummary — the enrolled wallet's dashboard:
   *   tiles (points + rank) →
   *   Earning now (live positions, engine-mirrored) →
   *   How you've earned (per-source ledger history) →
   *   Recent epochs (per-epoch timeline) →
   *   enrollment line.
   *
   * History comes from the points canister's audit ledger; "Earning now" reads
   * the same endpoints the accrual engine snapshots. The two can legitimately
   * disagree (a new deposit earns nothing until an epoch closes) — the copy
   * explains that instead of hiding it.
   */
  import type { PrincipalState, PublicEpochStatus } from '$declarations/rumi_points/rumi_points.did';
  import type { MyPointsDetail } from '$lib/stores/pointsStore';
  import { formatPoints, qualifyingActionLabel } from '$lib/utils/points';
  import {
    summarizeLedger,
    epochRangeByIndex,
    epochDateRange,
    SOURCE_META,
    type LedgerSummary,
    type PointSourceKey,
  } from '$lib/utils/pointsBreakdown';

  const srcShort = (k: PointSourceKey): string => SOURCE_META[k].short;
  import MultiplierBadge from './MultiplierBadge.svelte';

  interface Props {
    state: PrincipalState;
    rank: number | null;
    detail: MyPointsDetail;
    status: PublicEpochStatus | null;
  }
  // `state` stays the public prop name; bind it as `pState` locally so the
  // `$state` rune isn't shadowed by a binding named `state`.
  let { state: pState, rank, detail, status }: Props = $props();

  const usd = (n: number): string =>
    `$${n.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
  const pts = (n: number): string => n.toLocaleString('en-US', { maximumFractionDigits: 0 });

  const history = $derived<LedgerSummary | null>(
    detail.ledger ? summarizeLedger(detail.ledger, Number(pState.last_epoch_processed)) : null,
  );
  const live = $derived(detail.live);

  const openEpoch = $derived(status?.open_epoch?.[0] ?? null);
  const CLOSE_DATE = new Intl.DateTimeFormat('en-US', {
    month: 'short',
    day: 'numeric',
    timeZone: 'UTC',
  });
  const openEpochClose = $derived(
    openEpoch ? CLOSE_DATE.format(new Date(Number(openEpoch.epoch_end_ns / 1_000_000n))) : null,
  );

  const enrolledDate = $derived(
    new Date(Number(pState.registered_at_ns / 1_000_000n)).toLocaleDateString(),
  );

  // Epoch timeline: cap the list until "Show all".
  let showAllEpochs = $state(false);
  const EPOCH_PREVIEW = 6;
  const epochRows = $derived(
    history ? (showAllEpochs ? history.epochs : history.epochs.slice(0, EPOCH_PREVIEW)) : [],
  );

  const threePoolNote = $derived.by(() => {
    const tp = live?.threePool;
    if (!tp) return null;
    if (tp.verificationUnknown) {
      return {
        tone: 'info' as const,
        text: `Couldn't verify your 3USD holdings right now — showing your recorded ${usd(tp.recordedUsd)} 3pool deposit uncapped.`,
      };
    }
    if (!tp.underVerified) return null;
    if (tp.creditedUsd < 0.01) {
      return {
        tone: 'warning' as const,
        text: `Your ${usd(tp.recordedUsd)} 3pool deposit is NOT earning: 3pool credit only counts while you hold the 3USD it minted, and this wallet holds ${usd(tp.verifiedUsd)} across wallet, stability pool, and AMM.`,
      };
    }
    return {
      tone: 'warning' as const,
      text: `Only ${usd(tp.creditedUsd)} of your ${usd(tp.recordedUsd)} 3pool deposit is earning: you hold ${usd(tp.verifiedUsd)} of 3USD (wallet + stability pool + AMM). Hold the 3USD the deposit minted to restore full credit.`,
    };
  });
</script>

<div class="flex flex-col gap-4">
  <!-- No "Est. share" tile: a live share-of-pool percentage tells every user how
       concentrated the season is and promises a payout proportion nothing has
       committed to. Points + rank convey standing without that; the operator
       console (/points/admin) still shows share. -->
  <div class="grid grid-cols-2 gap-3">
    <div class="rounded-xl bg-gray-800/30 border border-gray-700/50 p-4">
      <p class="text-xs text-gray-400 uppercase tracking-wider">Your points</p>
      <p class="text-2xl font-semibold text-gray-100 mt-1 tabular-nums">{formatPoints(pState.total_points)}</p>
      <p class="text-xs text-gray-500">USD-days</p>
    </div>
    <div class="rounded-xl bg-gray-800/30 border border-gray-700/50 p-4">
      <p class="text-xs text-gray-400 uppercase tracking-wider">Rank</p>
      <p class="text-2xl font-semibold text-gray-100 mt-1 tabular-nums">{rank !== null ? `#${rank}` : '—'}</p>
      <p class="text-xs text-gray-500">{rank !== null ? 'on the leaderboard' : 'outside the top ranks'}</p>
    </div>
  </div>

  <!-- ── Earning now ─────────────────────────────────────────────────── -->
  <div class="rounded-xl bg-gray-800/30 border border-gray-700/50 p-4">
    <div class="flex items-baseline justify-between gap-3 mb-3">
      <p class="text-sm font-medium text-gray-200">Earning now</p>
      {#if live && live.rows.length > 0}
        <p class="text-xs text-gray-500">up to ≈{pts(live.weeklyEstimateUsdDays)} pts/week</p>
      {/if}
    </div>

    {#if detail.loading}
      <p class="text-sm text-gray-400">Checking your current positions…</p>
    {:else if !live}
      <p class="text-sm text-gray-400">Couldn't check your current positions. Retry from the top of the page.</p>
    {:else}
      {#if live.rows.length > 0}
        <ul class="flex flex-col gap-2">
          {#each live.rows as r (r.key)}
            <li class="flex items-center justify-between gap-3 rounded-lg border border-gray-700/40 bg-gray-900/30 px-3 py-2">
              <span class="min-w-0">
                <a href={r.meta.href} class="block text-sm text-gray-100 hover:text-teal-300">{r.meta.label}</a>
                <span class="block text-xs text-gray-500">{usd(r.valueUsd)} counting</span>
              </span>
              <MultiplierBadge multiplier={r.multiplier} />
            </li>
          {/each}
        </ul>
      {:else}
        <p class="text-sm text-gray-400">No positions are earning right now.</p>
      {/if}

      {#if threePoolNote}
        <p
          class="mt-2 text-xs leading-snug rounded-lg border px-3 py-2 {threePoolNote.tone === 'warning'
            ? 'bg-amber-400/10 border-amber-400/30 text-amber-200'
            : 'bg-gray-500/10 border-gray-500/25 text-gray-300'}"
        >
          {threePoolNote.text}
        </p>
      {/if}
      {#if live.unavailable.length > 0}
        <p class="mt-2 text-xs text-gray-500">
          Couldn't check: {live.unavailable.join(', ')}. These may still be earning.
        </p>
      {/if}
      <p class="mt-3 text-xs text-gray-500 leading-snug">
        Positions are credited at each weekly epoch's two random snapshots — the lower total counts,
        so a new deposit typically starts earning with the next full epoch.
        {#if openEpoch}Epoch {Number(openEpoch.epoch_index)} credits at its close on {openEpochClose}.{/if}
      </p>
    {/if}
  </div>

  <!-- ── How you've earned ───────────────────────────────────────────── -->
  <div class="rounded-xl bg-gray-800/30 border border-gray-700/50 p-4">
    <p class="text-sm font-medium text-gray-200 mb-3">How you've earned</p>
    {#if detail.loading}
      <p class="text-sm text-gray-400">Loading your earning history…</p>
    {:else if detail.ledgerFailed || !history}
      <p class="text-sm text-gray-400">Couldn't load your earning history. Retry from the top of the page.</p>
    {:else if history.sources.length === 0}
      <p class="text-sm text-gray-400">
        No points credited yet — your first credit lands when the current epoch closes.
      </p>
    {:else}
      <ul class="flex flex-col gap-2">
        {#each history.sources as s (s.key)}
          <li class="rounded-lg border border-gray-700/40 bg-gray-900/30 px-3 py-2">
            <div class="flex items-center justify-between gap-3">
              <span class="min-w-0 flex items-center gap-2">
                <span class="text-sm text-gray-100 truncate">{s.meta.label}</span>
                <MultiplierBadge multiplier={s.meta.multiplier} />
              </span>
              <span class="text-sm text-gray-100 tabular-nums whitespace-nowrap">{formatPoints(s.points)}</span>
            </div>
            <div class="mt-1.5 h-1 rounded-full bg-gray-700/40 overflow-hidden">
              <div class="h-full bg-teal-400/70" style={`width:${Math.max(2, s.sharePct)}%`}></div>
            </div>
            <div class="mt-1 flex items-center justify-between gap-2 text-xs">
              <span class="text-gray-500">
                {s.sharePct.toFixed(1)}% of your points ·
                {s.epochCount === 1 ? `epoch ${s.firstEpoch}` : `epochs ${s.firstEpoch}–${s.lastEpoch}`}
              </span>
              {#if s.status === 'stopped'}
                <span class="text-amber-300/90 whitespace-nowrap">
                  stopped after epoch {s.lastEpoch}{#if epochRangeByIndex(detail.epochs, s.lastEpoch)}&nbsp;({epochRangeByIndex(detail.epochs, s.lastEpoch)}){/if}
                </span>
              {:else}
                <span class="text-emerald-400/80">active</span>
              {/if}
            </div>
          </li>
        {/each}
      </ul>
    {/if}
  </div>

  <!-- ── Recent epochs ───────────────────────────────────────────────── -->
  {#if history && history.epochs.length > 0}
    <div class="rounded-xl bg-gray-800/30 border border-gray-700/50 p-4">
      <p class="text-sm font-medium text-gray-200 mb-3">Points by epoch</p>
      <ul class="flex flex-col gap-1.5">
        {#if openEpoch}
          <li class="flex items-center justify-between gap-3 rounded-lg border border-dashed border-gray-700/40 px-3 py-2">
            <span class="min-w-0">
              <span class="block text-sm text-gray-300">Epoch {Number(openEpoch.epoch_index)} · in progress</span>
              <span class="block text-xs text-gray-500">{epochDateRange(openEpoch.epoch_start_ns, openEpoch.epoch_end_ns)}</span>
            </span>
            <span class="text-xs text-gray-500 whitespace-nowrap">credits {openEpochClose}</span>
          </li>
        {/if}
        {#each epochRows as e (e.epoch)}
          <li class="flex items-center justify-between gap-3 rounded-lg border border-gray-700/40 bg-gray-900/30 px-3 py-2">
            <span class="min-w-0">
              <span class="block text-sm text-gray-100">
                Epoch {e.epoch}{#if epochRangeByIndex(detail.epochs, e.epoch)}<span class="text-gray-500 font-normal">&nbsp;· {epochRangeByIndex(detail.epochs, e.epoch)}</span>{/if}
              </span>
              <span class="block text-xs text-gray-500 truncate">
                {e.bySource.map((b) => `${srcShort(b.key)} ${formatPoints(b.points)}`).join(' · ')}
              </span>
            </span>
            <span class="text-sm text-gray-100 tabular-nums whitespace-nowrap">+{formatPoints(e.points)}</span>
          </li>
        {/each}
      </ul>
      {#if history.epochs.length > EPOCH_PREVIEW}
        <button
          class="mt-2 text-xs text-teal-400 hover:underline"
          onclick={() => (showAllEpochs = !showAllEpochs)}
        >
          {showAllEpochs ? 'Show fewer' : `Show all ${history.epochs.length} epochs`}
        </button>
      {/if}
    </div>
  {/if}

  <div class="rounded-xl bg-gray-800/30 border border-gray-700/50 p-4 text-sm text-gray-300">
    <p>Enrolled {enrolledDate} · first action: {qualifyingActionLabel(pState.first_qualifying_action)}</p>
  </div>
</div>
