/**
 * Test doubles for the Solana bridge branch. Used only by specs.
 *
 * - `FakeChain` is an in-memory Solana node behind the `SolanaRpc` interface: it never touches a network.
 * - `makeFakeWallet` is a scriptable Wallet Standard wallet.
 * - the config / view factories build the instance's answers with the backend's reference keys.
 */

import { Principal } from '@dfinity/principal';
import type {
  BootstrapStatus,
  BridgeInfo,
  DepositView,
  ReleaseView,
  ReturnView,
} from '$declarations/rumi_icusd_sol_bridge/rumi_icusd_sol_bridge.did';
import { makeSigning, TEST_BRIDGE_ID, type TestSigning } from '../baseBridge/testSupport';
import { base58Encode, fromHex, toHex, type Bytes } from './bytes';
import { resolveSolanaConfig, type SolanaBridgeConfig } from './config';
import { solanaDestination, type DestinationView } from '../baseBridge/destinations';
import { SOLANA_MAINNET } from './networks';
import { MEMO_PROGRAM_V3_B58, deriveAta, tokenProgramBytes, type TokenProgramKind } from './programs';
import type { BlockhashInfo, HolderTokenAccount, SignatureListing, SignatureStatus, SolanaRpc } from './rpc';
import { messageOfWire } from './transaction';
import type { Wallet, WalletAccount } from '@wallet-standard/base';
import {
  SOLANA_SIGN_TRANSACTION,
  STANDARD_CONNECT,
  STANDARD_DISCONNECT,
  STANDARD_EVENTS,
  type SolanaWalletSession,
  type WalletRegistry,
} from './walletStandard';

const fill = (byte: number, n = 32): Bytes => new Uint8Array(n).fill(byte);

/** The reference keys of the backend's golden vectors (see `reference.spec.ts`). */
export const KEYS = {
  mint: base58Encode(fill(0x44)),
  returnOwner: base58Encode(fill(0x33)),
  feePayer: base58Encode(fill(0x11)),
  mintAuthority: base58Encode(fill(0x22)),
  holder: base58Encode(fill(0xa0)),
  holderSource: base58Encode(fill(0xa1)),
  otherHolder: base58Encode(fill(0xa4)),
};

/** A real-looking, canonical instance canister id (not the management canister: the memo refuses that). */
export const SOL_BRIDGE_ID = 'ijlzs-2yaaa-aaaap-quaaq-cai';
export const SOL_LEDGER_ID = 't6bor-paaaa-aaaap-qrd5q-cai';
export const TEST_OWNER_TEXT = 'zegjz-jpi6k-qkand-c2bgf-qw6za-xk4si-nz3gx-qzzia-fk6fg-snepb-tae';

export function returnAtaOf(tokenProgram: TokenProgramKind): string {
  return base58Encode(deriveAta(fill(0x33), tokenProgramBytes(tokenProgram), fill(0x44)));
}

export function makeSolBridgeInfo(overrides: Partial<BridgeInfo> = {}, tokenProgram: TokenProgramKind = 'token2022'): BridgeInfo {
  const consensus = { Equality: null } as const;
  return {
    rpc: { response_size_estimate: [], verify: consensus, discover: consensus, cycles_per_call: 0n, cluster: [{ Mainnet: null }] },
    initialized: true,
    fees: { release_fee_e8s: 20_000n, deposit_fee_e8s: 10_000n },
    lane: {
      rebroadcast_interval_secs: 60n,
      discover_limit: 20,
      cancel_after_secs: 600n,
      nonce_pool_size: 4,
      reconcile_alarm_secs: 3600n,
      max_reissues: 2,
      history_horizon_secs: 86_400n,
    },
    memo_program: MEMO_PROGRAM_V3_B58,
    escrow_subaccount: '12f37a581370e84a29695a45f63396cd0b13e8a51179e770e0a2ae7d02b720a4',
    sol_rpc: Principal.fromText('tghme-zyaaa-aaaar-qarca-cai'),
    schema_version: 6,
    schnorr_key_name: 'key_1',
    admins: [],
    token_program: tokenProgram === 'token2022' ? { Token2022: null } : { Classic: null },
    halted: [],
    icusd_ledger: Principal.fromText(SOL_LEDGER_ID),
    genesis_hash: SOLANA_MAINNET.genesisHash,
    instance_salt: '00'.repeat(32),
    limits: {
      max_active_per_caller: 4,
      release_window_limit_e8s: 1_000_000_000_000n,
      max_deposit_e8s: 1_000_000_000n,
      max_active_global: 100,
      min_deposit_e8s: 100_000_000n,
      window_seconds: 86_400n,
      mint_window_limit_e8s: 1_000_000_000_000n,
      max_outstanding_e8s: 10_000_000_000_000n,
    },
    pauses: { mints: false, observer: false, releases: false, deposits: false },
    ...overrides,
  };
}

