interface Metric {
  label: string;
  value: string;
  sub?: string;
}

export function LensHealthStrip({ metrics }: { metrics: Metric[] }) {
  return (
    <div className="grid grid-cols-1 sm:grid-cols-2 md:grid-cols-3 lg:grid-cols-4 gap-4 mb-6">
      {metrics.map((m) => (
        <div key={m.label} className="bg-card border border-border rounded-lg p-4">
          <p className="text-xs uppercase text-muted-foreground tracking-wide">{m.label}</p>
          <p className="text-2xl font-semibold mt-1">{m.value}</p>
          {m.sub && <p className="text-xs text-muted-foreground mt-1">{m.sub}</p>}
        </div>
      ))}
    </div>
  );
}
