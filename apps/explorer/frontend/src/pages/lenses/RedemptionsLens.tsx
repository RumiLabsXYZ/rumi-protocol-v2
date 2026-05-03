import { useLensRedemptions } from "@/hooks/useBffQueries";
import { LensHealthStrip } from "@/components/lenses/LensHealthStrip";
import { MiniAreaChart } from "@/components/lenses/MiniAreaChart";

export function RedemptionsLens() {
  const { data, isLoading, error } = useLensRedemptions();

  if (isLoading) return <p className="text-muted-foreground">Loading...</p>;
  if (error || !data) return <p className="text-destructive">Failed to load.</p>;

  const points = data.series.map((p) => ({
    t: Number(p.timestamp_ns / 1_000_000n),
    v: p.volume_usd,
  }));

  return (
    <div>
      <h1 className="text-2xl font-semibold mb-2">Redemptions</h1>
      <p className="text-muted-foreground mb-6">Redemption history, tier distribution.</p>

      <LensHealthStrip
        metrics={[
          { label: "30d Count", value: String(data.total_count_30d) },
          { label: "30d Volume", value: `$${data.total_volume_30d_usd.toFixed(2)}` },
        ]}
      />

      <MiniAreaChart
        points={points}
        label="Daily Redemption Volume (30 days)"
        format={(v) => `$${v.toFixed(2)}`}
      />

      <h2 className="text-lg font-semibold mb-3">Recent Events</h2>
      {data.recent_events.length === 0 ? (
        <p className="text-sm text-muted-foreground">No events.</p>
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
              {data.recent_events.map((e, i) => (
                <tr key={`${e.global_id}-${i}`}>
                  <td className="px-4 py-2 font-mono text-xs text-muted-foreground whitespace-nowrap">{e.global_id}</td>
                  <td className="px-4 py-2 whitespace-nowrap">{e.kind}</td>
                  <td className="px-4 py-2 whitespace-nowrap">
                    {e.primary_amount ? e.primary_amount.formatted : "—"}
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