export function makeSolBootstrap(overrides: Partial<BootstrapStatus> = {}, tokenProgram: TokenProgramKind = 'token2022'): BootstrapStatus {
  return {
    return_account: [returnAtaOf(tokenProgram)],
    bind_note: [],
    mint_authority: [KEYS.mintAuthority],
    return_owner: [KEYS.returnOwner],
    pool_free: 4,
    mint_state: { Bound: null },
    pool_registered: 4,
    pool_reserved: 0,
    keys_derived: true,
    fee_payer: [KEYS.feePayer],
    pending_bootstrap_txs: 0,
    mint_account: [KEYS.mint],
    pool_unverified: 0,
    ...overrides,
  };
}

/** The enabled configuration of an instance, resolved the way the page resolves it. */
export function makeSolanaConfig(tokenProgram: TokenProgramKind = 'token2022', info: Partial<BridgeInfo> = {}): SolanaBridgeConfig {
  const result = resolveSolanaConfig(makeSolBridgeInfo(info, tokenProgram), makeSolBootstrap({}, tokenProgram), SOL_BRIDGE_ID);
  if (!result.ok) throw new Error(result.message);
  return result.config;
}

export function makeSolanaDestination(config: SolanaBridgeConfig = makeSolanaConfig()): DestinationView {
  return solanaDestination({ ok: true, config }, SOL_BRIDGE_ID);
}

export function makeSolSigning(destination: DestinationView = makeSolanaDestination()): TestSigning {
  return makeSigning(destination);
}

export { TEST_BRIDGE_ID };

// ---------------------------------------------------------------------------
// Canister views
// ---------------------------------------------------------------------------

export function makeSolDeposit(overrides: Partial<DepositView> = {}): DepositView {
  return {
    id: 7n,
    status: { Escrowed: null },
    client_request_id: new Uint8Array(16).fill(0xab),
    refund_amount_e8s: [],
    owner: Principal.fromText(TEST_OWNER_TEXT),
    from: [],
    kind: { UserDeposit: null },
    mint_id: [],
    pull_block_index: [],
    mint_amount_e8s: 190_000_000n,
    updated_at_ns: 1_700_000_000_000_000_000n,
    solana_recipient: KEYS.holder,
    created_at_ns: 1_700_000_000_000_000_000n,
    amount_e8s: 200_000_000n,
    delivered: [],
    refund_block_index: [],
    deposit_fee_e8s: 10_000_000n,
    mint_op_id: [],
    ...overrides,
  };
}

export function makeSolReturn(overrides: Partial<ReturnView> = {}): ReturnView {
  return {
    seq: 1n,
    status: { Custodied: null },
    return_id: 'aa'.repeat(32),
    destination: { owner: Principal.fromText(TEST_OWNER_TEXT), subaccount: [] },
    signature: base58Encode(fill(9, 64)),
    slot: 123n,
    sender: KEYS.holder,
    created_at_ns: 1_700_000_000_000_000_000n,
    amount_e8s: 300_000_000n,
    source_account: KEYS.holderSource,
    client_nonce: 42n,
    ...overrides,
  };
}

export function makeSolRelease(overrides: Partial<ReleaseView> = {}): ReleaseView {
  return {
    seq: 1n,
    status: { Pending: null },
    return_id: 'aa'.repeat(32),
    destination: { owner: Principal.fromText(TEST_OWNER_TEXT), subaccount: [] },
    solana_sender: KEYS.holder,
    release_block_index: [],
    updated_at_ns: 1_700_000_000_000_000_000n,
    burn_slot: 124n,
    return_slot: 123n,
    created_at_ns: 1_700_000_000_000_000_000n,
    burn_signature: base58Encode(fill(8, 64)),
    amount_e8s: 300_000_000n,
    release_fee_e8s: 20_000n,
    burn_op_id: 1n,
    ledger_fee_e8s: [10_000n],
    return_signature: base58Encode(fill(9, 64)),
    amount_to_receive_e8s: [],
    ...overrides,
  };
}

// ---------------------------------------------------------------------------
// A fake Solana node
// ---------------------------------------------------------------------------

