/**
 * The Solana wallet connection, over the Wallet Standard.
 *
 * Discovery and registration use the official `@wallet-standard/app` package (`getWallets`): a wallet that
 * registers before or after the page loads is found. Nothing is discovered, connected or signed until the page
 * asks, and the page asks only for an ENABLED Solana destination (see `SolanaBridgePanel.svelte`): the disabled path never
 * creates this store, so no wallet is touched.
 *
 * Features used (names and shapes from the Wallet Standard and the Solana wallet-standard feature packages):
 *  - `standard:connect`  : `connect()` resolves `{ accounts }`
 *  - `standard:disconnect`: `disconnect()` (optional)
 *  - `standard:events`   : `on('change', listener)` with `{ accounts?, chains?, features? }`, returns an off function
 *  - `solana:signTransaction`        : `signTransaction({ account, transaction, chain })` resolves `[{ signedTransaction }]`
 *  - `solana:signAndSendTransaction` : `signAndSendTransaction({ account, transaction, chain })` resolves `[{ signature }]`
 *
 * A wallet that returns signed bytes (`signTransaction`) is preferred: the page can save those bytes and their
 * signature before anything is broadcast. A sign-and-send-only wallet is supported, but its answer can be
 * ambiguous, and the flow treats it as such (see `solanaToIcp.ts`).
 *
 * The Wallet Standard `account.address` is the base58 public key. A wallet is only the destination-chain signer
 * here: ICP authentication, the allowance and the ICP-side consent stay with the existing ICP wallet.
 */

import { getWallets } from '@wallet-standard/app';
import type { Wallet, WalletAccount } from '@wallet-standard/base';
import { writable, type Readable } from 'svelte/store';
import type { Bytes } from './bytes';

export const STANDARD_CONNECT = 'standard:connect';
export const STANDARD_DISCONNECT = 'standard:disconnect';
export const STANDARD_EVENTS = 'standard:events';
export const SOLANA_SIGN_TRANSACTION = 'solana:signTransaction';
export const SOLANA_SIGN_AND_SEND_TRANSACTION = 'solana:signAndSendTransaction';

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
  signTransaction(...inputs: { account: WalletAccount; transaction: Bytes; chain?: string }[]): Promise<readonly { signedTransaction: Bytes }[]>;
}
interface SignAndSendFeature {
  signAndSendTransaction(...inputs: { account: WalletAccount; transaction: Bytes; chain: string }[]): Promise<readonly { signature: Bytes }[]>;
}

/** The wallet-discovery API (`getWallets()` of `@wallet-standard/app`). Injected by the specs. */
export interface WalletRegistry {
  get(): readonly Wallet[];
  on(event: 'register' | 'unregister', listener: (...wallets: Wallet[]) => void): () => void;
}

function feature<T>(wallet: Wallet, name: string): T | undefined {
  return (wallet.features as Record<string, unknown>)[name] as T | undefined;
}

/** A wallet this page can use: it connects, reports changes, is on a Solana chain and can sign. */
export function isUsableSolanaWallet(wallet: Wallet): boolean {
  return (
    feature(wallet, STANDARD_CONNECT) !== undefined &&
    feature(wallet, STANDARD_EVENTS) !== undefined &&
    wallet.chains.some((chain) => chain.startsWith('solana:')) &&
    (feature(wallet, SOLANA_SIGN_TRANSACTION) !== undefined || feature(wallet, SOLANA_SIGN_AND_SEND_TRANSACTION) !== undefined)
  );
}

const REJECTED = /reject|denied|declin|cancel|closed|dismiss|not approved/i;

/**
 * Whether an error LOOKS like the user saying no. Message matching is a hint, never proof. Use it only where the wallet
 * cannot have broadcast anything (`solana:signTransaction` returns signed bytes and sends nothing), where a wrong guess
 * only changes the wording. For a wallet that signs AND sends, use `isExplicitUserRejection`.
 */
export function isWalletRejection(error: unknown): boolean {
  if (error && typeof error === 'object') {
    const e = error as { code?: unknown; name?: unknown; message?: unknown };
    if (e.code === 4001) return true;
    if (typeof e.name === 'string' && /UserRejected|WalletSignTransactionError.*reject/i.test(e.name)) return true;
    if (typeof e.message === 'string') return REJECTED.test(e.message);
  }
  return typeof error === 'string' && REJECTED.test(error);
}

/**
 * A refusal the wallet raised BEFORE it signed or broadcast anything: the explicit user-rejection code (4001, the code the
 * Solana wallets raise when the user dismisses the prompt). Message text and error names are NOT proof: a wallet that
 * already broadcast can throw "connection closed" or a timeout that merely sounds like a refusal. Only this signal may
 * clear a sign-and-send attempt; anything else keeps the original attempt held.
 */
export function isExplicitUserRejection(error: unknown): boolean {
  return typeof error === 'object' && error !== null && (error as { code?: unknown }).code === 4001;
}

