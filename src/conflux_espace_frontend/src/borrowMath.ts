import type { CompositionGroup } from "./borrowView";

const WEI = 10n ** 18n;
const E8 = 100_000_000;
const U128_MAX = (1n << 128n) - 1n;

/** No parseFloat, exponent notation or implicit rounding before an EIP-712 intent. */
export function amountUnits(input: string, decimals: number): bigint | null {
  const text = input.trim();
  if (!Number.isInteger(decimals) || decimals < 0 || decimals > 18 || text.length > 80) return null;
  if (!/^\d+(?:\.\d*)?$/.test(text)) return null;
  const [whole, fraction = ""] = text.split(".");
  if (fraction.length > decimals) return null;
  const units = BigInt(whole) * 10n ** BigInt(decimals) + BigInt(fraction.padEnd(decimals, "0") || "0");
  return units <= U128_MAX ? units : null;
}

export const money = (value: number | null): string => value !== null && Number.isFinite(value)
  ? new Intl.NumberFormat("en-US", { style: "currency", currency: "USD", maximumFractionDigits: 2 }).format(value)
  : "Unavailable";

export const meterPosition = (percent: number): number => Math.max(0, Math.min(100, (percent - 100) / 2));

export function positionPreview(
  collateralWei: bigint | null, debtE8s: bigint | null, priceE8s: bigint | null,
  minCrE4: bigint | null, liquidationCrE4: bigint | null,
) {
  if (collateralWei === null || debtE8s === null || priceE8s === null || minCrE4 === null || liquidationCrE4 === null ||
      collateralWei <= 0n || debtE8s <= 0n || priceE8s <= 0n || minCrE4 <= 0n || liquidationCrE4 <= 0n) return null;
  // Mirrors the backend's integer ratio floor. Floats are display-only.
  const ratioE4 = collateralWei * priceE8s * 10_000n / (WEI * debtE8s);
  const ratioPercent = Number(ratioE4) / 100;
  const minPercent = Number(minCrE4) / 100;
  const liquidationPercent = Number(liquidationCrE4) / 100;
  const safePercent = minPercent * 1.234; // Rumi visual caution band, not a protocol gate.
  return {
    ratioPercent,
    position: meterPosition(ratioPercent),
    minPosition: meterPosition(minPercent),
    liquidationPosition: meterPosition(liquidationPercent),
    safePosition: meterPosition(safePercent),
    collateralUsd: Number(collateralWei * priceE8s / WEI) / E8,
    liquidationPrice: Number(debtE8s * liquidationCrE4 * WEI / (collateralWei * 10_000n)) / E8,
    belowMinimum: ratioE4 < minCrE4,
    tone: (ratioE4 < minCrE4 ? "danger" : ratioPercent < safePercent ? "caution" : "safe") as "danger" | "caution" | "safe",
  };
}

const FRIENDLY: Record<string, string> = { ckBTC: "BTC", ckETH: "ETH", ckXAUT: "XAUT", ckDOGE: "DOGE" };
const ICP_ECOSYSTEM = new Set(["ICP", "nICP", "BOB", "EXE"]);

export function groupComposition(rows: { symbol: string; amount: number; usd: number | null }[]): CompositionGroup[] {
  const groups = new Map<string, { usd: number | null; details: string[] }>();
  for (const row of rows) {
    const label = ICP_ECOSYSTEM.has(row.symbol) ? "ICP ecosystem" : FRIENDLY[row.symbol] ?? row.symbol;
    const group = groups.get(label) ?? { usd: 0, details: [] };
    group.usd = group.usd === null || row.usd === null ? null : group.usd + row.usd;
    group.details.push(`${row.amount.toLocaleString("en-US", { maximumFractionDigits: 8 })} ${row.symbol} · ${money(row.usd)}`);
    groups.set(label, group);
  }
  const order = ["ICP ecosystem", "BTC", "ETH", "XAUT", "XRP", "DOGE"];
  return [...groups].sort(([a], [b]) => (order.includes(a) ? order.indexOf(a) : 99) - (order.includes(b) ? order.indexOf(b) : 99) || a.localeCompare(b))
    .map(([label, group]) => ({ label, value: money(group.usd), details: group.details }));
}
