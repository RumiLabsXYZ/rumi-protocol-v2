import { useSearchParams } from "react-router-dom";
import { useOverview } from "@/hooks/useBffQueries";

export function Overview() {
  const { data, isLoading, error } = useOverview();
  const [params] = useSearchParams();
  const unresolved = params.get("q");

  if (isLoading) {
    return <p className="text-muted-foreground">Loading overview...</p>;
  }

  if (error) {
    return (
      <div className="bg-destructive/10 text-destructive border border-destructive/20 rounded-lg p-4">
        <p className="font-medium">Failed to load overview</p>
        <p className="text-sm mt-1">{error instanceof Error ? error.message : String(error)}</p>
      </div>
    );
  }

  if (!data) return null;

  const cards = [
    { label: "TVL", value: `$${data.tvl_usd.toLocaleString()}` },
    { label: "icUSD supply", value: data.icusd_supply.formatted },
    { label: "Peg", value: `$${data.icusd_peg_usd.toFixed(4)}` },
    { label: "Open vaults", value: String(data.vault_count_open) },
  ];

  return (
    <div>
      {unresolved && (
        <div className="bg-warning/10 text-warning border border-warning/20 rounded-lg p-3 mb-6 text-sm">
          Couldn't resolve <span className="font-mono">{unresolved}</span> as a principal, vault id, or event id.
        </div>
      )}

      <h1 className="text-2xl font-semibold mb-2">Overview</h1>
      <p className="text-muted-foreground mb-6">Protocol-wide health + recent activity.</p>

      <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4 mb-8">
        {cards.map((c) => (
          <div key={c.label} className="bg-card border border-border rounded-lg p-4">
            <p className="text-xs uppercase text-muted-foreground tracking-wide">{c.label}</p>
            <p className="text-2xl font-semibold mt-1">{c.value}</p>
          </div>
        ))}
      </div>

      <div>
        <h2 className="text-lg font-semibold mb-3">Recent activity (stub)</h2>
        <div className="bg-card border border-border rounded-lg divide-y divide-border">
          {data.recent_activity.map((e) => (
            <div key={e.global_id} className="px-4 py-3 text-sm flex items-center justify-between gap-4">
              <span className="font-mono text-xs text-muted-foreground">{e.global_id}</span>
              <span className="flex-1">{e.payload_summary}</span>
              <span className="text-xs text-muted-foreground whitespace-nowrap">{e.kind}</span>
            </div>
          ))}
        </div>
      </div>

      <p className="mt-6 text-xs text-muted-foreground">
        Currently rendering stub data from the BFF. Real data wires up in Plan 2.
      </p>
    </div>
  );
}
