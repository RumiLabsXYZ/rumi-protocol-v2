import { describe, expect, it } from "vitest";
import {
  applyFailedTransactionFinality,
  newCanaryOpenLock,
  newCanaryRecord,
  isRecoverableOpenCandidate,
  manualRecoveryTarget,
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
    debtE8s: CANARY_DEBT_E8S,
    pendingMintE8s: 0n,
    pendingInterestMintE8s: 0n,
    status: "AwaitingDeposit",
    ...overrides,
  };
}

describe("persisted production-canary lifecycle", () => {
  it("locks a submitted deposit across serialization until backend observation", () => {
    const opened = newCanaryRecord(OWNER, 7n);
    expect(validateCanaryAction(opened, vault(), "deposit")).toBeNull();
    const authorizing = { ...opened, phase: "deposit-authorizing" as const };
    const lockedBeforeHash = parseCanaryRecord(JSON.stringify(authorizing), OWNER)!;
    expect(validateCanaryAction(lockedBeforeHash, vault(), "deposit")).toContain("already submitted");
    expect(reconcileCanaryPhase(lockedBeforeHash, vault({ status: "Open" })).phase).toBe("mint-observed");
    const submitted = recordTransaction(opened, "deposit-submitted", "deposit", HASH);
    const restored = parseCanaryRecord(JSON.stringify(submitted), OWNER)!;
    expect(pendingTransaction(restored)).toEqual({ kind: "deposit", hash: HASH });
    expect(validateCanaryAction(restored, vault(), "deposit")).toContain("already submitted");
    const semanticallyReplaced = applyFailedTransactionFinality(restored, "deposit", "replaced");
    expect(semanticallyReplaced.phase).toBe("deposit-replaced");
    expect(semanticallyReplaced.transactions[0]).toEqual({ kind: "deposit", hash: HASH });
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
    expect(reconcileCanaryPhase(restored, vault({ status: "Open" })).phase).toBe("mint-observed");
  });

  it("locks burn across reload and requires exact debt with no pending interest", () => {
    const ready = { ...newCanaryRecord(OWNER, 7n), phase: "mint-observed" as const };
    const open = vault({ status: "Open" });
    expect(validateCanaryAction(ready, open, "burn")).toBeNull();
    expect(validateCanaryAction(ready, vault({ status: "Open", debtE8s: CANARY_DEBT_E8S + 1n }), "burn")).toContain("exactly 0.10");
    expect(validateCanaryAction(ready, vault({ status: "Open", pendingInterestMintE8s: 1n }), "burn")).toContain("no pending");
    const authorizing = { ...ready, phase: "burn-authorizing" as const };
    const lockedBeforeHash = parseCanaryRecord(JSON.stringify(authorizing), OWNER)!;
    expect(validateCanaryAction(lockedBeforeHash, open, "burn")).toContain("already submitted");
    expect(reconcileCanaryPhase(lockedBeforeHash, vault({ status: "Open", debtE8s: 0n })).phase).toBe("burn-observed");
    const submitted = recordTransaction(ready, "burn-submitted", "burn", HASH);
    const restored = parseCanaryRecord(JSON.stringify(submitted), OWNER)!;
    expect(validateCanaryAction(restored, open, "burn")).toContain("already submitted");
    const semanticallyReplaced = applyFailedTransactionFinality(restored, "burn", "replaced");
    expect(semanticallyReplaced.phase).toBe("burn-replaced");
    expect(semanticallyReplaced.transactions[0]).toEqual({ kind: "burn", hash: HASH });
    expect(pendingTransaction(semanticallyReplaced)).toBeNull();
    expect(validateCanaryAction(semanticallyReplaced, open, "burn")).toContain("already submitted");
    expect(manualRecoveryTarget(semanticallyReplaced)).toEqual({ phase: "mint-observed", action: "burn" });
    const recoveredReplacement = { ...semanticallyReplaced, phase: manualRecoveryTarget(semanticallyReplaced)!.phase };
    expect(validateCanaryAction(recoveredReplacement, open, "burn")).toBeNull();
    expect(reconcileCanaryPhase(restored, vault({ status: "Open", debtE8s: 0n })).phase).toBe("burn-observed");
  });

  it("rejects non-canary vault actions and makes completion terminal", () => {
    const openLock = parseCanaryRecord(JSON.stringify(newCanaryOpenLock(OWNER)), OWNER)!;
    expect(openLock.phase).toBe("open-authorizing");
    expect(openLock.vaultId).toBe("0");
    expect(productionLifecycleUsed(openLock, 0)).toBe(true);
    const awaitingDeposit = vault({ debtE8s: 0n, pendingMintE8s: CANARY_DEBT_E8S });
    expect(isRecoverableOpenCandidate(openLock, awaitingDeposit)).toBe(true);
    expect(isRecoverableOpenCandidate(openLock, vault())).toBe(false);
    expect(isRecoverableOpenCandidate(openLock, vault({ debtE8s: 0n, pendingMintE8s: 0n }))).toBe(false);
    const ready = { ...newCanaryRecord(OWNER, 7n), phase: "burn-observed" as const };
    expect(validateCanaryAction(ready, vault({ vaultId: 8n, status: "Open", debtE8s: 0n }), "close")).toContain("not the vault");
    expect(validateCanaryAction(ready, vault({ recipient: "0x2222222222222222222222222222222222222222", status: "Open", debtE8s: 0n }), "close")).toContain("recipient");
    const complete = reconcileCanaryPhase({ ...ready, phase: "close-submitted" }, vault({ status: "Closed", debtE8s: 0n }));
    expect(complete.phase).toBe("complete");
    expect(productionLifecycleUsed(complete, 0)).toBe(true);
    expect(productionLifecycleUsed(null, 1)).toBe(true);
    expect(productionLifecycleUsed(null, 0)).toBe(false);
  });
});
