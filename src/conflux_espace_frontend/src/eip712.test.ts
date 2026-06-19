import { describe, it, expect } from "vitest";
import { privateKeyToAccount } from "viem/accounts";
import { recoverTypedDataAddress } from "viem";
import { intentDigest, typedData, signIntent, type VaultIntentInput } from "./eip712";
import { ACTION } from "./config";

// The SAME golden vector the backend pins in chains/evm/tests_eip712.rs:
//   - signer = scalar=1 key → address 0x7e5f…95bdf
//   - domain verifyingContract = 0x…cf1c0de5 (the backend's test contract)
//   - intent: Open, 1400 CFX collateral, 100 icUSD debt, nonce 0
//   - digest + signature produced by python eth_account (independent of viem)
const GOLDEN_OWNER = "0x7e5f4552091a69125d5dfcb7b8c2659029395bdf";
const GOLDEN_CONTRACT = "0x00000000000000000000000000000000cf1c0de5";
const GOLDEN_DIGEST = "0x76fe467010b364bc9ed7caf7153a42bdc924e1cb7bf223d8182d9537717b9adc";
const GOLDEN_SIG =
  "0x06f8ac987a3a020f6e25dbfc7634ebfd95f10ab9d657a2dcd91323506381f8b65e4a53a93ee2d35887b616a4c0efccdd6406b9c93fdfcdcf0b7cbe20c3b2127a1c";

const goldenIntent: VaultIntentInput = {
  action: ACTION.Open,
  owner: GOLDEN_OWNER as `0x${string}`,
  vaultId: 0n,
  collateralWei: 1_400n * 10n ** 18n,
  debtE8s: 10_000_000_000n,
  nonce: 0n,
  deadlineSecs: 9_999_999_999n,
};

describe("EIP-712 frontend ↔ backend parity", () => {
  it("hashTypedData equals the backend's golden digest", () => {
    expect(intentDigest(goldenIntent, GOLDEN_CONTRACT as `0x${string}`)).toBe(GOLDEN_DIGEST);
  });

  it("the backend's golden signature recovers to the owner against our typed-data", async () => {
    const recovered = await recoverTypedDataAddress({
      ...typedData(goldenIntent, GOLDEN_CONTRACT as `0x${string}`),
      signature: GOLDEN_SIG as `0x${string}`,
    });
    expect(recovered.toLowerCase()).toBe(GOLDEN_OWNER);
  });

  it("signing with the scalar=1 key round-trips to the owner (the dev-key path)", async () => {
    const pk = ("0x" + "00".repeat(31) + "01") as `0x${string}`;
    const account = privateKeyToAccount(pk);
    expect(account.address.toLowerCase()).toBe(GOLDEN_OWNER);
    const sig = await signIntent(account, account, goldenIntent); // uses live-contract domain
    expect(sig.length).toBe(65); // r‖s‖v
    const recovered = await recoverTypedDataAddress({
      ...typedData(goldenIntent), // live IcUSD domain
      signature: ("0x" + Buffer.from(sig).toString("hex")) as `0x${string}`,
    });
    expect(recovered.toLowerCase()).toBe(GOLDEN_OWNER);
  });
});
