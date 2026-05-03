import { useParams } from "react-router-dom";
import { useAddress } from "@/hooks/useBffQueries";
import { VaultStatus } from "@/bindings/explorer_bff/explorer_bff";
import { ApproximateBadge } from "@/components/common/ApproximateBadge";

export function AddressDetail() {
  const { principal } = useParams<{ principal: string }>();
  const { data, isLoading, error } = useAddress(principal ?? "");

  if (!principal) return <p className="text-muted-foreground">No principal in URL.</p>;

  if (isLoading) return <p className="text-muted-foreground">Loading address...</p>;

  if (error) {
    return (
      <div className="bg-destructive/10 text-destructive border border-destructive/20 rounded-lg p-4">
        <p className="font-medium">Failed to load address</p>
        <p className="text-sm mt-1">{error instanceof Error ? error.message : String(error)}</p>
      </div>
    );
  }

  if (!data) return null;

  const isPending =
    data.total_value_usd === 0 &&
    data.recent_events.length === 0 &&
    data.vaults_owned.length === 0;

  return (
    <div>
      <h1 className="text-2xl font-semibold mb-1">Address</h1>
      <p className="text-sm font-mono text-muted-foreground mb-2 break-all">{principal}</p>
      {data.approximate_sources && data.approximate_sources.length > 0 && (
        <div className="mb-4">
          <ApproximateBadge sources={data.approximate_sources} />
        </div>
      )}

      {isPending && (
        <div className="bg-secondary/50 border border-border rounded-lg p-4 mb-6 text-sm text-muted-foreground">
          <p className="font-medium text-foreground mb-1">Address detail not yet wired for this canister.</p>
          <p>
            Per-address holdings (vaults, SP deposits, balances) require a
            per-address index on rumi_analytics that is not yet exposed. This
            lights up in a follow-up once the analytics canister exposes that
            endpoint.
          </p>
        </div>
      )}

      <div className="grid grid-cols-1 md:grid-cols-3 gap-4 mb-8">
        <div className="bg-card border border-border rounded-lg p-4">
          <p className="text-xs uppercase text-muted-foreground tracking-wide">Total value</p>
          <p className="text-2xl font-semibold mt-1">${data.total_value_usd.toLocaleString()}</p>
        </div>
        <div className="bg-card border border-border rounded-lg p-4">
          <p className="text-xs uppercase text-muted-foreground tracking-wide">Vaults</p>
          <p className="text-2xl font-semibold mt-1">{data.vaults_owned.length}</p>
        </div>
        <div className="bg-card border border-border rounded-lg p-4">
          <p className="text-xs uppercase text-muted-foreground tracking-wide">SP deposits</p>
          <p className="text-2xl font-semibold mt-1">{data.sp_deposits.length}</p>
        </div>
      </div>

      {data.token_balances.length > 0 && (
        <div className="mb-8">
          <h2 className="text-lg font-semibold mb-3">Token balances</h2>
          <div className="bg-card border border-border rounded-lg overflow-x-auto">
            <table className="w-full text-sm">
              <thead className="bg-secondary/30">
                <tr className="text-left text-xs uppercase text-muted-foreground">
                  <th className="px-4 py-2 font-medium">Symbol</th>
                  <th className="px-4 py-2 font-medium">Balance</th>
                  <th className="px-4 py-2 font-medium">Ledger</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-border">
                {data.token_balances.map((b, i) => (
                  <tr key={i}>
                    <td className="px-4 py-2 font-medium">{b.symbol}</td>
                    <td className="px-4 py-2">{b.balance.formatted}</td>
                    <td className="px-4 py-2 font-mono text-xs text-muted-foreground">{b.ledger.toText()}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}

      {data.vaults_owned.length > 0 && (
        <div className="mb-8">
          <h2 className="text-lg font-semibold mb-3">Vaults</h2>
          <div className="bg-card border border-border rounded-lg overflow-x-auto">
            <table className="w-full text-sm">
              <thead className="bg-secondary/30">
                <tr className="text-left text-xs uppercase text-muted-foreground">
                  <th className="px-4 py-2 font-medium">ID</th>
                  <th className="px-4 py-2 font-medium">Status</th>
                  <th className="px-4 py-2 font-medium">Collateral</th>
                  <th className="px-4 py-2 font-medium">Debt</th>
                  <th className="px-4 py-2 font-medium">CR</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-border">
                {data.vaults_owned.map((v, i) => (
                  <tr key={i}>
                    <td className="px-4 py-2 font-mono text-xs">
                      <a href={`/e/vault/${v.vault_id}`} className="hover:underline">#{String(v.vault_id)}</a>
                    </td>
                    <td className="px-4 py-2">{vaultStatusToText(v.status)}</td>
                    <td className="px-4 py-2">{v.collateral_amount.formatted}</td>
                    <td className="px-4 py-2">{v.debt_icusd.formatted}</td>
                    <td className="px-4 py-2">
                      {v.collateral_ratio !== undefined ? v.collateral_ratio.toFixed(2) : "—"}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}

      {data.recent_events.length > 0 && (
        <div>
          <h2 className="text-lg font-semibold mb-3">Recent activity</h2>
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
                {data.recent_events.map((e, i) => (
                  <tr key={`${e.global_id}-${i}`}>
                    <td className="px-4 py-2 font-mono text-xs text-muted-foreground">{e.global_id}</td>
                    <td className="px-4 py-2 whitespace-nowrap">{e.kind}</td>
                    <td className="px-4 py-2">{e.payload_summary}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}
    </div>
  );
}

function vaultStatusToText(s: VaultStatus): string {
  if (s === VaultStatus.Open) return "Open";
  if (s === VaultStatus.Closed) return "Closed";
  return "Liquidated";
}
