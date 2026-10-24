import { describe, expect, it, vi } from 'vitest';
import { get, type Readable } from 'svelte/store';
import { BASE_LOCAL, BASE_MAINNET, BASE_SEPOLIA } from './chains';
import {
  ChainSwitchError,
  EVM_DISCONNECT_KEY,
  EVM_WALLET_KEY,
  createEvmWalletStore,
  describeWalletError,
  discoverWallets,
  estimateGas,
  ethCall,
  getAccounts,
  getChainId,
  getEthBalance,
  getReceipt,
  isRequestPending,
  isUnrecognizedChain,
  isUserRejection,
  requestAccounts,
  sendTransaction,
  switchToChain,
  watchProvider,
} from './evmWallet';
import { EIP55_VECTORS, ORACLE_OR_ZERO, createFakeProvider, createMemoryStorage, flipCase, walletError } from './testSupport';

/** The Keccak-256 the wallet module sees. null mirrors the shipped state (nothing configured). */
const keccakMock = vi.hoisted(() => ({ fn: null as null | ((data: Uint8Array) => Uint8Array) }));
vi.mock('./keccak', () => ({
  get keccak256() {
    return keccakMock.fn;
  },
}));

const ACCOUNT = '0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa';
const OTHER = '0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb';
const HASH = `0x${'12'.repeat(32)}`;

async function flush() {
  for (let i = 0; i < 6; i++) await Promise.resolve();
}

const state = <T>(store: Readable<T>): T => get(store);

describe('error classification', () => {
  it('recognises user rejection in the usual shapes', () => {
    expect(isUserRejection(walletError(4001))).toBe(true);
    expect(isUserRejection({ code: 'ACTION_REJECTED' })).toBe(true);
    expect(isUserRejection({ error: { code: 4001 } })).toBe(true);
    expect(isUserRejection({ info: { error: { code: 4001 } } })).toBe(true);
    expect(isUserRejection(walletError(-32603))).toBe(false);
    expect(isUserRejection(new Error('boom'))).toBe(false);
    expect(isUserRejection(null)).toBe(false);
  });

  it('recognises an unrecognised chain and a pending request', () => {
    expect(isUnrecognizedChain(walletError(4902))).toBe(true);
    expect(isUnrecognizedChain(new Error('Unrecognized chain ID "0x2105". Try adding the chain'))).toBe(true);
    expect(isUnrecognizedChain(walletError(4001))).toBe(false);
    expect(isRequestPending(walletError(-32002))).toBe(true);
  });

  it('describes wallet errors in plain language', () => {
    expect(describeWalletError(walletError(4001))).toMatch(/declined/);
    expect(describeWalletError(walletError(-32002))).toMatch(/already has a request open/);
    expect(describeWalletError(new Error('x'.repeat(500)), 'fallback')).toBe('fallback');
    expect(describeWalletError(new Error('plain'))).toBe('plain');
  });
});

