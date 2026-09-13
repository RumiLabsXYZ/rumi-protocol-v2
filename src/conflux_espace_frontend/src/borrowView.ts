import type { Snippet } from "svelte";

export type StatRow = { label: string; value: string };
/** Legacy flat shape kept only so borrowMath.ts's groupComposition (still unit-tested there) keeps type-checking. */
export type CompositionGroup = { label: string; value: string; details: string[] };
export type CompositionMember = { symbol: string; amount: string; value: string; usd: number | null; color: string };
export type CompositionEntry = { label: string; value: string; usd: number | null; members: CompositionMember[] };
export type BorrowViewProps = {
  collateral: string;
  debt: string;
  onCollateral: (value: string) => void;
  onDebt: (value: string) => void;
  inputsDisabled: boolean;
  collateralValue: string;
  priceLabel: string;
  walletBalanceLabel?: string;
  walletBalanceLoading?: boolean;
  maxCollateralDisabled?: boolean;
  onMaxCollateral?: () => void;
  walletBalanceNote?: string;
  feeLabel: string;
  feeAmount: string;
  interestLabel: string;
  receivedAmount: string;
  ratioLabel: string;
  health: string;
  tone: "safe" | "caution" | "danger" | "unavailable";
  ratioPosition: number | null;
  minPosition: number | null;
  liquidationPosition: number | null;
  safePosition: number | null;
  minLabel: string;
  liquidationLabel: string;
  liquidationPrice: string;
  positionNote: string;
  protocolRows: StatRow[];
  composition: CompositionEntry[] | null;
  protocolNote: string;
  connected: boolean;
  onConnect: () => void;
  onOpen: () => void;
  openDisabled: boolean;
  openLabel: string;
  blocker: string | null;
  actionContent?: Snippet;
};

const clampThreshold = (value: number | null): number | null =>
  value === null || !Number.isFinite(value) ? null : Math.min(100, Math.max(0, value));

/** Neutral scale shown when either threshold is unknown, so no threshold is invented at 0 or 100. */
export const trackGradient = (liquidation: number | null, safe: number | null): string => {
  const lo = clampThreshold(liquidation);
  const hi = clampThreshold(safe);
  if (lo === null || hi === null) return "rgba(160, 155, 181, .28)";
  const from = Math.min(lo, hi);
  const to = Math.max(lo, hi);
  return `linear-gradient(90deg, var(--workspace-pink) 0%, var(--workspace-violet) ${from}%, var(--workspace-teal) ${to}%)`;
};
