import { useLensRevenue } from "@/hooks/useBffQueries";
import { LensHealthStrip } from "@/components/lenses/LensHealthStrip";
import { MiniAreaChart } from "@/components/lenses/MiniAreaChart";
import { LedgerEntry } from "@/components/design/LedgerEntry";

export function RevenueLens() {
  const { data, isLoading, error } = useLensRevenue();

  if (isLoading) return <p className="text-ink-muted">Loading...</p>;
  if (error || !data) return <p className="text-cinnabar">Failed to load.</p>;

  const borrowPoints = data.series.map((p) => ({
    t: Number(p.timestamp_ns / 1_000_000n),
    v: p.borrow_fees_usd,
  }));

  const redemptionPoints = data.series.map((p) => ({
    t: Number(p.timestamp_ns / 1_000_000n),
    v: p.redemption_fees_usd,
  }));

  const swapPoints = data.series.map((p) => ({
    t: Number(p.timestamp_ns / 1_000_000n),
    v: p.swap_fees_usd,
  }));

  return (
    <div>
      <header className="mb-8 pb-4 border-b border-quartz">
        <h1 className="text-3xl font-semibold tracking-tightest text-ink-primary">Revenue</h1>
        <p className="text-sm text-ink-muted mt-1">Borrowing fees, redemption fees, swap fees.</p>
      </header>

      <LensHealthStrip
        metrics={[
          { label: "30d Total Fees", value: `$${data.total_fees_30d_usd.toFixed(2)}` },
        ]}
      />

      <div className="grid grid-cols-1 md:grid-cols-3 gap-4 mb-6">
        <MiniAreaChart
          points={borrowPoints}
          label="Borrow fees"
          format={(v) => `$${v.toFixed(2)}`}
          height={160}
        />
        <MiniAreaChart
          points={redemptionPoints}
          label="Redemption fees"
          format={(v) => `$${v.toFixed(2)}`}
          height={160}
        />
        <MiniAreaChart
          points={swapPoints}
          label="Swap fees"
          format={(v) => `$${v.toFixed(2)}`}
          height={160}
        />
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