describe('typed requests', () => {
  it('reads chain id, accounts and call results through the provider', async () => {
    const provider = createFakeProvider({
      chainId: 84532,
      handlers: { eth_call: () => `0x${'0'.repeat(63)}1` },
    });
    expect(await getChainId(provider)).toBe(84532);
    expect(await getAccounts(provider)).toEqual([ACCOUNT]);
    expect(await requestAccounts(provider)).toEqual([ACCOUNT]);
    expect(await ethCall(provider, OTHER, '0x70a08231', ACCOUNT)).toBe(`0x${'0'.repeat(63)}1`);
    expect(provider.callsTo('eth_call')[0].params).toEqual([{ to: OTHER, data: '0x70a08231', from: ACCOUNT }, 'latest']);
  });

  it('lower-cases accounts and drops malformed entries', async () => {
    const provider = createFakeProvider({
      handlers: { eth_accounts: () => ['0xAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA', 'nope', 12] },
    });
    expect(await getAccounts(provider)).toEqual([ACCOUNT]);
  });

  it('drops a wallet account whose mixed-case checksum is verifiably wrong, and keeps valid and unchecksummed ones', async () => {
    keccakMock.fn = ORACLE_OR_ZERO;
    try {
      const bad = flipCase(EIP55_VECTORS[1], 1);
      const provider = createFakeProvider({
        handlers: { eth_accounts: () => [EIP55_VECTORS[0], bad, EIP55_VECTORS[2].toLowerCase(), `0x${EIP55_VECTORS[3].slice(2).toUpperCase()}`] },
      });
      expect(await getAccounts(provider)).toEqual([
        EIP55_VECTORS[0].toLowerCase(),
        EIP55_VECTORS[2].toLowerCase(),
        EIP55_VECTORS[3].toLowerCase(),
      ]);
    } finally {
      keccakMock.fn = null;
    }
  });

  it('keeps a mixed-case wallet account it cannot verify (no Keccak-256 configured)', async () => {
    keccakMock.fn = null;
    const provider = createFakeProvider({ handlers: { eth_accounts: () => [EIP55_VECTORS[0]] } });
    expect(await getAccounts(provider)).toEqual([EIP55_VECTORS[0].toLowerCase()]);
  });

  it('rejects garbage answers instead of guessing', async () => {
    const provider = createFakeProvider({
      handlers: { eth_chainId: () => 'abc', eth_call: () => 5, eth_getBalance: () => 'oops', eth_estimateGas: () => null },
    });
    await expect(getChainId(provider)).rejects.toThrow(/invalid chain id/);
    await expect(ethCall(provider, OTHER, '0x')).rejects.toThrow(/invalid call result/);
    await expect(getEthBalance(provider, ACCOUNT)).rejects.toThrow(/invalid ETH balance/);
    await expect(estimateGas(provider, { from: ACCOUNT, to: OTHER, data: '0x' })).rejects.toThrow(/invalid gas estimate/);
  });

  it('parses receipts and treats a missing block as not yet included', async () => {
    const provider = createFakeProvider({
      handlers: {
        eth_getTransactionReceipt: (params) =>
          (params as string[])[0] === HASH
            ? { transactionHash: HASH, blockNumber: '0x10', status: '0x1' }
            : (params as string[])[0] === `0x${'34'.repeat(32)}`
              ? { transactionHash: HASH, blockNumber: '0x11', status: '0x0' }
              : (params as string[])[0] === `0x${'56'.repeat(32)}`
                ? { blockNumber: null }
                : null,
      },
    });
    expect(await getReceipt(provider, HASH)).toEqual({ transactionHash: HASH, blockNumber: 16n, success: true });
    expect(await getReceipt(provider, `0x${'34'.repeat(32)}`)).toMatchObject({ blockNumber: 17n, success: false });
    expect(await getReceipt(provider, `0x${'56'.repeat(32)}`)).toBeNull();
    expect(await getReceipt(provider, `0x${'78'.repeat(32)}`)).toBeNull();
    await expect(getReceipt(provider, '0x12')).rejects.toThrow(/Invalid transaction hash/);
  });

  it('sends a zero value transaction and validates the returned hash', async () => {
    const provider = createFakeProvider({ handlers: { eth_sendTransaction: () => HASH.toUpperCase().replace('0X', '0x') } });
    expect(await sendTransaction(provider, { from: ACCOUNT, to: OTHER, data: '0xabcd' })).toBe(HASH);
    expect(provider.callsTo('eth_sendTransaction')[0].params).toEqual([{ from: ACCOUNT, to: OTHER, data: '0xabcd', value: '0x0' }]);

    const bad = createFakeProvider({ handlers: { eth_sendTransaction: () => 'pending' } });
    await expect(sendTransaction(bad, { from: ACCOUNT, to: OTHER, data: '0x' })).rejects.toThrow(/valid transaction hash/);
  });
});

