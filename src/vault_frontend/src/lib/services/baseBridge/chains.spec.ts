import { describe, expect, it } from 'vitest';
import { Principal } from '@dfinity/principal';
import type { BridgeInfo } from '$declarations/rumi_icusd_bridge/rumi_icusd_bridge.did';
import {
  BASE_LOCAL,
  BASE_MAINNET,
  BASE_SEPOLIA,
  SUPPORTED_CHAINS,
  addChainParams,
  chainIdToHex,
  explorerAddressUrl,
  explorerTxUrl,
  getChainConfig,
  parseChainId,
  resolveBridgeConfig,
} from './chains';

const LEDGER = Principal.fromText('t6bor-paaaa-aaaap-qrd5q-cai');
const CONTRACT = '0xAbCdEf0123456789aBcDeF0123456789AbCdEf01';

function info(overrides: Partial<BridgeInfo> = {}): BridgeInfo {
  return {
    chain_id: 8453n,
    contract: [
      {
        address: CONTRACT,
        bound_at_ns: 1n,
        start_block: 10n,
        supply_cap_e8s: 10n ** 15n,
        mint_window_limit_e8s: 10n ** 14n,
        mint_window_seconds: 86_400n,
        min_bridge_out_e8s: 100_000_000n,
      },
    ],
    limits: {
      min_deposit_e8s: 100_000_000n,
      max_deposit_e8s: 50_000_000_000n,
      max_outstanding_e8s: 0n,
      max_active_per_caller: 3,
      max_active_global: 100,
      window_seconds: 86_400n,
      mint_window_limit_e8s: 0n,
      release_window_limit_e8s: 0n,
    },
    fees: { deposit_fee_e8s: 10_000n, release_fee_e8s: 20_000n },
    ledger_fee_e8s: [100_000n],
    icusd_ledger: LEDGER,
    pauses: { mints: false, observer: false, releases: false, deposits: false },
    halted: [],
    version: 'rumi-base-icusd-bridge-v1',
    ...overrides,
  } as unknown as BridgeInfo;
}

describe('chain table', () => {
  it('describes Base, Base Sepolia and local anvil', () => {
    expect(BASE_MAINNET).toMatchObject({
      chainId: 8453,
      chainIdHex: '0x2105',
      name: 'Base',
      rpcUrl: 'https://mainnet.base.org',
      explorerUrl: 'https://basescan.org',
    });
    expect(BASE_SEPOLIA).toMatchObject({
      chainId: 84532,
      chainIdHex: '0x14a34',
      name: 'Base Sepolia',
      rpcUrl: 'https://sepolia.base.org',
      explorerUrl: 'https://sepolia.basescan.org',
    });
    expect(BASE_LOCAL).toMatchObject({ chainId: 31337, chainIdHex: '0x7a69', local: true });
  });

  it('keeps the hex form consistent with the numeric id for every chain', () => {
    for (const chain of SUPPORTED_CHAINS) {
      expect(chainIdToHex(chain.chainId)).toBe(chain.chainIdHex);
      expect(parseChainId(chain.chainIdHex)).toBe(chain.chainId);
    }
  });

  it('looks chains up by number or bigint and returns null for strangers', () => {
    expect(getChainConfig(8453)).toBe(BASE_MAINNET);
    expect(getChainConfig(84532n)).toBe(BASE_SEPOLIA);
    expect(getChainConfig(1)).toBeNull();
    expect(getChainConfig(null)).toBeNull();
  });

  it('parses wallet chain id values defensively', () => {
    expect(parseChainId('0x2105')).toBe(8453);
    expect(parseChainId('0X2105')).toBeNull(); // wallets report lower-case 0x
    expect(parseChainId(8453)).toBe(8453);
    expect(parseChainId('8453')).toBe(8453);
    expect(parseChainId('0x')).toBeNull();
    expect(parseChainId('nope')).toBeNull();
    expect(parseChainId(-1)).toBeNull();
    expect(parseChainId(undefined)).toBeNull();
  });

  it('builds explorer links, or none for a chain without an explorer', () => {
    expect(explorerTxUrl(BASE_MAINNET, '0xabc')).toBe('https://basescan.org/tx/0xabc');
    expect(explorerAddressUrl(BASE_SEPOLIA, '0xdef')).toBe('https://sepolia.basescan.org/address/0xdef');
    expect(explorerTxUrl(BASE_LOCAL, '0xabc')).toBeNull();
  });

  it('builds wallet_addEthereumChain params and omits an empty explorer list', () => {
    expect(addChainParams(BASE_MAINNET)).toEqual({
      chainId: '0x2105',
      chainName: 'Base',
      nativeCurrency: { name: 'Ether', symbol: 'ETH', decimals: 18 },
      rpcUrls: ['https://mainnet.base.org'],
      blockExplorerUrls: ['https://basescan.org'],
    });
    expect(addChainParams(BASE_LOCAL)).not.toHaveProperty('blockExplorerUrls');
  });
});

describe('resolveBridgeConfig', () => {
  it('takes the chain and contract from get_bridge_info, never from the build', () => {
    const mainnet = resolveBridgeConfig(info());
    expect(mainnet).toMatchObject({ ok: true });
    if (!mainnet.ok) throw new Error('unreachable');
    expect(mainnet.config.chain).toBe(BASE_MAINNET);
    expect(mainnet.config.contract).toBe(CONTRACT.toLowerCase());
    expect(mainnet.config.minBridgeOutE8s).toBe(100_000_000n);
    expect(mainnet.config.depositFeeE8s).toBe(10_000n);
    expect(mainnet.config.releaseFeeE8s).toBe(20_000n);
    expect(mainnet.config.ledgerFeeE8s).toBe(100_000n);
    expect(mainnet.config.icusdLedger).toBe('t6bor-paaaa-aaaap-qrd5q-cai');

    const sepolia = resolveBridgeConfig(info({ chain_id: 84532n }));
    expect(sepolia.ok && sepolia.config.chain).toBe(BASE_SEPOLIA);
    const local = resolveBridgeConfig(info({ chain_id: 31337n }));
    expect(local.ok && local.config.chain).toBe(BASE_LOCAL);
  });

  it('reports an unsupported chain', () => {
    expect(resolveBridgeConfig(info({ chain_id: 1n }))).toMatchObject({ ok: false, reason: 'unsupported_chain' });
  });

  it('reports an unbound contract and a malformed one', () => {
    expect(resolveBridgeConfig(info({ contract: [] }))).toMatchObject({ ok: false, reason: 'contract_not_bound' });
    const bad = info();
    bad.contract = [{ ...bad.contract[0]!, address: '0x1234' }];
    expect(resolveBridgeConfig(bad)).toMatchObject({ ok: false, reason: 'invalid_contract' });
    const zero = info();
    zero.contract = [{ ...zero.contract[0]!, address: '0x0000000000000000000000000000000000000000' }];
    expect(resolveBridgeConfig(zero)).toMatchObject({ ok: false, reason: 'invalid_contract' });
  });

  it('carries pauses, halt reason and a missing ledger fee', () => {
    const result = resolveBridgeConfig(
      info({ ledger_fee_e8s: [], halted: ['coverage mismatch'], pauses: { mints: true, observer: false, releases: true, deposits: true } }),
    );
    if (!result.ok) throw new Error('unreachable');
    expect(result.config.ledgerFeeE8s).toBeNull();
    expect(result.config.halted).toBe('coverage mismatch');
    expect(result.config).toMatchObject({ depositsPaused: true, releasesPaused: true, mintsPaused: true });
  });
});