/** What the transfer flows need from a connected wallet. */
export interface SolanaWalletSession {
  readonly walletName: string;
  /** Base58 address of the connected account. */
  readonly address: string;
  /** The Wallet Standard chains the account supports. */
  readonly chains: readonly string[];
  readonly canSign: boolean;
  readonly canSignAndSend: boolean;
  /** Resolves the signed transaction bytes. The wallet does not broadcast. */
  signTransaction(transaction: Bytes, chain: string): Promise<Bytes>;
  /** Resolves the signature bytes of the transaction the wallet signed AND broadcast. */
  signAndSendTransaction(transaction: Bytes, chain: string): Promise<Bytes>;
}

export function sessionOf(wallet: Wallet, account: WalletAccount): SolanaWalletSession {
  const signer = feature<SignTransactionFeature>(wallet, SOLANA_SIGN_TRANSACTION);
  const sender = feature<SignAndSendFeature>(wallet, SOLANA_SIGN_AND_SEND_TRANSACTION);
  return {
    walletName: wallet.name,
    address: account.address,
    chains: account.chains,
    canSign: signer !== undefined,
    canSignAndSend: sender !== undefined,
    async signTransaction(transaction, chain) {
      if (!signer) throw new Error('This wallet cannot sign a transaction without sending it.');
      const [output] = await signer.signTransaction({ account, transaction, chain });
      if (!output) throw new Error('The wallet returned no signed transaction.');
      return output.signedTransaction;
    },
    async signAndSendTransaction(transaction, chain) {
      if (!sender) throw new Error('This wallet cannot sign and send a transaction.');
      const [output] = await sender.signAndSendTransaction({ account, transaction, chain });
      if (!output) throw new Error('The wallet returned no signature.');
      return output.signature;
    },
  };
}

// ---------------------------------------------------------------------------
// The store
// ---------------------------------------------------------------------------

export type SolanaWalletStatus = 'unavailable' | 'disconnected' | 'connecting' | 'connected';

export interface SolanaWalletEntry {
  name: string;
  icon: string;
}

export interface SolanaWalletState {
  status: SolanaWalletStatus;
  /** The usable wallets found so far. */
  wallets: SolanaWalletEntry[];
  walletName: string | null;
  address: string | null;
  chains: readonly string[];
  canSign: boolean;
  canSignAndSend: boolean;
  error: string | null;
}

export const INITIAL_SOLANA_WALLET_STATE: SolanaWalletState = {
  status: 'unavailable',
  wallets: [],
  walletName: null,
  address: null,
  chains: [],
  canSign: false,
  canSignAndSend: false,
  error: null,
};

export interface SolanaWalletStore extends Readable<SolanaWalletState> {
  /** Start discovering wallets. Asks no wallet to connect. */
  init(): void;
  /** Ask the named wallet (or the only one found) to connect. Call from a click. */
  connect(walletName?: string): Promise<void>;
  disconnect(): Promise<void>;
  /** The connected wallet as a signing session, or null. */
  session(): SolanaWalletSession | null;
  destroy(): void;
}

export interface SolanaWalletStoreOptions {
  registry?: () => WalletRegistry;
}

export function createSolanaWalletStore(options: SolanaWalletStoreOptions = {}): SolanaWalletStore {
  const state = writable<SolanaWalletState>({ ...INITIAL_SOLANA_WALLET_STATE });
  let current: SolanaWalletState = { ...INITIAL_SOLANA_WALLET_STATE };
  let registry: WalletRegistry | null = null;
  let wallet: Wallet | null = null;
  let account: WalletAccount | null = null;
  let offRegister: (() => void) | null = null;
  let offUnregister: (() => void) | null = null;
  let offChange: (() => void) | null = null;
  let generation = 0;

  function publish(patch: Partial<SolanaWalletState>) {
    current = { ...current, ...patch };
    state.set(current);
  }

  function usable(): Wallet[] {
    return (registry?.get() ?? []).filter(isUsableSolanaWallet);
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
    publish({ status: usable().length > 0 ? 'disconnected' : 'unavailable', walletName: null, address: null, chains: [], canSign: false, canSignAndSend: false, error });
  }

  function adopt(next: Wallet, accounts: readonly WalletAccount[]) {
    wallet = next;
    account = accounts[0] ?? null;
    if (!account) {
      clearConnection('The wallet connected no account.');
      return;
    }
    const session = sessionOf(next, account);
    publish({ status: 'connected', walletName: next.name, address: session.address, chains: session.chains, canSign: session.canSign, canSignAndSend: session.canSignAndSend, error: null });
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
        publish({ error: list.length === 0 ? 'No Solana wallet was found in this browser.' : 'Choose a wallet to connect.' });
        return;
      }
      const mine = ++generation;
      publish({ status: 'connecting', error: null });
      try {
        const connect = feature<ConnectFeature>(target, STANDARD_CONNECT)!;
        const { accounts } = await connect.connect();
        if (mine !== generation) return;
        const solanaAccounts = accounts.filter((a) => a.chains.some((c) => c.startsWith('solana:')));
        adopt(target, solanaAccounts);
        offChange?.();
        offChange = feature<EventsFeature>(target, STANDARD_EVENTS)!.on('change', (properties) => {
          if (wallet !== target || !properties.accounts) return;
          const accounts = properties.accounts.filter((a) => a.chains.some((c) => c.startsWith('solana:')));
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