describe('switchToChain', () => {
  it('switches directly when the wallet knows the chain', async () => {
    const provider = createFakeProvider({ chainId: 1 });
    await switchToChain(provider, BASE_MAINNET);
    expect(provider.callsTo('wallet_switchEthereumChain')[0].params).toEqual([{ chainId: '0x2105' }]);
    expect(provider.callsTo('wallet_addEthereumChain')).toHaveLength(0);
    expect(provider.state.chainId).toBe(8453);
  });

  it('adds the chain on 4902 and then verifies the switch', async () => {
    const provider = createFakeProvider({
      chainId: 1,
      handlers: {
        wallet_switchEthereumChain: () => {
          throw walletError(4902, 'Unrecognized chain ID');
        },
        wallet_addEthereumChain: (params) => {
          provider.state.chainId = Number(BigInt((params as Array<{ chainId: string }>)[0].chainId));
          return null;
        },
      },
    });
    await switchToChain(provider, BASE_SEPOLIA);
    expect(provider.callsTo('wallet_addEthereumChain')[0].params).toEqual([
      {
        chainId: '0x14a34',
        chainName: 'Base Sepolia',
        nativeCurrency: { name: 'Ether', symbol: 'ETH', decimals: 18 },
        rpcUrls: ['https://sepolia.base.org'],
        blockExplorerUrls: ['https://sepolia.basescan.org'],
      },
    ]);
    expect(provider.state.chainId).toBe(84532);
  });

  it('adds the chain when the wallet only says "Unrecognized chain"', async () => {
    const provider = createFakeProvider({
      chainId: 1,
      handlers: {
        wallet_switchEthereumChain: () => {
          throw new Error('Unrecognized chain ID "0x7a69". Try adding the chain using wallet_addEthereumChain first.');
        },
        wallet_addEthereumChain: () => {
          provider.state.chainId = 31337;
          return null;
        },
      },
    });
    await switchToChain(provider, BASE_LOCAL);
    expect(provider.callsTo('wallet_addEthereumChain')).toHaveLength(1);
  });

  it('does not try to add a chain after the user rejects the switch', async () => {
    const provider = createFakeProvider({
      chainId: 1,
      handlers: {
        wallet_switchEthereumChain: () => {
          throw walletError(4001);
        },
      },
    });
    await expect(switchToChain(provider, BASE_MAINNET)).rejects.toMatchObject({ code: 4001 });
    expect(provider.callsTo('wallet_addEthereumChain')).toHaveLength(0);
  });

  it('fails when the wallet claims success but stays on another chain', async () => {
    const provider = createFakeProvider({ chainId: 1, handlers: { wallet_switchEthereumChain: () => null } });
    await expect(switchToChain(provider, BASE_MAINNET)).rejects.toBeInstanceOf(ChainSwitchError);
  });
});

describe('watchProvider', () => {
  it('forwards normalised events and removes exactly its own listeners', () => {
    const provider = createFakeProvider();
    const accountsChanged = vi.fn();
    const chainChanged = vi.fn();
    const disconnect = vi.fn();
    const other = vi.fn();
    provider.on!('accountsChanged', other);
    const stop = watchProvider(provider, { accountsChanged, chainChanged, disconnect });
    provider.emit('accountsChanged', ['0xAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA']);
    provider.emit('chainChanged', '0x2105');
    provider.emit('chainChanged', 'junk');
    provider.emit('disconnect', {});
    expect(accountsChanged).toHaveBeenCalledWith([ACCOUNT]);
    expect(chainChanged).toHaveBeenNthCalledWith(1, 8453);
    expect(chainChanged).toHaveBeenNthCalledWith(2, null);
    expect(disconnect).toHaveBeenCalledTimes(1);
    stop();
    expect(provider.listenerCount()).toBe(1);
    provider.emit('accountsChanged', [OTHER]);
    expect(accountsChanged).toHaveBeenCalledTimes(1);
    expect(other).toHaveBeenCalledTimes(2); // an unrelated listener is untouched
  });

  it('is a no-op for a provider without event support', () => {
    const stop = watchProvider({ request: async () => null }, {});
    expect(() => stop()).not.toThrow();
  });
});

