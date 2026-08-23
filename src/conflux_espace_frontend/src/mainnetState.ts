import { CANARY_CHAIN_ID, CANARY_ICUSD_CONTRACT } from "./config";

export type MainnetActionKind = "open" | "deposit" | "borrow" | "burn" | "withdraw" | "close";
export type MainnetActionPhase = "authorizing" | "submitted" | "ambiguous";

export type MainnetActionLock = {
  version: 1;
  owner: `0x${string}`;
  chainId: number;
  contract: `0x${string}`;
  kind: MainnetActionKind;
  phase: MainnetActionPhase;
  vaultId: string;
  amount: string;
  nonce: string | null;
  txHash: `0x${string}` | null;
  receiptSucceeded: boolean;
  baselineVaultIds: string[];
  baselineStatus: string | null;
  baselineDebtE8s: string;
  baselineCollateralWei: string;
};

const OWNER = /^0x[0-9a-f]{40}$/;
const HASH = /^0x[0-9a-f]{64}$/;
const UINT = /^\d+$/;
const U128_MAX = (1n << 128n) - 1n;
const KINDS = new Set<MainnetActionKind>(["open", "deposit", "borrow", "burn", "withdraw", "close"]);
const PHASES = new Set<MainnetActionPhase>(["authorizing", "submitted", "ambiguous"]);

export function mainnetStorageKey(owner: string): string {
  return `rumi:conflux-public:v1:${CANARY_CHAIN_ID}:${CANARY_ICUSD_CONTRACT.toLowerCase()}:${owner.toLowerCase()}`;
}

export function newMainnetActionLock(args: {
  owner: `0x${string}`;
  kind: MainnetActionKind;
  vaultId?: bigint;
  amount?: bigint;
  baselineVaultIds?: bigint[];
  baselineStatus?: string | null;
  baselineDebtE8s?: bigint;
  baselineCollateralWei?: bigint;
}): MainnetActionLock {
  return {
    version: 1,
    owner: args.owner.toLowerCase() as `0x${string}`,
    chainId: CANARY_CHAIN_ID,
    contract: CANARY_ICUSD_CONTRACT.toLowerCase() as `0x${string}`,
    kind: args.kind,
    phase: "authorizing",
    vaultId: (args.vaultId ?? 0n).toString(),
    amount: (args.amount ?? 0n).toString(),
    nonce: null,
    txHash: null,
    receiptSucceeded: false,
    baselineVaultIds: (args.baselineVaultIds ?? []).map(String),
    baselineStatus: args.baselineStatus ?? null,
    baselineDebtE8s: (args.baselineDebtE8s ?? 0n).toString(),
    baselineCollateralWei: (args.baselineCollateralWei ?? 0n).toString(),
  };
}

export function parseMainnetActionLock(raw: string | null, owner: string): MainnetActionLock | null {
  if (!raw) return null;
  try {
    const value = JSON.parse(raw) as MainnetActionLock;
    if (value.version !== 1 || value.owner !== owner.toLowerCase() || !OWNER.test(value.owner) ||
        value.chainId !== CANARY_CHAIN_ID || value.contract !== CANARY_ICUSD_CONTRACT.toLowerCase() ||
        !KINDS.has(value.kind) || !PHASES.has(value.phase) || !UINT.test(value.vaultId) ||
        !UINT.test(value.amount) || (value.nonce !== null && !UINT.test(value.nonce)) ||
        (value.txHash !== null && !HASH.test(value.txHash)) ||
        typeof value.receiptSucceeded !== "boolean" ||
        !Array.isArray(value.baselineVaultIds) || value.baselineVaultIds.some((id) => !UINT.test(id)) ||
        (value.baselineStatus !== null && typeof value.baselineStatus !== "string") ||
        !UINT.test(value.baselineDebtE8s) || !UINT.test(value.baselineCollateralWei)) return null;
    return value;
  } catch {
    return null;
  }
}

export function withMainnetNonce(lock: MainnetActionLock, nonce: bigint): MainnetActionLock {
  return { ...lock, nonce: nonce.toString() };
}

export function withMainnetTransaction(lock: MainnetActionLock, hash: `0x${string}`): MainnetActionLock {
  return { ...lock, phase: "submitted", txHash: hash, receiptSucceeded: false };
}

export function withMainnetReceiptSuccess(lock: MainnetActionLock): MainnetActionLock {
  return { ...lock, phase: "submitted", receiptSucceeded: true };
}

export function withMainnetVaultId(lock: MainnetActionLock, vaultId: bigint): MainnetActionLock {
  return { ...lock, vaultId: vaultId.toString() };
}

export function markMainnetSubmitted(lock: MainnetActionLock): MainnetActionLock {
  return { ...lock, phase: "submitted" };
}

export function markMainnetAmbiguous(lock: MainnetActionLock): MainnetActionLock {
  return { ...lock, phase: "ambiguous" };
}

