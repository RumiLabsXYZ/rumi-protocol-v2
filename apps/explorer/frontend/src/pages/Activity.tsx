import { useSearchParams, useNavigate } from "react-router-dom";
import { useActivity } from "@/hooks/useBffQueries";
import { parseFilters, filtersToParams, type ActivityFilters } from "@/lib/activityFilters";

const ALL_TYPES = [
  "open_vault", "close_vault", "borrow", "repay",
  "liquidation", "partial_liquidation",
  "redemption", "reserve_redemption",
  "stability_pool_deposit", "stability_pool_withdraw",
  "admin_mint", "admin_sweep_to_treasury",
  "price_update", "accrue_interest",
];

export function Activity() {
  const [searchParams] = useSearchParams();
  const navigate = useNavigate();
  const filters = parseFilters(searchParams);
  const { data, isLoading, error } = useActivity(filters);

  function updateFilters(patch: Partial<ActivityFilters>) {
    const next: ActivityFilters = { ...filters, ...patch, cursor: null }; // changing filters resets cursor
    navigate("/activity?" + filtersToParams(next).toString());
  }

  function loadMore() {
    if (!data?.next_cursor) return;
    const cursor = data.next_cursor;
    navigate("/activity?" + filtersToParams({ ...filters, cursor }).toString());
  }

  function toggleType(t: string) {
    const next = filters.types.includes(t)
      ? filters.types.filter((x) => x !== t)
      : [...filters.types, t];
    updateFilters({ types: next });
  }

  return (
    <div>
      <h1 className="text-2xl font-semibold mb-2">Activity</h1>
      <p className="text-muted-foreground mb-6">Filterable feed across every source.</p>

      <div className="bg-card border border-border rounded-lg p-4 mb-6">
        <p className="text-xs uppercase text-muted-foreground tracking-wide mb-2">Filter by type</p>
        <div className="flex flex-wrap gap-2">
          {ALL_TYPES.map((t) => (
            <button
              key={t}
              onClick={() => toggleType(t)}
              className={`text-xs px-2 py-1 rounded-md border transition-colors ${
                filters.types.includes(t)
                  ? "bg-primary text-primary-foreground border-primary"
                  : "bg-secondary text-secondary-foreground border-border hover:bg-secondary/80"
              }`}
            >
              {t}
            </button>
          ))}
          {filters.types.length > 0 && (
            <button
              onClick={() => updateFilters({ types: [] })}
              className="text-xs px-2 py-1 rounded-md border border-border text-muted-foreground hover:bg-secondary"
            >
              Clear
            </button>
          )}
        </div>
      </div>

      {isLoading && <p className="text-muted-foreground">Loading...</p>}

      {error && (
        <div className="bg-destructive/10 text-destructive border border-destructive/20 rounded-lg p-4">
          Failed: {error instanceof Error ? error.message : String(error)}
        </div>
      )}

      {data && (
        <>
          <p className="text-xs text-muted-foreground mb-3">
            {String(data.total_estimated)} total · showing {data.events.length}
          </p>

          <div className="bg-card border border-border rounded-lg overflow-x-auto">
            <table className="w-full text-sm">
              <thead className="bg-secondary/30">
                <tr className="text-left text-xs uppercase text-muted-foreground">
                  <th className="px-4 py-2 font-medium">ID</th>
                  <th className="px-4 py-2 font-medium">Kind</th>
                  <th className="px-4 py-2 font-medium">Amount</th>
                  <th className="px-4 py-2 font-medium">Description</th>
                  <th className="px-4 py-2 font-medium whitespace-nowrap">Time</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-border">
                {data.events.map((e, i) => (
                  <tr key={`${e.global_id}-${i}`}>
                    <td className="px-4 py-2 font-mono text-xs text-muted-foreground whitespace-nowrap">{e.global_id}</td>
                    <td className="px-4 py-2 whitespace-nowrap">{e.kind}</td>
                    <td className="px-4 py-2 whitespace-nowrap">
                      {e.primary_amount ? e.primary_amount.formatted : "—"}
                    </td>
                    <td className="px-4 py-2">{e.payload_summary}</td>
                    <td className="px-4 py-2 text-xs text-muted-foreground whitespace-nowrap">
                      {formatTimestamp(e.timestamp_ns)}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>

          {data.next_cursor && (
            <button
              onClick={loadMore}
              className="mt-4 px-4 py-2 bg-secondary text-secondary-foreground rounded-md border border-border text-sm hover:bg-secondary/80"
            >
              Load older →
            </button>
          )}
        </>
      )}
    </div>
  );
}

function formatTimestamp(ns: bigint): string {
  const ms = Number(ns / 1_000_000n);
  const d = new Date(ms);
  return d.toLocaleString("en-US", { dateStyle: "short", timeStyle: "short" });
}
