import { useLensAdmin } from "@/hooks/useBffQueries";
import { LensHealthStrip } from "@/components/lenses/LensHealthStrip";
import { LedgerEntry } from "@/components/design/LedgerEntry";

function modeLabel(mode: unknown): string {
  const m = mode as Record<string, null>;
  if ("GeneralAvailability" in m) return "General Availability";
  if ("Recovery" in m) return "Recovery";
  if ("Caution" in m) return "Caution";
  if ("ReadOnly" in m) return "Read Only";
  if ("Emergency" in m) return "Emergency";
  return "Unknown";
}

export function AdminLens() {
  const { data, isLoading, error } = useLensAdmin();

  if (isLoading) return <p className="text-ink-muted">Loading...</p>;
  if (error || !data) return <p className="text-cinnabar">Failed to load.</p>;

  const breakerMetric = data.any_breaker_tripped
    ? { label: "Circuit Breaker", value: "Tripped", sub: "Mass-liquidation paused" }
    : { label: "Circuit Breaker", value: "Clear" };

  return (
    <div>
      <header className="mb-8 pb-4 border-b border-quartz">
        <h1 className="text-3xl font-semibold tracking-tightest text-ink-primary">Admin</h1>
        <p className="text-sm text-ink-muted mt-1">Parameter changes, mode transitions, breaker events.</p>
      </header>

      <LensHealthStrip
        metrics={[
          { label: "Protocol Mode", value: modeLabel(data.protocol_mode) },
          breakerMetric,
        ]}
      />

      <h2 className="text-sm font-medium tracking-[0.08em] uppercase text-ink-muted mb-3">Audit log</h2>
      <div className="bg-vellum-raised border border-quartz rounded-md overflow-hidden">
        {data.recent_admin_events.length === 0 ? (
          <p className="px-4 py-6 text-sm text-ink-muted">No admin events.</p>
        ) : (
          data.recent_admin_events.map((e, i) => (
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
