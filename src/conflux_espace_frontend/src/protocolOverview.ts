import { Actor } from "@dfinity/agent";
import { Principal } from "@dfinity/principal";
import { idlFactory } from "../../declarations/icusd_ledger/icusd_ledger.did.js";
import type { _SERVICE as Ledger } from "../../declarations/icusd_ledger/icusd_ledger.did.js";
import type { ProtocolStatus, CollateralTotals, ReserveBalance, CollateralConfig } from "../../declarations/rumi_protocol_backend/rumi_protocol_backend.did.js";
import { backend, queryAgent } from "./backend";
import { BACKEND_CANISTER_ID } from "./config";
import { money } from "./borrowMath";
import type { CompositionEntry, CompositionMember, StatRow } from "./borrowView";

export type ProtocolOverview = { rows: StatRow[]; composition: CompositionEntry[] | null; note: string };
const unavailable = ["System mode", "Protocol collateral ratio", "Total collateral value", "Total borrowed", "Stablecoin reserves"];
export const unavailableOverview = (note = "Loading protocol totals…"): ProtocolOverview => ({
  rows: unavailable.map(label => ({ label, value: "Unavailable" })), composition: null, note,
});

const symbols = new Map<string, string>();
async function ledgerAt(id: Principal) {
  return Actor.createActor<Pick<Ledger, "icrc1_symbol" | "icrc1_balance_of" | "icrc1_decimals">>(idlFactory, {
    agent: await queryAgent(), canisterId: id,
  });
}

// Friendly labels for wrapped assets, and the set grouped as one nested "ICP Ecosystem" entry.
// Colors reuse src/vault_frontend collateralStore fallbacks (XRP blue / neutral gray) so
// dots reflect actual configured values instead of inventing a new palette.
const FRIENDLY: Record<string, string> = { ckBTC: "BTC", ckETH: "ETH", ckXAUT: "XAUT", ckDOGE: "DOGE" };
const ICP_ECOSYSTEM = new Set(["ICP", "nICP", "BOB", "EXE"]);
const NEUTRAL_COLOR = "#94A3B8";
const XRP_COLOR = "#4A90D9";

const byUsdDesc = (a: { usd: number | null }, b: { usd: number | null }): number =>
  a.usd === null && b.usd === null ? 0 : a.usd === null ? 1 : b.usd === null ? -1 : b.usd - a.usd;

function buildComposition(holdings: { symbol: string; amount: number; usd: number | null; color: string }[]): CompositionEntry[] {
  const groups = new Map<string, { usd: number | null; members: CompositionMember[] }>();
  for (const holding of holdings) {
    const label = ICP_ECOSYSTEM.has(holding.symbol) ? "ICP Ecosystem" : FRIENDLY[holding.symbol] ?? holding.symbol;
    const group = groups.get(label) ?? { usd: 0, members: [] };
    group.usd = group.usd === null || holding.usd === null ? null : group.usd + holding.usd;
    group.members.push({
      symbol: holding.symbol,
      amount: holding.amount.toLocaleString("en-US", { maximumFractionDigits: 8 }),
      value: money(holding.usd),
      usd: holding.usd,
      color: holding.color,
    });
    groups.set(label, group);
  }
  return [...groups.entries()]
    .map(([label, group]) => ({
      label,
      value: money(group.usd),
      usd: group.usd,
      members: [...group.members].sort((a, b) => byUsdDesc(a, b) || a.symbol.localeCompare(b.symbol)),
    }))
    .sort((a, b) => byUsdDesc(a, b) || a.label.localeCompare(b.label));
}

export async function fetchProtocolOverview(): Promise<ProtocolOverview> {
  try {
    const be = await backend();
    const [status, totals, reserves] = await Promise.all([
      be.get_protocol_status(), be.get_collateral_totals(), be.get_reserve_balances(),
    ]) as [ProtocolStatus, CollateralTotals[], ReserveBalance[]];
    const holdings = await Promise.all(totals.filter(t => t.total_collateral > 0n).map(async t => {
      const key = t.collateral_type.toText();
      let symbol = symbols.get(key) || t.symbol;
      // Always consult the config for the actual configured display color (never invent a token color);
      // this call also resolves the symbol on first sight, so it is not extra round-trips in practice.
      let color = NEUTRAL_COLOR;
      try {
        const cfg = ((await be.get_collateral_config(t.collateral_type)) as [] | [CollateralConfig])[0];
        const isNativeXrp = !!(cfg?.custody_kind[0] && "NativeXrp" in cfg.custody_kind[0]);
        if (!symbol) {
          symbol = isNativeXrp ? "XRP" : await (await ledgerAt(cfg?.ledger_canister_id ?? t.collateral_type)).icrc1_symbol();
          if (symbol) symbols.set(key, symbol);
        }
        color = cfg?.display_color?.[0] ?? (isNativeXrp ? XRP_COLOR : NEUTRAL_COLOR);
      } catch { /* Keep an explicit identifier and a neutral color, never guess the token. */ }
      const amount = Number(t.total_collateral) / 10 ** t.decimals;
      const usd = Number.isFinite(t.price) && t.price > 0 && Number.isFinite(amount) ? amount * t.price : null;
      return { symbol: symbol || `Asset ${key}`, amount, usd, color };
    }));
    // get_reserve_balances returns ledger identifiers with placeholder zero balances.
    // Query each actual ledger; never present those placeholders as live reserves.
    const balances = await Promise.all(reserves.map(async reserve => {
      try {
        const ledger = await ledgerAt(reserve.ledger);
        const [balance, decimals] = await Promise.all([
          ledger.icrc1_balance_of({ owner: Principal.fromText(BACKEND_CANISTER_ID), subaccount: [] }),
          ledger.icrc1_decimals(),
        ]);
        const value = Number(balance) / 10 ** decimals;
        return Number.isFinite(value) ? value : null;
      } catch { return null; }
    }));
    const collateralUsd = holdings.some(h => h.usd === null) ? null : holdings.reduce((sum, h) => sum + h.usd!, 0);
    const reservesUsd = !balances.length || balances.some(b => b === null) ? null : balances.reduce<number>((sum, b) => sum + b!, 0);
    const rawMode = Object.keys(status.mode)[0] ?? "Unavailable";
    const mode = status.frozen ? "Frozen" : rawMode === "GeneralAvailability" ? "Normal" : rawMode === "ReadOnly" ? "Read-only" : rawMode;
    const cr = status.total_icusd_borrowed === 0n ? "No debt"
      : Number.isFinite(status.total_collateral_ratio) && status.total_collateral_ratio >= 0
        ? `${(status.total_collateral_ratio * 100).toFixed(2)}%` : "Unavailable";
    return {
      rows: [
        { label: "System mode", value: mode },
        { label: "Protocol collateral ratio", value: cr },
        { label: "Total collateral value", value: money(collateralUsd) },
        { label: "Total borrowed", value: `${(Number(status.total_icusd_borrowed) / 1e8).toLocaleString("en-US", { maximumFractionDigits: 2 })} icUSD` },
        { label: "Stablecoin reserves", value: reservesUsd === null ? "Unavailable" : `≈ ${money(reservesUsd)}` },
      ],
      composition: buildComposition(holdings),
      note: `Core protocol pools, queried ${new Date().toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}. Conflux vaults are tracked separately. Stablecoin reserves shown at $1 per token.`,
    };
  } catch {
    return unavailableOverview("Protocol totals are unavailable. Refresh to try again. This does not change wallet or vault permissions.");
  }
}
