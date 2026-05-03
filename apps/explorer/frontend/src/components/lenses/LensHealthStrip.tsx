interface Metric { label: string; value: string; sub?: string }

export function LensHealthStrip({ metrics }: { metrics: Metric[] }) {
  return (
    <div className="grid grid-cols-1 sm:grid-cols-2 md:grid-cols-3 lg:grid-cols-4 gap-3 mb-8">
      {metrics.map((m) => (
        <div key={m.label} className="bg-vellum-raised border border-quartz rounded-md px-4 py-3">
          <p className="text-[10px] uppercase tracking-[0.1em] text-ink-muted font-medium">{m.label}</p>
          <p className="text-2xl font-semibold text-ink-primary mt-1 tabular-nums">{m.value}</p>
          {m.sub && <p className="text-[11px] text-ink-muted mt-0.5 tabular-nums">{m.sub}</p>}
        </div>
      ))}
    </div>
  );
}
