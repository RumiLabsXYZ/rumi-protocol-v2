/**
 * The Solana clusters the bridge knows, keyed by genesis hash (the profile's network identity).
 * The genesis hash a bridge instance reports decides which cluster it is: a hash that is not in this table
 * is an unsupported network and the destination stays unavailable.
 */

export type SolanaRole = 'mainnet' | 'testnet' | 'devnet';

export interface SolanaNetworkInfo {
  /** The per-network destination key: the profile's network id. */
  networkId: string;
  name: string;
  role: SolanaRole;
  /** Base58 genesis hash. */
  genesisHash: string;
  /** Wallet Standard chain identifier. */
  chain: `solana:${string}`;
  /** Query suffix for explorer links. */
  explorerSuffix: string;
  /** What the user is told to expect from finality. */
  finalityNote: string;
}

const FINALITY = 'about 15 seconds';

export const SOLANA_MAINNET: SolanaNetworkInfo = {
  networkId: 'solana-mainnet',
  name: 'Solana',
  role: 'mainnet',
  genesisHash: '5eykt4UsFv8P8NJdTREpY1vzqKqZKvdpKuc147dw2N9d',
  chain: 'solana:mainnet',
  explorerSuffix: '',
  finalityNote: FINALITY,
};
export const SOLANA_TESTNET: SolanaNetworkInfo = {
  networkId: 'solana-testnet',
  name: 'Solana testnet',
  role: 'testnet',
  genesisHash: '4uhcVJyU9pJkvQyS88uRDiswHXSCkY3zQawwpjk2NsNY',
  chain: 'solana:testnet',
  explorerSuffix: '?cluster=testnet',
  finalityNote: FINALITY,
};
export const SOLANA_DEVNET: SolanaNetworkInfo = {
  networkId: 'solana-devnet',
  name: 'Solana devnet',
  role: 'devnet',
  genesisHash: 'EtWTRABZaYq6iMfeYKouRu166VU2xqa1wcaWoxPkrZBG',
  chain: 'solana:devnet',
  explorerSuffix: '?cluster=devnet',
  finalityNote: FINALITY,
};

export const SOLANA_NETWORKS: readonly SolanaNetworkInfo[] = [SOLANA_MAINNET, SOLANA_TESTNET, SOLANA_DEVNET];

export function solanaNetworkByGenesis(genesisHash: string): SolanaNetworkInfo | null {
  return SOLANA_NETWORKS.find((n) => n.genesisHash === genesisHash) ?? null;
}

export function solanaNetworkById(networkId: string): SolanaNetworkInfo | null {
  return SOLANA_NETWORKS.find((n) => n.networkId === networkId) ?? null;
}

export function explorerTxUrl(network: SolanaNetworkInfo, signature: string): string {
  return `https://explorer.solana.com/tx/${signature}${network.explorerSuffix}`;
}
