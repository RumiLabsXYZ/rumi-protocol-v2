import { Actor } from "@dfinity/agent";
import { Principal } from "@dfinity/principal";
import { idlFactory } from "../../declarations/icusd_ledger/icusd_ledger.did.js";
import type { _SERVICE as Ledger } from "../../declarations/icusd_ledger/icusd_ledger.did.js";
import type { ProtocolStatus, CollateralTotals, ReserveBalance, CollateralConfig } from "../../declarations/rumi_protocol_backend/rumi_protocol_backend.did.js";
import { backend, queryAgent } from "./backend";
import { BACKEND_CANISTER_ID } from "./config";
import { groupComposition, money } from "./borrowMath";
import type { CompositionGroup, StatRow } from "./borrowView";

export type ProtocolOverview = { rows: StatRow[]; composition: CompositionGroup[] | null; note: string };
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

/** Public queries only. No login, updates, custom root keys, or signing authority. */
export async function fetchProtocolOverview(): Promise<ProtocolOverview> {
  try {
    const be = await backend();
    const [status, totals, reserves] = await Promise.all([
      be.get_protocol_status(), be.get_collateral_totals(), be.get_reserve_balances(),
    ]) as [ProtocolStatus, CollateralTotals[], ReserveBalance[]];
    const holdings = await Promise.all(totals.filter(t => t.total_collateral > 0n).map(async t => {
      const key = t.collateral_type.toText();
      let symbol = symbols.get(key) || t.symbol;
      if (!symbol) {
        try {
          const cfg = ((await be.get_collateral_config(t.collateral_type)) as [] | [CollateralConfig])[0];
          if (cfg?.custody_kind[0] && "NativeXrp" in cfg.custody_kind[0]) symbol = "XRP";
          else symbol = await (await ledgerAt(cfg?.ledger_canister_id ?? t.collateral_type)).icrc1_symbol();
          if (symbol) symbols.set(key, symbol);
        } catch { /* Keep an explicit identifier, never guess the token. */ }
      }
      const amount = Number(t.total_collateral) / 10 ** t.decimals;
      const usd = Number.isFinite(t.price) && t.price > 0 && Number.isFinite(amount) ? amount * t.price : null;
      return { symbol: symbol || `Asset ${key}`, amount, usd };
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
      composition: groupComposition(holdings),
      note: `Core protocol pools, queried ${new Date().toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}. Conflux vaults are tracked separately. Stablecoin reserves shown at $1 per token.`,
    };
  } catch {
    return unavailableOverview("Protocol totals are unavailable. Refresh to try again. This does not change wallet or vault permissions.");
  }
}
