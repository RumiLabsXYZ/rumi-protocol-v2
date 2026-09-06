import type { Wallet } from "./evm";

/** Build-time mainnet substitute: the testnet signer module is never bundled. */
export function connectDevKey(_value: string): Wallet {
  throw new Error("Development signer is excluded from mainnet builds.");
}
