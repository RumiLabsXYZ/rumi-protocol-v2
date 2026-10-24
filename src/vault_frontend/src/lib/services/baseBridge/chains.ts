/**
 * Base network configuration and the bridge's live configuration.
 *
 * Which chain and which contract the page talks to is never hard-coded per
 * build: it is read from the canister's `get_bridge_info()` and resolved here.
 * The table below only knows how to describe each supported chain to a wallet
 * (`wallet_addEthereumChain`) and where its block explorer lives.
 */

import type { BridgeInfo } from '$declarations/rumi_icusd_bridge/rumi_icusd_bridge.did';
import { isAddress, isZeroAddress } from './abi';

export interface BaseChainConfig {
  chainId: number;
  /** 0x-prefixed lower-case hex, as wallets report it from `eth_chainId`. */
  chainIdHex: string;
  name: string;
  rpcUrl: string;
  /** Empty for chains without a public explorer (local anvil). */
  explorerUrl: string;
  nativeCurrency: { name: string; symbol: string; decimals: number };
  testnet: boolean;
  local: boolean;
}

const ETH = { name: 'Ether', symbol: 'ETH', decimals: 18 } as const;

export const BASE_MAINNET: BaseChainConfig = {
  chainId: 8453,
  chainIdHex: '0x2105',
  name: 'Base',
  rpcUrl: 'https://mainnet.base.org',
  explorerUrl: 'https://basescan.org',
  nativeCurrency: ETH,
  testnet: false,
  local: false,
};

export const BASE_SEPOLIA: BaseChainConfig = {
  chainId: 84532,
  chainIdHex: '0x14a34',
  name: 'Base Sepolia',
  rpcUrl: 'https://sepolia.base.org',
  explorerUrl: 'https://sepolia.basescan.org',
  nativeCurrency: ETH,
  testnet: true,
  local: false,
};

export const BASE_LOCAL: BaseChainConfig = {
  chainId: 31337,
  chainIdHex: '0x7a69',
  name: 'Local Base (anvil)',
  rpcUrl: 'http://127.0.0.1:8545',
  explorerUrl: '',
  nativeCurrency: ETH,
  testnet: true,
  local: true,
};

export const SUPPORTED_CHAINS: readonly BaseChainConfig[] = [BASE_MAINNET, BASE_SEPOLIA, BASE_LOCAL];

export function getChainConfig(chainId: number | bigint | null | undefined): BaseChainConfig | null {
  if (chainId === null || chainId === undefined) return null;
  const id = Number(chainId);
  return SUPPORTED_CHAINS.find((chain) => chain.chainId === id) ?? null;
}

export function chainIdToHex(chainId: number | bigint): string {
  return `0x${BigInt(chainId).toString(16)}`;
}

/** Parse an `eth_chainId` / `chainChanged` value (hex string or number). Null when malformed. */
export function parseChainId(value: unknown): number | null {
  if (typeof value === 'number' && Number.isSafeInteger(value) && value >= 0) return value;
  if (typeof value === 'string' && /^0x[0-9a-fA-F]+$/.test(value)) {
    const parsed = Number(BigInt(value));
    return Number.isSafeInteger(parsed) ? parsed : null;
  }
  if (typeof value === 'string' && /^\d+$/.test(value)) {
    const parsed = Number(value);
    return Number.isSafeInteger(parsed) ? parsed : null;
  }
  return null;
}

export function explorerTxUrl(chain: BaseChainConfig, txHash: string): string | null {
  return chain.explorerUrl ? `${chain.explorerUrl}/tx/${txHash}` : null;
}

export function explorerAddressUrl(chain: BaseChainConfig, address: string): string | null {
  return chain.explorerUrl ? `${chain.explorerUrl}/address/${address}` : null;
}

/** Parameters for `wallet_addEthereumChain`. */
export function addChainParams(chain: BaseChainConfig): {
  chainId: string;
  chainName: string;
  nativeCurrency: { name: string; symbol: string; decimals: number };
  rpcUrls: string[];
  blockExplorerUrls?: string[];
} {
  return {
    chainId: chain.chainIdHex,
    chainName: chain.name,
    nativeCurrency: { ...chain.nativeCurrency },
    rpcUrls: [chain.rpcUrl],
    ...(chain.explorerUrl ? { blockExplorerUrls: [chain.explorerUrl] } : {}),
  };
}

// ---------------------------------------------------------------------------
// Live bridge configuration (from get_bridge_info)
// ---------------------------------------------------------------------------

export interface BridgeConfig {
  chain: BaseChainConfig;
  /** Bound BridgedIcUSD address, lower-cased. */
  contract: string;
  /** Contract minimum for bridgeOut, in e8s. */
  minBridgeOutE8s: bigint;
  minDepositE8s: bigint;
  maxDepositE8s: bigint;
  depositFeeE8s: bigint;
  releaseFeeE8s: bigint;
  /** The bridge canister's cached icUSD ledger fee, when it has one. */
  ledgerFeeE8s: bigint | null;
  /** Text form of the icUSD ledger the bridge escrows. */
  icusdLedger: string;
  depositsPaused: boolean;
  releasesPaused: boolean;
  mintsPaused: boolean;
  /** Reason text when the whole bridge is halted. */
  halted: string | null;
  version: string;
}

export type BridgeConfigFailure = 'unsupported_chain' | 'contract_not_bound' | 'invalid_contract';

export type BridgeConfigResult =
  | { ok: true; config: BridgeConfig }
  | { ok: false; reason: BridgeConfigFailure; message: string };

/** Turn `get_bridge_info()` into the configuration the page uses, or explain why it cannot. */
export function resolveBridgeConfig(info: BridgeInfo): BridgeConfigResult {
  const chain = getChainConfig(info.chain_id);
  if (!chain) {
    return {
      ok: false,
      reason: 'unsupported_chain',
      message: `The bridge is configured for chain ${info.chain_id.toString()}, which this app does not support.`,
    };
  }
  const binding = info.contract[0];
  if (!binding) {
    return { ok: false, reason: 'contract_not_bound', message: 'The Base contract is not bound yet.' };
  }
  if (!isAddress(binding.address) || isZeroAddress(binding.address)) {
    return { ok: false, reason: 'invalid_contract', message: 'The bridge reported an invalid Base contract address.' };
  }
  return {
    ok: true,
    config: {
      chain,
      contract: binding.address.toLowerCase(),
      minBridgeOutE8s: BigInt(binding.min_bridge_out_e8s),
      minDepositE8s: info.limits.min_deposit_e8s,
      maxDepositE8s: info.limits.max_deposit_e8s,
      depositFeeE8s: info.fees.deposit_fee_e8s,
      releaseFeeE8s: info.fees.release_fee_e8s,
      ledgerFeeE8s: info.ledger_fee_e8s[0] ?? null,
      icusdLedger: info.icusd_ledger.toText(),
      depositsPaused: info.pauses.deposits,
      releasesPaused: info.pauses.releases,
      mintsPaused: info.pauses.mints,
      halted: info.halted[0] ?? null,
      version: info.version,
    },
  };
}
