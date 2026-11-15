/**
 * Test doubles for the Sui bridge branch. Used only by specs.
 *
 * - `FakeSui` is an in-memory Sui node behind the `SuiRpc` interface: it never touches a network.
 * - `makeFakeWallet` and `makeStandardWallet` are scriptable wallets. They build the transaction from the SDK's JSON the way
 *   a real Wallet Standard wallet does (gas data added, then signed), with a TEST-ONLY deterministic key made from a public
 *   constant seed. That key is not any account's key: nothing in this file reads a keystore, a CLI config or a network.
 * - the config / view factories build the instance's answers.
 */

import { Principal } from '@dfinity/principal';
import { Transaction } from '@mysten/sui/transactions';
import { Ed25519Keypair } from '@mysten/sui/keypairs/ed25519';
import { toBase64 } from '@mysten/sui/utils';
import type { Wallet, WalletAccount } from '@wallet-standard/base';
import type {
  CandidateView,
  ConfigView,
  DepositView,
  MintLaneView,
  ObservedDeploymentView,
  ReleaseView,
  RoleView,
} from '$declarations/rumi_icusd_sui_bridge/rumi_icusd_sui_bridge.did';
import { suiDestination, type DestinationView } from '../baseBridge/destinations';
import { makeSigning, type TestSigning } from '../baseBridge/testSupport';
import { base58Encode, fromHex, type Bytes } from '../solanaBridge/bytes';
import { resolveSuiConfig, type SuiBridgeConfig } from './config';
import { SUI_MAINNET, SUI_TESTNET } from './networks';
import type { SuiReturnIntent } from './intents';
import type { SuiCoin, SuiExecution, SuiRpc, SuiTxStatus } from './rpc';
import { digestOf } from './transaction';
import {
  STANDARD_CONNECT,
  STANDARD_DISCONNECT,
  STANDARD_EVENTS,
  SUI_SIGN_TRANSACTION,
  type SuiExecutedTransaction,
  type SuiSignedTransaction,
  type SuiTransactionLike,
  type SuiWalletSession,
  type WalletRegistry,
} from './walletStandard';

const fill = (byte: number, n = 32): Bytes => new Uint8Array(n).fill(byte);
const addr = (byte: number) => `0x${byte.toString(16).padStart(2, '0').repeat(32)}`;

/** A TEST-ONLY key made from a public constant. It is not the key of any account. */
export const TEST_KEYPAIR = Ed25519Keypair.fromSecretKey(fill(0x44));
export const OTHER_KEYPAIR = Ed25519Keypair.fromSecretKey(fill(0x45));

export const KEYS = {
  packageId: addr(0x70),
  gateId: addr(0x6a),
  mintCap: addr(0x6d),
  adminCap: addr(0x6e),
  treasury: addr(0x81),
  gas: addr(0x82),
  custody: addr(0x83),
  /** The holder: the address of TEST_KEYPAIR. */
  holder: TEST_KEYPAIR.toSuiAddress(),
  otherHolder: OTHER_KEYPAIR.toSuiAddress(),
  coinA: addr(0xc1),
  coinB: addr(0xc2),
  gasCoin: addr(0x67),
};

/** A real-looking, canonical instance canister id (not the management canister). */
export const SUI_BRIDGE_ID = 'rrkah-fqaaa-aaaaa-aaaaq-cai';
export const SUI_LEDGER_ID = 't6bor-paaaa-aaaap-qrd5q-cai';
export const TEST_OWNER_TEXT = 'zegjz-jpi6k-qkand-c2bgf-qw6za-xk4si-nz3gx-qzzia-fk6fg-snepb-tae';

export const COIN_TYPE = `${KEYS.packageId}::rumi_icusd_gate::RUMI_ICUSD_GATE`;
export const GATE_TYPE = `${KEYS.packageId}::rumi_icusd_gate::Gate`;

