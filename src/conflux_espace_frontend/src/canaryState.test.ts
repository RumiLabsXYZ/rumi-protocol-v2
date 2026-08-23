import { describe, expect, it } from "vitest";
import {
  applyFailedTransactionFinality,
  newCanaryOpenLock,
  newCanaryRecord,
  isRecoverableOpenCandidate,
  manualRecoveryTarget,
  markTransactionReceiptSucceeded,
  parseCanaryRecord,
  pendingTransaction,
  productionLifecycleUsed,
  reconcileCanaryPhase,
  recordTransaction,
  validateCanaryAction,
  type CanaryVaultSnapshot,
} from "./canaryState";
import { CANARY_COLLATERAL_WEI, CANARY_DEBT_E8S } from "./config";

const OWNER = "0x1111111111111111111111111111111111111111" as const;
const HASH = ("0x" + "ab".repeat(32)) as `0x${string}`;

function vault(overrides: Partial<CanaryVaultSnapshot> = {}): CanaryVaultSnapshot {
  return {
    vaultId: 7n,
    chainId: 1030,
    owner: OWNER,
    recipient: OWNER,
    collateralWei: CANARY_COLLATERAL_WEI,
    debtE8s: 0n,
    pendingMintE8s: CANARY_DEBT_E8S,
    pendingInterestMintE8s: 0n,
    status: "AwaitingDeposit",
    ...overrides,
  };
}

