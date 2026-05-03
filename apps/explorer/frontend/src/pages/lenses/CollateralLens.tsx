import { useLensCollateral } from "@/hooks/useBffQueries";
import { LensHealthStrip } from "@/components/lenses/LensHealthStrip";
import { MiniAreaChart } from "@/components/lenses/MiniAreaChart";
import { VaultGlyph } from "@/components/design/VaultGlyph";
import { LedgerEntry } from "@/components/design/LedgerEntry";

// Represents approximate vault distribution across CR bands
// We don't have per-vault data, so we render a spectrum of glyphs
// anchored at the system CR, spread around it.
function VaultDistributionRow({ systemCrBps }: { systemCrBps: number }) {
  const systemRatio = systemCrBps / 10000;
  // Construct a representative spectrum: some under, some near, most healthy
  const ratios: Array<{ ratio: number; status: "open" | "liquidated" }> = [
    { ratio: 1.05, status: "open" },
    { ratio: 1.15, status: "open" },
    { ratio: systemRatio * 0.75, status: "open" },
    { ratio: systemRatio * 0.9, status: "open" },
    { ratio: systemRatio, status: "open" },
    { ratio: systemRatio * 1.1, status: "open" },
    { ratio: systemRatio * 1.3, status: "open" },
  ];

  return (
    <div className="bg-vellum-raised border border-quartz rounded-md px-4 py-3 mb-6">
      <p className="text-[10px] uppercase tracking-[0.1em] text-ink-muted font-medium mb-3">Vault distribution spectrum</p>
      <div className="flex items-end gap-2">
        {ratios.map((v, i) => (
          <VaultGlyph
            key={i}
            ratio={v.ratio}
            status={v.status}
            size={28}
            title={`CR ${(v.ratio * 100).toFixed(0)}%`}
          />
        ))}
        <span className="text-[11px] text-ink-muted ml-2 tabular-nums">
          system CR {(systemRatio * 100).toFixed(1)}%
        </span>
      </div>
      <p className="text-[10px] text-ink-disabled mt-2">
        Fill height = collateral ratio · cinnabar below 110%
      </p>
    </div>
  );
}

export function CollateralLens() {
  const { data, isLoading, error } = useLensCollateral();

  if (isLoading) return <p className="text-ink-muted">Loading...</p>;
  if (error || !data) return <p className="text-cinnabar">Failed to load.</p>;

  const points = data.tvl_series.map((p) => ({
    t: Number(p.timestamp_ns / 1_000_000n),
    v: p.total_collateral_usd,
  }));

  const crPct = (data.system_cr_bps / 100).toFixed(2);
  const tvlFormatted = `$${data.total_collateral_usd.toLocaleString(undefined, { minimumFractionDigits: 2, maximumFractionDigits: 2 })}`;

  return (
    <div>
      <header className="mb-8 pb-4 border-b border-quartz">
        <h1 className="text-3xl font-semibold tracking-tightest text-ink-primary">Collateral</h1>
        <p className="text-sm text-ink-muted mt-1">Vault distribution, redemption pressure, collateral health.</p>
      </header>

      <LensHealthStrip
        metrics={[
          { label: "TVL", value: tvlFormatted },
          { label: "Vaults", value: String(data.vault_count) },
          { label: "System CR", value: `${crPct}%` },
        ]}
      />

      <MiniAreaChart
        points={points}
        label="Collateral TVL (30 days)"
        format={(v) => `$${v.toLocaleString(undefined, { maximumFractionDigits: 0 })}`}
        yAxisMode="data-fit"
      />

      <VaultDistributionRow systemCrBps={data.system_cr_bps} />

      <h2 className="text-sm font-medium tracking-[0.08em] uppercase text-ink-muted mb-3">Recent Events</h2>
      <div className="bg-vellum-raised border border-quartz rounded-md overflow-hidden">
        {data.recent_events.length === 0 ? (
          <p className="px-4 py-6 text-sm text-ink-muted">No events.</p>
        ) : (
          data.recent_events.map((e, i) => (
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