export interface FakeChain extends SolanaRpc {
  calls: Array<{ method: string; args: unknown[] }>;
  callsTo(method: string): Array<{ method: string; args: unknown[] }>;
  blockhash: BlockhashInfo;
  height: bigint;
  tokenAccounts: HolderTokenAccount[];
  /** Signed wire bytes accepted by `sendTransaction`. */
  sent: Bytes[];
  /** Signature (base58) -> status. Missing: unknown to the node. */
  statuses: Map<string, SignatureStatus>;
  /** Landed transactions by signature (wire bytes), newest first in `listing`. */
  landed: Map<string, Bytes>;
  listing: SignatureListing[];
  failSend: Error | null;
  failReads: Error | null;
}

export function createFakeChain(init: Partial<Pick<FakeChain, 'blockhash' | 'height' | 'tokenAccounts'>> = {}): FakeChain {
  const calls: FakeChain['calls'] = [];
  const chain: FakeChain = {
    calls,
    callsTo: (method) => calls.filter((c) => c.method === method),
    blockhash: init.blockhash ?? { blockhash: base58Encode(fill(0x77)), lastValidBlockHeight: 1_000n },
    height: init.height ?? 900n,
    tokenAccounts: init.tokenAccounts ?? [{ address: KEYS.holderSource, amountE8s: 5_000_000_000n }],
    sent: [],
    statuses: new Map(),
    landed: new Map(),
    listing: [],
    failSend: null,
    failReads: null,
    async getLatestBlockhash() {
      calls.push({ method: 'getLatestBlockhash', args: [] });
      if (chain.failReads) throw chain.failReads;
      return chain.blockhash;
    },
    async getBlockHeight() {
      calls.push({ method: 'getBlockHeight', args: [] });
      if (chain.failReads) throw chain.failReads;
      return chain.height;
    },
    async getTokenAccounts(owner, mint, program) {
      calls.push({ method: 'getTokenAccounts', args: [owner, mint, program] });
      if (chain.failReads) throw chain.failReads;
      return chain.tokenAccounts;
    },
    async sendTransaction(wire) {
      calls.push({ method: 'sendTransaction', args: [wire] });
      if (chain.failSend) throw chain.failSend;
      chain.sent.push(wire);
      // The node answers with the first signature of the wire.
      return base58Encode(wire.slice(1, 65));
    },
    async getSignatureStatus(signature) {
      calls.push({ method: 'getSignatureStatus', args: [signature] });
      if (chain.failReads) throw chain.failReads;
      return chain.statuses.get(signature) ?? null;
    },
    async getSignatures(address, limit) {
      calls.push({ method: 'getSignatures', args: [address, limit] });
      if (chain.failReads) throw chain.failReads;
      return chain.listing.slice(0, limit);
    },
    async getTransactionWire(signature) {
      calls.push({ method: 'getTransactionWire', args: [signature] });
      if (chain.failReads) throw chain.failReads;
      return chain.landed.get(signature) ?? null;
    },
  };
  return chain;
}

/** Make a landed transaction appear on the fake node: its wire, its listing entry and its status. */
export function landOn(chain: FakeChain, wire: Bytes, confirmation: SignatureStatus['confirmation'] = 'finalized', failed = false): string {
  const signature = base58Encode(wire.slice(1, 65));
  chain.landed.set(signature, wire);
  chain.listing.unshift({ signature, failed });
  chain.statuses.set(signature, { confirmation, failed, slot: 500n });
  return signature;
}

export function wireMessage(wire: Bytes): Bytes {
  const message = messageOfWire(wire);
  if (!message) throw new Error('not a wire transaction');
  return message;
}

// ---------------------------------------------------------------------------
// A scriptable wallet
// ---------------------------------------------------------------------------

export interface FakeWallet extends SolanaWalletSession {
  calls: Array<{ method: 'signTransaction' | 'signAndSendTransaction'; transaction: Bytes; chain: string }>;
  /** What the next sign call does. Defaults: sign our message with signature 0xe5*64. */
  onSign: (transaction: Bytes, chain: string) => Promise<Bytes>;
  onSignAndSend: (transaction: Bytes, chain: string) => Promise<Bytes>;
  /** Mutable so a spec can switch the account or the network. */
  state: { address: string; chains: readonly string[] };
}

export function signWith(signature: Bytes): (transaction: Bytes) => Promise<Bytes> {
  return async (transaction) => {
    const out = transaction.slice();
    out.set(signature, 1);
    return out;
  };
}

