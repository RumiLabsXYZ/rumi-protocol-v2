import { useParams } from "react-router-dom";
import { useEvent } from "@/hooks/useBffQueries";

export function EventDetail() {
  const { globalId } = useParams<{ globalId: string }>();
  const { data, isLoading, error } = useEvent(globalId ?? "");

  if (!globalId) return <p className="text-muted-foreground">No event id in URL.</p>;

  if (isLoading) return <p className="text-muted-foreground">Loading event...</p>;

  if (error) {
    return (
      <div className="bg-destructive/10 text-destructive border border-destructive/20 rounded-lg p-4">
        Failed to load event: {error instanceof Error ? error.message : String(error)}
      </div>
    );
  }

  if (!data) return null;

  const isPending = data.payload_summary.includes("not yet available") || data.payload_summary === "Event not found";

  if (isPending) {
    return (
      <div>
        <h1 className="text-2xl font-semibold mb-1">Event</h1>
        <p className="text-sm font-mono text-muted-foreground mb-4">{data.global_id}</p>
        <div className="bg-secondary/50 border border-border rounded-lg p-4 text-sm text-muted-foreground">
          <p className="font-medium text-foreground mb-1">{data.payload_summary}</p>
          <p>
            Event detail requires decoding the full Rumi Event variant, which is not yet
            ported into the BFF shadow types. This lights up in a follow-up.
          </p>
        </div>
      </div>
    );
  }

  return (
    <div>
      <h1 className="text-2xl font-semibold mb-1">Event</h1>
      <p className="text-sm font-mono text-muted-foreground mb-6">{data.global_id}</p>

      <div className="grid grid-cols-1 md:grid-cols-3 gap-4 mb-8">
        <div className="bg-card border border-border rounded-lg p-4">
          <p className="text-xs uppercase text-muted-foreground tracking-wide">Kind</p>
          <p className="text-lg font-medium mt-1">{data.kind}</p>
        </div>
        <div className="bg-card border border-border rounded-lg p-4">
          <p className="text-xs uppercase text-muted-foreground tracking-wide">Source</p>
          <p className="text-lg font-medium mt-1">{data.source}</p>
        </div>
        <div className="bg-card border border-border rounded-lg p-4">
          <p className="text-xs uppercase text-muted-foreground tracking-wide">When</p>
          <p className="text-lg font-medium mt-1">{formatTimestamp(data.timestamp_ns)}</p>
        </div>
      </div>

      <h2 className="text-lg font-semibold mb-3">Summary</h2>
      <div className="bg-card border border-border rounded-lg p-4 mb-8">
        <p className="text-sm">{data.payload_summary}</p>
      </div>

      <h2 className="text-lg font-semibold mb-3">Payload</h2>
      <pre className="bg-card border border-border rounded-lg p-4 text-xs font-mono overflow-x-auto">
        {data.payload_json}
      </pre>
    </div>
  );
}

function formatTimestamp(ns: bigint): string {
  const ms = Number(ns / 1_000_000n);
  const d = new Date(ms);
  return d.toLocaleString("en-US", { dateStyle: "short", timeStyle: "short" });
}
