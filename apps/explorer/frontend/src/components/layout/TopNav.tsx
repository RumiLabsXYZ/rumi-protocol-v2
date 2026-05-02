import { Link, NavLink } from "react-router-dom";
import { ThemeToggle } from "@/theme/ThemeToggle";
import { SearchBar } from "./SearchBar";
import { HealthPill } from "./HealthPill";

const lenses = [
  { to: "/lens/collateral", label: "Collateral" },
  { to: "/lens/stability-pool", label: "Stability Pool" },
  { to: "/lens/revenue", label: "Revenue" },
  { to: "/lens/redemptions", label: "Redemptions" },
  { to: "/lens/dex", label: "DEX" },
  { to: "/lens/admin", label: "Admin" },
];

export function TopNav() {
  return (
    <header className="sticky top-0 z-10 border-b border-border bg-background/80 backdrop-blur">
      <div className="container mx-auto flex flex-col gap-3 py-3 md:flex-row md:items-center md:gap-6">
        <div className="flex items-center justify-between gap-4">
          <Link to="/" className="font-semibold text-lg whitespace-nowrap">
            Rumi Explorer
          </Link>
          <div className="flex items-center gap-3 md:hidden">
            <HealthPill />
            <ThemeToggle />
          </div>
        </div>
        <SearchBar />
        <nav className="hidden md:flex items-center gap-1 text-sm">
          <NavLink
            to="/activity"
            className={({ isActive }) =>
              `px-2 py-1 rounded-md ${isActive ? "bg-secondary text-secondary-foreground" : "text-muted-foreground hover:text-foreground"}`
            }
          >
            Activity
          </NavLink>
          {lenses.map((l) => (
            <NavLink
              key={l.to}
              to={l.to}
              className={({ isActive }) =>
                `px-2 py-1 rounded-md ${isActive ? "bg-secondary text-secondary-foreground" : "text-muted-foreground hover:text-foreground"}`
              }
            >
              {l.label}
            </NavLink>
          ))}
        </nav>
        <div className="hidden md:flex items-center gap-3 ml-auto">
          <HealthPill />
          <ThemeToggle />
        </div>
      </div>
    </header>
  );
}
