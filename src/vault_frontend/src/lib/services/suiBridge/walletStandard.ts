/**
 * The Sui wallet connection, over the Wallet Standard.
 *
 * Discovery and registration use the official `@wallet-standard/app` package (`getWallets`): a wallet that registers
 * before or after the page loads is found. Nothing is discovered, connected or signed until the page asks, and the page
 * asks only for an ENABLED Sui destination (see `SuiBridgePanel.svelte`): the disabled path never creates this store, so
 * no wallet is touched.
 *
 * Features used (names and shapes from the Sui Wallet Standard, https://docs.sui.io/standards/wallet-standard, and the
 * wallet-builder notes shipped in `@mysten/sui` 2.38.0 under `docs/migrations/sui-2.0/wallet-builders.md`):
 *  - `standard:connect`   : `connect()` resolves `{ accounts }`
 *  - `standard:disconnect`: `disconnect()` (optional)
 *  - `standard:events`    : `on('change', listener)` with `{ accounts?, chains?, features? }`, returns an off function
 *  - `sui:signTransaction`           : `signTransaction({ transaction, account, chain })` resolves `{ bytes, signature }`
 *    (base64 transaction bytes the wallet built and signed, and the base64 signature; the wallet does not broadcast)
 *  - `sui:signAndExecuteTransaction` : `signAndExecuteTransaction({ transaction, account, chain })` resolves
 *    `{ bytes, signature, digest, effects }` (the wallet builds, signs and executes)
 * `transaction` is an object with `toJSON(): Promise<string>`; the wallet resolves gas and anything the page left open.
 *
 * A wallet that returns signed bytes (`sui:signTransaction`) is preferred: the page can validate them against the
 * reviewed attempt and save the bytes, the signature and the digest before anything is broadcast. A
 * sign-and-execute-only wallet is supported, but its answer can be ambiguous, and the flow treats it as such.
 *
 * A wallet is only the destination-chain signer here: ICP authentication, the allowance and the ICP-side consent stay
 * with the existing ICP wallet.
 */

import { getWallets } from '@wallet-standard/app';
import type { Wallet, WalletAccount } from '@wallet-standard/base';
import { writable, type Readable } from 'svelte/store';

export const STANDARD_CONNECT = 'standard:connect';
export const STANDARD_DISCONNECT = 'standard:disconnect';
export const STANDARD_EVENTS = 'standard:events';
export const SUI_SIGN_TRANSACTION = 'sui:signTransaction';
export const SUI_SIGN_AND_EXECUTE_TRANSACTION = 'sui:signAndExecuteTransaction';

/** What the wallet is handed: the SDK `Transaction` serializes itself to JSON. */
export interface SuiTransactionLike {
  toJSON(): Promise<string>;
}

export interface SuiSignedTransaction {
  /** Base64 transaction bytes the wallet built and signed. */
  bytes: string;
  /** Base64 signature (flag, signature, public key). */
  signature: string;
}

export interface SuiExecutedTransaction extends SuiSignedTransaction {
  digest: string;
  /** Base64 BCS effects. */
  effects: string;
}

interface ConnectFeature {
  connect(input?: { silent?: boolean }): Promise<{ accounts: readonly WalletAccount[] }>;
}
interface DisconnectFeature {
  disconnect(): Promise<void>;
}
interface EventsFeature {
  on(event: 'change', listener: (properties: { accounts?: readonly WalletAccount[] }) => void): () => void;
}
interface SignTransactionFeature {
  signTransaction(input: { transaction: SuiTransactionLike; account: WalletAccount; chain: string }): Promise<SuiSignedTransaction>;
}
interface SignAndExecuteFeature {
  signAndExecuteTransaction(input: { transaction: SuiTransactionLike; account: WalletAccount; chain: string }): Promise<SuiExecutedTransaction>;
}

/** The wallet-discovery API (`getWallets()` of `@wallet-standard/app`). Injected by the specs. */
export interface WalletRegistry {
  get(): readonly Wallet[];
  on(event: 'register' | 'unregister', listener: (...wallets: Wallet[]) => void): () => void;
}

function feature<T>(wallet: Wallet, name: string): T | undefined {
  return (wallet.features as Record<string, unknown>)[name] as T | undefined;
}

/** A wallet this page can use: it connects, reports changes, is on a Sui chain and can sign. */
export function isUsableSuiWallet(wallet: Wallet): boolean {
  return (
    feature(wallet, STANDARD_CONNECT) !== undefined &&
    feature(wallet, STANDARD_EVENTS) !== undefined &&
    wallet.chains.some((chain) => chain.startsWith('sui:')) &&
    (feature(wallet, SUI_SIGN_TRANSACTION) !== undefined || feature(wallet, SUI_SIGN_AND_EXECUTE_TRANSACTION) !== undefined)
  );
}

