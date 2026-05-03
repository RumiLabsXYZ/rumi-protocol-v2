import { useParams } from "react-router-dom";
import { useAddress } from "@/hooks/useBffQueries";
import { VaultStatus } from "@/bindings/explorer_bff/explorer_bff";
import { ApproximateBadge } from "@/components/common/ApproximateBadge";
import { LedgerEntry } from "@/components/design/LedgerEntry";

export function AddressDetail() {
  const { principal } = useParams<{ principal: string }>();
  const { data, isLoading, error } = useAddress(principal ?? "");

  if (!principal) return <p className="text-ink-muted">No principal in URL.</p>;

  if (isLoading) return <p className="text-ink-muted">Loading address...</p>;

  if (error) {
    return (
      <div className="bg-cinnabar/10 text-cinnabar border border-cinnabar/20 rounded-md p-4">
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
      <header className="mb-8 pb-4 border-b border-quartz">
        <h1 className="text-3xl font-semibold tracking-tightest text-ink-primary">Address</h1>
        <p className="text-sm font-mono text-ink-muted mt-1 break-all">{principal}</p>
        {data.approximate_sources && data.approximate_sources.length > 0 && (
          <div className="mt-2">
            <ApproximateBadge sources={data.approximate_sources} />
          </div>
        )}
      </header>

      {isPending && (
        <div className="bg-vellum-raised border border-quartz rounded-md p-4 mb-6 text-sm text-ink-muted">
          <p className="font-medium text-ink-primary mb-1">Address detail not yet wired for this canister.</p>
          <p>
            Per-address holdings (vaults, SP deposits, balances) require a
            per-address index on rumi_analytics that is not yet exposed. This
            lights up in a follow-up once the analytics canister exposes that
            endpoint.
          </p>
        </div>
      )}

      <div className="grid grid-cols-1 md:grid-cols-3 gap-3 mb-8">
        <div className="bg-vellum-raised border border-quartz rounded-md p-4">
          <p className="text-[10px] uppercase tracking-[0.1em] text-ink-muted font-medium">Total value</p>
          <p className="text-2xl font-semibold tabular-nums font-mono text-ink-primary mt-1">
            ${data.total_value_usd.toLocaleString()}
          </p>
        </div>
        <div className="bg-vellum-raised border border-quartz rounded-md p-4">
          <p className="text-[10px] uppercase tracking-[0.1em] text-ink-muted font-medium">Vaults</p>
          <p className="text-2xl font-semibold tabular-nums font-mono text-ink-primary mt-1">
            {data.vaults_owned.length}
          </p>
        </div>
        <div className="bg-vellum-raised border border-quartz rounded-md p-4">
          <p className="text-[10px] uppercase tracking-[0.1em] text-ink-muted font-medium">SP deposits</p>
          <p className="text-2xl font-semibold tabular-nums font-mono text-ink-primary mt-1">
            {data.sp_deposits.length}
          </p>
        </div>
      </div>

      {data.token_balances.length > 0 && (
        <div className="mb-8">
          <h2 className="text-sm font-medium tracking-[0.08em] uppercase text-ink-muted mb-3">Token balances</h2>
          <div className="bg-vellum-raised border border-quartz rounded-md overflow-x-auto">
            <table className="w-full text-sm">
              <thead className="border-b border-quartz">
                <tr className="text-left text-[10px] uppercase tracking-[0.1em] text-ink-muted">
                  <th className="px-4 py-2 font-medium">Symbol</th>
                  <th className="px-4 py-2 font-medium">Balance</th>
                  <th className="px-4 py-2 font-medium">Ledger</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-quartz">
                {data.token_balances.map((b, i) => (
                  <tr key={i}>
                    <td className="px-4 py-2 font-medium text-ink-primary">{b.symbol}</td>
                    <td className="px-4 py-2 font-mono tabular-nums text-ink-primary">{b.balance.formatted}</td>
                    <td className="px-4 py-2 font-mono text-[11px] text-ink-muted">{b.ledger.toText()}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}

      {data.vaults_owned.length > 0 && (
        <div className="mb-8">
          <h2 className="text-sm font-medium tracking-[0.08em] uppercase text-ink-muted mb-3">Vaults</h2>
          <div className="bg-vellum-raised border border-quartz rounded-md overflow-x-auto">
            <table className="w-full text-sm">
              <thead className="border-b border-quartz">
                <tr className="text-left text-[10px] uppercase tracking-[0.1em] text-ink-muted">
                  <th className="px-4 py-2 font-medium">ID</th>
                  <th className="px-4 py-2 font-medium">Status</th>
                  <th className="px-4 py-2 font-medium">Collateral</th>
                  <th className="px-4 py-2 font-medium">Debt</th>
                  <th className="px-4 py-2 font-medium">CR</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-quartz">
                {data.vaults_owned.map((v, i) => (
                  <tr key={i}>
                    <td className="px-4 py-2 font-mono text-[11px] text-ink-secondary">
                      <a href={`/e/vault/${v.vault_id}`} className="hover:text-verdigris">#{String(v.vault_id)}</a>
                    </td>
                    <td className="px-4 py-2 text-ink-secondary">{vaultStatusToText(v.status)}</td>
                    <td className="px-4 py-2 font-mono tabular-nums text-ink-primary">{v.collateral_amount.formatted}</td>
                    <td className="px-4 py-2 font-mono tabular-nums text-ink-primary">{v.debt_icusd.formatted}</td>
                    <td className="px-4 py-2 font-mono tabular-nums text-ink-muted">
                      {v.collateral_ratio !== undefined ? v.collateral_ratio.toFixed(2) + "x" : "—"}
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
          <h2 className="text-sm font-medium tracking-[0.08em] uppercase text-ink-muted mb-3">Recent activity</h2>
          <div className="bg-vellum-raised border border-quartz rounded-md overflow-hidden">
            {data.recent_events.map((e, i) => (
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
