import { useParams } from "react-router-dom";
import { useEvent } from "@/hooks/useBffQueries";

export function EventDetail() {
  const { globalId } = useParams<{ globalId: string }>();
  const { data, isLoading, error } = useEvent(globalId ?? "");

  if (!globalId) return <p className="text-ink-muted">No event id in URL.</p>;

  if (isLoading) return <p className="text-ink-muted">Loading event...</p>;

  if (error) {
    return (
      <div className="bg-cinnabar/10 text-cinnabar border border-cinnabar/20 rounded-md p-4">
        Failed to load event: {error instanceof Error ? error.message : String(error)}
      </div>
    );
  }

  if (!data) return null;

  const isPending = data.payload_summary.includes("not yet available") || data.payload_summary === "Event not found";

  if (isPending) {
    return (
      <div>
        <header className="mb-8 pb-4 border-b border-quartz">
          <h1 className="text-3xl font-semibold tracking-tightest text-ink-primary">Event</h1>
          <p className="text-sm font-mono text-ink-muted mt-1">{data.global_id}</p>
        </header>
        <div className="bg-vellum-raised border border-quartz rounded-md p-4 text-sm text-ink-muted">
          <p className="font-medium text-ink-primary mb-1">{data.payload_summary}</p>
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
      <header className="mb-8 pb-4 border-b border-quartz">
        <h1 className="text-3xl font-semibold tracking-tightest text-ink-primary">Event</h1>
        <p className="text-sm font-mono text-ink-muted mt-1">{data.global_id}</p>
      </header>

      <div className="grid grid-cols-1 md:grid-cols-3 gap-3 mb-8">
        <div className="bg-vellum-raised border border-quartz rounded-md p-4">
          <p className="text-[10px] uppercase tracking-[0.1em] text-ink-muted font-medium">Kind</p>
          <p className="text-lg font-medium text-ink-primary mt-1">{data.kind}</p>
        </div>
        <div className="bg-vellum-raised border border-quartz rounded-md p-4">
          <p className="text-[10px] uppercase tracking-[0.1em] text-ink-muted font-medium">Source</p>
          <p className="text-lg font-medium text-ink-primary mt-1">{data.source}</p>
        </div>
        <div className="bg-vellum-raised border border-quartz rounded-md p-4">
          <p className="text-[10px] uppercase tracking-[0.1em] text-ink-muted font-medium">When</p>
          <p className="text-lg font-medium font-mono text-ink-primary mt-1 tabular-nums">
            {formatTimestamp(data.timestamp_ns)}
          </p>
        </div>
      </div>

      <h2 className="text-sm font-medium tracking-[0.08em] uppercase text-ink-muted mb-3">Summary</h2>
      <div className="bg-vellum-raised border border-quartz rounded-md p-4 mb-8">
        <p className="text-sm text-ink-primary">{data.payload_summary}</p>
      </div>

      <h2 className="text-sm font-medium tracking-[0.08em] uppercase text-ink-muted mb-3">Payload</h2>
      <pre className="bg-vellum-inset border border-quartz rounded-md p-4 text-xs font-mono text-ink-secondary overflow-x-auto">
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
