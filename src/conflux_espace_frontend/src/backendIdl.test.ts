import { IDL } from "@dfinity/candid";
import { describe, expect, it } from "vitest";
import { idlFactory as generatedIdlFactory } from "../../declarations/rumi_protocol_backend/rumi_protocol_backend.did.js";
import { idlFactory as frontendIdlFactory } from "./backend";

type ServiceWithFields = {
  _fields: Array<[string, {
    annotations: string[];
    display(): string;
  }]>;
};

const USED_METHODS = [
  "open_chain_vault_evm",
  "borrow_chain_vault_evm",
  "withdraw_chain_collateral_evm",
  "close_chain_vault_evm",
  "get_expected_evm_nonce",
  "get_chain_public_launch_status",
  "get_chain_vault",
  "list_chain_vaults_page",
] as const;

function usedMethodSignatures(factory: IDL.InterfaceFactory) {
  const service = factory({ IDL }) as unknown as ServiceWithFields;
  return Object.fromEntries(USED_METHODS.map((name) => {
    const method = service._fields.find(([candidate]) => candidate === name);
    if (!method) throw new Error(`${name} is absent from the IDL service`);
    return [name, { signature: method[1].display(), annotations: method[1].annotations }];
  }));
}

describe("frontend Candid projection", () => {
  it("uses the exact regenerated signatures for every frontend method", () => {
    const frontend = usedMethodSignatures(frontendIdlFactory);
    const generated = usedMethodSignatures(generatedIdlFactory);
    expect(frontend).toEqual(generated);

    for (const name of USED_METHODS.slice(0, 4)) expect(frontend[name]!.annotations).toEqual([]);
    for (const name of USED_METHODS.slice(4)) expect(frontend[name]!.annotations).toEqual(["query"]);

    const mutationResult = frontend.open_chain_vault_evm.signature;
    for (const tag of [
      "GenericError", "TemporarilyUnavailable", "TransferError", "AlreadyProcessing",
      "NotLowestCR", "SupplyInvariantHalted", "EvmAuth", "AnonymousCallerNotAllowed",
      "ChainAdmin", "AmountTooLow", "TransferFromError", "CallerNotOwner",
    ]) expect(mutationResult).toContain(tag);
    expect(frontend.list_chain_vaults_page.signature).toContain("pending_liquidation");
    expect(frontend.get_chain_public_launch_status.signature).toContain("hot_wallet_balance_is_fresh");
    expect(frontend.get_chain_public_launch_status.signature).toContain("effective_evm_rpc_principal");
    expect(frontend.get_chain_public_launch_status.signature).toContain("chains_ecdsa_key_matches_expected");
  });
});