/**
 * Whether an error LOOKS like the user saying no. Message matching is a hint, never proof. Use it only where the wallet
 * cannot have broadcast anything (`sui:signTransaction` returns signed bytes and sends nothing), where a wrong guess only
 * changes the wording. For a wallet that signs AND executes, use `isExplicitUserRejection`.
 */
const REJECTED = /reject|denied|declin|cancel|closed|dismiss|not approved/i;
export function isWalletRejection(error: unknown): boolean {
  if (error && typeof error === 'object') {
    const e = error as { code?: unknown; name?: unknown; message?: unknown };
    if (e.code === 4001) return true;
    if (typeof e.name === 'string' && /UserRejected/i.test(e.name)) return true;
    if (typeof e.message === 'string') return REJECTED.test(e.message);
  }
  return typeof error === 'string' && REJECTED.test(error);
}

/**
 * A refusal the wallet raised BEFORE it signed or executed anything: the explicit user-rejection code (4001). Message
 * text and error names are NOT proof: a wallet that already executed can throw "connection closed" or a timeout that
 * merely sounds like a refusal. Only this signal may clear a sign-and-execute attempt; anything else keeps the original
 * attempt held.
 */
export function isExplicitUserRejection(error: unknown): boolean {
  return typeof error === 'object' && error !== null && (error as { code?: unknown }).code === 4001;
}

/** What the transfer flows need from a connected wallet. */
export interface SuiWalletSession {
  readonly walletName: string;
  /** The connected account's Sui address (`0x` and 64 hex digits). */
  readonly address: string;
  /** The Wallet Standard chains the account supports. */
  readonly chains: readonly string[];
  readonly canSign: boolean;
  readonly canSignAndExecute: boolean;
  /** Resolves the signed transaction. The wallet does not execute it. */
  signTransaction(transaction: SuiTransactionLike, chain: string): Promise<SuiSignedTransaction>;
  /** Resolves the executed transaction: the wallet built, signed and executed it. */
  signAndExecuteTransaction(transaction: SuiTransactionLike, chain: string): Promise<SuiExecutedTransaction>;
}

export function sessionOf(wallet: Wallet, account: WalletAccount): SuiWalletSession {
  const signer = feature<SignTransactionFeature>(wallet, SUI_SIGN_TRANSACTION);
  const executor = feature<SignAndExecuteFeature>(wallet, SUI_SIGN_AND_EXECUTE_TRANSACTION);
  return {
    walletName: wallet.name,
    address: account.address,
    chains: account.chains,
    canSign: signer !== undefined,
    canSignAndExecute: executor !== undefined,
    async signTransaction(transaction, chain) {
      if (!signer) throw new Error('This wallet cannot sign a transaction without executing it.');
      const output = await signer.signTransaction({ transaction, account, chain });
      if (!output || typeof output.bytes !== 'string' || typeof output.signature !== 'string') throw new Error('The wallet returned no signed transaction.');
      return output;
    },
    async signAndExecuteTransaction(transaction, chain) {
      if (!executor) throw new Error('This wallet cannot sign and execute a transaction.');
      const output = await executor.signAndExecuteTransaction({ transaction, account, chain });
      if (!output || typeof output.bytes !== 'string' || typeof output.signature !== 'string' || typeof output.digest !== 'string') throw new Error('The wallet answered without a usable result.');
      return output;
    },
  };
}

// ---------------------------------------------------------------------------
// The store
// ---------------------------------------------------------------------------

export type SuiWalletStatus = 'unavailable' | 'disconnected' | 'connecting' | 'connected';

export interface SuiWalletEntry {
  name: string;
  icon: string;
}

export interface SuiWalletState {
  status: SuiWalletStatus;
  /** The usable wallets found so far. */
  wallets: SuiWalletEntry[];
  walletName: string | null;
  address: string | null;
  chains: readonly string[];
  canSign: boolean;
  canSignAndExecute: boolean;
  error: string | null;
}

export const INITIAL_SUI_WALLET_STATE: SuiWalletState = {
  status: 'unavailable',
  wallets: [],
  walletName: null,
  address: null,
  chains: [],
  canSign: false,
  canSignAndExecute: false,
  error: null,
};

export interface SuiWalletStore extends Readable<SuiWalletState> {
  /** Start discovering wallets. Asks no wallet to connect. */
  init(): void;
  /** Ask the named wallet (or the only one found) to connect. Call from a click. */
  connect(walletName?: string): Promise<void>;
  disconnect(): Promise<void>;
  /** The connected wallet as a signing session, or null. */
  session(): SuiWalletSession | null;
  destroy(): void;
}

