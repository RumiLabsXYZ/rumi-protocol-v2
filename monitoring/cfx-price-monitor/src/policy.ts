import { collateralRatioE4 } from "./cr.js";
import type { Alert, ChainVault, OnChainPrice } from "./types.js";

export interface PolicyConfig {
  /** Refresh when |market - on-chain| exceeds this many basis points. 200 = 2%. */
  driftBps: number;
  /** Refresh when the on-chain price is older than this many seconds. */
  maxAgeSec: number;
  /** Alert (warn) when a vault's true CR drops below this e4 ratio. 16000 = 1.6x. */
  crWarnBandE4: bigint;
  /** The minimum collateral ratio (e4). Below this a vault is under-collateralized
   *  (critical, since the chains rail has no liquidation). 13000 = 1.3x. */
  mcrE4: bigint;
  /** Native decimals of the collateral asset (18 for CFX). */
  nativeDecimals: number;
}

export interface PolicyInput {
  /** Trusted aggregated market price, in e8. */
  marketE8: bigint;
  /** Current on-chain manual price, or null if none is set. */
  onChain: OnChainPrice | null;
  /** Current time, in nanoseconds. */
  nowNs: bigint;
  /** Chain vaults to evaluate (true CR computed at the market price). */
  vaults: ChainVault[];
}

export interface PolicyDecision {
  shouldRefresh: boolean;
  refreshReason: string | null;
  alerts: Alert[];
}

/**
 * The decision engine. Pure: given a trusted market price, the current on-chain
 * price + its age, and the live vaults, decide whether to refresh the on-chain
 * price and which vaults to alert on. The monitor owns freshness, so age is
 * measured off the on-chain `set_at_ns` stamp.
 */
const NS_PER_SEC = 1_000_000_000n;

function refreshDecision(
  input: PolicyInput,
  cfg: PolicyConfig,
): { shouldRefresh: boolean; refreshReason: string | null } {
  const { marketE8, onChain, nowNs } = input;

  if (onChain === null) {
    return { shouldRefresh: true, refreshReason: "no on-chain price set" };
  }
  if (onChain.priceE8 === 0n) {
    return { shouldRefresh: true, refreshReason: "on-chain price is zero" };
  }

  // Drift in basis points vs the on-chain price.
  const diff = marketE8 > onChain.priceE8 ? marketE8 - onChain.priceE8 : onChain.priceE8 - marketE8;
  const driftBps = (diff * 10_000n) / onChain.priceE8;
  if (driftBps > BigInt(cfg.driftBps)) {
    return {
      shouldRefresh: true,
      refreshReason: `drift ${driftBps} bps > ${cfg.driftBps} bps (market ${marketE8} vs on-chain ${onChain.priceE8})`,
    };
  }

  // Age off the on-chain set-timestamp. A pre-V5 price (set_at_ns = 0) reads as
  // ancient and therefore stale. Clock skew (set_at_ns in the future) reads as 0.
  const ageNs = nowNs > onChain.setAtNs ? nowNs - onChain.setAtNs : 0n;
  const ageSec = ageNs / NS_PER_SEC;
  if (ageSec > BigInt(cfg.maxAgeSec)) {
    return {
      shouldRefresh: true,
      refreshReason: `stale: on-chain price age ${ageSec}s > ${cfg.maxAgeSec}s`,
    };
  }

  return { shouldRefresh: false, refreshReason: null };
}

function crAlerts(input: PolicyInput, cfg: PolicyConfig): Alert[] {
  const alerts: Alert[] = [];
  for (const v of input.vaults) {
    if (v.debtE8s === 0n) continue; // debt-free vaults cannot be under-collateralized
    const crE4 = collateralRatioE4(v.collateralAmountE18, cfg.nativeDecimals, input.marketE8, v.debtE8s);
    if (crE4 >= cfg.crWarnBandE4) continue;

    const underMcr = crE4 < cfg.mcrE4;
    alerts.push({
      level: underMcr ? "critical" : "warn",
      code: "vault_below_band",
      message: underMcr
        ? `vault ${v.vaultId} is UNDER-COLLATERALIZED at the market price: CR ${crE4} e4 < MCR ${cfg.mcrE4} (no liquidation exists — act now)`
        : `vault ${v.vaultId} CR ${crE4} e4 below warn band ${cfg.crWarnBandE4} at the market price`,
      context: {
        vaultId: v.vaultId.toString(),
        crE4: crE4.toString(),
        marketE8: input.marketE8.toString(),
        debtE8s: v.debtE8s.toString(),
        collateralAmountE18: v.collateralAmountE18.toString(),
        mcrE4: cfg.mcrE4.toString(),
        crWarnBandE4: cfg.crWarnBandE4.toString(),
      },
    });
  }
  return alerts;
}

export function decide(input: PolicyInput, cfg: PolicyConfig): PolicyDecision {
  const { shouldRefresh, refreshReason } = refreshDecision(input, cfg);
  return { shouldRefresh, refreshReason, alerts: crAlerts(input, cfg) };
}