describe('discoverWallets', () => {
  function announcingWindow(announcements: Array<{ name: string; rdns: string; provider: any }>, ethereum?: unknown) {
    const target = new EventTarget();
    (target as any).ethereum = ethereum;
    target.addEventListener('eip6963:requestProvider', () => {
      for (const entry of announcements) {
        target.dispatchEvent(
          new CustomEvent('eip6963:announceProvider', {
            detail: { info: { uuid: entry.rdns, name: entry.name, rdns: entry.rdns }, provider: entry.provider },
          }),
        );
      }
    });
    return target as unknown as Window & { ethereum?: unknown };
  }

  it('collects EIP-6963 announcements and does not duplicate window.ethereum', async () => {
    const a = createFakeProvider();
    const b = createFakeProvider();
    const win = announcingWindow(
      [
        { name: 'Alpha', rdns: 'io.alpha', provider: a },
        { name: 'Beta', rdns: 'io.beta', provider: b },
        { name: 'Alpha again', rdns: 'io.alpha', provider: a },
      ],
      a,
    );
    const wallets = await discoverWallets({ win, timeoutMs: 0 });
    expect(wallets.map((w) => w.id)).toEqual(['io.alpha', 'io.beta']);
  });

  it('falls back to window.ethereum when nothing announces', async () => {
    const injected = createFakeProvider();
    const wallets = await discoverWallets({ win: announcingWindow([], injected), timeoutMs: 0 });
    expect(wallets).toHaveLength(1);
    expect(wallets[0]).toMatchObject({ id: 'injected', provider: injected });
  });

  it('returns nothing when there is no wallet', async () => {
    expect(await discoverWallets({ win: announcingWindow([]), timeoutMs: 0 })).toEqual([]);
  });
});

