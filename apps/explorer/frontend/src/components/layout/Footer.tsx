import { useEffect, useState } from "react";

interface Versions {
  explorer_bff?: string;
  explorer_assets?: string;
  git_sha?: string;
  deployed_at?: string;
  environment?: string;
}

export function Footer() {
  const [v, setV] = useState<Versions | null>(null);

  useEffect(() => {
    fetch("/versions.json")
      .then((r) => (r.ok ? r.json() : null))
      .then((data) => setV(data))
      .catch(() => setV(null));
  }, []);

  return (
    <footer className="border-t border-border bg-background mt-12">
      <div className="container mx-auto py-6 text-sm text-muted-foreground flex flex-col gap-2 md:flex-row md:justify-between md:items-center">
        <p>Rumi Explorer · open source · public, read-only</p>
        <div className="flex flex-col md:flex-row md:items-center gap-2 md:gap-4 text-xs">
          {v && (
            <span className="font-mono" title={`BFF: ${v.explorer_bff}\nAssets: ${v.explorer_assets}\nDeployed: ${v.deployed_at}`}>
              {v.git_sha ?? "unknown"} · {v.environment ?? ""}
            </span>
          )}
          <a
            href="https://github.com/RumiLabsXYZ/rumi-protocol-v2"
            className="hover:text-foreground"
            target="_blank"
            rel="noopener noreferrer"
          >
            GitHub
          </a>
        </div>
      </div>
    </footer>
  );
}
