import { useLensRevenue } from "@/hooks/useBffQueries";
import { LensHealthStrip } from "@/components/lenses/LensHealthStrip";
import { MiniAreaChart } from "@/components/lenses/MiniAreaChart";

export function RevenueLens() {
  const { data, isLoading, error } = useLensRevenue();

  if (isLoading) return <p className="text-muted-foreground">Loading...</p>;
  if (error || !data) return <p className="text-destructive">Failed to load.</p>;

  // Sum all fee types per day for the chart
  const points = data.series.map((p) => ({
    t: Number(p.timestamp_ns / 1_000_000n),
    v: p.borrow_fees_usd + p.redemption_fees_usd + p.swap_fees_usd,
  }));

  return (
    <div>
      <h1 className="text-2xl font-semibold mb-2">Revenue</h1>
      <p className="text-muted-foreground mb-6">Borrowing fees, redemption fees, swap fees.</p>

      <LensHealthStrip
        metrics={[
          { label: "30d Total Fees", value: `$${data.total_fees_30d_usd.toFixed(2)}` },
        ]}
      />

      <MiniAreaChart
        points={points}
        label="Daily Fees (30 days)"
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
