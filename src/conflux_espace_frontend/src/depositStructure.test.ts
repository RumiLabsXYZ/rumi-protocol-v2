import { describe, expect, it } from "vitest";
import appSource from "./App.svelte?raw";
import stateSource from "./mainnetState.ts?raw";

describe("fresh deposit write choke point", () => {
  it("puts awaited preflight before the final read and has no await before guard/lock/send", () => {
    const start = appSource.indexOf('if (kind === "deposit")');
    const end = appSource.indexOf('\n      } else {', start);
    const depositBlock = appSource.slice(start, end);
    expect(start).toBeGreaterThanOrEqual(0);
    expect(end).toBeGreaterThan(start);
    expect(depositBlock.match(/sendDeposit\(/g)).toHaveLength(1);
    expect(depositBlock.indexOf('validateCanaryAction(canary, snapshot(fresh), "deposit")')).toBeLessThan(
      depositBlock.indexOf('beginMainnetLock("deposit"'),
    );
    expect(depositBlock.indexOf("sendFreshDepositAfterPreflight(")).toBeLessThan(
      depositBlock.indexOf('beginMainnetLock("deposit"'),
    );
    expect(depositBlock.indexOf('beginMainnetLock("deposit"')).toBeLessThan(
      depositBlock.indexOf("sendDeposit("),
    );
    expect(depositBlock).toContain('beginMainnetLock("deposit", fresh, fresh.pending_mint_e8s)');

    const helperStart = stateSource.indexOf("export async function sendFreshDepositAfterPreflight");
    const preflight = stateSource.indexOf("await preflight();", helperStart);
    const finalReadStatement = "const freshVault = await readFreshVault();";
    const finalRead = stateSource.indexOf(finalReadStatement, helperStart);
    const continuation = stateSource.indexOf("validateFreshVault(freshVault);", finalRead);
    const helperSend = stateSource.indexOf("return sendFromFreshAwaitingDeposit(", continuation);
    const noAwaitSlice = stateSource.slice(finalRead + finalReadStatement.length, helperSend);
    expect(helperStart).toBeGreaterThanOrEqual(0);
    expect(preflight).toBeGreaterThan(helperStart);
    expect(preflight).toBeLessThan(finalRead);
    expect(finalRead).toBeGreaterThan(helperStart);
    expect(continuation).toBeGreaterThan(finalRead);
    expect(helperSend).toBeGreaterThan(continuation);
    expect(noAwaitSlice).not.toMatch(/\bawait\b/);

    const guard = stateSource.indexOf('freshStatus !== "AwaitingDeposit"');
    const lock = stateSource.indexOf("createDurableLock();", guard);
    const send = stateSource.indexOf("requestWalletTransaction();", guard);
    expect(guard).toBeGreaterThanOrEqual(0);
    expect(lock).toBeGreaterThan(guard);
    expect(send).toBeGreaterThan(lock);
  });
});
