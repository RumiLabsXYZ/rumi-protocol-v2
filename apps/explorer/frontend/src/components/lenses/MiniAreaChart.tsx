import { Area, AreaChart, ResponsiveContainer, Tooltip, XAxis, YAxis } from "recharts";

interface Point {
  t: number;
  v: number;
}

interface Props {
  points: Point[];
  label: string;
  height?: number;
  yAxisMode?: "data-fit" | "zero-anchored";
  format?: (v: number) => string;
}

export function MiniAreaChart({
  points,
  label,
  height = 240,
  yAxisMode = "zero-anchored",
  format,
}: Props) {
  if (points.length === 0) {
    return (
      <div className="bg-card border border-border rounded-lg p-4 mb-4">
        <p className="text-xs uppercase text-muted-foreground tracking-wide mb-3">{label}</p>
        <p className="text-sm text-muted-foreground">No data</p>
      </div>
    );
  }
  return (
    <div className="bg-card border border-border rounded-lg p-4 mb-4">
      <p className="text-xs uppercase text-muted-foreground tracking-wide mb-3">{label}</p>
      <div style={{ width: "100%", height }}>
        <ResponsiveContainer>
          <AreaChart data={points} margin={{ top: 8, right: 8, bottom: 8, left: 8 }}>
            <defs>
              <linearGradient id="areaFill" x1="0" y1="0" x2="0" y2="1">
                <stop offset="0%" stopColor="hsl(var(--primary))" stopOpacity={0.4} />
                <stop offset="100%" stopColor="hsl(var(--primary))" stopOpacity={0} />
              </linearGradient>
            </defs>
            <XAxis
              dataKey="t"
              tickFormatter={(t) =>
                new Date(t).toLocaleDateString("en-US", { month: "short", day: "numeric" })
              }
              tick={{ fontSize: 11, fill: "hsl(var(--muted-foreground))" }}
              stroke="hsl(var(--border))"
            />
            <YAxis
              domain={yAxisMode === "data-fit" ? (["auto", "auto"] as [string, string]) : ([0, "auto"] as [number, string])}
              tickFormatter={(v) => (format ? format(v) : String(v))}
              tick={{ fontSize: 11, fill: "hsl(var(--muted-foreground))" }}
              stroke="hsl(var(--border))"
              width={60}
            />
            <Tooltip
              contentStyle={{
                background: "hsl(var(--card))",
                border: "1px solid hsl(var(--border))",
                borderRadius: 6,
              }}
              labelFormatter={(t) => new Date(t as number).toLocaleString()}
              formatter={(v) => (format ? format(Number(v)) : v)}
            />
            <Area type="monotone" dataKey="v" stroke="hsl(var(--primary))" fill="url(#areaFill)" />
          </AreaChart>
        </ResponsiveContainer>
      </div>
    </div>
  );
}
