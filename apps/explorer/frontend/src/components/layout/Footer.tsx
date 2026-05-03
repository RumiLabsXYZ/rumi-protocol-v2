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
    <footer className="border-t border-quartz bg-vellum mt-12">
      <div className="container mx-auto py-4 text-[11px] font-mono text-ink-muted flex flex-col gap-1.5 md:flex-row md:justify-between md:items-center">
        <p>Rumi Explorer · open source · public read-only</p>
        <div className="flex flex-col md:flex-row md:items-center gap-1.5 md:gap-4">
          {v && (
            <span title={`BFF: ${v.explorer_bff}\nAssets: ${v.explorer_assets}\nDeployed: ${v.deployed_at}`}>
              {v.git_sha ?? "unknown"} · {v.environment ?? ""}
            </span>
          )}
          <a
            href="https://github.com/RumiLabsXYZ/rumi-protocol-v2"
            className="hover:text-ink-secondary"
            target="_blank"
            rel="noopener noreferrer"
          >
            GitHub ↗
          </a>
        </div>
      </div>
    </footer>
  );
}
