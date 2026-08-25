import { describe, expect, it } from "vitest";
import {
  CANARY_COLLATERAL_WEI,
  CANARY_DEBT_E8S,
  MIN_CR,
  openTermsFor,
  receiptHasRequiredConfirmations,
  resolveDeploymentConfig,
  signatureAttemptLimit,
  suggestedCollateralWei,
} from "./config";

describe("deployment config", () => {
  it("preserves testnet as the default", () => {
    const config = resolveDeploymentConfig();
    expect(config).toMatchObject({
      mode: "testnet",
      mainnet: false,
      guidedLifecycle: false,
      chainId: 71,
      backendCanisterId: "kvg63-wiaaa-aaaao-bbabq-cai",
      rpcUrl: "https://evmtestnet.confluxrpc.com",
      explorerUrl: "https://evmtestnet.confluxscan.org",
      icusdContract: "0xBD02222D388BC43095A4758C3e977d5dF8f68f7a",
      receiptConfirmations: 1,
    });
  });

  it("pins every production-canary target", () => {
    const config = resolveDeploymentConfig("production-canary");
    expect(config).toMatchObject({
      guidedLifecycle: true,
      mainnet: true,
      chainId: 1030,
      backendCanisterId: "tfesu-vyaaa-aaaap-qrd7a-cai",
      rpcUrl: "https://evm.confluxrpc.com",
      explorerUrl: "https://evm.confluxscan.io",
      icusdContract: "0x8DdB0a13B26ed28912e4B8cCa99Bc3E8c66Df7Ff",
      receiptConfirmations: 400,
    });
  });

  it("pins a distinct production-public mode to the production backend, chain, and contract", () => {
    const config = resolveDeploymentConfig("production-public");
    expect(config).toMatchObject({
      mode: "production-public",
      mainnet: true,
      guidedLifecycle: false,
      chainId: 1030,
      backendCanisterId: "tfesu-vyaaa-aaaap-qrd7a-cai",
      rpcUrl: "https://evm.confluxrpc.com",
      explorerUrl: "https://evm.confluxscan.io",
      icusdContract: "0x8DdB0a13B26ed28912e4B8cCa99Bc3E8c66Df7Ff",
      receiptConfirmations: 400,
    });
  });

  it("refuses an unknown deployment mode", () => {
    expect(() => resolveDeploymentConfig("production")).toThrow("Unsupported VITE_DEPLOYMENT_MODE");
  });

  it("allows one wallet prompt per mainnet click and keeps the testnet retry", () => {
    expect(signatureAttemptLimit(true)).toBe(1);
    expect(signatureAttemptLimit(false)).toBe(2);
  });

  it("keeps production receipt locks through 399 confirmations and resolves at 400", () => {
    const receiptBlock = 100n;
    expect(receiptHasRequiredConfirmations(receiptBlock, 498n, 400)).toBe(false);
    expect(receiptHasRequiredConfirmations(receiptBlock, 499n, 400)).toBe(true);
    expect(receiptHasRequiredConfirmations(receiptBlock, 99n, 400)).toBe(false);
    expect(receiptHasRequiredConfirmations(receiptBlock, 100n, 1)).toBe(true);
  });

  it("hard-locks production open terms and leaves testnet terms configurable", () => {
    const requested = { collateralWei: 99n, debtE8s: 88n };
    expect(openTermsFor(resolveDeploymentConfig("production-canary"), 99n, 88n)).toEqual({
      collateralWei: CANARY_COLLATERAL_WEI,
      debtE8s: CANARY_DEBT_E8S,
    });
    expect(openTermsFor(resolveDeploymentConfig("testnet"), 99n, 88n)).toEqual(requested);
  });
});

describe("suggestedCollateralWei", () => {
  // Recomputes the collateral ratio (collateral USD / debt USD) implied by a
  // suggestedCollateralWei() result — the same ratio the backend's open/borrow
  // gate checks against min_cr_e4 in chains/collateral_config.rs.
  function impliedCr(debtIcusd: number, cfxPriceUsd: number): number {
    const wei = suggestedCollateralWei(debtIcusd, cfxPriceUsd);
    const collateralCfx = Number(wei) / 1e18;
    return (collateralCfx * cfxPriceUsd) / debtIcusd;
  }

  it("mirrors the backend's 150% open/borrow gate (min_cr_e4 = 15_000)", () => {
    expect(MIN_CR).toBe(1.5);
  });

  it("clears MIN_CR for the canary's fixed 5 CFX / 0.10 icUSD debt shape", () => {
    expect(impliedCr(0.1, 0.15)).toBeGreaterThanOrEqual(MIN_CR);
  });

  it("clears MIN_CR for representative generic testnet debt/price shapes", () => {
    // Default testnet form values (debtInput "0.2", cfxPrice "0.15").
    expect(impliedCr(0.2, 0.15)).toBeGreaterThanOrEqual(MIN_CR);
    // Larger debt at a higher price.
    expect(impliedCr(5, 0.42)).toBeGreaterThanOrEqual(MIN_CR);
    // Large debt at a low price.
    expect(impliedCr(1000, 0.08)).toBeGreaterThanOrEqual(MIN_CR);
    // Small debt at a high price.
    expect(impliedCr(0.15, 3.5)).toBeGreaterThanOrEqual(MIN_CR);
  });

  it("returns 0 for non-positive debt or price", () => {
    expect(suggestedCollateralWei(0, 0.15)).toBe(0n);
    expect(suggestedCollateralWei(1, 0)).toBe(0n);
    expect(suggestedCollateralWei(-1, 0.15)).toBe(0n);
    expect(suggestedCollateralWei(1, -0.15)).toBe(0n);
  });
});
