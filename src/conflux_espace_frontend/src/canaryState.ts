import {
  CANARY_CHAIN_ID,
  CANARY_COLLATERAL_WEI,
  CANARY_DEBT_E8S,
  CANARY_ICUSD_CONTRACT,
} from "./config";

export type CanaryPhase =
  | "open-authorizing"
  | "opened"
  | "deposit-authorizing"
  | "deposit-submitted"
  | "deposit-replaced"
  | "deposit-failed"
  | "mint-observed"
  | "burn-authorizing"
  | "burn-submitted"
  | "burn-replaced"
  | "burn-failed"
  | "burn-observed"
  | "close-authorizing"
  | "close-submitted"
  | "complete";

export type CanaryTransaction = {
  kind: "deposit" | "burn";
  hash: `0x${string}`;
};

export type CanaryRecord = {
  version: 1;
  owner: `0x${string}`;
  chainId: number;
  contract: `0x${string}`;
  vaultId: string;
  phase: CanaryPhase;
  transactions: CanaryTransaction[];
};

export type CanaryVaultSnapshot = {
  vaultId: bigint;
  chainId: number;
  owner: string | null;
  recipient: string;
  collateralWei: bigint;
  debtE8s: bigint;
  pendingMintE8s: bigint;
  pendingInterestMintE8s: bigint;
  status: string;
};

const PHASES = new Set<CanaryPhase>([
  "open-authorizing", "opened", "deposit-authorizing", "deposit-submitted", "deposit-replaced", "deposit-failed", "mint-observed",
  "burn-authorizing", "burn-submitted", "burn-replaced", "burn-failed", "burn-observed", "close-authorizing", "close-submitted", "complete",
]);
const HASH = /^0x[0-9a-fA-F]{64}$/;

export function canaryStorageKey(owner: string): string {
  return `rumi:conflux-canary:v1:${CANARY_CHAIN_ID}:${CANARY_ICUSD_CONTRACT.toLowerCase()}:${owner.toLowerCase()}`;
}

export function newCanaryRecord(owner: `0x${string}`, vaultId: bigint): CanaryRecord {
  return {
    version: 1,
    owner: owner.toLowerCase() as `0x${string}`,
    chainId: CANARY_CHAIN_ID,
    contract: CANARY_ICUSD_CONTRACT.toLowerCase() as `0x${string}`,
    vaultId: vaultId.toString(),
    phase: "opened",
    transactions: [],
  };
}

/** Persisted before requesting the Open signature. Vault id zero is a sentinel
 * until the backend returns (or inventory recovery discovers) the real id. */
export function newCanaryOpenLock(owner: `0x${string}`): CanaryRecord {
  return { ...newCanaryRecord(owner, 0n), phase: "open-authorizing" };
}

/** A persisted record or any backend vault makes the one-lifecycle limit terminal. */
export function productionLifecycleUsed(record: CanaryRecord | null, ownedVaultCount: number): boolean {
  return record !== null || ownedVaultCount > 0;
}

/** Bind an ambiguous successful Open response to the single backend vault that
 * has Open's pre-deposit shape: confirmed debt zero, requested mint pending. */
export function isRecoverableOpenCandidate(record: CanaryRecord, vault: CanaryVaultSnapshot): boolean {
  return record.phase === "open-authorizing" && record.vaultId === "0" &&
    vault.chainId === CANARY_CHAIN_ID &&
    vault.owner?.toLowerCase() === record.owner && vault.recipient.toLowerCase() === record.owner &&
    vault.collateralWei === CANARY_COLLATERAL_WEI && vault.status === "AwaitingDeposit" &&
    vault.debtE8s === 0n && vault.pendingMintE8s === CANARY_DEBT_E8S && vault.pendingInterestMintE8s === 0n;
}

export function parseCanaryRecord(raw: string | null, owner: string): CanaryRecord | null {
  if (!raw) return null;
  try {
    const value = JSON.parse(raw) as CanaryRecord;
    if (value.version !== 1 || value.owner !== owner.toLowerCase() || value.chainId !== CANARY_CHAIN_ID ||
        value.contract !== CANARY_ICUSD_CONTRACT.toLowerCase() || !/^\d+$/.test(value.vaultId) ||
        !PHASES.has(value.phase) || !Array.isArray(value.transactions)) return null;
    if (value.transactions.some((tx) =>
      (tx.kind !== "deposit" && tx.kind !== "burn") || typeof tx.hash !== "string" || !HASH.test(tx.hash))) return null;
    return value;
  } catch {
    return null;
  }
}

export function recordTransaction(
  record: CanaryRecord,
  phase: "deposit-submitted" | "burn-submitted",
  kind: "deposit" | "burn",
  hash: `0x${string}`
): CanaryRecord {
  return { ...record, phase, transactions: [{ kind, hash }, ...record.transactions] };
}

