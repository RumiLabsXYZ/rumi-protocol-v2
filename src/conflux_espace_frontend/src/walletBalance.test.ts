import { describe, expect, it } from "vitest";
import { CHAIN_ID } from "./config";
import type { Wallet } from "./evm";
import {
  FALLBACK_GAS_RESERVE_WEI,
  computeMaxCollateralWei,
  createWalletBalanceController,
  formatWalletBalanceLabel,
  formatWalletBalanceNote,
  gasReserveWeiFromFeeEstimate,
  maxCollateralInputValue,
  type WalletBalanceReaders,
} from "./walletBalance";

const ADDRESS = "0x1111111111111111111111111111111111111111" as const;
const WALLET: Wallet = {
  address: ADDRESS,
  kind: "injected",
  walletName: "Test wallet",
  client: {} as Wallet["client"],
  account: ADDRESS,
};

function readers(overrides: Partial<WalletBalanceReaders> = {}): WalletBalanceReaders {
  return {
    getChainId: async () => CHAIN_ID,
    controlsAddress: async () => true,
    getBalance: async () => 5_000_000_000_000_000_000n,
    getFeePerGas: async () => 2_000_000_000n,
    ...overrides,
  };
}

describe("createWalletBalanceController", () => {
  it("resolves a fresh snapshot on success", async () => {
    const controller = createWalletBalanceController(readers());
    const result = await controller.fetch(WALLET);
    expect(result).toEqual({
      stale: false,
      invalid: false,
      snapshot: { address: ADDRESS, balanceWei: 5_000_000_000_000_000_000n, feePerGasWei: 2_000_000_000n },
    });
  });

  it("reports invalid when the wallet's chain does not match", async () => {
    const controller = createWalletBalanceController(readers({ getChainId: async () => CHAIN_ID + 1 }));
    const result = await controller.fetch(WALLET);
    expect(result).toEqual({ stale: false, invalid: true });
  });

  it("reports invalid when the wallet no longer controls the address", async () => {
    const controller = createWalletBalanceController(readers({ controlsAddress: async () => false }));
    const result = await controller.fetch(WALLET);
    expect(result).toEqual({ stale: false, invalid: true });
  });

  it("returns an unknown (null) snapshot on a balance read failure, never zero", async () => {
    const controller = createWalletBalanceController(readers({ getBalance: async () => { throw new Error("rpc down"); } }));
    const result = await controller.fetch(WALLET);
    expect(result).toEqual({ stale: false, invalid: false, snapshot: null });
  });

  it("keeps the balance known with a null fee when the fee estimate fails", async () => {
    const controller = createWalletBalanceController(readers({ getFeePerGas: async () => { throw new Error("fee rpc down"); } }));
    const result = await controller.fetch(WALLET);
    expect(result).toEqual({
      stale: false,
      invalid: false,
      snapshot: { address: ADDRESS, balanceWei: 5_000_000_000_000_000_000n, feePerGasWei: null },
    });
  });

  it("marks a superseded request as stale and never lets it resolve non-stale", async () => {
    let releaseFirst: () => void = () => {};
    const firstGate = new Promise<void>((resolve) => { releaseFirst = resolve; });
    const controller = createWalletBalanceController(readers({
      getBalance: async () => {
        await firstGate;
        return 1n;
      },
    }));
    const first = controller.fetch(WALLET);
    const second = controller.fetch(WALLET);
    releaseFirst();
    const [firstResult, secondResult] = await Promise.all([first, second]);
    expect(firstResult).toEqual({ stale: true });
    expect(secondResult.stale).toBe(false);
  });

  it("invalidate() discards an in-flight fetch even without a superseding request", async () => {
    let releaseFirst: () => void = () => {};
    const firstGate = new Promise<void>((resolve) => { releaseFirst = resolve; });
    const controller = createWalletBalanceController(readers({
      getBalance: async () => {
        await firstGate;
        return 1n;
      },
    }));
    const first = controller.fetch(WALLET);
    controller.invalidate();
    releaseFirst();
    expect(await first).toEqual({ stale: true });
  });

  it("reverifies identity after the awaited reads: a chain switch mid-read is reported invalid", async () => {
    let chainCalls = 0;
    const controller = createWalletBalanceController(readers({
      getChainId: async () => {
        chainCalls += 1;
        return chainCalls === 1 ? CHAIN_ID : CHAIN_ID + 1;
      },
    }));
    const result = await controller.fetch(WALLET);
    expect(result).toEqual({ stale: false, invalid: true });
    expect(chainCalls).toBe(2);
  });

  it("reverifies identity after the awaited reads: an address switch mid-read is reported invalid", async () => {
    let controlsCalls = 0;
    const controller = createWalletBalanceController(readers({
      controlsAddress: async () => {
        controlsCalls += 1;
        return controlsCalls === 1;
      },
    }));
    const result = await controller.fetch(WALLET);
    expect(result).toEqual({ stale: false, invalid: true });
    expect(controlsCalls).toBe(2);
  });

  it("treats a post-read identity check failure as unknown, never as a stale-identity snapshot", async () => {
    let chainCalls = 0;
    const controller = createWalletBalanceController(readers({
      getChainId: async () => {
        chainCalls += 1;
        if (chainCalls === 1) return CHAIN_ID;
        throw new Error("rpc down");
      },
    }));
    const result = await controller.fetch(WALLET);
    expect(result).toEqual({ stale: false, invalid: false, snapshot: null });
  });
});

