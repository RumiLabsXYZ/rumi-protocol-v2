export function Overview() {
  return (
    <div>
      <h1 className="text-2xl font-semibold mb-2">Overview</h1>
      <p className="text-muted-foreground mb-6">Protocol-wide health + recent activity.</p>
      <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4">
        {["TVL", "icUSD supply", "Peg", "Open vaults"].map((label) => (
          <div key={label} className="bg-card border border-border rounded-lg p-4">
            <p className="text-xs uppercase text-muted-foreground tracking-wide">{label}</p>
            <p className="text-2xl font-semibold mt-1">—</p>
          </div>
        ))}
      </div>
      <p className="mt-8 text-sm text-muted-foreground">Real data wires up in Task 13.</p>
    </div>
  );
}