export function replaceLatestTransactionHash(
  record: CanaryRecord,
  kind: "deposit" | "burn",
  hash: `0x${string}`
): CanaryRecord {
  const transactions = [...record.transactions];
  const index = transactions.findIndex((tx) => tx.kind === kind);
  if (index >= 0) transactions[index] = { kind, hash };
  return { ...record, transactions };
}

export function pendingTransaction(record: CanaryRecord): CanaryTransaction | null {
  const kind = record.phase === "deposit-submitted" ? "deposit"
    : record.phase === "burn-submitted" ? "burn"
      : null;
  return kind ? (record.transactions.find((tx) => tx.kind === kind) ?? null) : null;
}

/** Apply a failed receipt outcome to the persisted retry gate. Viem reports
 * `cancelled` only for a zero-value self-transaction; a generic semantic
 * replacement may still have transferred/burned funds and remains locked. */
export function applyFailedTransactionFinality(
  record: CanaryRecord,
  kind: "deposit" | "burn",
  replacementReason: "repriced" | "replaced" | "cancelled" | null,
): CanaryRecord {
  if (replacementReason === "replaced") {
    return { ...record, phase: kind === "deposit" ? "deposit-replaced" : "burn-replaced" };
  }
  return { ...record, phase: kind === "deposit" ? "deposit-failed" : "burn-failed" };
}

/** State/action to restore only after fresh backend reconciliation plus the
 * operator's explicit confirmation that an ambiguous intended action did not
 * occur. */
export function manualRecoveryTarget(record: CanaryRecord): {
  phase: "opened" | "mint-observed" | "burn-observed";
  action: "deposit" | "burn" | "close";
} | null {
  if (record.phase === "deposit-authorizing" || record.phase === "deposit-replaced") {
    return { phase: "opened", action: "deposit" };
  }
  if (record.phase === "burn-authorizing" || record.phase === "burn-replaced") {
    return { phase: "mint-observed", action: "burn" };
  }
  if (record.phase === "close-authorizing") return { phase: "burn-observed", action: "close" };
  return null;
}

export function reconcileCanaryPhase(record: CanaryRecord, vault: CanaryVaultSnapshot): CanaryRecord {
  if (vault.status === "Closed") return { ...record, phase: "complete" };
  if (vault.status === "Open" && vault.debtE8s === 0n &&
      (record.phase === "burn-authorizing" || record.phase === "burn-submitted" || record.phase === "burn-replaced" || record.phase === "burn-failed" || record.phase === "burn-observed" || record.phase === "close-submitted")) {
    return record.phase === "close-submitted" ? record : { ...record, phase: "burn-observed" };
  }
  if (vault.status === "Open" && vault.debtE8s === CANARY_DEBT_E8S &&
      ["opened", "deposit-authorizing", "deposit-submitted", "deposit-replaced", "deposit-failed", "mint-observed"].includes(record.phase)) {
    return { ...record, phase: "mint-observed" };
  }
  return record;
}

export function validateCanaryAction(
  record: CanaryRecord | null,
  vault: CanaryVaultSnapshot,
  action: "deposit" | "burn" | "close"
): string | null {
  if (!record || BigInt(record.vaultId) !== vault.vaultId) return "This is not the vault opened by this canary build.";
  if (record.owner !== vault.owner?.toLowerCase() || record.owner !== vault.recipient.toLowerCase()) {
    return "Canary owner and mint recipient must both match the connected wallet.";
  }
  if (vault.chainId !== CANARY_CHAIN_ID || vault.collateralWei !== CANARY_COLLATERAL_WEI) {
    return "Canary vault must be chain 1030 with exactly 5 CFX declared collateral.";
  }
  if (action === "deposit") {
    if (vault.status !== "AwaitingDeposit") return "Deposit is only available while awaiting the first deposit.";
    if (record.phase !== "opened" && record.phase !== "deposit-failed") return "The deposit is already submitted or observed.";
  } else if (action === "burn") {
    if (vault.status !== "Open" || vault.debtE8s !== CANARY_DEBT_E8S || vault.pendingMintE8s !== 0n || vault.pendingInterestMintE8s !== 0n) {
      return "Burn is allowed only at exactly 0.10 icUSD debt with no pending mint or interest.";
    }
    if (record.phase !== "mint-observed" && record.phase !== "burn-failed") return "The burn is already submitted or not ready.";
  } else {
    if (vault.status !== "Open" || vault.debtE8s !== 0n || vault.pendingMintE8s !== 0n || vault.pendingInterestMintE8s !== 0n) {
      return "Close is allowed only after zero debt and zero pending mint or interest are observed.";
    }
    if (record.phase !== "burn-observed") return "Close is already submitted or the burn is not yet observed.";
  }
  return null;
}
