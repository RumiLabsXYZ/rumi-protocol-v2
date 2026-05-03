import { useLensStabilityPool } from "@/hooks/useBffQueries";
import { LensHealthStrip } from "@/components/lenses/LensHealthStrip";
import { MiniAreaChart } from "@/components/lenses/MiniAreaChart";

export function StabilityPoolLens() {
  const { data, isLoading, error } = useLensStabilityPool();

  if (isLoading) return <p className="text-muted-foreground">Loading...</p>;
  if (error || !data) return <p className="text-destructive">Failed to load.</p>;

  const points = data.series.map((p) => ({
    t: Number(p.timestamp_ns / 1_000_000n),
    v: p.total_deposits_usd,
  }));

  return (
    <div>
      <h1 className="text-2xl font-semibold mb-2">Stability Pool</h1>
      <p className="text-muted-foreground mb-6">Deposits, current yield, recent activity.</p>

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
