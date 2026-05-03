import { useParams } from "react-router-dom";
import { useVault } from "@/hooks/useBffQueries";
import { VaultStatus } from "@/bindings/explorer_bff/explorer_bff";
import { VaultGlyph } from "@/components/design/VaultGlyph";
import { LedgerEntry } from "@/components/design/LedgerEntry";

export function VaultDetail() {
  const { id } = useParams<{ id: string }>();
  const { data, isLoading, error } = useVault(id ?? "0");

  if (!id) return <p className="text-ink-muted">No id in URL.</p>;

  if (isLoading) return <p className="text-ink-muted">Loading vault...</p>;

  if (error) {
    return (
      <div className="bg-cinnabar/10 text-cinnabar border border-cinnabar/20 rounded-md p-4">
        Failed: {error instanceof Error ? error.message : String(error)}
      </div>
    );
  }

  if (!data) return null;

  const isEmptyVault =
    data.collateral_amount.raw_e8s === 0n &&
    data.history.length === 0;

  if (isEmptyVault && data.closed_synthesized) {
    return (
      <div>
        <h1 className="text-3xl font-semibold tracking-tightest text-ink-primary mb-1">
          Vault <span className="font-mono tabular-nums">#{id}</span>
        </h1>
        <div className="bg-vellum-raised border border-quartz rounded-md p-4 mt-4 text-sm text-ink-muted">
          <p className="font-medium text-ink-primary mb-1">Vault data not available.</p>
          <p>
            Vault detail requires get_vault_summary and get_vault_history from the backend,
            which currently return a full Event variant not yet decoded by the BFF.
            This lights up in a follow-up.
          </p>
        </div>
      </div>
    );
  }

  const status = data.status;
  const statusKey: "open" | "closed" | "liquidated" =
    status === VaultStatus.Open ? "open" :
    status === VaultStatus.Closed ? "closed" :
    "liquidated";

  const statusLabel =
    statusKey === "open" ? "Open" :
    statusKey === "closed" ? "Closed" :
    "Liquidated";

  const statusColor =
    statusKey === "open"
      ? "bg-verdigris/10 text-verdigris border-verdigris/20"
      : statusKey === "liquidated"
        ? "bg-cinnabar/10 text-cinnabar border-cinnabar/20"
        : "bg-vellum-inset text-ink-secondary border-quartz";

  // collateral_ratio is a f64 multiple (e.g. 1.62 = 162%), not bps
  const ratio = data.collateral_ratio ?? null;

  return (
    <div>
      <header className="mb-8 pb-4 border-b border-quartz flex items-end gap-4">
        <VaultGlyph
          ratio={ratio ?? undefined}
          status={statusKey}
          size={48}
          title={`Vault #${id} — CR ${ratio ? ratio.toFixed(2) + "x" : "—"}`}
        />
        <div>
          <h1 className="text-3xl font-semibold tracking-tightest text-ink-primary">
            Vault <span className="font-mono tabular-nums">#{id}</span>
          </h1>
          <p className="text-sm text-ink-muted mt-1 flex items-center gap-2">
            <span className={`text-xs px-2 py-0.5 rounded-full border ${statusColor}`}>
              {statusLabel}
            </span>
            {data.closed_synthesized && (
              <span
                className="text-xs px-2 py-0.5 rounded-full border border-sodium/40 bg-sodium/10 text-sodium"
                title="This vault is closed; the summary was synthesized from event history rather than fetched from live state."
              >
                synthesized
              </span>
            )}
          </p>
        </div>
      </header>

      <p className="text-sm font-mono text-ink-muted mb-6">Owner: {data.owner.toText()}</p>

      <div className="grid grid-cols-1 md:grid-cols-3 gap-3 mb-8">
        <div className="bg-vellum-raised border border-quartz rounded-md p-4">
          <p className="text-[10px] uppercase tracking-[0.1em] text-ink-muted font-medium">Collateral</p>
          <p className="text-2xl font-semibold tabular-nums font-mono text-ink-primary mt-1">{data.collateral_amount.formatted}</p>
        </div>
        <div className="bg-vellum-raised border border-quartz rounded-md p-4">
          <p className="text-[10px] uppercase tracking-[0.1em] text-ink-muted font-medium">Debt</p>
          <p className="text-2xl font-semibold tabular-nums font-mono text-ink-primary mt-1">{data.debt_icusd.formatted}</p>
        </div>
        <div className="bg-vellum-raised border border-quartz rounded-md p-4">
          <p className="text-[10px] uppercase tracking-[0.1em] text-ink-muted font-medium">Collateral ratio</p>
          <p className="text-2xl font-semibold tabular-nums font-mono text-ink-primary mt-1">
            {ratio !== null ? `${ratio.toFixed(2)}x` : "—"}
          </p>
        </div>
      </div>

      <h2 className="text-sm font-medium tracking-[0.08em] uppercase text-ink-muted mb-3">History</h2>
      <div className="bg-vellum-raised border border-quartz rounded-md overflow-hidden">
        {data.history.length === 0 ? (
          <p className="px-4 py-6 text-sm text-ink-muted">No history events recorded.</p>
        ) : (
          data.history.map((e, i) => (
            <LedgerEntry
              key={`${e.global_id}-${i}`}
              timestampNs={e.timestamp_ns}
              kind={e.kind}
              summary={e.payload_summary}
              amount={e.primary_amount?.formatted ?? null}
              id={e.global_id}
            />
          ))
        )}
      </div>
    </div>
  );
}
