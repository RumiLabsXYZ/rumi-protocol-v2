/**
 * Minimal EIP-1193 wallet layer for the Base bridge.
 *
 * Every Base read and write goes through the user's connected wallet provider
 * (`eth_call`, `eth_getTransactionReceipt`, `eth_getBalance`, `eth_estimateGas`,
 * `eth_chainId`). This keeps the page free of any public RPC host, so the CSP
 * `connect-src` allow list does not change. No EVM library is used.
 *
 * Wallet discovery uses EIP-6963 when wallets announce themselves and falls
 * back to `window.ethereum`.
 */

import { writable, type Readable } from 'svelte/store';
import { addChainParams, parseChainId, type BaseChainConfig } from './chains';
import { isAddress, isBytes32 } from './abi';
import { checksumStatus } from './eip55';

export interface Eip1193Provider {
  request(args: { method: string; params?: unknown[] | Record<string, unknown> }): Promise<unknown>;
  on?: (event: string, listener: (...args: any[]) => void) => void;
  removeListener?: (event: string, listener: (...args: any[]) => void) => void;
}

export interface DiscoveredWallet {
  /** Stable id: the EIP-6963 `rdns` when announced, else `injected`. */
  id: string;
  name: string;
  icon?: string;
  provider: Eip1193Provider;
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

function errorCode(error: unknown): number | string | undefined {
  const seen = new Set<unknown>();
  const visit = (value: unknown, depth: number): number | string | undefined => {
    if (depth > 4 || !value || typeof value !== 'object' || seen.has(value)) return undefined;
    seen.add(value);
    const record = value as Record<string, unknown>;
    if (typeof record.code === 'number' || typeof record.code === 'string') return record.code;
    for (const key of ['error', 'data', 'originalError', 'cause', 'info']) {
      const found = visit(record[key], depth + 1);
      if (found !== undefined) return found;
    }
    return undefined;
  };
  return visit(error, 0);
}

function errorText(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (error && typeof error === 'object' && typeof (error as { message?: unknown }).message === 'string') {
    return (error as { message: string }).message;
  }
  return String(error ?? '');
}

/** EIP-1193 4001 (and ethers' ACTION_REJECTED): the user declined, so nothing was sent. */
export function isUserRejection(error: unknown): boolean {
  const code = errorCode(error);
  return code === 4001 || code === 'ACTION_REJECTED';
}

/** EIP-1193 4902: the wallet does not know this chain id yet. */
export function isUnrecognizedChain(error: unknown): boolean {
  return errorCode(error) === 4902 || /unrecognized chain|try adding the chain|add this chain/i.test(errorText(error));
}

/** MetaMask -32002: a wallet request is already waiting for the user. */
export function isRequestPending(error: unknown): boolean {
  return errorCode(error) === -32002;
}

/** Plain-language message for a wallet error. */
export function describeWalletError(error: unknown, fallback = 'The wallet request failed.'): string {
  if (isUserRejection(error)) return 'The request was declined in your wallet.';
  if (isRequestPending(error)) return 'Your wallet already has a request open. Finish or dismiss it, then try again.';
  const text = errorText(error).trim();
  return text && text.length <= 240 ? text : fallback;
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

interface Eip6963Detail {
  info?: { uuid?: string; name?: string; icon?: string; rdns?: string };
  provider?: Eip1193Provider;
}

function isProvider(value: unknown): value is Eip1193Provider {
  return !!value && typeof (value as Eip1193Provider).request === 'function';
}

/**
 * Collect EIP-6963 announcements for `timeoutMs`, then add `window.ethereum`
 * when no announcement already covers it.
 */
export async function discoverWallets(
  opts: { win?: Window & { ethereum?: unknown }; timeoutMs?: number } = {},
): Promise<DiscoveredWallet[]> {
  const win = opts.win ?? (typeof window !== 'undefined' ? (window as Window & { ethereum?: unknown }) : undefined);
  if (!win) return [];
  const found = new Map<string, DiscoveredWallet>();
  const onAnnounce = (event: Event) => {
    const detail = (event as CustomEvent<Eip6963Detail>).detail;
    if (!detail || !isProvider(detail.provider)) return;
    const id = detail.info?.rdns || detail.info?.uuid || `wallet-${found.size + 1}`;
    if (found.has(id)) return;
    found.set(id, { id, name: detail.info?.name || 'Browser wallet', icon: detail.info?.icon, provider: detail.provider });
  };
  win.addEventListener('eip6963:announceProvider', onAnnounce);
  try {
    win.dispatchEvent(new Event('eip6963:requestProvider'));
    await new Promise<void>((resolve) => setTimeout(resolve, opts.timeoutMs ?? 120));
  } finally {
    win.removeEventListener('eip6963:announceProvider', onAnnounce);
  }
  const wallets = [...found.values()];
  const injected = win.ethereum;
  if (isProvider(injected) && !wallets.some((wallet) => wallet.provider === injected)) {
    wallets.push({ id: 'injected', name: 'Browser wallet', provider: injected });
  }
  return wallets;
}

// ---------------------------------------------------------------------------
// Typed requests
// ---------------------------------------------------------------------------

async function rpc(provider: Eip1193Provider, method: string, params?: unknown[]): Promise<unknown> {
  return provider.request(params === undefined ? { method } : { method, params });
}

function expectHexQuantity(value: unknown, what: string): bigint {
  if (typeof value !== 'string' || !/^0x[0-9a-fA-F]+$/.test(value)) {
    throw new Error(`The wallet returned an invalid ${what}.`);
  }
  return BigInt(value);
}

export async function getChainId(provider: Eip1193Provider): Promise<number> {
  const id = parseChainId(await rpc(provider, 'eth_chainId'));
  if (id === null) throw new Error('The wallet returned an invalid chain id.');
  return id;
}

/** `eth_accounts`: authorised accounts only, never prompts. Lower-cased. */
export async function getAccounts(provider: Eip1193Provider): Promise<string[]> {
  return normalizeAccounts(await rpc(provider, 'eth_accounts'));
}

/** `eth_requestAccounts`: prompts the user to connect. Lower-cased. */
export async function requestAccounts(provider: Eip1193Provider): Promise<string[]> {
  return normalizeAccounts(await rpc(provider, 'eth_requestAccounts'));
}

/**
 * Wallet-reported accounts, lower-cased. An entry is dropped when it is malformed or when its mixed-case
 * EIP-55 checksum is verifiably wrong. A mixed-case entry that cannot be verified (no Keccak-256
 * configured) is kept: it came from the user's own wallet, not from a typed or pasted address.
 */
function normalizeAccounts(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  return value
    .filter((entry): entry is string => isAddress(entry) && checksumStatus(entry) !== 'invalid')
    .map((entry) => entry.toLowerCase());
}

export async function ethCall(provider: Eip1193Provider, to: string, data: string, from?: string): Promise<string> {
  const tx: Record<string, string> = { to, data };
  if (from) tx.from = from;
  const result = await rpc(provider, 'eth_call', [tx, 'latest']);
  if (typeof result !== 'string' || !/^0x[0-9a-fA-F]*$/.test(result)) throw new Error('The wallet returned an invalid call result.');
  return result;
}

export async function getEthBalance(provider: Eip1193Provider, account: string): Promise<bigint> {
  return expectHexQuantity(await rpc(provider, 'eth_getBalance', [account, 'latest']), 'ETH balance');
}

export async function estimateGas(
  provider: Eip1193Provider,
  tx: { from: string; to: string; data: string },
): Promise<bigint> {
  return expectHexQuantity(await rpc(provider, 'eth_estimateGas', [{ ...tx, value: '0x0' }]), 'gas estimate');
}

export async function getGasPrice(provider: Eip1193Provider): Promise<bigint> {
  return expectHexQuantity(await rpc(provider, 'eth_gasPrice'), 'gas price');
}

export interface EvmReceipt {
  transactionHash: string;
  blockNumber: bigint;
  /** True when the transaction succeeded (status 0x1). */
  success: boolean;
}

/** `eth_getTransactionReceipt`. Null while the transaction is not yet included. A hint only. */
export async function getReceipt(provider: Eip1193Provider, txHash: string): Promise<EvmReceipt | null> {
  if (!isBytes32(txHash)) throw new Error('Invalid transaction hash.');
  const raw = (await rpc(provider, 'eth_getTransactionReceipt', [txHash])) as
    | { transactionHash?: unknown; blockNumber?: unknown; status?: unknown }
    | null
    | undefined;
  if (!raw) return null;
  if (raw.blockNumber === null || raw.blockNumber === undefined) return null;
  return {
    transactionHash: typeof raw.transactionHash === 'string' ? raw.transactionHash : txHash,
    blockNumber: expectHexQuantity(raw.blockNumber, 'block number'),
    success: raw.status === undefined ? true : expectHexQuantity(raw.status, 'receipt status') === 1n,
  };
}

export async function sendTransaction(
  provider: Eip1193Provider,
  tx: { from: string; to: string; data: string },
): Promise<string> {
  const hash = await rpc(provider, 'eth_sendTransaction', [{ ...tx, value: '0x0' }]);
  if (!isBytes32(hash)) {
    throw new Error('The wallet did not return a valid transaction hash. Check your wallet activity before sending again.');
  }
  return hash.toLowerCase();
}

export class ChainSwitchError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'ChainSwitchError';
  }
}