describe('createEvmWalletStore', () => {
  function windowWith(provider: any) {
    const target = new EventTarget();
    (target as any).ethereum = provider;
    return target as unknown as Window & { ethereum?: unknown };
  }

  it('reports unavailable when no wallet exists', async () => {
    const store = createEvmWalletStore({ win: windowWith(undefined), storage: createMemoryStorage(), discoveryTimeoutMs: 0 });
    await store.init();
    expect(state(store).status).toBe('unavailable');
    await store.connect();
    expect(state(store).error).toMatch(/No Base-compatible wallet/);
    store.destroy();
  });

  it('silently restores an authorised account on init without prompting', async () => {
    const provider = createFakeProvider({ chainId: 84532 });
    const store = createEvmWalletStore({ win: windowWith(provider), storage: createMemoryStorage(), discoveryTimeoutMs: 0 });
    await store.init();
    expect(state(store)).toMatchObject({ status: 'connected', account: ACCOUNT, chainId: 84532 });
    expect(provider.callsTo('eth_requestAccounts')).toHaveLength(0);
    store.destroy();
  });

  it('does not restore after an explicit disconnect and remembers that choice', async () => {
    const provider = createFakeProvider();
    const storage = createMemoryStorage();
    const store = createEvmWalletStore({ win: windowWith(provider), storage, discoveryTimeoutMs: 0 });
    await store.init();
    store.disconnect();
    expect(storage.map.get(EVM_DISCONNECT_KEY)).toBe('true');
    expect(provider.listenerCount()).toBe(0);
    expect(state(store)).toMatchObject({ status: 'disconnected', account: null });
    store.destroy();

    const again = createEvmWalletStore({ win: windowWith(provider), storage, discoveryTimeoutMs: 0 });
    provider.calls.length = 0;
    await again.init();
    expect(provider.callsTo('eth_accounts')).toHaveLength(0);
    expect(state(again).account).toBeNull();
    await again.connect();
    expect(provider.callsTo('eth_requestAccounts')).toHaveLength(1);
    expect(storage.map.has(EVM_DISCONNECT_KEY)).toBe(false);
    expect(storage.map.get(EVM_WALLET_KEY)).toBe('injected');
    expect(state(again)).toMatchObject({ status: 'connected', account: ACCOUNT });
    again.destroy();
  });

  it('follows accountsChanged and chainChanged, and ignores them after a manual disconnect', async () => {
    const provider = createFakeProvider({ chainId: 8453 });
    const store = createEvmWalletStore({ win: windowWith(provider), storage: createMemoryStorage(), discoveryTimeoutMs: 0 });
    await store.init();
    provider.emit('accountsChanged', [OTHER]);
    expect(state(store).account).toBe(OTHER);
    provider.emit('chainChanged', '0x1');
    expect(state(store).chainId).toBe(1);
    provider.emit('accountsChanged', []);
    expect(state(store)).toMatchObject({ status: 'disconnected', account: null });
    store.destroy();
  });

  it('removes every provider listener on destroy', async () => {
    const provider = createFakeProvider();
    const store = createEvmWalletStore({ win: windowWith(provider), storage: createMemoryStorage(), discoveryTimeoutMs: 0 });
    await store.init();
    expect(provider.listenerCount()).toBe(3);
    store.destroy();
    expect(provider.listenerCount()).toBe(0);
    provider.emit('accountsChanged', [OTHER]);
    expect(state(store).account).toBeNull();
  });

  it('reports a declined connection without leaving a half-connected state', async () => {
    const provider = createFakeProvider({
      handlers: {
        eth_accounts: () => [],
        eth_requestAccounts: () => {
          throw walletError(4001);
        },
      },
    });
    const store = createEvmWalletStore({ win: windowWith(provider), storage: createMemoryStorage(), discoveryTimeoutMs: 0 });
    await store.init();
    await store.connect();
    expect(state(store)).toMatchObject({ status: 'disconnected', account: null });
    expect(state(store).error).toMatch(/declined/);
    store.destroy();
  });

  it('switches network through the provider and reports failures', async () => {
    const provider = createFakeProvider({ chainId: 1 });
    const store = createEvmWalletStore({ win: windowWith(provider), storage: createMemoryStorage(), discoveryTimeoutMs: 0 });
    await store.init();
    await store.switchNetwork(BASE_MAINNET);
    expect(state(store).chainId).toBe(8453);

    provider.handlers.wallet_switchEthereumChain = () => {
      throw walletError(4001);
    };
    await store.switchNetwork(BASE_SEPOLIA);
    expect(state(store).error).toMatch(/declined/);
    expect(state(store).chainId).toBe(8453);
    store.destroy();
    await flush();
  });

  it('asks the user to choose when several wallets are announced', async () => {
    const a = createFakeProvider();
    const b = createFakeProvider({ account: OTHER });
    const target = new EventTarget();
    target.addEventListener('eip6963:requestProvider', () => {
      for (const [rdns, provider] of [['io.a', a], ['io.b', b]] as const) {
        target.dispatchEvent(new CustomEvent('eip6963:announceProvider', { detail: { info: { uuid: rdns, name: rdns, rdns }, provider } }));
      }
    });
    const store = createEvmWalletStore({ win: target as unknown as Window, storage: createMemoryStorage(), discoveryTimeoutMs: 0 });
    await store.init();
    expect(state(store).wallets).toHaveLength(2);
    expect(state(store).status).toBe('disconnected');
    await store.connect();
    expect(state(store).error).toMatch(/Choose a wallet/);
    await store.connect('io.b');
    expect(state(store)).toMatchObject({ status: 'connected', account: OTHER, walletId: 'io.b' });
    store.destroy();
  });
});