export function makeFakeWallet(options: { signs?: boolean; sendsAndSigns?: boolean; address?: string; chains?: readonly string[] } = {}): FakeWallet {
  const state = { address: options.address ?? KEYS.holder, chains: options.chains ?? [SOLANA_MAINNET.chain] };
  const calls: FakeWallet['calls'] = [];
  const wallet: FakeWallet = {
    walletName: 'Fake Wallet',
    get address() {
      return state.address;
    },
    get chains() {
      return state.chains;
    },
    canSign: options.signs ?? true,
    canSignAndSend: options.sendsAndSigns ?? false,
    calls,
    state,
    onSign: async (transaction) => signWith(fill(0xe5, 64))(transaction),
    onSignAndSend: async () => fill(0xe5, 64),
    async signTransaction(transaction, chain) {
      calls.push({ method: 'signTransaction', transaction, chain });
      return wallet.onSign(transaction, chain);
    },
    async signAndSendTransaction(transaction, chain) {
      calls.push({ method: 'signAndSendTransaction', transaction, chain });
      return wallet.onSignAndSend(transaction, chain);
    },
  };
  return wallet;
}

export function rejection(message = 'User rejected the request.'): Error {
  return Object.assign(new Error(message), { code: 4001 });
}

export { fromHex, toHex };

// ---------------------------------------------------------------------------
// A Wallet Standard wallet and registry (the shape `@wallet-standard/app` serves)
// ---------------------------------------------------------------------------

type Listener = (properties: { accounts?: readonly WalletAccount[] }) => void;

export function makeAccount(address: string, chains: string[] = ['solana:mainnet']): WalletAccount {
  return { address, publicKey: new Uint8Array(32), chains, features: [] } as unknown as WalletAccount;
}

export interface FakeStandardWallet extends Wallet {
  connects: number;
  disconnects: number;
  signs: Array<{ transaction: Uint8Array; chain?: string }>;
  emit(properties: { accounts?: readonly WalletAccount[] }): void;
  offs: number;
  /** What `solana:signTransaction` returns for the transaction it was given. Defaults to the transaction itself. */
  signer: (transaction: Uint8Array) => Uint8Array;
}

export function makeStandardWallet(name: string, accounts: WalletAccount[], features: Record<string, unknown> = {}, chains: string[] = ['solana:mainnet']): FakeStandardWallet {
  const listeners = new Set<Listener>();
  const w: Record<string, unknown> = {
    version: '1.0.0',
    name,
    icon: 'data:image/png;base64,AAAA',
    chains,
    accounts: [],
    connects: 0,
    disconnects: 0,
    signs: [] as Array<{ transaction: Uint8Array; chain?: string }>,
    offs: 0,
    signer: (transaction: Uint8Array) => transaction,
    emit: (properties: { accounts?: readonly WalletAccount[] }) => listeners.forEach((l) => l(properties)),
  };
  w.features = {
    [STANDARD_CONNECT]: { version: '1.0.0', connect: async () => ((w.connects as number)++, { accounts }) },
    [STANDARD_DISCONNECT]: { version: '1.0.0', disconnect: async () => void ((w.disconnects as number)++) },
    [STANDARD_EVENTS]: {
      version: '1.0.0',
      on: (_event: 'change', listener: Listener) => {
        listeners.add(listener);
        return () => {
          listeners.delete(listener);
          (w.offs as number)++;
        };
      },
    },
    [SOLANA_SIGN_TRANSACTION]: {
      version: '1.0.0',
      supportedTransactionVersions: ['legacy'],
      signTransaction: async (input: { transaction: Uint8Array; chain?: string }) => {
        (w.signs as unknown[]).push(input);
        return [{ signedTransaction: (w.signer as (t: Uint8Array) => Uint8Array)(input.transaction) }];
      },
    },
    ...features,
  };
  return w as unknown as FakeStandardWallet;
}

export function makeRegistry(initial: Wallet[]): WalletRegistry & { add(w: Wallet): void; remove(w: Wallet): void; getCalls: number } {
  const wallets = [...initial];
  const listeners = { register: new Set<(...w: Wallet[]) => void>(), unregister: new Set<(...w: Wallet[]) => void>() };
  const api = {
    getCalls: 0,
    get() {
      api.getCalls++;
      return wallets;
    },
    on(event: 'register' | 'unregister', listener: (...w: Wallet[]) => void) {
      listeners[event].add(listener);
      return () => listeners[event].delete(listener);
    },
    add(w: Wallet) {
      wallets.push(w);
      listeners.register.forEach((l) => l(w));
    },
    remove(w: Wallet) {
      wallets.splice(wallets.indexOf(w), 1);
      listeners.unregister.forEach((l) => l(w));
    },
  };
  return api;
}
