import { Area, AreaChart, ResponsiveContainer, Tooltip, XAxis, YAxis, CartesianGrid } from "recharts";
import { PegMeridian } from "@/components/design/PegMeridian";

interface Point { t: number; v: number }

interface Props {
  points: Point[];
  label: string;
  height?: number;
  yAxisMode?: "data-fit" | "zero-anchored";
  format?: (v: number) => string;
  /** Optional peg-line value. When set, renders the peg meridian on the chart. */
  peg?: number;
}

export function MiniAreaChart({ points, label, height = 220, yAxisMode = "zero-anchored", format, peg }: Props) {
  if (points.length === 0) {
    return (
      <div className="bg-vellum-raised border border-quartz rounded-md p-5 mb-4">
        <p className="text-[11px] uppercase tracking-[0.08em] text-ink-muted mb-3 font-medium">{label}</p>
        <p className="text-sm text-ink-muted">No data.</p>
      </div>
    );
  }

  return (
    <div className="bg-vellum-raised border border-quartz rounded-md p-5 mb-4">
      <p className="text-[11px] uppercase tracking-[0.08em] text-ink-muted mb-3 font-medium">{label}</p>
      <div style={{ width: "100%", height }}>
        <ResponsiveContainer>
          <AreaChart data={points} margin={{ top: 8, right: 32, bottom: 8, left: 8 }}>
            <CartesianGrid stroke="hsl(var(--quartz-rule-soft))" strokeDasharray="0" vertical={false} />
            <XAxis
              dataKey="t"
              tickFormatter={(t) => new Date(t).toLocaleDateString("en-US", { month: "short", day: "numeric" })}
              tick={{ fontSize: 10, fill: "hsl(var(--ink-muted))", fontFamily: "JetBrains Mono, ui-monospace, monospace" }}
              axisLine={{ stroke: "hsl(var(--quartz-rule))" }}
              tickLine={false}
            />
            <YAxis
              domain={yAxisMode === "data-fit" ? ["auto", "auto"] : [0, "auto"]}
              tickFormatter={(v) => (format ? format(v) : String(v))}
              tick={{ fontSize: 10, fill: "hsl(var(--ink-muted))", fontFamily: "JetBrains Mono, ui-monospace, monospace" }}
              axisLine={false}
              tickLine={false}
              width={64}
            />
            <Tooltip
              contentStyle={{
                background: "hsl(var(--vellum-raised))",
                border: "1px solid hsl(var(--quartz-rule-emphasis))",
                borderRadius: 4,
                fontSize: 11,
                fontFamily: "JetBrains Mono, ui-monospace, monospace",
              }}
              labelStyle={{ color: "hsl(var(--ink-secondary))" }}
              itemStyle={{ color: "hsl(var(--ink-primary))" }}
              labelFormatter={(t) => new Date(t).toLocaleDateString("en-US", { weekday: "short", month: "short", day: "numeric" })}
              formatter={(v) => (format ? format(Number(v)) : v)}
            />
            {peg !== undefined && <PegMeridian y={peg} />}
            <Area
              type="monotone"
              dataKey="v"
              stroke="hsl(var(--verdigris))"
              strokeWidth={1.5}
              fill="none"
              dot={false}
              activeDot={{ r: 3, fill: "hsl(var(--verdigris))", stroke: "hsl(var(--vellum))", strokeWidth: 2 }}
            />
          </AreaChart>
        </ResponsiveContainer>
      </div>
    </div>
  );
}
