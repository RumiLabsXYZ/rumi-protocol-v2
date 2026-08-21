import { describe, expect, it } from "vitest";
import {
  CANARY_COLLATERAL_WEI,
  CANARY_DEBT_E8S,
  openTermsFor,
  resolveDeploymentConfig,
} from "./config";

describe("deployment config", () => {
  it("preserves testnet as the default", () => {
    const config = resolveDeploymentConfig();
    expect(config).toMatchObject({
      mode: "testnet",
      productionCanary: false,
      chainId: 71,
      backendCanisterId: "kvg63-wiaaa-aaaao-bbabq-cai",
      rpcUrl: "https://evmtestnet.confluxrpc.com",
      explorerUrl: "https://evmtestnet.confluxscan.org",
      icusdContract: "0xBD02222D388BC43095A4758C3e977d5dF8f68f7a",
    });
  });

  it("pins every production-canary target", () => {
    const config = resolveDeploymentConfig("production-canary");
    expect(config).toMatchObject({
      productionCanary: true,
      chainId: 1030,
      backendCanisterId: "tfesu-vyaaa-aaaap-qrd7a-cai",
      rpcUrl: "https://evm.confluxrpc.com",
      explorerUrl: "https://evm.confluxscan.io",
      icusdContract: "0x8DdB0a13B26ed28912e4B8cCa99Bc3E8c66Df7Ff",
    });
  });

  it("refuses an unknown deployment mode", () => {
    expect(() => resolveDeploymentConfig("production")).toThrow("Unsupported VITE_DEPLOYMENT_MODE");
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
