import { useHealth } from "@/hooks/useBffQueries";

export function HealthPill() {
  const { data, isLoading } = useHealth();

  if (isLoading || !data) {
    return (
      <span className="inline-flex items-center gap-1.5 bg-muted/40 text-muted-foreground rounded-full px-2.5 py-0.5 text-xs font-medium border border-border">
        <span className="w-1.5 h-1.5 bg-muted-foreground rounded-full"></span>
        Loading
      </span>
    );
  }

  // Candid variant tag: `level` is `{ 'Green': null } | { 'Yellow': null } | { 'Red': null }`
  const level = data.level as unknown as { Green?: null; Yellow?: null; Red?: null };
  const levelKey: "green" | "yellow" | "red" =
    level.Green !== undefined ? "green" : level.Yellow !== undefined ? "yellow" : "red";

  const styles = {
    green: {
      bg: "bg-success/10",
      text: "text-success",
      dot: "bg-success",
      border: "border-success/20",
      label: "Healthy",
    },
    yellow: {
      bg: "bg-warning/10",
      text: "text-warning",
      dot: "bg-warning",
      border: "border-warning/20",
      label: "Degraded",
    },
    red: {
      bg: "bg-destructive/10",
      text: "text-destructive",
      dot: "bg-destructive",
      border: "border-destructive/20",
      label: "Unhealthy",
    },
  }[levelKey];

  const tooltipText = `${data.message}\nTailer lag: ${formatDuration(data.analytics_cursor_lag_seconds)}`;

  return (
    <span
      className={`inline-flex items-center gap-1.5 ${styles.bg} ${styles.text} rounded-full px-2.5 py-0.5 text-xs font-medium border ${styles.border}`}
      title={tooltipText}
    >
      <span className={`w-1.5 h-1.5 ${styles.dot} rounded-full`}></span>
      {styles.label}
    </span>
  );
}

function formatDuration(seconds: bigint): string {
  const s = Number(seconds);
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m`;
  if (s < 86400) {
    const h = Math.floor(s / 3600);
    const m = Math.floor((s % 3600) / 60);
    return m > 0 ? `${h}h ${m}m` : `${h}h`;
  }
  return `${Math.floor(s / 86400)}d`;
}
