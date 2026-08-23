// Testnet-only development signer. Mainnet builds resolve this module to
// `devWallet.disabled.ts`, keeping the signer implementation and its crypto
// dependency out of the emitted artifact entirely.
import { createWalletClient, http, type Hex } from "viem";
import { privateKeyToAccount } from "viem/accounts";
import { ESPACE_RPC, IS_MAINNET, confluxESpaceChain } from "./config";
import type { Wallet } from "./evm";

export function connectDevKey(pk: string): Wallet {
  if (IS_MAINNET) throw new Error("Development signer is excluded from mainnet builds.");
  const hex = (pk.startsWith("0x") ? pk : `0x${pk}`) as Hex;
  const account = privateKeyToAccount(hex);
  const client = createWalletClient({ account, chain: confluxESpaceChain, transport: http(ESPACE_RPC) });
  return { address: account.address, kind: "devkey", walletName: "Dev key", client, account };
}