/**
 * Switch the wallet to `chain`, adding it first when the wallet does not know
 * it (4902 or "Unrecognized chain"). Verifies the result with `eth_chainId`.
 */
export async function switchToChain(provider: Eip1193Provider, chain: BaseChainConfig): Promise<void> {
  try {
    await rpc(provider, 'wallet_switchEthereumChain', [{ chainId: chain.chainIdHex }]);
  } catch (error) {
    if (isUserRejection(error)) throw error;
    if (!isUnrecognizedChain(error)) throw error;
    await rpc(provider, 'wallet_addEthereumChain', [addChainParams(chain)]);
  }
  const now = await getChainId(provider);
  if (now !== chain.chainId) {
    throw new ChainSwitchError(`Your wallet is still on chain ${now}. Switch to ${chain.name} and try again.`);
  }
}

// ---------------------------------------------------------------------------
// Event subscription
// ---------------------------------------------------------------------------

export interface ProviderHandlers {
  accountsChanged?: (accounts: string[]) => void;
  chainChanged?: (chainId: number | null) => void;
  disconnect?: () => void;
}

/** Subscribe to provider events. The returned function removes exactly the listeners it added. */
export function watchProvider(provider: Eip1193Provider, handlers: ProviderHandlers): () => void {
  if (typeof provider.on !== 'function') return () => {};
  const onAccounts = (accounts: unknown) => handlers.accountsChanged?.(normalizeAccounts(accounts));
  const onChain = (chainId: unknown) => handlers.chainChanged?.(parseChainId(chainId));
  const onDisconnect = () => handlers.disconnect?.();
  provider.on('accountsChanged', onAccounts);
  provider.on('chainChanged', onChain);
  provider.on('disconnect', onDisconnect);
  return () => {
    provider.removeListener?.('accountsChanged', onAccounts);
    provider.removeListener?.('chainChanged', onChain);
    provider.removeListener?.('disconnect', onDisconnect);
  };
}