export type MainnetVaultSnapshot = {
  vaultId: bigint;
  owner: string | null;
  status: string;
  debtE8s: bigint;
  pendingMintE8s: bigint;
  collateralWei: bigint;
};

/** Authoritative backend-state evidence that a submitted action took effect. */
export function mainnetActionObserved(lock: MainnetActionLock, vaults: MainnetVaultSnapshot[]): boolean {
  if (lock.kind === "open") {
    const baseline = new Set(lock.baselineVaultIds);
    const exactVaultId = lock.vaultId === "0" ? null : lock.vaultId;
    return vaults.some((v) => v.owner?.toLowerCase() === lock.owner && !baseline.has(v.vaultId.toString()) &&
      (exactVaultId === null || v.vaultId.toString() === exactVaultId) &&
      v.collateralWei === BigInt(lock.baselineCollateralWei) &&
      (v.debtE8s === BigInt(lock.amount) || v.pendingMintE8s === BigInt(lock.amount)));
  }
  const vault = vaults.find((v) => v.vaultId.toString() === lock.vaultId);
  if (!vault) return false;
  if (lock.kind === "deposit") {
    if (!lock.receiptSucceeded || lock.baselineStatus !== "AwaitingDeposit" ||
        vault.collateralWei !== BigInt(lock.baselineCollateralWei)) return false;
    const baselineDebt = BigInt(lock.baselineDebtE8s);
    const amount = BigInt(lock.amount);
    if (amount === 0n || baselineDebt > U128_MAX || amount > U128_MAX || amount > U128_MAX - baselineDebt) return false;
    if (vault.status === "MintPending") {
      return vault.debtE8s === baselineDebt && vault.pendingMintE8s === amount;
    }
    if (vault.status === "Open") {
      return vault.debtE8s === baselineDebt + amount && vault.pendingMintE8s === 0n;
    }
    return false;
  }
  if (lock.kind === "burn") {
    const baseline = BigInt(lock.baselineDebtE8s);
    const amount = BigInt(lock.amount);
    return lock.receiptSucceeded && amount <= baseline && vault.debtE8s === baseline - amount;
  }
  // Synchronous signed actions resolve by their unique spend-on-success nonce,
  // never by a coincidental debt/collateral/status movement.
  return false;
}

export function isSignedMainnetAction(kind: MainnetActionKind): boolean {
  return kind === "open" || kind === "borrow" || kind === "withdraw" || kind === "close";
}

export function signedActionResolvedByNonce(lock: MainnetActionLock, expectedNonce: bigint): boolean {
  return lock.kind !== "open" && isSignedMainnetAction(lock.kind) && lock.nonce !== null &&
    expectedNonce === BigInt(lock.nonce) + 1n;
}

/** A resolved failed/cancelled receipt proves this exact EVM transaction did
 * not execute. It may clear the duplicate-action lock for a later explicit
 * user retry. Missing/pending receipts and semantic replacements are not this
 * evidence and remain locked; this helper never initiates a retry. */
export function finalizedFailureProvesNonExecution(
  ok: boolean,
  replacementReason: "repriced" | "replaced" | "cancelled" | null,
): boolean {
  return !ok && replacementReason !== "replaced";
}

/**
 * Fresh-state deposit choke point. The lifecycle check runs before durable lock
 * creation and before the wallet callback, so stale polling/UI state cannot
 * produce a duplicate transfer prompt.
 */
export async function sendFromFreshAwaitingDeposit<T>(
  freshStatus: string | null,
  createDurableLock: () => void,
  requestWalletTransaction: () => Promise<T>,
): Promise<T> {
  if (freshStatus !== "AwaitingDeposit") {
    const observed = freshStatus ?? "missing";
    throw new Error(
      `Fresh vault state is ${observed}; only AwaitingDeposit can accept a deposit. ` +
      "No safety lock or wallet transaction was created.",
    );
  }
  createDurableLock();
  return requestWalletTransaction();
}

/**
 * Run every asynchronous preflight before the final exact vault read. Once
 * that read resolves, validation, the AwaitingDeposit guard, durable locking,
 * and wallet invocation run without another await boundary.
 */
export async function sendFreshDepositAfterPreflight<TVault, TResult>(
  preflight: () => Promise<void>,
  readFreshVault: () => Promise<TVault>,
  freshStatus: (vault: TVault) => string | null,
  validateFreshVault: (vault: TVault) => void,
  createDurableLock: (vault: TVault) => void,
  requestWalletTransaction: (vault: TVault) => Promise<TResult>,
): Promise<TResult> {
  await preflight();
  const freshVault = await readFreshVault();
  validateFreshVault(freshVault);
  return sendFromFreshAwaitingDeposit(
    freshStatus(freshVault),
    () => createDurableLock(freshVault),
    () => requestWalletTransaction(freshVault),
  );
}
