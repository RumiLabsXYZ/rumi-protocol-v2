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

// ---------------------------------------------------------------------------
// Other EVM destinations (rows only: no bridge exists for them yet)
// ---------------------------------------------------------------------------
//
// These rows let the destination list name the networks the bridge canister knows
// about. They are deliberately NOT part of SUPPORTED_CHAINS: that list decides which
// chain id `get_bridge_info` may report for the one live (Base) bridge, and a
// canister reporting any of these ids to the Base page is still "unsupported".
// A value that no primary source gave (a public RPC for Ethereum, most explorers)
// is left empty rather than guessed. `finality` mirrors the wording the bridge
// canister puts in its own consent text (`consent_wait` in the canister's chain table).

export interface EvmDestinationChain extends BaseChainConfig {
  /** The destination profile this network belongs to (a family label such as `ethereum`). */
  profileId: string;
  /**
   * The per-network destination key: the profile's `network_id` (`ethereum-mainnet`, `ethereum-sepolia`).
   * A mainnet and a testnet never share one. This is the key of the selector entry and of saved records.
   */
  networkId: string;
  /** Completes "Delivery is expected ...". */
  finality: string;
}

const HYPE = { name: 'HYPE', symbol: 'HYPE', decimals: 18 } as const;
const MON = { name: 'MON', symbol: 'MON', decimals: 18 } as const;

const ETH_FINALITY = 'after Ethereum finalization, about 13 minutes';
const ARB_FINALITY = 'after the Arbitrum batch is finalized on Ethereum, about 13 minutes';
const RHOOD_FINALITY = 'after Robinhood Chain finality (timing not yet confirmed)';
const HYPER_FINALITY = 'after HyperEVM finality (timing not yet confirmed)';
const MONAD_FINALITY = 'after Monad Verified state (Finalized plus 3 blocks), within seconds';

function evmRow(
  profileId: string,
  networkId: string,
  row: Omit<BaseChainConfig, 'chainIdHex' | 'local'>,
  finality: string,
): EvmDestinationChain {
  return { ...row, chainIdHex: `0x${row.chainId.toString(16)}`, local: false, profileId, networkId, finality };
}

export const ETHEREUM_MAINNET = evmRow('ethereum', 'ethereum-mainnet', { chainId: 1, name: 'Ethereum', rpcUrl: '', explorerUrl: '', nativeCurrency: ETH, testnet: false }, ETH_FINALITY);
export const ETHEREUM_SEPOLIA = evmRow('ethereum', 'ethereum-sepolia', { chainId: 11155111, name: 'Ethereum Sepolia', rpcUrl: '', explorerUrl: '', nativeCurrency: ETH, testnet: true }, ETH_FINALITY);
export const ARBITRUM_ONE = evmRow('arbitrum', 'arbitrum-mainnet', { chainId: 42161, name: 'Arbitrum One', rpcUrl: 'https://arb1.arbitrum.io/rpc', explorerUrl: '', nativeCurrency: ETH, testnet: false }, ARB_FINALITY);
export const ARBITRUM_SEPOLIA = evmRow('arbitrum', 'arbitrum-sepolia', { chainId: 421614, name: 'Arbitrum Sepolia', rpcUrl: 'https://sepolia-rollup.arbitrum.io/rpc', explorerUrl: '', nativeCurrency: ETH, testnet: true }, ARB_FINALITY);
export const ROBINHOOD_CHAIN = evmRow('robinhood', 'robinhood-mainnet', { chainId: 4663, name: 'Robinhood Chain', rpcUrl: 'https://rpc.mainnet.chain.robinhood.com', explorerUrl: 'https://robinhoodchain.blockscout.com', nativeCurrency: ETH, testnet: false }, RHOOD_FINALITY);
export const ROBINHOOD_CHAIN_TESTNET = evmRow('robinhood', 'robinhood-testnet', { chainId: 46630, name: 'Robinhood Chain testnet', rpcUrl: 'https://rpc.testnet.chain.robinhood.com', explorerUrl: 'https://explorer.testnet.chain.robinhood.com', nativeCurrency: ETH, testnet: true }, RHOOD_FINALITY);
export const HYPEREVM_MAINNET = evmRow('hyperevm', 'hyperevm-mainnet', { chainId: 999, name: 'HyperEVM', rpcUrl: 'https://rpc.hyperliquid.xyz/evm', explorerUrl: '', nativeCurrency: HYPE, testnet: false }, HYPER_FINALITY);
export const HYPEREVM_TESTNET = evmRow('hyperevm', 'hyperevm-testnet', { chainId: 998, name: 'HyperEVM testnet', rpcUrl: 'https://rpc.hyperliquid-testnet.xyz/evm', explorerUrl: '', nativeCurrency: HYPE, testnet: true }, HYPER_FINALITY);
export const MONAD_MAINNET = evmRow('monad', 'monad-mainnet', { chainId: 143, name: 'Monad', rpcUrl: 'https://rpc.monad.xyz', explorerUrl: '', nativeCurrency: MON, testnet: false }, MONAD_FINALITY);
export const MONAD_TESTNET = evmRow('monad', 'monad-testnet', { chainId: 10143, name: 'Monad testnet', rpcUrl: 'https://testnet-rpc.monad.xyz', explorerUrl: '', nativeCurrency: MON, testnet: true }, MONAD_FINALITY);

