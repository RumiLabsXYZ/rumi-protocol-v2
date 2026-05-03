import { useSearchParams } from "react-router-dom";
import { useOverview } from "@/hooks/useBffQueries";
import { LedgerEntry } from "@/components/design/LedgerEntry";
import { VaultGlyph } from "@/components/design/VaultGlyph";

// ── Inline peg meridian bar ──────────────────────────────────────────────────
// A small SVG that shows how far the current peg is from $1.00.
// Range: ±0.5% from $1.00 (i.e., 0.995 – 1.005).

function PegBar({ value }: { value: number }) {
  const MIN = 0.995;
  const MAX = 1.005;
  const clamped = Math.max(MIN, Math.min(MAX, value));
  const pct = ((clamped - MIN) / (MAX - MIN)) * 100;
  const centerPct = 50; // $1.00 is center
  const abovePeg = value >= 1.0;
  const dotColor = abovePeg ? "hsl(var(--verdigris))" : "hsl(var(--cinnabar))";

  const W = 120;
  const H = 18;
  const dotX = (pct / 100) * (W - 8) + 4;

  return (
    <svg
      width={W}
      height={H}
      viewBox={`0 0 ${W} ${H}`}
      aria-label={`Peg position: $${value.toFixed(4)}`}
    >
      {/* baseline */}
      <line x1="4" y1={H / 2} x2={W - 4} y2={H / 2} stroke="hsl(var(--quartz-rule-emphasis))" strokeWidth="1" />
      {/* center tick (peg) */}
      <line x1={(centerPct / 100) * (W - 8) + 4} y1={H / 2 - 4} x2={(centerPct / 100) * (W - 8) + 4} y2={H / 2 + 4} stroke="hsl(var(--peg))" strokeWidth="1" />
      {/* edge ticks */}
      <line x1="4" y1={H / 2 - 3} x2="4" y2={H / 2 + 3} stroke="hsl(var(--quartz-rule))" strokeWidth="1" />
      <line x1={W - 4} y1={H / 2 - 3} x2={W - 4} y2={H / 2 + 3} stroke="hsl(var(--quartz-rule))" strokeWidth="1" />
      {/* dot */}
      <circle cx={dotX} cy={H / 2} r="3.5" fill={dotColor} />
    </svg>
  );
}

// ── Timestamp formatter ──────────────────────────────────────────────────────
function formatDateTime(ns: bigint): string {
  const ms = Number(ns / 1_000_000n);
  const d = new Date(ms);
  const day = d.getDate();
  const month = d.toLocaleString("en-US", { month: "short" });
  const hh = String(d.getHours()).padStart(2, "0");
  const mm = String(d.getMinutes()).padStart(2, "0");
  return `${day} ${month} ${hh}:${mm}`;
}

// ── Metric cards ─────────────────────────────────────────────────────────────

function PegMetric({ value }: { value: number }) {
  const dir =
    value > 1.0001 ? "above peg" : value < 0.9999 ? "below peg" : "at peg";
  return (
    <div className="bg-vellum-raised border border-quartz rounded-md px-4 py-3 min-w-[160px]">
      <p className="text-[10px] uppercase tracking-[0.1em] text-ink-muted font-medium mb-1">Peg</p>
      <p className="text-2xl font-semibold tabular-nums font-mono text-ink-primary">
        ${value.toFixed(4)}
      </p>
      <div className="mt-2">
        <PegBar value={value} />
      </div>
      <p className="text-[11px] text-ink-muted mt-1 tabular-nums">{dir}</p>
    </div>
  );
}

function SupplyMetric({ value }: { value: string }) {
  return (
    <div className="bg-vellum-raised border border-quartz rounded-md px-4 py-3 min-w-[160px]">
      <p className="text-[10px] uppercase tracking-[0.1em] text-ink-muted font-medium mb-1">icUSD Supply</p>
      <p className="text-2xl font-semibold tabular-nums font-mono text-ink-primary">{value}</p>
      <p className="text-[11px] text-ink-muted mt-1">circulating</p>
    </div>
  );
}

function TvlMetric({ value }: { value: number }) {
  const formatted = `$${value.toLocaleString(undefined, {
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  })}`;
  return (
    <div className="bg-vellum-raised border border-quartz rounded-md px-4 py-3 min-w-[160px]">
      <p className="text-[10px] uppercase tracking-[0.1em] text-ink-muted font-medium mb-1">TVL</p>
      <p className="text-2xl font-semibold tabular-nums font-mono text-ink-primary">{formatted}</p>
      <p className="text-[11px] text-ink-muted mt-1">total collateral</p>
    </div>
  );
}

function VaultsMetric({ count, systemCrBps }: { count: bigint; systemCrBps?: number }) {
  const ratio = systemCrBps !== undefined ? systemCrBps / 10000 : null;
  return (
    <div className="bg-vellum-raised border border-quartz rounded-md px-4 py-3 min-w-[120px]">
      <p className="text-[10px] uppercase tracking-[0.1em] text-ink-muted font-medium mb-1">Vaults</p>
      <div className="flex items-end gap-2 mt-1">
        <p className="text-2xl font-semibold tabular-nums font-mono text-ink-primary">{String(count)}</p>
        <VaultGlyph ratio={ratio ?? undefined} status="open" size={24} title={`System CR: ${ratio ? (ratio * 100).toFixed(0) + "%" : "—"}`} />
      </div>
      <p className="text-[11px] text-ink-muted mt-1">open vaults</p>
    </div>
  );
}

// ── Main page ─────────────────────────────────────────────────────────────────

export function Overview() {
  const { data, isLoading, error } = useOverview();
  const [params] = useSearchParams();
  const unresolved = params.get("q");

  if (isLoading) {
    return <p className="text-ink-muted">Loading overview...</p>;
  }

  if (error) {
    return (
      <div className="bg-cinnabar/10 text-cinnabar border border-cinnabar/20 rounded-md p-4">
        <p className="font-medium">Failed to load overview</p>
        <p className="text-sm mt-1">{error instanceof Error ? error.message : String(error)}</p>
      </div>
    );
  }

  if (!data) return null;

  return (
    <div>
      {unresolved && (
        <div className="bg-sodium-soft text-ink-primary border border-sodium/30 rounded-md p-3 mb-6 text-sm">
          Couldn't resolve <span className="font-mono">{unresolved}</span> as a principal, vault id, or event id.
        </div>
      )}

      <header className="mb-8 pb-4 border-b border-quartz">
        <h1 className="text-3xl font-semibold tracking-tightest text-ink-primary">Overview</h1>
        <p className="text-sm text-ink-muted mt-1 tabular-nums">
          Protocol-wide health · {formatDateTime(data.generated_at_ns)}
        </p>
      </header>

      <div className="flex flex-wrap gap-3 mb-8">
        <PegMetric value={data.icusd_peg_usd} />
        <SupplyMetric value={data.icusd_supply.formatted} />
        <TvlMetric value={data.tvl_usd} />
        <VaultsMetric count={data.vault_count_open} />
      </div>

      <section>
        <h2 className="text-sm font-medium tracking-[0.08em] uppercase text-ink-muted mb-3">Recent activity</h2>
        <div className="bg-vellum-raised border border-quartz rounded-md overflow-hidden">
          {data.recent_activity.length === 0 ? (
            <p className="px-4 py-6 text-sm text-ink-muted">No recent activity.</p>
          ) : (
            data.recent_activity.map((e, i) => (
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
      </section>
    </div>
  );
}