export interface SuiWalletStoreOptions {
  registry?: () => WalletRegistry;
}

export function createSuiWalletStore(options: SuiWalletStoreOptions = {}): SuiWalletStore {
  const state = writable<SuiWalletState>({ ...INITIAL_SUI_WALLET_STATE });
  let current: SuiWalletState = { ...INITIAL_SUI_WALLET_STATE };
  let registry: WalletRegistry | null = null;
  let wallet: Wallet | null = null;
  let account: WalletAccount | null = null;
  let offRegister: (() => void) | null = null;
  let offUnregister: (() => void) | null = null;
  let offChange: (() => void) | null = null;
  let generation = 0;

  function publish(patch: Partial<SuiWalletState>) {
    current = { ...current, ...patch };
    state.set(current);
  }

  function usable(): Wallet[] {
    return (registry?.get() ?? []).filter(isUsableSuiWallet);
  }

  function refreshList() {
    const list = usable();
    publish({
      wallets: list.map((w) => ({ name: w.name, icon: w.icon })),
      status: current.status === 'unavailable' && list.length > 0 ? 'disconnected' : list.length === 0 && current.status === 'disconnected' ? 'unavailable' : current.status,
    });
    // The connected wallet went away.
    if (wallet && !list.includes(wallet)) clearConnection('The wallet was removed.');
  }

  function clearConnection(error: string | null = null) {
    offChange?.();
    offChange = null;
    wallet = null;
    account = null;
    publish({ status: usable().length > 0 ? 'disconnected' : 'unavailable', walletName: null, address: null, chains: [], canSign: false, canSignAndExecute: false, error });
  }

  function adopt(next: Wallet, accounts: readonly WalletAccount[]) {
    wallet = next;
    account = accounts[0] ?? null;
    if (!account) {
      clearConnection('The wallet connected no account.');
      return;
    }
    const session = sessionOf(next, account);
    publish({ status: 'connected', walletName: next.name, address: session.address, chains: session.chains, canSign: session.canSign, canSignAndExecute: session.canSignAndExecute, error: null });
  }

  return {
    subscribe: state.subscribe,

    init() {
      if (registry) return;
      registry = (options.registry ?? (() => getWallets()))();
      offRegister = registry.on('register', refreshList);
      offUnregister = registry.on('unregister', refreshList);
      refreshList();
    },

    async connect(walletName) {
      if (!registry) this.init();
      const list = usable();
      const target = walletName ? list.find((w) => w.name === walletName) : list.length === 1 ? list[0] : undefined;
      if (!target) {
        publish({ error: list.length === 0 ? 'No Sui wallet was found in this browser.' : 'Choose a wallet to connect.' });
        return;
      }
      const mine = ++generation;
      publish({ status: 'connecting', error: null });
      try {
        const connect = feature<ConnectFeature>(target, STANDARD_CONNECT)!;
        const { accounts } = await connect.connect();
        if (mine !== generation) return;
        const suiAccounts = accounts.filter((a) => a.chains.some((c) => c.startsWith('sui:')));
        adopt(target, suiAccounts);
        offChange?.();
        offChange = feature<EventsFeature>(target, STANDARD_EVENTS)!.on('change', (properties) => {
          if (wallet !== target || !properties.accounts) return;
          const accounts = properties.accounts.filter((a) => a.chains.some((c) => c.startsWith('sui:')));
          // The same account stays; a switch to another account or a disconnect changes what the page sees.
          const stays = accounts.find((a) => a.address === account?.address);
          if (stays) adopt(target, [stays, ...accounts.filter((a) => a !== stays)]);
          else if (accounts.length > 0) adopt(target, accounts);
          else clearConnection();
        });
      } catch (cause) {
        if (mine !== generation) return;
        wallet = null;
        account = null;
        publish({
          status: usable().length > 0 ? 'disconnected' : 'unavailable',
          error: cause instanceof Error && cause.message ? cause.message : 'The wallet did not connect.',
        });
      }
    },

    async disconnect() {
      generation += 1;
      const target = wallet;
      clearConnection();
      const off = target ? feature<DisconnectFeature>(target, STANDARD_DISCONNECT) : undefined;
      try {
        await off?.disconnect();
      } catch {
        // The page already forgot the connection.
      }
    },

    session() {
      return wallet && account ? sessionOf(wallet, account) : null;
    },

    destroy() {
      generation += 1;
      offRegister?.();
      offUnregister?.();
      offChange?.();
      offRegister = offUnregister = offChange = null;
      registry = null;
      wallet = null;
      account = null;
    },
  };
}
