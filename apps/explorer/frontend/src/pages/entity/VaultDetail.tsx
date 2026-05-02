import { useParams } from "react-router-dom";

export function VaultDetail() {
  const { id } = useParams<{ id: string }>();
  return (
    <div>
      <h1 className="text-2xl font-semibold mb-1">Vault #{id}</h1>
      <div className="bg-card border border-border rounded-lg p-4 text-sm text-muted-foreground">
        Vault detail wires up in Plan 4.
      </div>
    </div>
  );
}
