import { useLensDex } from "@/hooks/useBffQueries";
import { LensHealthStrip } from "@/components/lenses/LensHealthStrip";
import { MiniAreaChart } from "@/components/lenses/MiniAreaChart";
import { LedgerEntry } from "@/components/design/LedgerEntry";

export function DexLens() {
  const { data, isLoading, error } = useLensDex();

  if (isLoading) return <p className="text-ink-muted">Loading...</p>;
  if (error || !data) return <p className="text-cinnabar">Failed to load.</p>;

  const volumePoints = data.series.map((p) => ({
    t: Number(p.timestamp_ns / 1_000_000n),
    v: p.volume_usd,
  }));

  const metrics = [
    { label: "24h Swaps", value: String(data.swap_count_24h) },
    { label: "24h Volume", value: `$${data.volume_24h_usd.toFixed(2)}` },
  ];
  if (data.virtual_price !== undefined) {
    metrics.push({
      label: "Virtual Price",
      value: data.virtual_price.toFixed(6),
      // Sub label shows distance from $1.00 peg
      sub: data.virtual_price >= 1.0 ? "above $1.00" : "below $1.00",
    } as typeof metrics[0]);
  }

  return (
    <div>
      <header className="mb-8 pb-4 border-b border-quartz">
        <h1 className="text-3xl font-semibold tracking-tightest text-ink-primary">DEX</h1>
        <p className="text-sm text-ink-muted mt-1">3pool + AMM swap volume, liquidity, virtual price.</p>
      </header>

      <LensHealthStrip metrics={metrics} />

      <MiniAreaChart
        points={volumePoints}
        label="Daily swap volume (30 days)"
        format={(v) => `$${v.toFixed(2)}`}
      />

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
