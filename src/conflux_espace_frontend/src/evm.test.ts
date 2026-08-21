import { describe, expect, it } from "vitest";
import { IS_PRODUCTION_CANARY } from "./config";
import { connectDevKey, isExplicitWalletRejection } from "./evm";

const DEMO_KEY = "0x" + "00".repeat(31) + "01";

describe("private-key signer boundary", () => {
  it("is available only in the testnet build", () => {
    if (IS_PRODUCTION_CANARY) {
      expect(() => connectDevKey(DEMO_KEY)).toThrow("disabled in production-canary builds");
    } else {
      expect(connectDevKey(DEMO_KEY)).toMatchObject({ kind: "devkey", walletName: "Dev key" });
    }
  });

  it("only treats canonical wallet rejection as proof no write was authorized", () => {
    expect(isExplicitWalletRejection({ code: 4001 })).toBe(true);
    expect(isExplicitWalletRejection({ cause: { name: "UserRejectedRequestError" } })).toBe(true);
    expect(isExplicitWalletRejection(new Error("provider disconnected after request"))).toBe(false);
  });

});
