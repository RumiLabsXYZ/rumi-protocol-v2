<script lang="ts">
  import { onMount } from 'svelte';
  import { Principal } from '@dfinity/principal';
  import LensHealthStrip from '../LensHealthStrip.svelte';
  import LensActivityPanel from '../LensActivityPanel.svelte';
  import MiniAreaChart from '../MiniAreaChart.svelte';
  import { fetchFeeSeries, fetchFeeBreakdownWindow } from '$services/explorer/analyticsService';
  import {
    fetchRedemptionRate, fetchRedemptionFeeFloor, fetchRedemptionFeeCeiling,
    fetchRedemptionTier, fetchCollateralTotals, fetchProtocolConfig, fetchProtocolStatus,
  } from '$services/explorer/explorerService';
  import { publicActor } from '$lib/services/protocol/apiClient';
  import {
    e8sToNumber, formatCompact, CHART_COLORS, COLLATERAL_SYMBOLS, getCollateralSymbol,
  } from '$utils/explorerChartHelpers';

  let feeRows: any[] = $state([]);
  let rate: number | null = $state(null);
  let feeFloor: number | null = $state(null);
  let feeCeiling: number | null = $state(null);
  let rmrFloor: number | null = $state(null);
  let rmrCeiling: number | null = $state(null);
  let rmrFloorCr: number | null = $state(null);
  let rmrCeilingCr: number | null = $state(null);
  let totalCollateralRatio: number | null = $state(null);
  let collateralTotals: any[] = $state([]);
  let tierMap: Map<string, number | null> = $state(new Map());
  let loading = $state(true);
  // Live 90-day count + fee totals computed from the source-of-truth event
  // logs (analytics for count, backend event log for fees) instead of from
  // the daily rollup series. Rollups only see the trailing 24h at scrape time
  // and don't backfill, so historical redemption events that pre-date a
  // collector deploy never get a non-zero rollup row. The backend event log
  // is authoritative for fees because some early AnalyticsVaultEvent rows
  // were tailed before fee_amount was tracked and decode as None.
  let live90dRedemptionCount: number | null = $state(null);
  let live90dRedemptionFeesIcusd: number | null = $state(null);
  // Daily-bucketed live redemption activity (count + fees) over the same 90d
  // window. Populated alongside the headline numbers so the two chart cards
  // ("Daily redemption count" / "Daily redemption fees") show real activity
  // instead of the all-zeros rollup output.
  let liveDailyCountPoints: { t: number; v: number }[] | null = $state(null);
  let liveDailyFeePoints: { t: number; v: number }[] | null = $state(null);

  onMount(async () => {
    try {
      const principals = Object.keys(COLLATERAL_SYMBOLS);
      const [feeR, rateR, floorR, ceilR, totalsR, cfgR, statusR, ...tierRs] = await Promise.allSettled([
        fetchFeeSeries(90),
        fetchRedemptionRate(),
        fetchRedemptionFeeFloor(),
        fetchRedemptionFeeCeiling(),
        fetchCollateralTotals(),
        fetchProtocolConfig(),
        fetchProtocolStatus(),
        ...principals.map(p => fetchRedemptionTier(Principal.fromText(p))),
      ]);
      if (feeR.status === 'fulfilled') feeRows = feeR.value ?? [];
      if (rateR.status === 'fulfilled') rate = rateR.value;
      if (floorR.status === 'fulfilled') feeFloor = floorR.value;
      if (ceilR.status === 'fulfilled') feeCeiling = ceilR.value;
      if (totalsR.status === 'fulfilled') collateralTotals = totalsR.value ?? [];
      if (cfgR.status === 'fulfilled' && cfgR.value) {
        rmrFloor = typeof cfgR.value.rmr_floor === 'number' ? cfgR.value.rmr_floor : null;
        rmrCeiling = typeof cfgR.value.rmr_ceiling === 'number' ? cfgR.value.rmr_ceiling : null;
        rmrFloorCr = typeof cfgR.value.rmr_floor_cr === 'number' ? cfgR.value.rmr_floor_cr : null;
        rmrCeilingCr = typeof cfgR.value.rmr_ceiling_cr === 'number' ? cfgR.value.rmr_ceiling_cr : null;
      }
      if (statusR.status === 'fulfilled' && statusR.value) {
        totalCollateralRatio = typeof statusR.value.total_collateral_ratio === 'number'
          ? statusR.value.total_collateral_ratio
          : null;
      }

      const tm = new Map<string, number | null>();
      for (let i = 0; i < principals.length; i++) {
        const r = tierRs[i];
        if (r.status === 'fulfilled') tm.set(principals[i], r.value);
        else tm.set(principals[i], null);
      }
      tierMap = tm;
    } catch (err) {
      console.error('[RedemptionsLens] onMount error:', err);
    } finally {
      loading = false;
    }

    // Live 90d redemption totals — fire after the main load so the UI is not
    // blocked. Count comes from the analytics live query (cheap, single
    // round-trip). Fees come from the backend event log filtered to
    // Redemption type, summed client-side, because the AnalyticsVaultEvent
    // rows for historical redemptions stored fee_amount=None and the rollup
    // path therefore reports $0 even when there were real fees collected.
    fetchFeeBreakdownWindow(90)
      .then((b) => { live90dRedemptionCount = b.redemptionCount; })
      .catch((err) => console.error('[RedemptionsLens] fee breakdown failed:', err));

    (async () => {
      try {
        const nowMs = Date.now();
        const cutoffMs = nowMs - 90 * 24 * 60 * 60 * 1000;
        const cutoffNs = BigInt(cutoffMs) * 1_000_000n;
        const DAY_MS = 86_400_000;
        // Bucket day-keyed sums so the chart can render daily granularity even
        // when the rollup table is all zeros (historic redemptions in the
        // analytics canister have fee_amount=None so the daily rollup misses
        // them; backend event log is authoritative).
        const dayBucket = (tsMs: number) => Math.floor(tsMs / DAY_MS) * DAY_MS;
        const countByDay = new Map<number, number>();
        const feeByDay = new Map<number, number>();
        let totalFee = 0n;
        const PAGE = 200n;
        let page = 0n;
        // get_events_filtered returns newest-first; stop once we cross the 90d boundary.
        while (true) {
          const resp = await publicActor.get_events_filtered({
            start: page,
            length: PAGE,
            types: [[{ Redemption: null }]],
            principal: [],
            collateral_token: [],
            time_range: [],
            min_size_e8s: [],
            admin_labels: [],
          });
          // events: Array<[bigint, Event]>
          const tuples = (resp as any)?.events ?? [];
          if (tuples.length === 0) break;
          let crossed = false;
          for (const pair of tuples) {
            const evt = Array.isArray(pair) ? pair[1] : (pair?.event ?? pair);
            const et = (evt as any)?.event_type ?? evt;
            const inner = et?.redemption_on_vaults;
            if (!inner) continue;
            const tsArr = inner.timestamp;
            const ts = Array.isArray(tsArr) ? tsArr[0] : tsArr;
            if (ts != null && BigInt(ts) < cutoffNs) {
              crossed = true;
              break;
            }
            const fee = inner.fee_amount;
            if (fee != null) totalFee += BigInt(fee);
            const tsMs = ts != null ? Number(BigInt(ts) / 1_000_000n) : nowMs;
            const bucket = dayBucket(tsMs);
            countByDay.set(bucket, (countByDay.get(bucket) ?? 0) + 1);
            if (fee != null) {
              feeByDay.set(bucket, (feeByDay.get(bucket) ?? 0) + Number(fee) / 1e8);
            }
          }
          if (crossed || tuples.length < Number(PAGE)) break;
          page += 1n;
        }
        live90dRedemptionFeesIcusd = Number(totalFee) / 1e8;

        // Pad zero-buckets across the full 90d window so the chart renders a
        // proper daily bar series instead of just the active days.
        const startBucket = dayBucket(cutoffMs);
        const endBucket = dayBucket(nowMs);
        const cPts: { t: number; v: number }[] = [];
        const fPts: { t: number; v: number }[] = [];
        for (let t = startBucket; t <= endBucket; t += DAY_MS) {
          cPts.push({ t, v: countByDay.get(t) ?? 0 });
          fPts.push({ t, v: feeByDay.get(t) ?? 0 });
        }
        liveDailyCountPoints = cPts;
        liveDailyFeePoints = fPts;
      } catch (err) {
        console.error('[RedemptionsLens] backend redemption fee scan failed:', err);
      }
    })();
  });

  // Prefer the live total when it has resolved; fall back to the rollup sum
  // until the live query lands so the card never shows a stale 0 longer than
  // necessary.
  const rollupRedemptions90d = $derived(feeRows.reduce((s, d: any) => s + Number(d.redemption_count ?? 0), 0));
  const rollupRedemptionFees90d = $derived(
    feeRows.reduce((s, d: any) => s + e8sToNumber(d.redemption_fees_e8s?.[0] ?? d.redemption_fees_e8s ?? 0n), 0)
  );
  const redemptions90d = $derived(live90dRedemptionCount ?? rollupRedemptions90d);
  const redemptionFees90d = $derived(live90dRedemptionFeesIcusd ?? rollupRedemptionFees90d);
  // Use live-scan daily buckets when they've resolved (real activity); fall
  // back to the rollup-derived points only as a placeholder until then. The
  // rollup table is all-zeros for redemptions today (see headline-numbers
  // comment above) so the live points are usually authoritative.
  const redemptionCountPoints = $derived(
    liveDailyCountPoints
      ?? feeRows.map((r: any) => ({ t: Number(r.timestamp_ns) / 1_000_000, v: Number(r.redemption_count ?? 0) }))
  );
  const redemptionFeePoints = $derived(
    liveDailyFeePoints
      ?? feeRows.map((r: any) => ({
        t: Number(r.timestamp_ns) / 1_000_000,
        v: e8sToNumber(r.redemption_fees_e8s?.[0] ?? r.redemption_fees_e8s ?? 0n),
      }))
  );

  const formatPct = (v: number | null) =>
    v == null ? '--' : `${(v * 100).toFixed(2)}%`;

  // Active RMR: linear interpolation matching backend get_redemption_margin_ratio().
  // tcr (total_collateral_ratio) is an absolute ratio (e.g. 2.5 = 250% CR).
  // rmrFloorCr / rmrCeilingCr are also absolute ratios (e.g. 2.25 / 1.50).
  // rmrFloor / rmrCeiling are decimal fractions (e.g. 0.96 / 1.0).
  const activeRmr = $derived.by((): number | null => {
    if (rmrFloor == null || rmrCeiling == null || rmrFloorCr == null || rmrCeilingCr == null) return null;
    const tcr = totalCollateralRatio;
    if (tcr == null) return null;
    if (tcr <= rmrCeilingCr) return rmrCeiling;
    if (tcr >= rmrFloorCr) return rmrFloor;
    const range = rmrFloorCr - rmrCeilingCr;
    const position = tcr - rmrCeilingCr;
    const spread = rmrCeiling - rmrFloor;
    return rmrCeiling - (position / range) * spread;
  });

  const activeRmrLabel = $derived.by(() => {
    if (activeRmr == null) return '--';
    return `${(activeRmr * 100).toFixed(2)}%`;
  });

  // Sub-text for the RMR tile: show what CR drove it and the configured range.
  const activeRmrSub = $derived.by(() => {
    const crPct = totalCollateralRatio != null ? `at ${(totalCollateralRatio * 100).toFixed(0)}% CR` : '';
    if (rmrFloor == null || rmrCeiling == null || rmrFloorCr == null || rmrCeilingCr == null) return crPct || undefined;
    const floorPct = `${(rmrFloor * 100).toFixed(0)}%`;
    const ceilPct = `${(rmrCeiling * 100).toFixed(0)}%`;
    const floorCrPct = `${(rmrFloorCr * 100).toFixed(0)}%`;
    const ceilCrPct = `${(rmrCeilingCr * 100).toFixed(0)}%`;
    const range = `${floorPct} (CR ${floorCrPct}) to ${ceilPct} (CR ${ceilCrPct})`;
    return crPct ? `${crPct}; slides ${range}` : `slides ${range}`;
  });

  const healthMetrics = $derived.by(() => [
    { label: 'Live rate', value: formatPct(rate), sub: 'current redemption fee' },
    { label: 'Fee floor', value: formatPct(feeFloor), tone: 'muted' as const },
    { label: 'Fee ceiling', value: formatPct(feeCeiling), tone: 'muted' as const },
    { label: 'Current RMR', value: activeRmrLabel, sub: activeRmrSub },
    { label: 'Redemptions (90d)', value: redemptions90d.toLocaleString() },
    { label: 'Fees collected (90d)', value: `$${formatCompact(redemptionFees90d)}`, sub: 'icUSD' },
  ]);

  const tierRows = $derived.by(() => {
    const totalsByPid = new Map<string, any>();
    for (const t of collateralTotals) {
      const pid = t.collateral_type?.toText?.() ?? String(t.collateral_type ?? '');
      if (pid) totalsByPid.set(pid, t);
    }
    const rows: { principal: string; symbol: string; tier: number | null; debt: number; vaultCount: number }[] = [];
    for (const [pid, sym] of Object.entries(COLLATERAL_SYMBOLS)) {
      const t = totalsByPid.get(pid);
      rows.push({
        principal: pid,
        symbol: sym,
        tier: tierMap.get(pid) ?? null,
        debt: t?.total_debt != null ? e8sToNumber(t.total_debt) : 0,
        vaultCount: t?.vault_count != null ? Number(t.vault_count) : 0,
      });
    }
    rows.sort((a, b) => {
      const at = a.tier ?? 999;
      const bt = b.tier ?? 999;
      if (at !== bt) return at - bt;
      return b.debt - a.debt;
    });
    return rows;
  });

  function tierTone(tier: number | null): string {
    if (tier == null) return 'text-gray-500';
    if (tier === 1) return 'text-teal-400';
    if (tier === 2) return 'text-amber-400';
    return 'text-violet-400';
  }
