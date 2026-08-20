<script lang="ts">
  /**
   * /points/admin — the Season-1 airdrop operator console. Full participant
   * breakdown: engine health, ingest cursors, ranked participants with a
   * per-source / per-epoch decomposition of every principal's points, epoch
   * history, and the excluded list.
   *
   * The gate (connected principal must equal PointsConfig.admin) is a UI wall
   * only: every read here is an unauthenticated public query that anyone can
   * make against the canister directly. Nothing sensitive is unlocked by it —
   * it just keeps an operator page out of ordinary users' way.
   *
   * The per-source breakdown needs the ledger-reader upgrade
   * (get_principal_point_entries); against an older canister those panels say
   * "pending upgrade" instead of failing.
   */
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { POINTS_ENABLED, ADMIN_VIEW_PRINCIPALS } from '$lib/config';
  import { principal, isConnected } from '$lib/stores/wallet';
  import {
    getPointsConfig,
    getEpochStatus,
    getIngestStatus,
    getEpochHistory,
    getLeaderboard,
    getPrincipalState,
    getExcludedPrincipals,
    getPrincipalPointEntries,
    getPointLedgerLen,
    invalidatePointsCache,
  } from '$lib/services/pointsService';
  import { formatPoints, qualifyingActionLabel } from '$lib/utils/points';
  import { truncatePrincipal, copyToClipboard } from '$lib/utils/principalHelpers';
  import { toastStore } from '$lib/stores/toast';
  import type {
    PointsConfig,
    PublicEpochStatus,
    IngestStatus,
    EpochSummary,
    LeaderboardEntry,
    PrincipalState,
    PointEntry,
  } from '$declarations/rumi_points/rumi_points.did';

  // ── data ──────────────────────────────────────────────────────────────────
  let config = $state<PointsConfig | null>(null);
  let status = $state<PublicEpochStatus | null>(null);
  let ingest = $state<IngestStatus | null>(null);
  let epochs = $state<EpochSummary[]>([]);
  let board = $state<LeaderboardEntry[]>([]);
  let excluded = $state<string[]>([]);
  let ledgerLen = $state<bigint | null>(null);
  let loading = $state(true);
  let error = $state(false);

  // Per-participant detail, loaded on expand. `entries: null` = canister
  // predates the ledger readers ("pending upgrade"), distinct from not-loaded.
  type Detail = {
    state: PrincipalState | null;
    entries: PointEntry[] | null;
    loading: boolean;
    failed: boolean;
  };
  let details = $state<Record<string, Detail>>({});
  let expanded = $state<Record<string, boolean>>({});
  let loadingAll = $state(false);

  async function loadAll() {
    loading = true;
    error = false;
    try {
      const [cfg, st, ing, eps, lb, exc, len] = await Promise.all([
        getPointsConfig(),
        getEpochStatus(),
        getIngestStatus(),
        getEpochHistory(0, 200),
        getLeaderboard(0, 1000),
        getExcludedPrincipals(),
        getPointLedgerLen(),
      ]);
      config = cfg;
      status = st;
      ingest = ing;
      epochs = eps;
      board = lb;
      excluded = exc.map((p) => p.toText());
      ledgerLen = len;
    } catch (e) {
      console.error('[points/admin] load failed', e);
      error = true;
      toastStore.error('Could not load the airdrop admin data.');
    } finally {
      loading = false;
    }
  }

  onMount(() => {
    if (!POINTS_ENABLED) {
      goto('/');
      return;
    }
    loadAll();
  });

  async function refresh() {
    invalidatePointsCache();
    details = {};
    await loadAll();
    // Re-expand nothing; a refresh restarts from the summary view.
    expanded = {};
  }

  async function loadDetail(entry: LeaderboardEntry) {
    const key = entry.principal.toText();
    if (details[key]?.state || details[key]?.loading) return;
    details[key] = { state: null, entries: null, loading: true, failed: false };
    try {
      const [ps, entries] = await Promise.all([
        getPrincipalState(entry.principal),
        getPrincipalPointEntries(entry.principal),
      ]);
      details[key] = { state: ps, entries, loading: false, failed: false };
    } catch (e) {
      console.error('[points/admin] detail load failed', key, e);
      details[key] = { state: null, entries: null, loading: false, failed: true };
    }
  }

  function toggle(entry: LeaderboardEntry) {
    const key = entry.principal.toText();
    expanded[key] = !expanded[key];
    if (expanded[key]) loadDetail(entry);
  }

  async function loadEveryDetail() {
    loadingAll = true;
    try {
      await Promise.all(board.map((e) => loadDetail(e)));
    } finally {
      loadingAll = false;
    }
  }

  async function copyAddress(text: string) {
    if (await copyToClipboard(text)) toastStore.success('Address copied');
  }

  // ── formatting ────────────────────────────────────────────────────────────
  function fmtNs(ns: bigint | number | undefined | null, withTime = false): string {
    if (!ns) return '—';
    const d = new Date(Number(ns) / 1_000_000);
    return withTime
      ? d.toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' })
      : d.toLocaleDateString(undefined, { dateStyle: 'medium' });
  }
  function fmtUsdE8s(v: bigint): string {
    return `$${(Number(v / 1_000_000n) / 100).toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;
  }
  function fmtBps(bps: number): string {
    return `${(bps / 100).toFixed(2)}%`;
  }
  function variantKey(v: object): string {
    return Object.keys(v)[0] ?? '?';
  }

  /** Source labels with the multiplier accrual applies (accrual::snapshot_weights). */
  const SOURCE_META: Record<string, { label: string; mult: string }> = {
    Registration: { label: 'Registration marker', mult: '—' },
    IcUsdDebt: { label: 'icUSD vault debt', mult: '1×' },
    IcUsd3Pool: { label: 'icUSD in 3pool', mult: '1×' },
    CkStable3PoolMatched: { label: 'ck-stable 3pool (matched)', mult: '10×' },
    CkStable3PoolUnmatched: { label: 'ck-stable 3pool (unmatched)', mult: '3×' },
    IcUsdStabilityPool: { label: 'icUSD in stability pool', mult: '1×' },
    ThreeUsdStabilityPool: { label: '3USD in stability pool', mult: '2×' },
    AmmLp: { label: 'AMM LP', mult: '2×' },
    VaultRepayment: { label: 'ck-stable vault repayment', mult: '5×' },
  };
  const SOURCE_TAGS: Record<number, string> = {
    0: 'rumi_protocol_backend',
    1: 'rumi_3pool',
    2: 'rumi_stability_pool',
    3: 'rumi_amm',
  };
  const ASSET_LABEL: Record<string, string> = {
    IcUsd: 'icUSD',
    CkUsdc: 'ckUSDC',
    CkUsdt: 'ckUSDT',
    ThreeUsd: '3USD',
    Icp: 'ICP',
  };

  /** Group ledger rows: per-source totals and a per-epoch timeline. */
  function breakdown(entries: PointEntry[]) {
    const bySource = new Map<string, bigint>();
    const byEpoch = new Map<bigint, bigint>();
    for (const e of entries) {
      const src = variantKey(e.source);
      if (e.points_delta === 0n && src === 'Registration') continue;
      bySource.set(src, (bySource.get(src) ?? 0n) + e.points_delta);
      byEpoch.set(e.epoch_index, (byEpoch.get(e.epoch_index) ?? 0n) + e.points_delta);
    }
    const total = [...bySource.values()].reduce((a, b) => a + b, 0n);
    return {
      total,
      sources: [...bySource.entries()]
        .filter(([, v]) => v > 0n)
        .sort((a, b) => (b[1] > a[1] ? 1 : -1)),
      epochs: [...byEpoch.entries()].sort((a, b) => (a[0] < b[0] ? -1 : 1)),
    };
  }
  function pct(part: bigint, total: bigint): string {
    if (total === 0n) return '0%';
    return `${(Number((part * 10_000n) / total) / 100).toFixed(1)}%`;
  }

  // Season-wide by-source aggregate, once every participant's detail is loaded.
  const seasonBySource = $derived.by(() => {
    const loaded = board
      .map((e) => details[e.principal.toText()])
      .filter((d) => d && !d.loading && d.entries !== null && !d.failed);
    if (loaded.length !== board.length || board.length === 0) return null;
    const agg = new Map<string, bigint>();
    for (const d of loaded) {
      for (const e of d!.entries!) {
        const src = variantKey(e.source);
        if (e.points_delta === 0n) continue;
        agg.set(src, (agg.get(src) ?? 0n) + e.points_delta);
      }
    }
    const total = [...agg.values()].reduce((a, b) => a + b, 0n);
    return {
      total,
      sources: [...agg.entries()].sort((a, b) => (b[1] > a[1] ? 1 : -1)),
    };
  });

  function exportCsv() {
    const head = 'rank,principal,points_usd_days,share_pct,registered_utc,first_action,last_epoch,active_deposits_usd';
    const lines = board.map((e) => {
      const d = details[e.principal.toText()];
      const st = d?.state ?? null;
      const deposits = st
        ? st.active_deposits.reduce((s, [, rec]) => s + rec.recorded_value_usd, 0n)
        : null;
      return [
        e.rank,
        e.principal.toText(),
        (Number(e.total_points / 1_000_000n) / 100).toFixed(2),
        (e.estimated_share_bps / 100).toFixed(2),
        st ? new Date(Number(st.registered_at_ns) / 1_000_000).toISOString() : '',
        st ? variantKey(st.first_qualifying_action) : '',
        st ? st.last_epoch_processed.toString() : '',
        deposits !== null ? (Number(deposits / 1_000_000n) / 100).toFixed(2) : '',
      ].join(',');
    });
    const blob = new Blob([[head, ...lines].join('\n')], { type: 'text/csv' });
    const a = document.createElement('a');
    a.href = URL.createObjectURL(blob);
    a.download = `rumi-airdrop-participants-${new Date().toISOString().slice(0, 10)}.csv`;
    a.click();
    URL.revokeObjectURL(a.href);
  }

  // ── derived ───────────────────────────────────────────────────────────────
  const adminText = $derived(config ? config.admin.toText() : null);
  const connectedText = $derived($isConnected && $principal !== null ? $principal.toText() : null);
  // The on-chain admin is the CLI deploy identity, which no browser wallet can
  // present — so the operator's app-wallet principals come from the
  // ADMIN_VIEW_PRINCIPALS allowlist in config.ts. UI wall only (see header).
  const isAdmin = $derived(
    connectedText !== null &&
      (connectedText === adminText || ADMIN_VIEW_PRINCIPALS.includes(connectedText))
  );
  const openEpoch = $derived(status?.open_epoch?.[0] ?? null);
  const totalPoints = $derived(board.reduce((s, e) => s + e.total_points, 0n));
  const seasonPct = $derived.by(() => {
    if (!config) return null;
    const start = Number(config.season_start_ns);
    const end = Number(config.season_end_ns);
    const now = Date.now() * 1e6;
    if (now <= start) return 0;
    if (now >= end) return 100;
    return Math.round(((now - start) / (end - start)) * 100);
  });

  const epochColumns = [
    { key: 'epoch', label: 'Epoch', align: 'left' as const, width: '8%' },
    { key: 'window', label: 'Window', align: 'left' as const },
    { key: 'snapshots', label: 'Snapshots (A / B)', align: 'left' as const },
    { key: 'accrued', label: 'Accrued', align: 'right' as const },
    { key: 'cumulative', label: 'Cumulative', align: 'right' as const },
    { key: 'active', label: 'Active / Reg.', align: 'right' as const, width: '12%' },
  ];
</script>

<svelte:head><title>Airdrop Admin · Rumi Points</title></svelte:head>

<div class="max-w-6xl mx-auto px-4 py-6 flex flex-col gap-4">
  <div class="flex items-center justify-between flex-wrap gap-2">
    <div>
      <h1 class="text-xl font-semibold text-gray-100">Airdrop Admin — Season 1</h1>
      {#if config}
        <p class="text-xs text-gray-500 mt-0.5">
          {fmtNs(config.season_start_ns)} → {fmtNs(config.season_end_ns)}
          {#if seasonPct !== null}· {seasonPct}% elapsed{/if}
        </p>
      {/if}
    </div>
    {#if isAdmin}
      <div class="flex items-center gap-2">
        <button
          class="px-3 py-1.5 rounded-lg text-sm border border-gray-700/50 text-gray-300 hover:border-teal-500/40"
          onclick={refresh}
        >Refresh</button>
        <button
          class="px-3 py-1.5 rounded-lg text-sm border border-gray-700/50 text-teal-400 hover:border-teal-500/40"
          onclick={exportCsv}
        >Export CSV</button>
      </div>
    {/if}
  </div>

  {#if loading}
    <div class="rounded-xl bg-gray-800/30 border border-gray-700/50 p-8 text-center text-sm text-gray-400">
      Loading airdrop state…
    </div>
  {:else if error}
    <div class="rounded-xl bg-gray-800/30 border border-gray-700/50 p-4 flex items-center justify-between gap-3">
      <span class="text-sm text-gray-300">Couldn't load the airdrop admin data.</span>
      <button class="px-3 py-1.5 rounded-lg text-sm border border-gray-700/50 text-teal-400 hover:border-teal-500/40" onclick={loadAll}>
        Retry
      </button>
    </div>
  {:else if !isAdmin}
    <div class="rounded-xl bg-gray-800/30 border border-gray-700/50 p-8 text-center flex flex-col gap-2">
      <span class="text-sm text-gray-200 font-medium">Admin wallet required</span>
      {#if !connectedText}
        <span class="text-xs text-gray-400">
          Connect an allowlisted admin wallet to open this console.
        </span>
      {:else}
        <span class="text-xs text-gray-400">
          The connected wallet is not on the admin allowlist. To grant it access,
          add this principal to ADMIN_VIEW_PRINCIPALS in config.ts and redeploy:
        </span>
        <span class="inline-flex items-center justify-center gap-2 mt-1">
          <span class="font-mono text-[11px] text-gray-300 break-all">{connectedText}</span>
          <button
            class="text-gray-500 hover:text-teal-400 text-xs shrink-0"
            title="Copy principal"
            aria-label="Copy principal"
            onclick={() => copyAddress(connectedText!)}
          >⧉</button>
        </span>
      {/if}
    </div>
  {:else}
    <!-- ── Engine strip ─────────────────────────────────────────────────── -->
    <div class="grid grid-cols-2 md:grid-cols-4 gap-3">
      <div class="rounded-xl bg-gray-800/30 border border-gray-700/50 p-3">
        <div class="text-xs text-gray-500">Current epoch</div>
        <div class="text-lg text-gray-100 tabular-nums">{config ? config.current_epoch_index : '—'}</div>
        {#if openEpoch}
          <div class="text-xs text-gray-500 mt-1">
            open · ends {fmtNs(openEpoch.epoch_end_ns)}
          </div>
        {:else}
          <div class="text-xs text-gray-500 mt-1">no epoch open</div>
        {/if}
      </div>
      <div class="rounded-xl bg-gray-800/30 border border-gray-700/50 p-3">
        <div class="text-xs text-gray-500">Snapshots (open epoch)</div>
        <div class="text-sm text-gray-100 mt-1">
          A: {openEpoch?.snapshot_a_ns?.[0] ? fmtNs(openEpoch.snapshot_a_ns[0], true) : 'pending'}
        </div>
        <div class="text-sm text-gray-100">
          B: {openEpoch?.snapshot_b_ns?.[0] ? fmtNs(openEpoch.snapshot_b_ns[0], true) : 'pending'}
        </div>
      </div>
      <div class="rounded-xl bg-gray-800/30 border border-gray-700/50 p-3">
        <div class="text-xs text-gray-500">Epoch driver</div>
        <div class="text-sm mt-1 {status?.driver_enabled ? 'text-emerald-300' : 'text-amber-300'}">
          {status?.driver_enabled ? 'on' : 'OFF'} · every {status ? Number(status.driver_interval_secs) : '—'}s
        </div>
        <div class="text-xs text-gray-500 mt-1">
          seed {status?.snapshot_seed_committed ? 'committed' : 'MISSING'} ·
          {status ? Number(status.revealed_seed_count) : 0} revealed
        </div>
      </div>
      <div class="rounded-xl bg-gray-800/30 border border-gray-700/50 p-3">
        <div class="text-xs text-gray-500">Ingest poller</div>
        <div class="text-sm mt-1 {ingest?.poll_enabled ? 'text-emerald-300' : 'text-amber-300'}">
          {ingest?.poll_enabled ? 'on' : 'OFF'} · every {ingest ? Number(ingest.poll_interval_secs) : '—'}s
        </div>
        <div class="text-xs text-gray-500 mt-1">
          {board.length} registered · {excluded.length} excluded ·
          {ledgerLen !== null ? `${ledgerLen.toLocaleString()} ledger rows` : 'ledger reader pending upgrade'}
        </div>
      </div>
    </div>

    <!-- ── Ingest sources ───────────────────────────────────────────────── -->
    {#if ingest}
      <div class="rounded-xl bg-gray-800/30 border border-gray-700/50 p-4">
        <h2 class="text-sm font-medium text-gray-300 mb-2">Event sources</h2>
        <div class="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-2">
          {#each ingest.sources as s (s.tag)}
            <div class="rounded-lg bg-gray-900/40 border border-gray-700/30 p-2.5">
              <div class="text-xs text-gray-400">{SOURCE_TAGS[s.tag] ?? `source ${s.tag}`}</div>
              <div class="font-mono text-[11px] text-gray-500 mt-0.5 truncate" title={s.canister.toText()}>
                {s.canister.toText()}
              </div>
              <div class="text-xs text-gray-300 tabular-nums mt-1">cursor {Number(s.cursor).toLocaleString()}</div>
            </div>
          {/each}
        </div>
      </div>
    {/if}

    <!-- ── Season by-source aggregate (needs every breakdown loaded) ────── -->
    <div class="rounded-xl bg-gray-800/30 border border-gray-700/50 p-4">
      <div class="flex items-center justify-between mb-2">
        <h2 class="text-sm font-medium text-gray-300">Season points by source</h2>
        {#if !seasonBySource}
          <button
            class="px-3 py-1.5 rounded-lg text-xs border border-gray-700/50 text-teal-400 hover:border-teal-500/40 disabled:opacity-40"
            disabled={loadingAll}
            onclick={loadEveryDetail}
          >{loadingAll ? 'Loading…' : 'Load all breakdowns'}</button>
        {/if}
      </div>
      {#if seasonBySource}
        {#if seasonBySource.sources.length === 0}
          <p class="text-xs text-gray-500">No accrual rows yet.</p>
        {:else}
          <div class="space-y-1.5">
            {#each seasonBySource.sources as [src, points] (src)}
              {@const meta = SOURCE_META[src] ?? { label: src, mult: '?' }}
              <div class="flex items-center gap-3 text-xs">
                <span class="w-56 shrink-0 text-gray-400">{meta.label} <span class="text-gray-600">({meta.mult})</span></span>
                <div class="flex-1 h-2 rounded bg-gray-900/60 overflow-hidden">
                  <div
                    class="h-full bg-teal-500/60"
                    style={`width: ${seasonBySource.total > 0n ? Number((points * 1000n) / seasonBySource.total) / 10 : 0}%`}
                  ></div>
                </div>
                <span class="w-28 text-right tabular-nums text-gray-200">{formatPoints(points)}</span>
                <span class="w-14 text-right tabular-nums text-gray-500">{pct(points, seasonBySource.total)}</span>
              </div>
            {/each}
          </div>
        {/if}
      {:else}
        <p class="text-xs text-gray-500">
          Load every participant's breakdown to aggregate season points by source
          {#if ledgerLen === null}(needs the ledger-reader canister upgrade){/if}.
        </p>
      {/if}
    </div>

    <!-- ── Participants ─────────────────────────────────────────────────── -->
    <div class="rounded-xl bg-gray-800/30 border border-gray-700/50 p-4">
      <div class="flex items-baseline justify-between mb-2">
        <h2 class="text-sm font-medium text-gray-300">Participants</h2>
        <span class="text-xs text-gray-500 tabular-nums">
          {board.length} registered · {formatPoints(totalPoints)} USD-days total
        </span>
      </div>
      <div class="overflow-x-auto">
        <table class="w-full text-sm">
          <thead>
            <tr class="text-xs text-gray-500 border-b border-gray-700/40">
              <th class="text-left px-2 py-2 font-medium w-12">#</th>
              <th class="text-left px-2 py-2 font-medium">Principal</th>
              <th class="text-right px-2 py-2 font-medium">Points (USD-days)</th>
              <th class="text-right px-2 py-2 font-medium w-20">Share</th>
              <th class="w-8"></th>
            </tr>
          </thead>
          <tbody>
            {#each board as entry (entry.principal.toText())}
              {@const key = entry.principal.toText()}
              {@const d = details[key]}
              <tr
                class="border-b border-gray-700/20 hover:bg-gray-700/10 cursor-pointer"
                onclick={() => toggle(entry)}
              >
                <td class="px-2 py-2.5 text-gray-400 tabular-nums">#{entry.rank}</td>
                <td class="px-2 py-2.5">
                  <span class="inline-flex items-center gap-2">
                    <span class="font-mono text-xs text-teal-400">{truncatePrincipal(key)}</span>
                    <button
                      class="text-gray-500 hover:text-teal-400 text-xs"
                      title="Copy principal"
                      aria-label="Copy principal"
                      onclick={(ev) => { ev.stopPropagation(); copyAddress(key); }}
                    >⧉</button>
                  </span>
                </td>
                <td class="px-2 py-2.5 text-right text-gray-100 tabular-nums">{formatPoints(entry.total_points)}</td>
                <td class="px-2 py-2.5 text-right text-gray-300 tabular-nums">{fmtBps(entry.estimated_share_bps)}</td>
                <td class="px-2 py-2.5 text-center text-gray-500">{expanded[key] ? '▾' : '▸'}</td>
              </tr>
              {#if expanded[key]}
                <tr class="border-b border-gray-700/20 bg-gray-900/30">
                  <td colspan="5" class="px-4 py-3">
                    {#if !d || d.loading}
                      <p class="text-xs text-gray-500">Loading breakdown…</p>
                    {:else if d.failed}
                      <p class="text-xs text-amber-300">Failed to load this principal's detail. <button class="underline" onclick={() => { details[key] = undefined as any; loadDetail(entry); }}>Retry</button></p>
                    {:else}
                      <div class="grid grid-cols-1 lg:grid-cols-2 gap-4">
                        <!-- Registration + positions -->
                        <div class="flex flex-col gap-3">
                          {#if d.state}
                            <div class="grid grid-cols-2 gap-2 text-xs">
                              <div>
                                <div class="text-gray-500">Registered</div>
                                <div class="text-gray-200 mt-0.5">{fmtNs(d.state.registered_at_ns, true)}</div>
                              </div>
                              <div>
                                <div class="text-gray-500">First qualifying action</div>
                                <div class="text-gray-200 mt-0.5">{qualifyingActionLabel(d.state.first_qualifying_action)}</div>
                              </div>
                              <div>
                                <div class="text-gray-500">Last epoch processed</div>
                                <div class="text-gray-200 mt-0.5 tabular-nums">{d.state.last_epoch_processed}</div>
                              </div>
                              <div>
                                <div class="text-gray-500">Repayment windows</div>
                                <div class="text-gray-200 mt-0.5">
                                  {#if d.state.repayment_events.length === 0}
                                    none
                                  {:else}
                                    {d.state.repayment_events.length} ·
                                    {fmtUsdE8s(d.state.repayment_events.reduce((s, r) => s + r.amount_usd, 0n))}
                                  {/if}
                                </div>
                              </div>
                            </div>
                            <div>
                              <div class="text-xs text-gray-500 mb-1">
                                Recorded 3pool composition
                                <span class="text-gray-600">(ingest-tracked, not a live balance; pre-fix records stay inflated until admin_rebuild_3pool_recorded runs)</span>
                              </div>
                              {#if d.state.active_deposits.length === 0}
                                <p class="text-xs text-gray-400">none recorded</p>
                              {:else}
                                <div class="space-y-1">
                                  {#each d.state.active_deposits as [dk, rec] (variantKey(dk.venue) + variantKey(dk.asset))}
                                    <div class="flex items-baseline justify-between text-xs border-b border-white/[0.03] py-0.5">
                                      <span class="text-gray-400">
                                        {variantKey(dk.venue)} · {ASSET_LABEL[variantKey(dk.asset)] ?? variantKey(dk.asset)}
                                      </span>
                                      <span class="tabular-nums text-gray-200">
                                        {fmtUsdE8s(rec.recorded_value_usd)}
                                        <span class="text-gray-500">· since {fmtNs(rec.deposited_at)} · verified {fmtNs(rec.last_verified_at)}</span>
                                      </span>
                                    </div>
                                  {/each}
                                </div>
                              {/if}
                            </div>
                          {:else}
                            <p class="text-xs text-gray-400">No principal state (not registered?).</p>
                          {/if}
                        </div>

                        <!-- Points decomposition -->
                        <div class="flex flex-col gap-3">
                          {#if d.entries === null}
                            <p class="text-xs text-amber-300/80">
                              Per-source breakdown needs the ledger-reader canister upgrade
                              (get_principal_point_entries). The audit rows are on-chain; they
                              just have no reader on the deployed build yet.
                            </p>
                          {:else}
                            {@const b = breakdown(d.entries)}
                            <div>
                              <div class="text-xs text-gray-500 mb-1">Points by source</div>
                              {#if b.sources.length === 0}
                                <p class="text-xs text-gray-400">no accrual rows yet</p>
                              {:else}
                                <div class="space-y-1">
                                  {#each b.sources as [src, points] (src)}
                                    {@const meta = SOURCE_META[src] ?? { label: src, mult: '?' }}
                                    <div class="flex items-baseline justify-between text-xs border-b border-white/[0.03] py-0.5">
                                      <span class="text-gray-400">{meta.label} <span class="text-gray-600">({meta.mult})</span></span>
                                      <span class="tabular-nums text-gray-200">
                                        {formatPoints(points)}
                                        <span class="text-gray-500">· {pct(points, b.total)}</span>
                                      </span>
                                    </div>
                                  {/each}
                                </div>
                              {/if}
                            </div>
                            {#if b.epochs.length > 0}
                              <div>
                                <div class="text-xs text-gray-500 mb-1">Accrual by epoch</div>
                                <div class="flex flex-wrap gap-1.5">
                                  {#each b.epochs as [epoch, points] (epoch)}
                                    <span class="px-2 py-1 rounded bg-gray-900/50 border border-gray-700/30 text-[11px] tabular-nums text-gray-300">
                                      E{epoch}: {formatPoints(points)}
                                    </span>
                                  {/each}
                                </div>
                              </div>
                            {/if}
                          {/if}
                        </div>
                      </div>
                    {/if}
                  </td>
                </tr>
              {/if}
            {/each}
          </tbody>
        </table>
      </div>
    </div>

    <!-- ── Epoch history ────────────────────────────────────────────────── -->
    <div class="rounded-xl bg-gray-800/30 border border-gray-700/50 p-4">
      <h2 class="text-sm font-medium text-gray-300 mb-2">Epoch history</h2>
      {#if epochs.length === 0}
        <p class="text-xs text-gray-500">No closed epochs yet.</p>
      {:else}
        <div class="overflow-x-auto">
          <table class="w-full text-xs">
            <thead>
              <tr class="text-gray-500 border-b border-gray-700/40">
                {#each epochColumns as c (c.key)}
                  <th class="px-2 py-2 font-medium {c.align === 'right' ? 'text-right' : 'text-left'}" style={c.width ? `width:${c.width}` : ''}>{c.label}</th>
                {/each}
              </tr>
            </thead>
            <tbody>
              {#each epochs as e (e.epoch_index)}
                <tr class="border-b border-gray-700/20">
                  <td class="px-2 py-2 text-gray-300 tabular-nums">E{e.epoch_index}</td>
                  <td class="px-2 py-2 text-gray-400">{fmtNs(e.epoch_start_ns)} → {fmtNs(e.epoch_end_ns)}</td>
                  <td class="px-2 py-2 text-gray-400">{fmtNs(e.snapshot_a_ns, true)} / {fmtNs(e.snapshot_b_ns, true)}</td>
                  <td class="px-2 py-2 text-right text-gray-200 tabular-nums">{formatPoints(e.points_accrued_this_epoch)}</td>
                  <td class="px-2 py-2 text-right text-gray-200 tabular-nums">{formatPoints(e.total_points_all)}</td>
                  <td class="px-2 py-2 text-right text-gray-400 tabular-nums">{e.active_principals} / {e.registered_principals}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
    </div>

    <!-- ── Excluded principals ──────────────────────────────────────────── -->
    <details class="rounded-xl bg-gray-800/30 border border-gray-700/50 p-4">
      <summary class="text-sm font-medium text-gray-300 cursor-pointer">
        Excluded principals ({excluded.length})
      </summary>
      <div class="mt-2 grid grid-cols-1 md:grid-cols-2 gap-1">
        {#each excluded as p (p)}
          <div class="font-mono text-[11px] text-gray-400 flex items-center gap-2">
            {p}
            <button
              class="text-gray-600 hover:text-teal-400"
              title="Copy principal"
              aria-label="Copy principal"
              onclick={() => copyAddress(p)}
            >⧉</button>
          </div>
        {/each}
      </div>
      <p class="text-xs text-gray-600 mt-2">
        Excluded principals never register and never accrue (protocol canisters).
      </p>
    </details>

    <p class="text-xs text-gray-600">
      All figures are usd_e8s ÷ 1e8; points are USD-days, not dollars. Every panel
      on this page reads public canister queries — the admin gate is a UI wall,
      not an access control.
    </p>
  {/if}
</div>
