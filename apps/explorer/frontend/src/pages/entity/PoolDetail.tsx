import { useParams } from "react-router-dom";
import { usePool } from "@/hooks/useBffQueries";

export function PoolDetail() {
  const { id } = useParams<{ id: string }>();
  const { data, isLoading, error } = usePool(id ?? "");

  if (!id) return <p className="text-muted-foreground">No id in URL.</p>;

  if (isLoading) return <p className="text-muted-foreground">Loading pool...</p>;

  if (error) {
    return (
      <div className="bg-destructive/10 text-destructive border border-destructive/20 rounded-lg p-4">
        Failed to load pool: {error instanceof Error ? error.message : String(error)}
      </div>
    );
  }

  if (!data) return null;

  return (
    <div>
      <h1 className="text-2xl font-semibold mb-1">{data.pool_label}</h1>
      <p className="text-sm font-mono text-muted-foreground mb-1">{data.pool_id}</p>
      <p className="text-xs text-muted-foreground mb-6">Kind: {data.pool_kind}</p>

      <div className="grid grid-cols-1 md:grid-cols-2 gap-4 mb-8">
        <div className="bg-card border border-border rounded-lg p-4">
          <p className="text-xs uppercase text-muted-foreground tracking-wide">LP supply</p>
          <p className="text-2xl font-semibold mt-1">{data.lp_total_supply.formatted}</p>
        </div>
        {data.virtual_price !== undefined && (
          <div className="bg-card border border-border rounded-lg p-4">
            <p className="text-xs uppercase text-muted-foreground tracking-wide">Virtual price</p>
            <p className="text-2xl font-semibold mt-1">{data.virtual_price.toFixed(4)}</p>
          </div>
        )}
      </div>

      <h2 className="text-lg font-semibold mb-3">Reserves</h2>
      <div className="bg-card border border-border rounded-lg overflow-x-auto">
        <table className="w-full text-sm">
          <thead className="bg-secondary/30">
            <tr className="text-left text-xs uppercase text-muted-foreground">
              <th className="px-4 py-2 font-medium">Asset</th>
              <th className="px-4 py-2 font-medium">Balance</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-border">
            {data.reserves.map(([principal, balance], i) => (
              <tr key={i}>
                <td className="px-4 py-2 font-mono text-xs text-muted-foreground">{principal.toText()}</td>
                <td className="px-4 py-2">{balance.formatted}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}
