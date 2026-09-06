import { describe, expect, it } from "vitest";
import {
  finalizedFailureProvesNonExecution,
  mainnetActionObserved,
  newMainnetActionLock,
  parseMainnetActionLock,
  sendFreshDepositAfterPreflight,
  sendFromFreshAwaitingDeposit,
  signedActionResolvedByNonce,
  withMainnetNonce,
  withMainnetReceiptSuccess,
  withMainnetTransaction,
  withMainnetVaultId,
} from "./mainnetState";

const OWNER = "0x1111111111111111111111111111111111111111" as const;
const HASH = ("0x" + "ab".repeat(32)) as `0x${string}`;
const vault = (overrides = {}) => ({
  vaultId: 7n,
  owner: OWNER,
  status: "Open",
  debtE8s: 20_000_000n,
  pendingMintE8s: 0n,
  collateralWei: 5n * 10n ** 18n,
  ...overrides,
});

describe("durable production-public action locks", () => {
  it("checks fresh AwaitingDeposit before creating a lock or requesting a wallet transaction", async () => {
    for (const status of [null, "MintPending", "Open", "Closing", "Closed", "Unexpected"]) {
      const calls: string[] = [];
      await expect(sendFromFreshAwaitingDeposit(
        status,
        () => { calls.push("lock"); },
        async () => { calls.push("send"); return HASH; },
      )).rejects.toThrow("only AwaitingDeposit");
      expect(calls).toEqual([]);
    }

    const calls: string[] = [];
    await expect(sendFromFreshAwaitingDeposit(
      "AwaitingDeposit",
      () => { calls.push("lock"); },
      async () => { calls.push("send"); return HASH; },
    )).resolves.toBe(HASH);
    expect(calls).toEqual(["lock", "send"]);
  });

  it("reads the exact vault after awaited preflight and sees a transition that happened during it", async () => {
    let releasePreflight!: () => void;
    const preflight = new Promise<void>((resolve) => { releasePreflight = resolve; });
    let backendStatus = "AwaitingDeposit";
    const calls: string[] = [];

    const attempt = sendFreshDepositAfterPreflight(
      () => preflight,
      async () => {
        calls.push(`read:${backendStatus}`);
        return { status: backendStatus };
      },
      (fresh) => fresh.status,
      () => { calls.push("validate"); },
      () => { calls.push("lock"); },
      async () => { calls.push("send"); return HASH; },
    );

    expect(calls).toEqual([]);
    backendStatus = "Open";
    releasePreflight();
    await expect(attempt).rejects.toThrow("only AwaitingDeposit");
    expect(calls).toEqual(["read:Open", "validate"]);
  });

  it("round-trips the pre-wallet lock, nonce, and transaction hash", () => {
    let lock = newMainnetActionLock({ owner: OWNER, kind: "deposit", vaultId: 7n, amount: 5n });
    lock = withMainnetNonce(lock, 4n);
    lock = withMainnetTransaction(lock, HASH);
    expect(parseMainnetActionLock(JSON.stringify(lock), OWNER)).toEqual(lock);
    expect(parseMainnetActionLock(JSON.stringify({ ...lock, txHash: "0x01" }), OWNER)).toBeNull();
    expect(parseMainnetActionLock(JSON.stringify(lock), "0x2222222222222222222222222222222222222222")).toBeNull();
  });

  it("requires an authoritative state transition before each action is observed", () => {
    const open = newMainnetActionLock({ owner: OWNER, kind: "open", amount: 10_000_000n, baselineVaultIds: [7n], baselineCollateralWei: 5n * 10n ** 18n });
    expect(mainnetActionObserved(open, [vault()])).toBe(false);
    expect(mainnetActionObserved(open, [vault(), vault({ vaultId: 8n })])).toBe(false);
    expect(mainnetActionObserved(open, [vault(), vault({ vaultId: 8n, debtE8s: 0n, pendingMintE8s: 10_000_000n })])).toBe(true);
    const exactOpen = withMainnetVaultId(open, 9n);
    expect(mainnetActionObserved(exactOpen, [vault(), vault({ vaultId: 8n, debtE8s: 0n, pendingMintE8s: 10_000_000n })])).toBe(false);
    expect(mainnetActionObserved(exactOpen, [vault(), vault({ vaultId: 9n, debtE8s: 0n, pendingMintE8s: 10_000_000n })])).toBe(true);

    let deposit = newMainnetActionLock({
      owner: OWNER,
      kind: "deposit",
      vaultId: 7n,
      amount: 10_000_000n,
      baselineStatus: "AwaitingDeposit",
      baselineDebtE8s: 0n,
      baselineCollateralWei: 5n * 10n ** 18n,
    });
    expect(mainnetActionObserved(deposit, [vault({ status: "AwaitingDeposit" })])).toBe(false);
    expect(mainnetActionObserved(deposit, [vault({ status: "MintPending", debtE8s: 0n, pendingMintE8s: 10_000_000n })])).toBe(false);
    deposit = withMainnetReceiptSuccess(withMainnetTransaction(deposit, HASH));
    expect(mainnetActionObserved(deposit, [vault({ status: "MintPending", debtE8s: 0n, pendingMintE8s: 10_000_000n })])).toBe(true);
    expect(mainnetActionObserved(deposit, [vault({ status: "Open", debtE8s: 10_000_000n, pendingMintE8s: 0n })])).toBe(true);
    expect(mainnetActionObserved(deposit, [vault({ status: "MintPending", debtE8s: 1n, pendingMintE8s: 10_000_000n })])).toBe(false);
    expect(mainnetActionObserved(deposit, [vault({ status: "MintPending", debtE8s: 0n, pendingMintE8s: 9_999_999n })])).toBe(false);
    expect(mainnetActionObserved(deposit, [vault({ status: "MintPending", debtE8s: 0n, pendingMintE8s: 10_000_000n, collateralWei: 4n * 10n ** 18n })])).toBe(false);
    expect(mainnetActionObserved(deposit, [vault({ status: "Open", debtE8s: 9_999_999n, pendingMintE8s: 0n })])).toBe(false);
    expect(mainnetActionObserved(deposit, [vault({ status: "Open", debtE8s: 10_000_000n, pendingMintE8s: 1n })])).toBe(false);
    expect(mainnetActionObserved(deposit, [vault({ status: "Open", debtE8s: 10_000_000n, pendingMintE8s: 0n, collateralWei: 6n * 10n ** 18n })])).toBe(false);
    expect(mainnetActionObserved(deposit, [vault({ status: "AwaitingDeposit" })])).toBe(false);
    expect(mainnetActionObserved(deposit, [vault({ status: "Closing" })])).toBe(false);
    expect(mainnetActionObserved(deposit, [vault({ status: "Closed" })])).toBe(false);
    expect(mainnetActionObserved(deposit, [vault({ status: "Unexpected" })])).toBe(false);

    const overflowingDeposit = withMainnetReceiptSuccess(withMainnetTransaction(newMainnetActionLock({
      owner: OWNER,
      kind: "deposit",
      vaultId: 7n,
      amount: 1n,
      baselineStatus: "AwaitingDeposit",
      baselineDebtE8s: (1n << 128n) - 1n,
      baselineCollateralWei: 5n * 10n ** 18n,
    }), HASH));
    expect(mainnetActionObserved(overflowingDeposit, [vault({
      status: "Open",
      debtE8s: 1n << 128n,
      pendingMintE8s: 0n,
    })])).toBe(false);

    let borrow = newMainnetActionLock({ owner: OWNER, kind: "borrow", vaultId: 7n, baselineDebtE8s: 20_000_000n });
    borrow = withMainnetNonce(borrow, 4n);
    expect(mainnetActionObserved(borrow, [vault()])).toBe(false);
    expect(mainnetActionObserved(borrow, [vault({ debtE8s: 30_000_000n })])).toBe(false);
    expect(signedActionResolvedByNonce(borrow, 4n)).toBe(false);
    expect(signedActionResolvedByNonce(borrow, 5n)).toBe(true);
    expect(signedActionResolvedByNonce(borrow, 6n)).toBe(false);

    let burn = newMainnetActionLock({ owner: OWNER, kind: "burn", vaultId: 7n, amount: 10_000_000n, baselineDebtE8s: 20_000_000n });
    expect(mainnetActionObserved(burn, [vault()])).toBe(false);
    expect(mainnetActionObserved(burn, [vault({ debtE8s: 10_000_000n })])).toBe(false);
    burn = withMainnetReceiptSuccess(withMainnetTransaction(burn, HASH));
    expect(mainnetActionObserved(burn, [vault({ debtE8s: 15_000_000n })])).toBe(false);
    expect(mainnetActionObserved(burn, [vault({ debtE8s: 21_000_000n })])).toBe(false);
    expect(mainnetActionObserved(burn, [vault({ debtE8s: 10_000_000n })])).toBe(true);

    let withdraw = newMainnetActionLock({ owner: OWNER, kind: "withdraw", vaultId: 7n, baselineCollateralWei: 5n * 10n ** 18n });
    withdraw = withMainnetNonce(withdraw, 8n);
    expect(mainnetActionObserved(withdraw, [vault()])).toBe(false);
    expect(mainnetActionObserved(withdraw, [vault({ collateralWei: 4n * 10n ** 18n })])).toBe(false);
    expect(signedActionResolvedByNonce(withdraw, 9n)).toBe(true);

    let close = newMainnetActionLock({ owner: OWNER, kind: "close", vaultId: 7n });
    close = withMainnetNonce(close, 11n);
    expect(mainnetActionObserved(close, [vault()])).toBe(false);
    expect(mainnetActionObserved(close, [vault({ status: "Closing" })])).toBe(false);
    expect(signedActionResolvedByNonce(close, 12n)).toBe(true);
    expect(signedActionResolvedByNonce(withMainnetNonce(open, 2n), 3n)).toBe(false);
  });

  it("clears only cryptographically finalized non-execution and never classifies ambiguity as retryable", () => {
    expect(finalizedFailureProvesNonExecution(false, null)).toBe(true);
    expect(finalizedFailureProvesNonExecution(false, "cancelled")).toBe(true);
    expect(finalizedFailureProvesNonExecution(false, "repriced")).toBe(true);
    expect(finalizedFailureProvesNonExecution(false, "replaced")).toBe(false);
    expect(finalizedFailureProvesNonExecution(true, null)).toBe(false);
    // Pending or missing receipts never call this resolved-finality helper and
    // therefore leave the durable lock in place for read-only polling.
  });
});