</script>

<LensHealthStrip title="Redemptions" metrics={healthMetrics} loading={loading} />

<div class="grid grid-cols-1 lg:grid-cols-2 gap-3">
  <div class="explorer-card">
    <MiniAreaChart
      points={redemptionCountPoints}
      label="Daily redemption count (90d)"
      color={CHART_COLORS.teal}
      fillColor={CHART_COLORS.tealDim}
      valueFormat={(v) => v.toLocaleString(undefined, { maximumFractionDigits: 0 })}
      headlineValue={redemptions90d}
      loading={loading}
    />
  </div>
  <div class="explorer-card">
    <MiniAreaChart
      points={redemptionFeePoints}
      label="Daily redemption fees (90d)"
      color={CHART_COLORS.purple}
      fillColor={CHART_COLORS.purpleDim}
      valueFormat={(v) => `$${formatCompact(v)}`}
      headlineValue={redemptionFees90d}
      loading={loading}
    />
  </div>
</div>

<div class="explorer-card">
  <h3 class="text-sm font-medium text-gray-300 mb-3">Redemption tiers</h3>
  {#if loading}
    <div class="flex items-center justify-center py-6">
      <div class="w-5 h-5 border-2 border-gray-600 border-t-teal-400 rounded-full animate-spin"></div>
    </div>
  {:else}
    <div class="overflow-x-auto">
      <table class="w-full text-sm">
        <thead>
          <tr class="border-b border-white/5">
            <th class="text-left py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">Token</th>
            <th class="text-right py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">Tier</th>
            <th class="text-right py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">Vaults</th>
            <th class="text-right py-2 px-2 text-xs font-medium text-gray-500 uppercase tracking-wider">Outstanding debt</th>
          </tr>
        </thead>
        <tbody>
          {#each tierRows as r}
            <tr class="border-b border-white/[0.03] hover:bg-white/[0.02]">
              <td class="py-2 px-2 font-medium text-gray-200">{r.symbol}</td>
              <td class="py-2 px-2 text-right tabular-nums {tierTone(r.tier)}">
                {r.tier != null ? `Tier ${r.tier}` : '--'}
              </td>
              <td class="py-2 px-2 text-right tabular-nums text-gray-400">{r.vaultCount}</td>
              <td class="py-2 px-2 text-right tabular-nums text-gray-300">{formatCompact(r.debt)} icUSD</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
    <p class="text-xs text-gray-500 mt-3">
      Lower tiers are redeemed against first. Tier 1 absorbs redemptions before tier 2, which absorbs before tier 3.
    </p>
  {/if}
</div>

<LensActivityPanel scope="redemptions" title="Redemption activity" viewAllHref="/explorer/activity?type=redemption" />
