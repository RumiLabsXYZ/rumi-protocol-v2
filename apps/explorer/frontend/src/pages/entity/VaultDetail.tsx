import { useParams } from "react-router-dom";
import { useVault } from "@/hooks/useBffQueries";
import { VaultStatus } from "@/bindings/explorer_bff/explorer_bff";

export function VaultDetail() {
  const { id } = useParams<{ id: string }>();
  const { data, isLoading, error } = useVault(id ?? "0");

  if (!id) return <p className="text-muted-foreground">No id in URL.</p>;

  if (isLoading) return <p className="text-muted-foreground">Loading vault...</p>;

  if (error) {
    return (
      <div className="bg-destructive/10 text-destructive border border-destructive/20 rounded-lg p-4">
        Failed: {error instanceof Error ? error.message : String(error)}
      </div>
    );
  }

  if (!data) return null;

  const status = data.status;
  const statusLabel =
    status === VaultStatus.Open ? "Open" :
    status === VaultStatus.Closed ? "Closed" :
    "Liquidated";
  const statusColor =
    status === VaultStatus.Open
      ? "bg-success/10 text-success border-success/20"
      : status === VaultStatus.Liquidated
        ? "bg-destructive/10 text-destructive border-destructive/20"
        : "bg-secondary text-secondary-foreground border-border";

  return (
    <div>
      <div className="flex flex-wrap items-baseline gap-3 mb-1">
        <h1 className="text-2xl font-semibold">Vault #{id}</h1>
        <span className={`text-xs px-2 py-0.5 rounded-full border ${statusColor}`}>{statusLabel}</span>
        {data.closed_synthesized && (
          <span
            className="text-xs px-2 py-0.5 rounded-full border border-warning/40 bg-warning/10 text-warning"
            title="This vault is closed; the summary was synthesized from event history rather than fetched from live state."
          >
            synthesized
          </span>
        )}
      </div>
      <p className="text-sm font-mono text-muted-foreground mb-6">Owner: {data.owner.toText()}</p>

      <div className="grid grid-cols-1 md:grid-cols-3 gap-4 mb-8">
        <div className="bg-card border border-border rounded-lg p-4">
          <p className="text-xs uppercase text-muted-foreground tracking-wide">Collateral</p>
          <p className="text-2xl font-semibold mt-1">{data.collateral_amount.formatted}</p>
        </div>
        <div className="bg-card border border-border rounded-lg p-4">
          <p className="text-xs uppercase text-muted-foreground tracking-wide">Debt</p>
          <p className="text-2xl font-semibold mt-1">{data.debt_icusd.formatted}</p>
        </div>
        <div className="bg-card border border-border rounded-lg p-4">
          <p className="text-xs uppercase text-muted-foreground tracking-wide">Collateral ratio</p>
          <p className="text-2xl font-semibold mt-1">
            {data.collateral_ratio !== undefined ? data.collateral_ratio.toFixed(2) : "—"}
          </p>
        </div>
      </div>

      <h2 className="text-lg font-semibold mb-3">History</h2>
      {data.history.length === 0 ? (
        <div className="bg-card border border-border rounded-lg p-4 text-sm text-muted-foreground">
          No history events recorded.
        </div>
      ) : (
        <div className="bg-card border border-border rounded-lg overflow-x-auto">
          <table className="w-full text-sm">
            <thead className="bg-secondary/30">
              <tr className="text-left text-xs uppercase text-muted-foreground">
                <th className="px-4 py-2 font-medium">ID</th>
                <th className="px-4 py-2 font-medium">Kind</th>
                <th className="px-4 py-2 font-medium">Description</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-border">
              {data.history.map((e, i) => (
                <tr key={`${e.global_id}-${i}`}>
                  <td className="px-4 py-2 font-mono text-xs text-muted-foreground">{e.global_id}</td>
                  <td className="px-4 py-2 whitespace-nowrap">{e.kind}</td>
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
