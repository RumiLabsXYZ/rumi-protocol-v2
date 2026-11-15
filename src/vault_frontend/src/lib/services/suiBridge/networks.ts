/**
 * The Sui networks the bridge knows, keyed by the full 32-byte genesis checkpoint digest (what the Gate's
 * `chain_identifier` holds and what `sui_getChainIdentifier` reports in full). A digest that is not in this table is an
 * unsupported network and the destination stays unavailable: nothing is guessed from a four-byte prefix.
 *
 * The two digests are the base58 constants of `sui-types` `digests.rs` (mainnet-v1.82.3) that
 * `src/rumi_icusd_sui_bridge/src/ids.rs` decodes (`MAINNET_CHAIN_IDENTIFIER`, `TESTNET_CHAIN_IDENTIFIER`); the hex form
 * is what `get_config` returns. A spec decodes each base58 string and compares it with its hex.
 */

export type SuiRole = 'mainnet' | 'testnet';

export interface SuiNetworkInfo {
  /** The per-network destination key: the profile's network id. */
  networkId: string;
  name: string;
  role: SuiRole;
  /** The full chain identifier, lowercase hex (64 digits): what `get_config.chain_identifier` reports. */
  chainIdentifier: string;
  /** The same digest in base58: what a Sui node reports as the chain identifier. */
  chainIdentifierBase58: string;
  /** Wallet Standard chain identifier. */
  chain: `sui:${string}`;
  /** The explorer's path segment for this network. */
  explorerPath: string;
  /** What the user is told to expect from finality. */
  finalityNote: string;
}

const FINALITY = 'a few seconds, once Sui includes the transaction in a checkpoint';

export const SUI_MAINNET: SuiNetworkInfo = {
  networkId: 'sui-mainnet',
  name: 'Sui',
  role: 'mainnet',
  chainIdentifier: '35834a8ac17ca48fb14ac8f99c17c98747e95dd07294ae41a46b382246a4499b',
  chainIdentifierBase58: '4btiuiMPvEENsttpZC7CZ53DruC3MAgfznDbASZ7DR6S',
  chain: 'sui:mainnet',
  explorerPath: 'mainnet',
  finalityNote: FINALITY,
};
export const SUI_TESTNET: SuiNetworkInfo = {
  networkId: 'sui-testnet',
  name: 'Sui testnet',
  role: 'testnet',
  chainIdentifier: '4c78adacf2a2f5ad80f27ed7d54aa69d3a78f1ca67fdef9ecf5754f5b8bb77b0',
  chainIdentifierBase58: '69WiPg3DAQiwdxfncX6wYQ2siKwAe6L9BZthQea3JNMD',
  chain: 'sui:testnet',
  explorerPath: 'testnet',
  finalityNote: FINALITY,
};

export const SUI_NETWORKS: readonly SuiNetworkInfo[] = [SUI_MAINNET, SUI_TESTNET];

export function suiNetworkByChainIdentifier(hex: string): SuiNetworkInfo | null {
  return SUI_NETWORKS.find((n) => n.chainIdentifier === hex) ?? null;
}

export function suiNetworkById(networkId: string): SuiNetworkInfo | null {
  return SUI_NETWORKS.find((n) => n.networkId === networkId) ?? null;
}

export function explorerTxUrl(network: SuiNetworkInfo, digest: string): string {
  return `https://suiscan.xyz/${network.explorerPath}/tx/${digest}`;
}