describe("gasReserveWeiFromFeeEstimate", () => {
  it("reserves gas limit x safety multiplier x fee per gas", () => {
    expect(gasReserveWeiFromFeeEstimate(2_000_000_000n)).toBe(2_000_000_000n * 21_000n * 2n);
  });
});

describe("computeMaxCollateralWei", () => {
  it("returns null (Max disabled) when the balance is unknown", () => {
    expect(computeMaxCollateralWei(null, 2_000_000_000n)).toBeNull();
    expect(computeMaxCollateralWei(null, null)).toBeNull();
  });

  it("subtracts the live fee-derived reserve from the balance", () => {
    const balance = 5_000_000_000_000_000_000n;
    const fee = 2_000_000_000n;
    const result = computeMaxCollateralWei(balance, fee);
    const reserve = gasReserveWeiFromFeeEstimate(fee);
    expect(result).toEqual({ maxWei: balance - reserve, reserveWei: reserve, reserveIsFallback: false });
  });

  it("uses the bounded fallback reserve, clearly labeled, when the fee estimate is unavailable", () => {
    const balance = 5_000_000_000_000_000_000n;
    const result = computeMaxCollateralWei(balance, null);
    expect(result).toEqual({ maxWei: balance - FALLBACK_GAS_RESERVE_WEI, reserveWei: FALLBACK_GAS_RESERVE_WEI, reserveIsFallback: true });
  });

  it("floors Max at zero rather than going negative for a balance at or below the reserve", () => {
    const fee = 2_000_000_000n;
    const reserve = gasReserveWeiFromFeeEstimate(fee);
    expect(computeMaxCollateralWei(reserve, fee)).toEqual({ maxWei: 0n, reserveWei: reserve, reserveIsFallback: false });
    expect(computeMaxCollateralWei(reserve - 1n, fee)).toEqual({ maxWei: 0n, reserveWei: reserve, reserveIsFallback: false });
    expect(computeMaxCollateralWei(0n, fee)).toEqual({ maxWei: 0n, reserveWei: reserve, reserveIsFallback: false });
  });

  it("never authorizes a Max amount off an unknown balance, even when a fee estimate exists", () => {
    expect(computeMaxCollateralWei(null, 1n)).toBeNull();
  });

  it("treats a zero fee estimate as unavailable, falling back to the bounded reserve rather than a zero reserve", () => {
    const balance = 5_000_000_000_000_000_000n;
    const result = computeMaxCollateralWei(balance, 0n);
    expect(result).toEqual({ maxWei: balance - FALLBACK_GAS_RESERVE_WEI, reserveWei: FALLBACK_GAS_RESERVE_WEI, reserveIsFallback: true });
  });

  it("treats a negative fee estimate as unavailable, falling back to the bounded reserve", () => {
    const balance = 5_000_000_000_000_000_000n;
    const result = computeMaxCollateralWei(balance, -1n);
    expect(result).toEqual({ maxWei: balance - FALLBACK_GAS_RESERVE_WEI, reserveWei: FALLBACK_GAS_RESERVE_WEI, reserveIsFallback: true });
  });

  it("never authorizes Max above the wallet balance, even off a bogus nonpositive fee estimate", () => {
    const balance = 5_000_000_000_000_000_000n;
    for (const fee of [0n, -1n, -1_000_000_000_000n]) {
      const result = computeMaxCollateralWei(balance, fee)!;
      expect(result.maxWei).toBeLessThanOrEqual(balance);
      expect(result.reserveWei).toBeGreaterThan(0n);
    }
  });
});

