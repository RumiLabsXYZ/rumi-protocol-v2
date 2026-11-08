import { describe, expect, it } from 'vitest';
import {
  ARBITRUM_ONE,
  ARBITRUM_SEPOLIA,
  BASE_LOCAL,
  BASE_MAINNET,
  BASE_SEPOLIA,
  ETHEREUM_MAINNET,
  ETHEREUM_SEPOLIA,
  EVM_DESTINATION_CHAINS,
  HYPEREVM_MAINNET,
  HYPEREVM_TESTNET,
  MONAD_MAINNET,
  MONAD_TESTNET,
  ROBINHOOD_CHAIN,
  ROBINHOOD_CHAIN_TESTNET,
  SUPPORTED_CHAINS,
  addChainParams,
  baseNetworkId,
  chainIdToHex,
  explorerTxUrl,
  getChainConfig,
  getEvmDestinationChain,
  parseChainId,
  resolveBridgeConfig,
} from './chains';
import { makeBridgeInfo } from './testSupport';

/** En and em dashes are not allowed in UI copy. Built from code points so this file holds none itself. */
const DASHES = new RegExp(`[${String.fromCharCode(0x2013, 0x2014)}]`);

describe('EVM destination chain rows', () => {
  it('has the five chains with their mainnet and testnet ids, names and gas symbols', () => {
    expect(
      EVM_DESTINATION_CHAINS.map((chain) => [chain.profileId, chain.networkId, chain.chainId, chain.name, chain.nativeCurrency.symbol, chain.testnet]),
    ).toEqual([
      ['ethereum', 'ethereum-mainnet', 1, 'Ethereum', 'ETH', false],
      ['ethereum', 'ethereum-sepolia', 11155111, 'Ethereum Sepolia', 'ETH', true],
      ['arbitrum', 'arbitrum-mainnet', 42161, 'Arbitrum One', 'ETH', false],
      ['arbitrum', 'arbitrum-sepolia', 421614, 'Arbitrum Sepolia', 'ETH', true],
      ['robinhood', 'robinhood-mainnet', 4663, 'Robinhood Chain', 'ETH', false],
      ['robinhood', 'robinhood-testnet', 46630, 'Robinhood Chain testnet', 'ETH', true],
      ['hyperevm', 'hyperevm-mainnet', 999, 'HyperEVM', 'HYPE', false],
      ['hyperevm', 'hyperevm-testnet', 998, 'HyperEVM testnet', 'HYPE', true],
      ['monad', 'monad-mainnet', 143, 'Monad', 'MON', false],
      ['monad', 'monad-testnet', 10143, 'Monad testnet', 'MON', true],
    ]);
    expect(EVM_DESTINATION_CHAINS).toEqual([
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
    ]);
  });

  it('gives every network its own key, so a mainnet and a testnet never share one', () => {
    const keys = EVM_DESTINATION_CHAINS.map((chain) => chain.networkId);
    expect(new Set(keys).size).toBe(keys.length);
    expect(baseNetworkId(8453)).toBe('base-mainnet');
    expect(baseNetworkId(84532n)).toBe('base-sepolia');
    expect(baseNetworkId(31337)).toBe('base-local');
    expect(baseNetworkId(1)).toBeNull();
    expect(baseNetworkId(null)).toBeNull();
    expect(keys).not.toContain('base-mainnet');
  });

  it('keeps the hex form consistent with the numeric id, and 18 decimals everywhere', () => {
    for (const chain of EVM_DESTINATION_CHAINS) {
      expect(chain.chainIdHex, chain.name).toBe(chainIdToHex(chain.chainId));
      expect(parseChainId(chain.chainIdHex)).toBe(chain.chainId);
      expect(chain.nativeCurrency.decimals).toBe(18);
      expect(chain.local).toBe(false);
    }
  });

  it('has finality wording for each, in plain words and without an em dash', () => {
    for (const chain of EVM_DESTINATION_CHAINS) {
      expect(chain.finality.startsWith('after '), chain.name).toBe(true);
      expect(chain.finality).not.toMatch(DASHES);
    }
    expect(ETHEREUM_MAINNET.finality).toBe('after Ethereum finalization, about 13 minutes');
    expect(ARBITRUM_ONE.finality).toBe('after the Arbitrum batch is finalized on Ethereum, about 13 minutes');
    expect(ROBINHOOD_CHAIN.finality).toBe('after Robinhood Chain finality (timing not yet confirmed)');
    expect(HYPEREVM_MAINNET.finality).toBe('after HyperEVM finality (timing not yet confirmed)');
    expect(MONAD_MAINNET.finality).toBe('after Monad Verified state (Finalized plus 3 blocks), within seconds');
  });

  it('looks a row up by number or bigint', () => {
    expect(getEvmDestinationChain(143)).toBe(MONAD_MAINNET);
    expect(getEvmDestinationChain(10143n)).toBe(MONAD_TESTNET);
    expect(getEvmDestinationChain(8453)).toBeNull();
    expect(getEvmDestinationChain(null)).toBeNull();
  });

  it('leaves a value no primary source gave empty instead of guessing, and never sends an empty RPC to a wallet', () => {
    expect(ETHEREUM_MAINNET.rpcUrl).toBe('');
    expect(addChainParams(ETHEREUM_MAINNET).rpcUrls).toEqual([]);
    expect(addChainParams(ROBINHOOD_CHAIN).rpcUrls).toEqual(['https://rpc.mainnet.chain.robinhood.com']);
    expect(explorerTxUrl(ETHEREUM_MAINNET, '0xabc')).toBeNull();
    expect(explorerTxUrl(ROBINHOOD_CHAIN, '0xabc')).toBe('https://robinhoodchain.blockscout.com/tx/0xabc');
  });
});

describe('Base is untouched', () => {
  it('the Base table and the supported-chain lookup do not know the other chains', () => {
    expect(SUPPORTED_CHAINS).toEqual([BASE_MAINNET, BASE_SEPOLIA, BASE_LOCAL]);
    for (const chain of EVM_DESTINATION_CHAINS) expect(getChainConfig(chain.chainId), chain.name).toBeNull();
  });

  it('a canister that reports another EVM chain to the Base page is still unsupported', () => {
    for (const chain of EVM_DESTINATION_CHAINS) {
      expect(resolveBridgeConfig(makeBridgeInfo({ chain_id: BigInt(chain.chainId) })), chain.name).toMatchObject({
        ok: false,
        reason: 'unsupported_chain',
      });
    }
  });

  it('Base wallet parameters are exactly what they were', () => {
    expect(addChainParams(BASE_MAINNET)).toEqual({
      chainId: '0x2105',
      chainName: 'Base',
      nativeCurrency: { name: 'Ether', symbol: 'ETH', decimals: 18 },
      rpcUrls: ['https://mainnet.base.org'],
      blockExplorerUrls: ['https://basescan.org'],
    });
  });
});
