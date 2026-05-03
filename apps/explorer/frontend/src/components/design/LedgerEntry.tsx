import type { ReactNode } from "react";

interface Props {
  /** Nanosecond timestamp from the candid record. */
  timestampNs: bigint;
  /** Action kind, e.g. "open_vault", "borrow", "redemption". */
  kind: string;
  /** One-line summary text. */
  summary: string;
  /** Pre-formatted amount text. Pass null for actions without an amount. */
  amount?: string | null;
  /** Optional ID badge (e.g., "backend:42"). */
  id?: string;
  /** Optional click handler, makes the whole row interactive. */
  onClick?: () => void;
  /** Optional trailing chip (custom react node, e.g., status). */
  trailing?: ReactNode;
}

const GLYPH_FOR_KIND: Record<string, string> = {
  open_vault: "⊕",
  close_vault: "⊖",
  adjust_vault: "↻",
  borrow: "↗",
  repay: "↙",
  liquidation: "✕",
  partial_liquidation: "⚠",
  redemption: "↺",
  reserve_redemption: "↺",
  stability_pool_deposit: "▼",
  stability_pool_withdraw: "▲",
  admin_mint: "+",
  admin_sweep_to_treasury: "→",
  price_update: "·",
  accrue_interest: "%",
};

const KIND_LABEL: Record<string, string> = {
  open_vault: "Vault opened",
  close_vault: "Vault closed",
  adjust_vault: "Vault adjusted",
  borrow: "Borrow",
  repay: "Repay",
  liquidation: "Liquidation",
  partial_liquidation: "Partial liquidation",
  redemption: "Redemption",
  reserve_redemption: "Reserve redemption",
  stability_pool_deposit: "SP deposit",
  stability_pool_withdraw: "SP withdraw",
  admin_mint: "Admin mint",
  admin_sweep_to_treasury: "Treasury sweep",
  price_update: "Price update",
  accrue_interest: "Interest accrual",
};

function formatLedgerTime(ns: bigint): { date: string; time: string } {
  const ms = Number(ns / 1_000_000n);
  const d = new Date(ms);
  const date = d.toLocaleDateString("en-US", { month: "short", day: "2-digit" });
  const time = d.toLocaleTimeString("en-US", { hour: "2-digit", minute: "2-digit", hour12: false });
  return { date, time };
}

export function LedgerEntry({
  timestampNs,
  kind,
  summary,
  amount,
  id,
  onClick,
  trailing,
}: Props) {
  const { date, time } = formatLedgerTime(timestampNs);
  const glyph = GLYPH_FOR_KIND[kind] ?? "·";
  const label = KIND_LABEL[kind] ?? kind;

  const interactive = !!onClick;

  return (
    <div
      onClick={onClick}
      onKeyDown={interactive ? (e) => { if (e.key === "Enter") onClick?.(); } : undefined}
      tabIndex={interactive ? 0 : undefined}
      role={interactive ? "button" : undefined}
      className={[
        "grid grid-cols-[auto_auto_1fr_auto] gap-4 items-baseline",
        "px-4 py-2.5 text-sm",
        "border-b border-quartz",
        interactive && "cursor-pointer hover:bg-vellum-inset",
      ].filter(Boolean).join(" ")}
    >
      <div className="font-mono text-[11px] tabular-nums text-ink-muted whitespace-nowrap">
        <span>{date}</span>
        <span className="ml-2 opacity-70">{time}</span>
      </div>
      <div
        className="text-ink-secondary tabular-nums w-4 text-center"
        aria-hidden="true"
        title={label}
      >
        {glyph}
      </div>
      <div className="text-ink-primary leading-snug">
        <span className="text-ink-secondary mr-2">{label}</span>
        <span className="text-ink-muted">{summary}</span>
      </div>
      <div className="text-right flex items-center gap-3">
        {trailing}
        {amount && <span className="font-mono tabular-nums text-ink-primary">{amount}</span>}
        {id && <span className="font-mono tabular-nums text-[11px] text-ink-disabled">{id}</span>}
      </div>
    </div>
  );
}