describe("persisted production-canary lifecycle", () => {
  it("locks a submitted deposit across serialization until backend observation", () => {
    const opened = newCanaryRecord(OWNER, 7n);
    expect(validateCanaryAction(opened, vault(), "deposit")).toBeNull();
    expect(validateCanaryAction(opened, vault({ debtE8s: 1n }), "deposit")).toContain("zero debt");
    expect(validateCanaryAction(opened, vault({ pendingMintE8s: CANARY_DEBT_E8S - 1n }), "deposit")).toContain("exactly 0.10");
    expect(validateCanaryAction(opened, vault({ pendingInterestMintE8s: 1n }), "deposit")).toContain("zero pending interest");
    expect(validateCanaryAction(opened, vault({ collateralWei: CANARY_COLLATERAL_WEI - 1n }), "deposit")).toContain("exactly 5 CFX");
    expect(validateCanaryAction(opened, vault({ status: "Open" }), "deposit")).toContain("AwaitingDeposit");
    const mintedOpen = vault({ status: "Open", debtE8s: CANARY_DEBT_E8S, pendingMintE8s: 0n });
    const authorizing = { ...opened, phase: "deposit-authorizing" as const };
    const lockedBeforeHash = parseCanaryRecord(JSON.stringify(authorizing), OWNER)!;
    expect(validateCanaryAction(lockedBeforeHash, vault(), "deposit")).toContain("already submitted");
    expect(reconcileCanaryPhase(lockedBeforeHash, mintedOpen).phase).toBe("deposit-authorizing");
    const submitted = recordTransaction(opened, "deposit-submitted", "deposit", HASH);
    const restored = parseCanaryRecord(JSON.stringify(submitted), OWNER)!;
    expect(pendingTransaction(restored)).toEqual({ kind: "deposit", hash: HASH, receiptSucceeded: false });
    expect(validateCanaryAction(restored, vault(), "deposit")).toContain("already submitted");
    const mintPending = vault({ status: "MintPending", debtE8s: 0n, pendingMintE8s: CANARY_DEBT_E8S });
    expect(reconcileCanaryPhase(restored, mintPending).phase).toBe("deposit-submitted");
    expect(reconcileCanaryPhase(restored, mintedOpen).phase).toBe("deposit-submitted");
    expect(reconcileCanaryPhase(restored, vault({ status: "Closing" })).phase).toBe("deposit-submitted");
    expect(reconcileCanaryPhase(restored, vault({ status: "Closed" })).phase).toBe("deposit-submitted");

    const wrongReceipt = markTransactionReceiptSucceeded(restored, "deposit", ("0x" + "cd".repeat(32)) as `0x${string}`);
    expect(reconcileCanaryPhase(wrongReceipt, mintedOpen).phase).toBe("deposit-submitted");
    const receiptSucceeded = markTransactionReceiptSucceeded(restored, "deposit", HASH);
    expect(receiptSucceeded.transactions[0]?.receiptSucceeded).toBe(true);
    const depositObserved = reconcileCanaryPhase(receiptSucceeded, mintPending);
    expect(depositObserved.phase).toBe("deposit-observed");
    expect(reconcileCanaryPhase(receiptSucceeded, vault({ status: "MintPending", debtE8s: 0n, pendingMintE8s: 1n })).phase).toBe("deposit-submitted");
    expect(reconcileCanaryPhase(receiptSucceeded, vault({ status: "Closing" })).phase).toBe("deposit-submitted");
    expect(reconcileCanaryPhase(receiptSucceeded, vault({ status: "Closed" })).phase).toBe("deposit-submitted");
    expect(reconcileCanaryPhase(receiptSucceeded, mintedOpen).phase).toBe("mint-observed");
    expect(reconcileCanaryPhase(depositObserved, mintedOpen).phase).toBe("mint-observed");
    expect(reconcileCanaryPhase(depositObserved, vault({ status: "Closed" })).phase).toBe("deposit-observed");

    const legacy = JSON.parse(JSON.stringify(submitted));
    delete legacy.transactions[0].receiptSucceeded;
    const migratedLegacy = parseCanaryRecord(JSON.stringify(legacy), OWNER)!;
    expect(migratedLegacy.transactions[0]?.receiptSucceeded).toBe(false);
    expect(reconcileCanaryPhase(migratedLegacy, mintedOpen).phase).toBe("deposit-submitted");
    expect(parseCanaryRecord(JSON.stringify({
      ...submitted,
      transactions: [{ ...submitted.transactions[0], receiptSucceeded: "yes" }],
    }), OWNER)).toBeNull();

    const semanticallyReplaced = applyFailedTransactionFinality(restored, "deposit", "replaced");
    expect(semanticallyReplaced.phase).toBe("deposit-replaced");
    expect(semanticallyReplaced.transactions[0]).toEqual({ kind: "deposit", hash: HASH, receiptSucceeded: false });
    expect(pendingTransaction(semanticallyReplaced)).toBeNull();
    expect(validateCanaryAction(semanticallyReplaced, vault(), "deposit")).toContain("already submitted");
    expect(manualRecoveryTarget(semanticallyReplaced)).toEqual({ phase: "opened", action: "deposit" });
    const recoveredReplacement = { ...semanticallyReplaced, phase: manualRecoveryTarget(semanticallyReplaced)!.phase };
    expect(validateCanaryAction(recoveredReplacement, vault(), "deposit")).toBeNull();
    const zeroValueCancelled = applyFailedTransactionFinality(restored, "deposit", "cancelled");
    expect(zeroValueCancelled.phase).toBe("deposit-failed");
    expect(validateCanaryAction(zeroValueCancelled, vault(), "deposit")).toBeNull();
    const reverted = applyFailedTransactionFinality(restored, "deposit", null);
    expect(reverted.phase).toBe("deposit-failed");
    expect(reconcileCanaryPhase(reverted, mintedOpen).phase).toBe("deposit-failed");
  });

  it("locks burn across reload and requires exact debt with no pending interest", () => {
    const ready = { ...newCanaryRecord(OWNER, 7n), phase: "mint-observed" as const };
    const open = vault({ status: "Open", debtE8s: CANARY_DEBT_E8S, pendingMintE8s: 0n });
    const zeroDebtOpen = vault({ status: "Open", debtE8s: 0n, pendingMintE8s: 0n });
    expect(validateCanaryAction(ready, open, "burn")).toBeNull();
    expect(validateCanaryAction(ready, vault({ status: "Open", debtE8s: CANARY_DEBT_E8S + 1n, pendingMintE8s: 0n }), "burn")).toContain("exactly 0.10");
    expect(validateCanaryAction(ready, vault({ status: "Open", debtE8s: CANARY_DEBT_E8S, pendingMintE8s: 0n, pendingInterestMintE8s: 1n }), "burn")).toContain("no pending");
    const authorizing = { ...ready, phase: "burn-authorizing" as const };
    const lockedBeforeHash = parseCanaryRecord(JSON.stringify(authorizing), OWNER)!;
    expect(validateCanaryAction(lockedBeforeHash, open, "burn")).toContain("already submitted");
    expect(reconcileCanaryPhase(lockedBeforeHash, zeroDebtOpen).phase).toBe("burn-authorizing");
    const submitted = recordTransaction(ready, "burn-submitted", "burn", HASH);
    const restored = parseCanaryRecord(JSON.stringify(submitted), OWNER)!;
    expect(validateCanaryAction(restored, open, "burn")).toContain("already submitted");
    expect(reconcileCanaryPhase(restored, zeroDebtOpen).phase).toBe("burn-submitted");
    const semanticallyReplaced = applyFailedTransactionFinality(restored, "burn", "replaced");
    expect(semanticallyReplaced.phase).toBe("burn-replaced");
    expect(semanticallyReplaced.transactions[0]).toEqual({ kind: "burn", hash: HASH, receiptSucceeded: false });
    expect(pendingTransaction(semanticallyReplaced)).toBeNull();
    expect(validateCanaryAction(semanticallyReplaced, open, "burn")).toContain("already submitted");
    expect(manualRecoveryTarget(semanticallyReplaced)).toEqual({ phase: "mint-observed", action: "burn" });
    const recoveredReplacement = { ...semanticallyReplaced, phase: manualRecoveryTarget(semanticallyReplaced)!.phase };
    expect(validateCanaryAction(recoveredReplacement, open, "burn")).toBeNull();
    const failed = applyFailedTransactionFinality(restored, "burn", null);
    expect(failed.phase).toBe("burn-failed");
    expect(validateCanaryAction(failed, open, "burn")).toBeNull();
    expect(reconcileCanaryPhase(failed, zeroDebtOpen).phase).toBe("burn-failed");

    const receiptSucceeded = markTransactionReceiptSucceeded(restored, "burn", HASH);
    expect(reconcileCanaryPhase(receiptSucceeded, vault({ status: "Open", debtE8s: 0n, pendingMintE8s: 0n, pendingInterestMintE8s: 1n })).phase).toBe("burn-submitted");
    const observed = reconcileCanaryPhase(receiptSucceeded, zeroDebtOpen);
    expect(observed.phase).toBe("burn-observed");
    expect(validateCanaryAction(observed, zeroDebtOpen, "close")).toBeNull();

    const legacyObserved = JSON.parse(JSON.stringify({ ...receiptSucceeded, phase: "burn-observed" }));
    delete legacyObserved.transactions[0].receiptSucceeded;
    const migratedLegacy = parseCanaryRecord(JSON.stringify(legacyObserved), OWNER)!;
    expect(migratedLegacy.phase).toBe("burn-submitted");
    expect(validateCanaryAction(migratedLegacy, zeroDebtOpen, "close")).toContain("successful final receipt");
  });

  it("rejects non-canary vault actions and makes completion terminal", () => {
    const openLock = parseCanaryRecord(JSON.stringify(newCanaryOpenLock(OWNER)), OWNER)!;
    expect(openLock.phase).toBe("open-authorizing");
    expect(openLock.vaultId).toBe("0");
    expect(productionLifecycleUsed(openLock, 0)).toBe(true);
    const awaitingDeposit = vault();
    expect(isRecoverableOpenCandidate(openLock, awaitingDeposit)).toBe(true);
    expect(isRecoverableOpenCandidate(openLock, vault({ debtE8s: 1n }))).toBe(false);
    expect(isRecoverableOpenCandidate(openLock, vault({ debtE8s: 0n, pendingMintE8s: 0n }))).toBe(false);
    const burnSubmitted = recordTransaction(
      { ...newCanaryRecord(OWNER, 7n), phase: "mint-observed" },
      "burn-submitted",
      "burn",
      HASH,
    );
    const ready = reconcileCanaryPhase(
      markTransactionReceiptSucceeded(burnSubmitted, "burn", HASH),
      vault({ status: "Open", debtE8s: 0n, pendingMintE8s: 0n }),
    );
    expect(ready.phase).toBe("burn-observed");
    expect(validateCanaryAction(ready, vault({ vaultId: 8n, status: "Open", debtE8s: 0n, pendingMintE8s: 0n }), "close")).toContain("not the vault");
    expect(validateCanaryAction(ready, vault({ recipient: "0x2222222222222222222222222222222222222222", status: "Open", debtE8s: 0n, pendingMintE8s: 0n }), "close")).toContain("recipient");
    const complete = reconcileCanaryPhase({ ...ready, phase: "close-submitted" }, vault({ status: "Closed", debtE8s: 0n, pendingMintE8s: 0n }));
    expect(complete.phase).toBe("complete");
    expect(productionLifecycleUsed(complete, 0)).toBe(true);
    expect(productionLifecycleUsed(null, 1)).toBe(true);
    expect(productionLifecycleUsed(null, 0)).toBe(false);
  });
});
