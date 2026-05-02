import { useParams } from "react-router-dom";

export function EventDetail() {
  const { globalId } = useParams<{ globalId: string }>();
  return (
    <div>
      <h1 className="text-2xl font-semibold mb-1">Event</h1>
      <p className="text-sm font-mono text-muted-foreground mb-6 break-all">{globalId}</p>
      <div className="bg-card border border-border rounded-lg p-4 text-sm text-muted-foreground">
        Event detail wires up in Plan 4.
      </div>
    </div>
  );
}
