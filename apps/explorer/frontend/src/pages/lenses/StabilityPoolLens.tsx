import { useLensStabilityPool } from "@/hooks/useBffQueries";
import { LensHealthStrip } from "@/components/lenses/LensHealthStrip";
import { MiniAreaChart } from "@/components/lenses/MiniAreaChart";
import { LedgerEntry } from "@/components/design/LedgerEntry";

export function StabilityPoolLens() {
  const { data, isLoading, error } = useLensStabilityPool();

  if (isLoading) return <p className="text-ink-muted">Loading...</p>;
  if (error || !data) return <p className="text-cinnabar">Failed to load.</p>;

  const points = data.series.map((p) => ({
    t: Number(p.timestamp_ns / 1_000_000n),
    v: p.total_deposits_usd,
  }));

  return (
    <div>
      <header className="mb-8 pb-4 border-b border-quartz">
        <h1 className="text-3xl font-semibold tracking-tightest text-ink-primary">Stability Pool</h1>
        <p className="text-sm text-ink-muted mt-1">Deposits, current yield, recent activity.</p>
      </header>

      <LensHealthStrip
        metrics={[
          { label: "Total Deposits", value: `$${data.total_deposits_usd.toFixed(2)}` },
          { label: "Current APY", value: `${data.current_apy_pct.toFixed(2)}%` },
        ]}
      />

      <MiniAreaChart
        points={points}
        label="Deposits (30 days)"
        format={(v) => `$${v.toFixed(0)}`}
        yAxisMode="data-fit"
      />

      <div className="bg-vellum-raised border border-quartz rounded-md px-4 py-3 mb-6 max-w-prose">
        <p className="text-[10px] uppercase tracking-[0.1em] text-ink-muted font-medium mb-1">About the Stability Pool</p>
        <p className="text-sm text-ink-muted leading-relaxed">
          The Stability Pool absorbs icUSD debt during liquidations. When a vault falls
          below the minimum collateral ratio, the pool's icUSD is burned to cover the
          debt and depositors receive the collateral at a discount. Yield comes from
          liquidation discounts and protocol fees.
        </p>
      </div>

      <h2 className="text-sm font-medium tracking-[0.08em] uppercase text-ink-muted mb-3">Recent Events</h2>
      <div className="bg-vellum-raised border border-quartz rounded-md overflow-hidden">
        {data.recent_events.length === 0 ? (
          <p className="px-4 py-6 text-sm text-ink-muted">No events.</p>
        ) : (
          data.recent_events.map((e, i) => (
            <LedgerEntry
              key={`${e.global_id}-${i}`}
              timestampNs={e.timestamp_ns}
              kind={e.kind}
              summary={e.payload_summary}
              amount={e.primary_amount?.formatted ?? null}
              id={e.global_id}
            />
          ))
        )}
      </div>
    </div>
  );
}
