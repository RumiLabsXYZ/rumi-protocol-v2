import { useSearchParams, useNavigate } from "react-router-dom";
import { useActivity } from "@/hooks/useBffQueries";
import { parseFilters, filtersToParams, type ActivityFilters } from "@/lib/activityFilters";
import { LedgerEntry } from "@/components/design/LedgerEntry";

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
      <header className="mb-8 pb-4 border-b border-quartz">
        <h1 className="text-3xl font-semibold tracking-tightest text-ink-primary">Activity</h1>
        <p className="text-sm text-ink-muted mt-1">Filterable feed across every source.</p>
      </header>

      <div className="bg-vellum-raised border border-quartz rounded-md p-4 mb-6">
        <p className="text-[10px] uppercase tracking-[0.1em] text-ink-muted font-medium mb-2">Filter by type</p>
        <div className="flex flex-wrap gap-1.5">
          {ALL_TYPES.map((t) => (
            <button
              key={t}
              onClick={() => toggleType(t)}
              className={`text-xs px-2 py-1 rounded-sm border transition-colors ${
                filters.types.includes(t)
                  ? "bg-verdigris-soft text-verdigris border-verdigris/30"
                  : "bg-vellum-inset border-quartz text-ink-secondary hover:bg-vellum-inset"
              }`}
            >
              {t}
            </button>
          ))}
          {filters.types.length > 0 && (
            <button
              onClick={() => updateFilters({ types: [] })}
              className="text-xs px-2 py-1 rounded-sm border border-quartz text-ink-muted hover:bg-vellum-inset"
            >
              Clear
            </button>
          )}
        </div>
      </div>

      {isLoading && <p className="text-ink-muted">Loading...</p>}

      {error && (
        <div className="bg-vellum-raised border border-quartz rounded-md p-6 text-sm text-ink-muted">
          <p className="font-medium text-ink-primary mb-1">Activity feed wiring is in progress.</p>
          <p>
            The full Event variant from the Rumi backend is not yet ported into the BFF
            shadow types — this lights up in a follow-up. Other parts of the Explorer
            (Overview, Lens pages) are already rendering live data.
          </p>
        </div>
      )}

      {data && data.events.length === 0 && !error && !isLoading && (
        <div className="bg-vellum-raised border border-quartz rounded-md p-6 text-sm text-ink-muted">
          <p className="font-medium text-ink-primary mb-1">Activity feed wiring is in progress.</p>
          <p>
            The full Event variant from the Rumi backend is not yet ported into the BFF
            shadow types — this lights up in a follow-up. Other parts of the Explorer
            (Overview, Lens pages) are already rendering live data.
          </p>
        </div>
      )}

      {data && data.events.length > 0 && (
        <>
          <p className="text-[11px] text-ink-muted font-mono mb-3 tabular-nums">
            {String(data.total_estimated)} total · showing {data.events.length}
          </p>

          <div className="bg-vellum-raised border border-quartz rounded-md overflow-hidden">
            {data.events.map((e, i) => (
              <LedgerEntry
                key={`${e.global_id}-${i}`}
                timestampNs={e.timestamp_ns}
                kind={e.kind}
                summary={e.payload_summary}
                amount={e.primary_amount?.formatted ?? null}
                id={e.global_id}
              />
            ))}
          </div>

          {data.next_cursor && (
            <button
              onClick={loadMore}
              className="mt-4 px-4 py-2 text-sm font-medium text-ink-secondary border border-quartz rounded-md hover:bg-vellum-inset"
            >
              Load older entries →
            </button>
          )}
        </>
      )}
    </div>
  );
}
