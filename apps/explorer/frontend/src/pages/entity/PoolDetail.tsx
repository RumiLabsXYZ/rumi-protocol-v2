import { useParams } from "react-router-dom";
import { usePool } from "@/hooks/useBffQueries";

export function PoolDetail() {
  const { id } = useParams<{ id: string }>();
  const { data, isLoading, error } = usePool(id ?? "");

  if (!id) return <p className="text-ink-muted">No id in URL.</p>;

  if (isLoading) return <p className="text-ink-muted">Loading pool...</p>;

  if (error) {
    return (
      <div className="bg-cinnabar/10 text-cinnabar border border-cinnabar/20 rounded-md p-4">
        Failed to load pool: {error instanceof Error ? error.message : String(error)}
      </div>
    );
  }

  if (!data) return null;

  if (data.reserves.length === 0) {
    return (
      <div>
        <header className="mb-8 pb-4 border-b border-quartz">
          <h1 className="text-3xl font-semibold tracking-tightest text-ink-primary">{data.pool_id}</h1>
          <p className="text-sm text-ink-muted mt-1">Liquidity pool</p>
        </header>
        <div className="bg-vellum-raised border border-quartz rounded-md p-4 text-sm text-ink-muted">
          <p className="font-medium text-ink-primary mb-1">Pool data not available.</p>
          <p>
            Pool detail requires get_pool_state on rumi_analytics, which is not yet
            exposed on the mainnet canister. This lights up in a follow-up.
          </p>
        </div>
      </div>
    );
  }

  return (
    <div>
      <header className="mb-8 pb-4 border-b border-quartz">
        <h1 className="text-3xl font-semibold tracking-tightest text-ink-primary">{data.pool_label}</h1>
        <p className="text-sm font-mono text-ink-muted mt-1">{data.pool_id}</p>
        <p className="text-xs text-ink-disabled mt-0.5">{data.pool_kind}</p>
      </header>

      <div className="grid grid-cols-1 md:grid-cols-2 gap-3 mb-8">
        <div className="bg-vellum-raised border border-quartz rounded-md p-4">
          <p className="text-[10px] uppercase tracking-[0.1em] text-ink-muted font-medium">LP supply</p>
          <p className="text-2xl font-semibold tabular-nums font-mono text-ink-primary mt-1">
            {data.lp_total_supply.formatted}
          </p>
        </div>
        {data.virtual_price !== undefined && (
          <div className="bg-vellum-raised border border-quartz rounded-md p-4">
            <p className="text-[10px] uppercase tracking-[0.1em] text-ink-muted font-medium">Virtual price</p>
            <p className="text-2xl font-semibold tabular-nums font-mono text-ink-primary mt-1">
              {data.virtual_price.toFixed(6)}
            </p>
            <p className="text-[11px] text-ink-muted mt-0.5">
              {data.virtual_price >= 1.0 ? "above $1.00" : "below $1.00"}
            </p>
          </div>
        )}
      </div>

      <h2 className="text-sm font-medium tracking-[0.08em] uppercase text-ink-muted mb-3">Reserves</h2>
      <div className="bg-vellum-raised border border-quartz rounded-md overflow-x-auto">
        <table className="w-full text-sm">
          <thead className="border-b border-quartz">
            <tr className="text-left text-[10px] uppercase tracking-[0.1em] text-ink-muted">
              <th className="px-4 py-2 font-medium">Asset</th>
              <th className="px-4 py-2 font-medium">Balance</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-quartz">
            {data.reserves.map(([principal, balance], i) => (
              <tr key={i}>
                <td className="px-4 py-2 font-mono text-[11px] text-ink-muted">{principal.toText()}</td>
                <td className="px-4 py-2 font-mono tabular-nums text-ink-primary">{balance.formatted}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}
