import { useParams } from "react-router-dom";

export function AddressDetail() {
  const { principal } = useParams<{ principal: string }>();
  return (
    <div>
      <h1 className="text-2xl font-semibold mb-1">Address</h1>
      <p className="text-sm font-mono text-muted-foreground mb-6 break-all">{principal}</p>
      <div className="bg-card border border-border rounded-lg p-4 text-sm text-muted-foreground">
        Address detail wires up in Plan 4.
      </div>
    </div>
  );
}
