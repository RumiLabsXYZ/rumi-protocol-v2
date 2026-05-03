import { useParams } from "react-router-dom";
import { useToken } from "@/hooks/useBffQueries";

export function TokenDetail() {
  const { ledger } = useParams<{ ledger: string }>();
  const { data, isLoading, error } = useToken(ledger ?? "");

  if (!ledger) return <p className="text-ink-muted">No ledger in URL.</p>;

  if (isLoading) return <p className="text-ink-muted">Loading token...</p>;

  if (error) {
    return (
      <div className="bg-cinnabar/10 text-cinnabar border border-cinnabar/20 rounded-md p-4">
        Failed to load token: {error instanceof Error ? error.message : String(error)}
      </div>
    );
  }

  if (!data) return null;

  if (data.symbol === "?") {
    return (
      <div>
        <header className="mb-8 pb-4 border-b border-quartz">
          <h1 className="text-3xl font-semibold tracking-tightest text-ink-primary">Token</h1>
          <p className="text-sm font-mono text-ink-muted mt-1 break-all">{ledger}</p>
        </header>
        <div className="bg-vellum-raised border border-quartz rounded-md p-4 text-sm text-ink-muted">
          <p className="font-medium text-ink-primary mb-1">Token data not available.</p>
          <p>
            Token detail requires get_token_metadata on rumi_analytics, which is not yet
            exposed on the mainnet canister. This lights up in a follow-up.
          </p>
        </div>
      </div>
    );
  }

  return (
    <div>
      <header className="mb-8 pb-4 border-b border-quartz">
        <h1 className="text-3xl font-semibold tracking-tightest text-ink-primary">{data.symbol}</h1>
        <p className="text-sm font-mono text-ink-muted mt-1 break-all">{ledger}</p>
      </header>

      <div className="grid grid-cols-1 md:grid-cols-3 gap-3">
        <div className="bg-vellum-raised border border-quartz rounded-md p-4">
          <p className="text-[10px] uppercase tracking-[0.1em] text-ink-muted font-medium">Total supply</p>
          <p className="text-2xl font-semibold tabular-nums font-mono text-ink-primary mt-1">
            {data.total_supply.formatted}
          </p>
        </div>
        <div className="bg-vellum-raised border border-quartz rounded-md p-4">
          <p className="text-[10px] uppercase tracking-[0.1em] text-ink-muted font-medium">Decimals</p>
          <p className="text-2xl font-semibold tabular-nums font-mono text-ink-primary mt-1">
            {data.decimals}
          </p>
        </div>
        <div className="bg-vellum-raised border border-quartz rounded-md p-4">
          <p className="text-[10px] uppercase tracking-[0.1em] text-ink-muted font-medium">Transfer fee</p>
          <p className="text-2xl font-semibold tabular-nums font-mono text-ink-primary mt-1">
            {data.fee.formatted}
          </p>
        </div>
      </div>
    </div>
  );
}
