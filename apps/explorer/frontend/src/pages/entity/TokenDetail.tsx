import { useParams } from "react-router-dom";
import { useToken } from "@/hooks/useBffQueries";

export function TokenDetail() {
  const { ledger } = useParams<{ ledger: string }>();
  const { data, isLoading, error } = useToken(ledger ?? "");

  if (!ledger) return <p className="text-muted-foreground">No ledger in URL.</p>;

  if (isLoading) return <p className="text-muted-foreground">Loading token...</p>;

  if (error) {
    return (
      <div className="bg-destructive/10 text-destructive border border-destructive/20 rounded-lg p-4">
        Failed to load token: {error instanceof Error ? error.message : String(error)}
      </div>
    );
  }

  if (!data) return null;

  return (
    <div>
      <h1 className="text-2xl font-semibold mb-1">{data.symbol}</h1>
      <p className="text-sm font-mono text-muted-foreground mb-6 break-all">{ledger}</p>

      <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
        <div className="bg-card border border-border rounded-lg p-4">
          <p className="text-xs uppercase text-muted-foreground tracking-wide">Total supply</p>
          <p className="text-2xl font-semibold mt-1">{data.total_supply.formatted}</p>
        </div>
        <div className="bg-card border border-border rounded-lg p-4">
          <p className="text-xs uppercase text-muted-foreground tracking-wide">Decimals</p>
          <p className="text-2xl font-semibold mt-1">{data.decimals}</p>
        </div>
        <div className="bg-card border border-border rounded-lg p-4">
          <p className="text-xs uppercase text-muted-foreground tracking-wide">Fee</p>
          <p className="text-2xl font-semibold mt-1">{data.fee.formatted}</p>
        </div>
      </div>
    </div>
  );
}
