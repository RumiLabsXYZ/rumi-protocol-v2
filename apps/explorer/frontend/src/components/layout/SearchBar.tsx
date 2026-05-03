import { useNavigate, useSearchParams } from "react-router-dom";
import { useState, type FormEvent } from "react";

export function SearchBar() {
  const navigate = useNavigate();
  const [params] = useSearchParams();
  const [value, setValue] = useState(params.get("q") ?? "");

  function onSubmit(e: FormEvent) {
    e.preventDefault();
    const v = value.trim();
    if (!v) return;
    // Principal: matches xxxxx-xxxxx-...-cai pattern
    if (/^[a-z0-9]{5}-[a-z0-9-]+(-cai)?$/.test(v)) {
      navigate(`/e/address/${v}`);
      return;
    }
    // Vault id: positive integer
    if (/^\d+$/.test(v)) {
      navigate(`/e/vault/${v}`);
      return;
    }
    // Event id: source:index
    if (/^[a-z_]+:\d+$/.test(v)) {
      navigate(`/e/event/${v}`);
      return;
    }
    // Fallback: bounce to / with the query for the Overview page to surface a "couldn't parse" notice
    navigate(`/?q=${encodeURIComponent(v)}`);
  }

  return (
    <form onSubmit={onSubmit} className="flex-1 max-w-xl">
      <input
        type="text"
        value={value}
        onChange={(e) => setValue(e.target.value)}
        placeholder="search principal, vault id, event id..."
        className="w-full bg-vellum-inset text-ink-primary placeholder:text-ink-disabled rounded-sm px-3 py-1.5 text-sm font-mono border border-quartz focus:outline-none focus:ring-1 focus:ring-verdigris/40"
        aria-label="Search"
      />
    </form>
  );
}