// ---------------------------------------------------------------------------
// Wallet store
// ---------------------------------------------------------------------------

export interface KeyValueStorage {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

export type EvmWalletStatus = 'unavailable' | 'disconnected' | 'connecting' | 'connected';

export interface EvmWalletState {
  status: EvmWalletStatus;
  wallets: DiscoveredWallet[];
  walletId: string | null;
  walletName: string | null;
  /** Lower-cased connected account. */
  account: string | null;
  chainId: number | null;
  error: string | null;
}

export const EVM_DISCONNECT_KEY = 'rumi:base-bridge:evm-disconnected';
export const EVM_WALLET_KEY = 'rumi:base-bridge:evm-wallet';

export interface EvmWalletStore extends Readable<EvmWalletState> {
  /** Discover wallets and, unless the user disconnected this session, silently restore an authorised account. */
  init(): Promise<void>;
  connect(walletId?: string): Promise<void>;
  /** Local disconnect only (a page cannot revoke wallet permissions). */
  disconnect(): void;
  switchNetwork(chain: BaseChainConfig): Promise<void>;
  /** The provider of the connected wallet, or null. */
  getProvider(): Eip1193Provider | null;
  /** Remove every provider listener. Call from onDestroy. */
  destroy(): void;
}

export interface EvmWalletStoreOptions {
  win?: Window & { ethereum?: unknown };
  /** sessionStorage-like store for the disconnect flag and last wallet choice. */
  storage?: KeyValueStorage | null;
  discoveryTimeoutMs?: number;
}

export function createEvmWalletStore(options: EvmWalletStoreOptions = {}): EvmWalletStore {
  const initial: EvmWalletState = {
    status: 'disconnected',
    wallets: [],
    walletId: null,
    walletName: null,
    account: null,
    chainId: null,
    error: null,
  };
  const { subscribe, update, set } = writable<EvmWalletState>(initial);
  const storage = options.storage === undefined ? safeSessionStorage() : options.storage;

  let provider: Eip1193Provider | null = null;
  let unwatch: (() => void) | null = null;
  let epoch = 0;
  let destroyed = false;
  let manuallyDisconnected = readFlag(storage, EVM_DISCONNECT_KEY);
  let wallets: DiscoveredWallet[] = [];

  function attach(wallet: DiscoveredWallet) {
    unwatch?.();
    provider = wallet.provider;
    unwatch = watchProvider(wallet.provider, {
      accountsChanged: (accounts) => {
        if (destroyed || manuallyDisconnected) return;
        epoch += 1;
        const account = accounts[0] ?? null;
        update((s) => ({ ...s, account, status: account ? 'connected' : 'disconnected', error: null }));
      },
      chainChanged: (chainId) => {
        if (destroyed || manuallyDisconnected) return;
        epoch += 1;
        update((s) => ({ ...s, chainId }));
      },
      disconnect: () => {
        if (destroyed) return;
        epoch += 1;
        update((s) => ({ ...s, account: null, status: 'disconnected' }));
      },
    });
  }

  function pick(walletId?: string): DiscoveredWallet | null {
    if (walletId) return wallets.find((w) => w.id === walletId) ?? null;
    const remembered = readItem(storage, EVM_WALLET_KEY);
    const rememberedWallet = remembered ? wallets.find((w) => w.id === remembered) : undefined;
    if (rememberedWallet) return rememberedWallet;
    return wallets.length === 1 ? wallets[0] : null;
  }

  async function init() {
    const myEpoch = ++epoch;
    wallets = await discoverWallets({ win: options.win, timeoutMs: options.discoveryTimeoutMs });
    if (destroyed || myEpoch !== epoch) return;
    if (wallets.length === 0) {
      update((s) => ({ ...s, wallets, status: 'unavailable' }));
      return;
    }
    update((s) => ({ ...s, wallets, status: s.status === 'unavailable' ? 'disconnected' : s.status }));
    if (manuallyDisconnected) return;
    const wallet = pick();
    if (!wallet) return;
    try {
      const [accounts, chainId] = await Promise.all([getAccounts(wallet.provider), getChainId(wallet.provider)]);
      if (destroyed || myEpoch !== epoch || manuallyDisconnected) return;
      if (accounts.length === 0) return;
      attach(wallet);
      update((s) => ({
        ...s,
        status: 'connected',
        walletId: wallet.id,
        walletName: wallet.name,
        account: accounts[0],
        chainId,
        error: null,
      }));
    } catch {
      // A locked or missing wallet is handled when the user presses Connect.
    }
  }

  async function connect(walletId?: string) {
    const wallet = pick(walletId);
    if (!wallet) {
      update((s) => ({
        ...s,
        error: wallets.length === 0 ? 'No Base-compatible wallet was found in this browser.' : 'Choose a wallet to connect.',
      }));
      return;
    }
    const myEpoch = ++epoch;
    update((s) => ({ ...s, status: 'connecting', error: null }));
    try {
      const accounts = await requestAccounts(wallet.provider);
      if (accounts.length === 0) throw new Error('The wallet did not return an account.');
      const chainId = await getChainId(wallet.provider);
      if (destroyed || myEpoch !== epoch) return;
      manuallyDisconnected = false;
      removeItem(storage, EVM_DISCONNECT_KEY);
      writeItem(storage, EVM_WALLET_KEY, wallet.id);
      attach(wallet);
      update((s) => ({
        ...s,
        status: 'connected',
        walletId: wallet.id,
        walletName: wallet.name,
        account: accounts[0],
        chainId,
        error: null,
      }));
    } catch (error) {
      if (destroyed || myEpoch !== epoch) return;
      update((s) => ({
        ...s,
        status: s.account ? 'connected' : 'disconnected',
        error: describeWalletError(error, 'The wallet connection was not completed.'),
      }));
    }
  }

  function disconnect() {
    epoch += 1;
    manuallyDisconnected = true;
    writeItem(storage, EVM_DISCONNECT_KEY, 'true');
    unwatch?.();
    unwatch = null;
    provider = null;
    update((s) => ({ ...s, status: 'disconnected', walletId: null, walletName: null, account: null, chainId: null, error: null }));
  }

  async function switchNetwork(chain: BaseChainConfig) {
    if (!provider) return;
    const active = provider;
    update((s) => ({ ...s, error: null }));
    try {
      await switchToChain(active, chain);
      if (destroyed || provider !== active) return;
      update((s) => ({ ...s, chainId: chain.chainId }));
    } catch (error) {
      if (destroyed || provider !== active) return;
      update((s) => ({ ...s, error: describeWalletError(error, `Could not switch to ${chain.name}.`) }));
    }
  }

  function destroy() {
    destroyed = true;
    epoch += 1;
    unwatch?.();
    unwatch = null;
    provider = null;
    set(initial);
  }

  return { subscribe, init, connect, disconnect, switchNetwork, getProvider: () => provider, destroy };
}

function safeSessionStorage(): KeyValueStorage | null {
  try {
    return typeof sessionStorage !== 'undefined' ? sessionStorage : null;
  } catch {
    return null;
  }
}

function readItem(storage: KeyValueStorage | null, key: string): string | null {
  try {
    return storage ? storage.getItem(key) : null;
  } catch {
    return null;
  }
}

function writeItem(storage: KeyValueStorage | null, key: string, value: string): void {
  try {
    storage?.setItem(key, value);
  } catch {
    // The in-memory flag still applies for this page load.
  }
}

function removeItem(storage: KeyValueStorage | null, key: string): void {
  try {
    storage?.removeItem(key);
  } catch {
    // Nothing to clean up.
  }
}

function readFlag(storage: KeyValueStorage | null, key: string): boolean {
  return readItem(storage, key) === 'true';
}