describe("maxCollateralInputValue", () => {
  it("is null when there is nothing safe to fill in", () => {
    expect(maxCollateralInputValue(null)).toBeNull();
    expect(maxCollateralInputValue({ maxWei: 0n, reserveWei: 1n, reserveIsFallback: false })).toBeNull();
  });

  it("renders the exact wei amount as a decimal string with no float rounding", () => {
    // 1234567890123456789 wei = 1.234567890123456789 CFX, exact to 18 decimals.
    const maxWei = 1_234_567_890_123_456_789n;
    const value = maxCollateralInputValue({ maxWei, reserveWei: 1n, reserveIsFallback: false });
    expect(value).toBe("1.234567890123456789");
  });

  it("keeps exact precision at 18 decimal places for a balance with a nontrivial reserve subtraction", () => {
    const balance = 3_000_000_000_000_000_001n; // 3 CFX + 1 wei
    const fee = 1_500_000_000n;
    const result = computeMaxCollateralWei(balance, fee);
    const value = maxCollateralInputValue(result);
    const reserve = gasReserveWeiFromFeeEstimate(fee);
    const expectedWei = balance - reserve;
    expect(BigInt(value!.replace(".", "")) % (10n ** 18n) === (expectedWei % (10n ** 18n))).toBe(true);
    expect(result!.maxWei).toBe(expectedWei);
  });
});

describe("formatWalletBalanceLabel", () => {
  it("distinguishes not-connected, wrong network/address, loading, unavailable, and a known balance", () => {
    expect(formatWalletBalanceLabel(null, false, false, false)).toBe("Not connected");
    expect(formatWalletBalanceLabel(null, false, true, true)).toBe("Unavailable - wrong network or address");
    expect(formatWalletBalanceLabel(null, true, false, true)).toBe("Loading...");
    expect(formatWalletBalanceLabel(null, false, false, true)).toBe("Unavailable");
    expect(formatWalletBalanceLabel({ address: ADDRESS, balanceWei: 10n ** 18n, feePerGasWei: null }, false, false, true)).toBe("1 CFX");
  });
});

describe("formatWalletBalanceNote", () => {
  it("explains why Max is disabled or which reserve policy is active", () => {
    expect(formatWalletBalanceNote(null, false, false)).toMatch(/connect a wallet/i);
    expect(formatWalletBalanceNote(null, true, true)).toMatch(/network and address/i);
    expect(formatWalletBalanceNote(null, false, true)).toMatch(/unavailable/i);
    expect(formatWalletBalanceNote({ maxWei: 0n, reserveWei: 1n, reserveIsFallback: true }, false, true)).toMatch(/too low/i);
    expect(formatWalletBalanceNote({ maxWei: 1n, reserveWei: 1n, reserveIsFallback: true }, false, true)).toMatch(/fallback/i);
    expect(formatWalletBalanceNote({ maxWei: 1n, reserveWei: 1n, reserveIsFallback: false }, false, true)).toMatch(/estimated/i);
  });

  it("never labels a genuinely positive reserve as 0 CFX, even when it rounds to zero at 4 decimals", () => {
    const tinyReserve = 1n; // 1 wei, rounds to "0" under fmtCfx's 4-decimal display
    const note = formatWalletBalanceNote({ maxWei: 1n, reserveWei: tinyReserve, reserveIsFallback: false }, false, true);
    expect(note).not.toMatch(/\b0 CFX\b/);
    expect(note).toContain("0.000000000000000001");
  });
});