/** Every non-Base EVM network the bridge canister knows. Mainnets first, each followed by its testnet. */
export const EVM_DESTINATION_CHAINS: readonly EvmDestinationChain[] = [
  ETHEREUM_MAINNET,
  ETHEREUM_SEPOLIA,
  ARBITRUM_ONE,
  ARBITRUM_SEPOLIA,
  ROBINHOOD_CHAIN,
  ROBINHOOD_CHAIN_TESTNET,
  HYPEREVM_MAINNET,
  HYPEREVM_TESTNET,
  MONAD_MAINNET,
  MONAD_TESTNET,
];

export function getEvmDestinationChain(chainId: number | bigint | null | undefined): EvmDestinationChain | null {
  if (chainId === null || chainId === undefined) return null;
  const id = Number(chainId);
  return EVM_DESTINATION_CHAINS.find((chain) => chain.chainId === id) ?? null;
}

/**
 * The per-network destination key of a Base chain, from the chain id the bridge canister reports.
 * These are the Base profile's `network_id` values; the local dev chain has no profile, so its key is
 * `base-local`. A key for a chain this app does not know is never made up: the caller gets null.
 */
export function baseNetworkId(chainId: number | bigint | null | undefined): string | null {
  if (chainId === null || chainId === undefined) return null;
  const id = Number(chainId);
  if (id === BASE_MAINNET.chainId) return 'base-mainnet';
  if (id === BASE_SEPOLIA.chainId) return 'base-sepolia';
  if (id === BASE_LOCAL.chainId) return 'base-local';
  return null;
}

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
    rpcUrls: chain.rpcUrl ? [chain.rpcUrl] : [],
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
  /**
   * True while the Minter lane (the signer that mints on Base) reports it has no
   * Base ETH to pay gas. Deliveries to Base wait, escrowed icUSD is safe. The
   * Admin lane's gas never affects users, so it is not surfaced.
   */
  minterAwaitingGas: boolean;
  version: string;
}

/** Whether `get_bridge_info().lanes` says the Minter lane is waiting for gas. Tolerates a missing list. */
export function minterAwaitingGas(info: Pick<BridgeInfo, 'lanes'>): boolean {
  const lanes = Array.isArray(info.lanes) ? info.lanes : [];
  return lanes.some((lane) => !!lane && typeof lane === 'object' && 'Minter' in lane.lane && lane.awaiting_gas === true);
}

export type BridgeConfigFailure = 'unsupported_chain' | 'contract_not_bound' | 'invalid_contract';

export type BridgeConfigResult =
  | { ok: true; config: BridgeConfig }
  /** `chain` is set when the reported chain id is a supported one (the contract is what is wrong). */
  | { ok: false; reason: BridgeConfigFailure; message: string; chain?: BaseChainConfig };

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
    return { ok: false, reason: 'contract_not_bound', message: 'The Base contract is not bound yet.', chain };
  }
  if (!isAddress(binding.address) || isZeroAddress(binding.address)) {
    return { ok: false, reason: 'invalid_contract', message: 'The bridge reported an invalid Base contract address.', chain };
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
      minterAwaitingGas: minterAwaitingGas(info),
      version: info.version,
    },
  };
}
