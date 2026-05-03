import { useLensRedemptions } from "@/hooks/useBffQueries";
import { LensHealthStrip } from "@/components/lenses/LensHealthStrip";
import { LedgerEntry } from "@/components/design/LedgerEntry";

export function RedemptionsLens() {
  const { data, isLoading, error } = useLensRedemptions();

  if (isLoading) return <p className="text-ink-muted">Loading...</p>;
  if (error || !data) return <p className="text-cinnabar">Failed to load.</p>;

  return (
    <div>
      <header className="mb-8 pb-4 border-b border-quartz">
        <h1 className="text-3xl font-semibold tracking-tightest text-ink-primary">Redemptions</h1>
        <p className="text-sm text-ink-muted mt-1">Redemption history, tier distribution.</p>
      </header>

      <LensHealthStrip
        metrics={[
          { label: "30d Count", value: String(data.total_count_30d) },
          { label: "30d Volume", value: `$${data.total_volume_30d_usd.toFixed(2)}` },
        ]}
      />

      <h2 className="text-sm font-medium tracking-[0.08em] uppercase text-ink-muted mb-3">Redemption history</h2>
      <div className="bg-vellum-raised border border-quartz rounded-md overflow-hidden">
        {data.recent_events.length === 0 ? (
          <p className="px-4 py-6 text-sm text-ink-muted">No redemption events recorded.</p>
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
