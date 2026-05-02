import { useNavigate, useSearchParams } from "react-router-dom";
import { useState, type FormEvent } from "react";

export function SearchBar() {
  const navigate = useNavigate();
  const [params] = useSearchParams();
  const [value, setValue] = useState(params.get("q") ?? "");

  function onSubmit(e: FormEvent) {
    e.preventDefault();
    const trimmed = value.trim();
    if (!trimmed) return;
    navigate(`/?q=${encodeURIComponent(trimmed)}`);
  }

  return (
    <form onSubmit={onSubmit} className="flex-1 max-w-xl">
      <input
        type="text"
        value={value}
        onChange={(e) => setValue(e.target.value)}
        placeholder="Search principal, vault id, or event id..."
        className="w-full bg-secondary text-secondary-foreground placeholder:text-muted-foreground rounded-md px-3 py-1.5 text-sm border border-border focus:outline-none focus:ring-2 focus:ring-ring"
        aria-label="Search"
      />
    </form>
  );
}
