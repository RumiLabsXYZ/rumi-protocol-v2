// EIP-712 VaultIntent typed-data. MUST be byte-identical to the backend's
// chains/evm/eip712.rs (same domain, same struct, same field order/types). The
// `eip712.test.ts` golden-vector test pins this against the backend's own vector.

import { hashTypedData, hexToBytes, type Account, type TypedDataDomain } from "viem";
import { CHAIN_ID, EIP712_DOMAIN } from "./config";

/** Inputs a caller provides; `recipient` is always forced to `owner` (M2 rule). */
export interface VaultIntentInput {
  action: number; // ACTION.* (uint8)
  owner: `0x${string}`;
  vaultId: bigint;
  collateralWei: bigint;
  debtE8s: bigint;
  nonce: bigint;
  deadlineSecs: bigint;
}

// Field order + types MUST match the backend type string:
// VaultIntent(uint8 action,uint64 chainId,address owner,uint64 vaultId,
//   uint256 collateralWei,uint256 debtE8s,address recipient,uint256 nonce,
//   uint256 deadline)
export const VAULT_INTENT_TYPES = {
  VaultIntent: [
    { name: "action", type: "uint8" },
    { name: "chainId", type: "uint64" },
    { name: "owner", type: "address" },
    { name: "vaultId", type: "uint64" },
    { name: "collateralWei", type: "uint256" },
    { name: "debtE8s", type: "uint256" },
    { name: "recipient", type: "address" },
    { name: "nonce", type: "uint256" },
    { name: "deadline", type: "uint256" },
  ],
} as const;

// `EIP712_DOMAIN` is the single source of truth for name/version/chainId/
// verifyingContract; only the parity test overrides verifyingContract.
function domainFor(verifyingContract: `0x${string}`): TypedDataDomain {
  return { ...EIP712_DOMAIN, verifyingContract };
}

/** The full typed-data object viem signs/hashes. `verifyingContract` defaults to
 *  the live IcUSD; the parity test overrides it to reproduce the backend vector. */
export function typedData(
  i: VaultIntentInput,
  verifyingContract: `0x${string}` = EIP712_DOMAIN.verifyingContract
) {
  return {
    domain: domainFor(verifyingContract),
    types: VAULT_INTENT_TYPES,
    primaryType: "VaultIntent" as const,
    message: {
      action: i.action,
      chainId: BigInt(CHAIN_ID),
      owner: i.owner,
      vaultId: i.vaultId,
      collateralWei: i.collateralWei,
      debtE8s: i.debtE8s,
      recipient: i.owner, // recipient == owner
      nonce: i.nonce,
      deadline: i.deadlineSecs,
    },
  };
}

/** keccak256(0x1901 ‖ domainSep ‖ structHash) — the digest the backend recovers. */
export function intentDigest(
  i: VaultIntentInput,
  verifyingContract?: `0x${string}`
): `0x${string}` {
  return hashTypedData(typedData(i, verifyingContract));
}

type TypedDataSigner = { signTypedData: (args: any) => Promise<`0x${string}`> };

/** Sign with a viem account (an injected `walletClient`, or a dev-key `Account`).
 *  Returns the 65-byte r‖s‖v signature ready for the canister `blob` argument. */
export async function signIntent(
  signer: TypedDataSigner,
  account: Account | `0x${string}`,
  i: VaultIntentInput
): Promise<Uint8Array> {
  const sigHex = await signer.signTypedData({ account, ...typedData(i) });
  return hexToBytes(sigHex);
}

/** The Candid `VaultIntent` record. owner/recipient lowercased to match the
 *  backend's recovered (lowercase) address; nat fields stay bigint. */
export function toCandidIntent(i: VaultIntentInput) {
  const owner = i.owner.toLowerCase();
  return {
    action: i.action,
    chain_id: BigInt(CHAIN_ID),
    owner,
    vault_id: i.vaultId,
    collateral_wei: i.collateralWei,
    debt_e8s: i.debtE8s,
    recipient: owner,
    nonce: i.nonce,
    deadline_secs: i.deadlineSecs,
  };
}
