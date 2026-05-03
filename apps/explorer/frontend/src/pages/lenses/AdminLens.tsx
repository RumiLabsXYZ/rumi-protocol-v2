import { useLensAdmin } from "@/hooks/useBffQueries";
import { LensHealthStrip } from "@/components/lenses/LensHealthStrip";

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

  if (isLoading) return <p className="text-muted-foreground">Loading...</p>;
  if (error || !data) return <p className="text-destructive">Failed to load.</p>;

  return (
    <div>
      <h1 className="text-2xl font-semibold mb-2">Admin</h1>
      <p className="text-muted-foreground mb-6">Parameter changes, mode transitions, breaker events.</p>

      <LensHealthStrip
        metrics={[
          { label: "Protocol Mode", value: modeLabel(data.protocol_mode) },
          {
            label: "Circuit Breaker",
            value: data.any_breaker_tripped ? "Tripped" : "Clear",
            sub: data.any_breaker_tripped ? "Mass-liquidation paused" : undefined,
          },
        ]}
      />

      <h2 className="text-lg font-semibold mb-3">Recent Admin Events</h2>
      {data.recent_admin_events.length === 0 ? (
        <p className="text-sm text-muted-foreground">No admin events.</p>
      ) : (
        <div className="bg-card border border-border rounded-lg overflow-x-auto">
          <table className="w-full text-sm">
            <thead className="bg-secondary/30">
              <tr className="text-left text-xs uppercase text-muted-foreground">
                <th className="px-4 py-2 font-medium">ID</th>
                <th className="px-4 py-2 font-medium">Kind</th>
                <th className="px-4 py-2 font-medium">Amount</th>
                <th className="px-4 py-2 font-medium">Description</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {data.recent_admin_events.map((e, i) => (
                <tr key={`${e.global_id}-${i}`}>
                  <td className="px-4 py-2 font-mono text-xs text-muted-foreground whitespace-nowrap">{e.global_id}</td>
                  <td className="px-4 py-2 whitespace-nowrap">{e.kind}</td>
                  <td className="px-4 py-2 whitespace-nowrap">
                    {e.primary_amount?.formatted ?? "—"}
                  </td>
                  <td className="px-4 py-2">{e.payload_summary}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
