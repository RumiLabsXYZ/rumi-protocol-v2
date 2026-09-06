import { describe, expect, it } from "vitest";
import { blockingReasonText, priceE8, publicActionRefusal, publicWriteRefusal, ratioE4 } from "./launchStatus";
import type { ChainPublicLaunchStatus } from "./backend";

const ready = {
  chain_id: 1030,
  bound_icusd_contract: ["0x8DdB0a13B26ed28912e4B8cCa99Bc3E8c66Df7Ff"],
  icusd_contract_matches_expected: true,
  effective_evm_rpc_principal: { toText: () => "7hfb6-caaaa-aaaar-qadga-cai" },
  evm_rpc_principal_matches_expected: true,
  chains_ecdsa_key_name: "key_1",
  chains_ecdsa_key_matches_expected: true,
  collateral_config_matches_expected: true,
  debt_config_matches_expected: true,
  liquidation_config_matches_expected: true,
  liquidation_config_digest: ["reviewed-digest"],
  expected_liquidation_config_digest: "reviewed-digest",
  hot_wallet_balance_is_fresh: true,
  public_open_ready: true,
  blocking_reasons: [],
} as unknown as ChainPublicLaunchStatus;

describe("production-public readiness projection", () => {
  it("fails closed on missing status, wrong binding, or backend blockers", () => {
    expect(publicWriteRefusal(null)).toContain("unavailable");
    expect(publicWriteRefusal({ ...ready, bound_icusd_contract: ["0x0000000000000000000000000000000000000001"] })).toContain("not bound");
    expect(publicWriteRefusal({ ...ready, icusd_contract_matches_expected: false })).toContain("not bound");
    expect(publicWriteRefusal({ ...ready, evm_rpc_principal_matches_expected: false })).toContain("EVM RPC canister");
    expect(publicWriteRefusal({ ...ready, chains_ecdsa_key_matches_expected: false })).toContain("chain-signing key");
    expect(publicWriteRefusal({ ...ready, collateral_config_matches_expected: false })).toContain("collateral configuration");
    expect(publicWriteRefusal({ ...ready, debt_config_matches_expected: false })).toContain("debt configuration");
    expect(publicWriteRefusal({ ...ready, liquidation_config_matches_expected: false })).toContain("liquidation configuration");
    expect(publicWriteRefusal({ ...ready, liquidation_config_digest: [] })).toContain("digest");
    expect(publicWriteRefusal({ ...ready, liquidation_config_digest: ["wrong-digest"] })).toContain("digest");
    expect(publicWriteRefusal({ ...ready, expected_liquidation_config_digest: "" })).toContain("digest");
    expect(publicWriteRefusal({ ...ready, hot_wallet_balance_is_fresh: false })).toContain("not fresh");
    expect(publicWriteRefusal({ ...ready, public_open_ready: false, blocking_reasons: ["chain_disabled"] })).toContain("disabled");
    expect(publicWriteRefusal(ready)).toBeNull();
  });

  it("keeps recovery actions available during degraded readiness only when exact binding is valid", () => {
    const degraded = { ...ready, public_open_ready: false, blocking_reasons: ["chain_disabled"] };
    expect(publicActionRefusal(degraded, true)).toContain("disabled");
    expect(publicActionRefusal(degraded, false)).toBeNull();
    const staleConfiguration = {
      ...ready,
      collateral_config_matches_expected: false,
      debt_config_matches_expected: false,
      liquidation_config_matches_expected: false,
      liquidation_config_digest: [] as [] | [string],
      hot_wallet_balance_is_fresh: false,
    };
    expect(publicActionRefusal(staleConfiguration, true)).toContain("collateral configuration");
    expect(publicActionRefusal(staleConfiguration, false)).toBeNull();
    expect(publicActionRefusal({ ...degraded, bound_icusd_contract: ["0x0000000000000000000000000000000000000001"] }, false)).toContain("not bound");
    expect(publicActionRefusal(null, false)).toContain("unavailable");
  });

  it("formats live fixed-point facts without hard-coded production ratios", () => {
    expect(ratioE4([15_000n])).toBe(1.5);
    expect(ratioE4([])).toBeNull();
    expect(priceE8([12_345_678n])).toBe(0.12345678);
    expect(blockingReasonText("future_guard")).toContain("future guard");
  });
});