export function makeSuiConfigView(overrides: Partial<ConfigView> = {}, network = SUI_MAINNET): ConfigView {
  return {
    signing_enabled: false,
    bridge_version: 'rumi-sui-icusd-bridge-v1',
    initialized: true,
    fee_recipient: [],
    fees: { release_fee_e8s: 20_000n, deposit_fee_e8s: 10_000_000n },
    mirror: { max_response_bytes: 0, min_quorum: 2, enabled: true, min_independent_providers: 2, providers: [] },
    network: network.role === 'mainnet' ? { Mainnet: null } : { Testnet: null },
    escrow_subaccount: 'ee'.repeat(32),
    schema_version: 1,
    binding: [
      {
        decimals: 8,
        admin_cap_id: KEYS.adminCap,
        coin_type: COIN_TYPE,
        mint_cap_id: KEYS.mintCap,
        gate_id: KEYS.gateId,
        gate_type: GATE_TYPE,
        package_id: KEYS.packageId,
      },
    ],
    schnorr_key_name: network.role === 'mainnet' ? 'key_1' : 'test_key_1',
    short_chain_id: network.chainIdentifier.slice(0, 8),
    admins: [],
    halted: [],
    chain_identifier: network.chainIdentifier,
    icusd_ledger: Principal.fromText(SUI_LEDGER_ID),
    instance_salt: 'ab'.repeat(32),
    limits: {
      max_active_per_caller: 3,
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

export function makeRoles(overrides: Partial<Record<'Treasury' | 'Gas' | 'UpgradeCustody', string | null>> = {}): RoleView[] {
  const entry = (role: RoleView['role'], address: string | null): RoleView => ({ role, public_key: [], derivation_path: [], address: address ? [address] : [] });
  return [
    entry({ Treasury: null }, overrides.Treasury === undefined ? KEYS.treasury : overrides.Treasury),
    entry({ Gas: null }, overrides.Gas === undefined ? KEYS.gas : overrides.Gas),
    entry({ UpgradeCustody: null }, overrides.UpgradeCustody === undefined ? KEYS.custody : overrides.UpgradeCustody),
  ];
}

export function makeObserved(overrides: Partial<ObservedDeploymentView> = {}): ObservedDeploymentView {
  return {
    bridge_out_count: 0n,
    decimals: 8,
    mint_count: 0n,
    supply_cap: 1_000_000_000_000n,
    mint_cap_owner: KEYS.treasury,
    initial_shared_version: 42n,
    burned_total: 0n,
    checkpoint: 1_000n,
    mint_cap_version: 7n,
    observed_at_ns: 1_700_000_000_000_000_000n,
    gate_version: 1n,
    admin_cap_id: KEYS.adminCap,
    min_bridge_out: 1n,
    admin_cap_role: [{ UpgradeCustody: null }],
    epoch: 123n,
    mint_cap_id: KEYS.mintCap,
    admin_cap_owner: KEYS.custody,
    // What the accepted canister really reports (bind.rs always stores false: the module bytes are not compared, MC-DEF-13).
    module_digest_observed: false,
    total_supply: 0n,
    minted_total: 0n,
    symbol: 'icUSD',
    checkpoint_digest: base58Encode(fill(0x99)),
    ...overrides,
  };
}

export function makeLane(overrides: Partial<MintLaneView> = {}): MintLaneView {
  return {
    active_op: [],
    observed: [makeObserved()],
    gas_budget: 5_000_000n,
    last_final_checkpoint: 1_000n,
    max_attempts: 3,
    blocked_by: [],
    gas_coin: [KEYS.gasCoin],
    last_wait: [],
    lane_halt: [],
    ready: true,
    ...overrides,
  };
}

export function makeSuiConfig(overrides: { config?: Partial<ConfigView>; observed?: Partial<ObservedDeploymentView> | null; lane?: Partial<MintLaneView>; network?: typeof SUI_MAINNET } = {}): SuiBridgeConfig {
  const result = resolveSuiConfig(
    makeSuiConfigView(overrides.config, overrides.network ?? SUI_MAINNET),
    makeRoles(),
    makeLane(overrides.lane),
    overrides.observed === null ? null : makeObserved(overrides.observed),
    SUI_BRIDGE_ID,
  );
  if (!result.ok) throw new Error(result.message);
  return result.config;
}

export function makeSuiDestination(config: SuiBridgeConfig = makeSuiConfig()): DestinationView {
  return suiDestination({ ok: true, config }, SUI_BRIDGE_ID);
}

export function makeSuiSigning(destination: DestinationView = makeSuiDestination()): TestSigning {
  return makeSigning(destination);
}

export { SUI_MAINNET, SUI_TESTNET };

// ---------------------------------------------------------------------------
// Canister views
// ---------------------------------------------------------------------------

export function makeSuiDeposit(overrides: Partial<DepositView> = {}): DepositView {
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
    created_at_ns: 1_700_000_000_000_000_000n,
    amount_e8s: 200_000_000n,
    delivered: [],
    refund_block_index: [],
    sui_recipient: KEYS.holder,
    deposit_fee_e8s: 10_000_000n,
    mint_op_id: [],
    ...overrides,
  };
}

export function makeSuiRelease(overrides: Partial<ReleaseView> = {}): ReleaseView {
  return {
    seq: 1n,
    status: { Pending: null },
    return_id: 'aa'.repeat(32),
    destination: { owner: Principal.fromText(TEST_OWNER_TEXT), subaccount: [] },
    sui_sender: KEYS.holder,
    checkpoint: 1_001n,
    release_block_index: [],
    updated_at_ns: 1_700_000_000_000_000_000n,
    gate_sequence: 1n,
    created_at_ns: 1_700_000_000_000_000_000n,
    amount_e8s: 300_000_000n,
    release_fee_e8s: 20_000n,
    tx_digest: base58Encode(fill(0x98)),
    ledger_fee_e8s: [10_000n],
    amount_to_receive_e8s: [],
    client_nonce: 1n,
    ...overrides,
  };
}

/** The canister's release record for EXACTLY this attempt: its transfer id, sender, nonce, amount, ICP account and burn digest. */
export function releaseFor(intent: SuiReturnIntent, overrides: Partial<ReleaseView> = {}): ReleaseView {
  return makeSuiRelease({
    return_id: intent.transferId,
    sui_sender: intent.sender,
    client_nonce: BigInt(intent.nonce),
    amount_e8s: intent.amountE8s,
    destination: { owner: Principal.fromText(intent.icpPrincipal), subaccount: intent.icpSubaccount ? [fromHex(intent.icpSubaccount)!] : [] },
    tx_digest: intent.digest ?? makeSuiRelease().tx_digest,
    ...overrides,
  });
}

export function makeCandidate(overrides: Partial<CandidateView> = {}): CandidateView {
  return {
    verified: false,
    tries: 0,
    sui_sender: KEYS.holder,
    source: { Hint: null },
    claimed_amount_e8s: 300_000_000n,
    claimed_sequence: 1n,
    tx_digest: base58Encode(fill(0x98)),
    waiting: [],
    client_nonce: 1n,
    ...overrides,
  };
}

// ---------------------------------------------------------------------------
// A fake Sui node
// ---------------------------------------------------------------------------

export interface FakeSui extends SuiRpc {
  calls: Array<{ method: string; args: unknown[] }>;
  callsTo(method: string): Array<{ method: string; args: unknown[] }>;
  coins: SuiCoin[];
  /** SUI balance in MIST (9 decimals): gas. */
  gas: bigint;
  /** Digest -> status. Missing: unknown to the node. */
  statuses: Map<string, SuiTxStatus>;
  /** What `executeTransaction` was handed. */
  executed: Array<{ bytes: Uint8Array; signatures: string[] }>;
  failExecute: Error | null;
  failReads: Error | null;
}

const COIN_DIGEST = base58Encode(fill(0x57));

export function coin(objectId: string, balance: bigint, version = '7'): SuiCoin {
  return { objectId, version, digest: COIN_DIGEST, balance };
}

export function createFakeSui(init: { coins?: SuiCoin[]; gas?: bigint } = {}): FakeSui {
  const calls: FakeSui['calls'] = [];
  const node: FakeSui = {
    calls,
    callsTo: (method) => calls.filter((c) => c.method === method),
    coins: init.coins ?? [coin(KEYS.coinA, 5_000_000_000n)],
    gas: init.gas ?? 2_000_000_000n,
    statuses: new Map(),
    executed: [],
    failExecute: null,
    failReads: null,
    async getChainIdentifier() {
      calls.push({ method: 'getChainIdentifier', args: [] });
      if (node.failReads) throw node.failReads;
      return SUI_MAINNET.chainIdentifierBase58;
    },
    async getReferenceGasPrice() {
      calls.push({ method: 'getReferenceGasPrice', args: [] });
      if (node.failReads) throw node.failReads;
      return 1000n;
    },
    async getEpoch() {
      calls.push({ method: 'getEpoch', args: [] });
      if (node.failReads) throw node.failReads;
      return 123n;
    },
    async listCoins(owner, coinType) {
      calls.push({ method: 'listCoins', args: [owner, coinType] });
      if (node.failReads) throw node.failReads;
      return node.coins;
    },
    async getSuiBalance(owner) {
      calls.push({ method: 'getSuiBalance', args: [owner] });
      if (node.failReads) throw node.failReads;
      return node.gas;
    },
    async executeTransaction(bytes, signatures) {
      calls.push({ method: 'executeTransaction', args: [bytes, signatures] });
      if (node.failExecute) throw node.failExecute;
      node.executed.push({ bytes, signatures });
      const digest = digestOf(bytes);
      const result: SuiExecution = { digest, success: true, checkpoint: null, epoch: 123n, error: null };
      return result;
    },
    async getTransactionStatus(digest) {
      calls.push({ method: 'getTransactionStatus', args: [digest] });
      if (node.failReads) throw node.failReads;
      return node.statuses.get(digest) ?? null;
    },
  };
  return node;
}

/** Make a transaction appear on the fake node with a status (default: executed and final). */
export function landOn(node: FakeSui, bytes: Uint8Array, status: Partial<SuiTxStatus> = {}): string {
  const digest = digestOf(bytes);
  node.statuses.set(digest, { success: true, checkpoint: 500n, epoch: 123n, error: null, ...status });
  return digest;
}

// ---------------------------------------------------------------------------
// Wallets
// ---------------------------------------------------------------------------

export const GAS_REF = { objectId: KEYS.gasCoin, version: '9', digest: base58Encode(fill(0x56)) };

/**
 * What a real Wallet Standard wallet does with the SDK's JSON: rebuild it, add the sender's gas data, serialize and sign
 * with the account's key. `mutate` lets a spec play a wallet that changes the transaction.
 */
export async function signLikeAWallet(
  transaction: SuiTransactionLike,
  options: { keypair?: Ed25519Keypair; mutate?: (tx: Transaction) => void } = {},
): Promise<SuiExecutedTransaction> {
  const keypair = options.keypair ?? TEST_KEYPAIR;
  const tx = Transaction.from(await transaction.toJSON());
  tx.setSenderIfNotSet(keypair.toSuiAddress());
  tx.setGasPrice(1000n);
  tx.setGasBudget(5_000_000n);
  tx.setGasPayment([GAS_REF]);
  tx.setExpiration({ Epoch: 123 });
  options.mutate?.(tx);
  const bytes = await tx.build();
  const { signature } = await keypair.signTransaction(bytes);
  return { bytes: toBase64(bytes), signature, digest: digestOf(bytes), effects: '' };
}

export interface FakeWallet extends SuiWalletSession {
  calls: Array<{ method: 'signTransaction' | 'signAndExecuteTransaction'; transaction: SuiTransactionLike; chain: string }>;
  /** What the next sign call does. Defaults: build and sign like a real wallet. */
  onSign: (transaction: SuiTransactionLike, chain: string) => Promise<SuiSignedTransaction>;
  onSignAndExecute: (transaction: SuiTransactionLike, chain: string) => Promise<SuiExecutedTransaction>;
  /** Mutable so a spec can switch the account or the network. */
  state: { address: string; chains: readonly string[] };
}

export function makeFakeWallet(options: { signs?: boolean; executes?: boolean; address?: string; chains?: readonly string[] } = {}): FakeWallet {
  const state = { address: options.address ?? KEYS.holder, chains: options.chains ?? [SUI_MAINNET.chain] };
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
    canSignAndExecute: options.executes ?? false,
    calls,
    state,
    onSign: async (transaction) => {
      const signed = await signLikeAWallet(transaction);
      return { bytes: signed.bytes, signature: signed.signature };
    },
    onSignAndExecute: (transaction) => signLikeAWallet(transaction),
    async signTransaction(transaction, chain) {
      calls.push({ method: 'signTransaction', transaction, chain });
      return wallet.onSign(transaction, chain);
    },
    async signAndExecuteTransaction(transaction, chain) {
      calls.push({ method: 'signAndExecuteTransaction', transaction, chain });
      return wallet.onSignAndExecute(transaction, chain);
    },
  };
  return wallet;
}

/** An explicit user rejection (code 4001) and a lookalike transport error. */
export function rejection(message = 'User rejected the request.'): Error {
  return Object.assign(new Error(message), { code: 4001 });
}

// ---------------------------------------------------------------------------
// A Wallet Standard wallet and registry (the shape `@wallet-standard/app` serves)
// ---------------------------------------------------------------------------

type Listener = (properties: { accounts?: readonly WalletAccount[] }) => void;

export function makeAccount(address: string, chains: string[] = [SUI_MAINNET.chain]): WalletAccount {
  return { address, publicKey: new Uint8Array(32), chains, features: [] } as unknown as WalletAccount;
}

export interface FakeStandardWallet extends Wallet {
  connects: number;
  disconnects: number;
  /** Every `sui:signTransaction` call: the transaction JSON and the chain. */
  signs: Array<{ json: string; chain?: string }>;
  emit(properties: { accounts?: readonly WalletAccount[] }): void;
  offs: number;
  /** How a `sui:signTransaction` call is answered. Defaults to a real wallet's build-and-sign. */
  respond: (transaction: SuiTransactionLike) => Promise<SuiSignedTransaction>;
}

export function makeStandardWallet(name: string, accounts: WalletAccount[], features: Record<string, unknown> = {}, chains: string[] = [SUI_MAINNET.chain]): FakeStandardWallet {
  const listeners = new Set<Listener>();
  const w: Record<string, unknown> = {
    version: '1.0.0',
    name,
    icon: 'data:image/png;base64,AAAA',
    chains,
    accounts: [],
    connects: 0,
    disconnects: 0,
    signs: [] as Array<{ json: string; chain?: string }>,
    offs: 0,
    respond: async (transaction: SuiTransactionLike) => {
      const signed = await signLikeAWallet(transaction);
      return { bytes: signed.bytes, signature: signed.signature };
    },
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
    [SUI_SIGN_TRANSACTION]: {
      version: '2.0.0',
      signTransaction: async (input: { transaction: SuiTransactionLike; chain?: string }) => {
        (w.signs as unknown[]).push({ json: await input.transaction.toJSON(), chain: input.chain });
        return (w.respond as (t: SuiTransactionLike) => Promise<SuiSignedTransaction>)(input.transaction);
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
