/**
 * Test double for an EIP-1193 provider. Used only by specs.
 *
 * It records every request, serves a small set of default answers, lets a test
 * override or script any method, and exposes `emit` to fire provider events.
 */

import { Principal } from '@dfinity/principal';
import type {
  BridgeInfo,
  DepositView,
  LaneStatus,
  ReleaseView,
} from '$declarations/rumi_icusd_bridge/rumi_icusd_bridge.did';
import type { Keccak256 } from './keccak';
import type { Eip1193Provider } from './evmWallet';

export type RequestHandler = (params: unknown[] | undefined, call: { method: string }) => unknown | Promise<unknown>;

export interface FakeProviderOptions {
  account?: string | null;
  chainId?: number;
  handlers?: Record<string, RequestHandler>;
}

export interface FakeProvider extends Eip1193Provider {
  calls: Array<{ method: string; params?: unknown }>;
  handlers: Record<string, RequestHandler>;
  state: { account: string | null; chainId: number };
  listeners: Map<string, Set<(...args: any[]) => void>>;
  emit(event: string, ...args: unknown[]): void;
  listenerCount(): number;
  callsTo(method: string): Array<{ method: string; params?: unknown }>;
}

export function createFakeProvider(options: FakeProviderOptions = {}): FakeProvider {
  const state = {
    account: options.account === undefined ? '0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' : options.account,
    chainId: options.chainId ?? 8453,
  };
  const listeners = new Map<string, Set<(...args: any[]) => void>>();
  const calls: Array<{ method: string; params?: unknown }> = [];
  const handlers: Record<string, RequestHandler> = { ...(options.handlers ?? {}) };

  const provider: FakeProvider = {
    calls,
    handlers,
    state,
    listeners,
    async request({ method, params }) {
      calls.push({ method, params });
      const handler = handlers[method];
      if (handler) return handler(params as unknown[] | undefined, { method });
      switch (method) {
        case 'eth_accounts':
        case 'eth_requestAccounts':
          return state.account ? [state.account] : [];
        case 'eth_chainId':
          return `0x${state.chainId.toString(16)}`;
        case 'wallet_switchEthereumChain': {
          const target = (params as Array<{ chainId: string }>)[0].chainId;
          state.chainId = Number(BigInt(target));
          return null;
        }
        default:
          throw new Error(`Unexpected EVM method ${method}`);
      }
    },
    on(event, listener) {
      if (!listeners.has(event)) listeners.set(event, new Set());
      listeners.get(event)!.add(listener);
    },
    removeListener(event, listener) {
      listeners.get(event)?.delete(listener);
    },
    emit(event, ...args) {
      for (const listener of [...(listeners.get(event) ?? [])]) listener(...args);
    },
    listenerCount() {
      let total = 0;
      for (const set of listeners.values()) total += set.size;
      return total;
    },
    callsTo(method) {
      return calls.filter((call) => call.method === method);
    },
  };
  return provider;
}

/** A provider-shaped error like the ones wallets throw. */
export function walletError(code: number, message = 'wallet error', data?: unknown): Error & { code: number; data?: unknown } {
  return Object.assign(new Error(message), { code, data });
}

/** In-memory Storage for specs. */
export function createMemoryStorage(initial: Record<string, string> = {}) {
  const map = new Map<string, string>(Object.entries(initial));
  return {
    map,
    getItem: (key: string) => (map.has(key) ? map.get(key)! : null),
    setItem: (key: string, value: string) => void map.set(key, value),
    removeItem: (key: string) => void map.delete(key),
  };
}

// ---------------------------------------------------------------------------
// Candid view factories
// ---------------------------------------------------------------------------

export const TEST_OWNER = Principal.fromText('zegjz-jpi6k-qkand-c2bgf-qw6za-xk4si-nz3gx-qzzia-fk6fg-snepb-tae');
export const TEST_LEDGER = Principal.fromText('t6bor-paaaa-aaaap-qrd5q-cai');
export const TEST_BRIDGE_ID = 'tfesu-vyaaa-aaaap-qrd7a-cai';

export function hexRequestId(byte = 0xab): Uint8Array {
  return new Uint8Array(16).fill(byte);
}

export function makeDeposit(overrides: Partial<DepositView> = {}): DepositView {
  return {
    id: 1n,
    base_recipient: '0x1111111111111111111111111111111111111111',
    status: { PullPending: null },
    mint_tx_hashes: [],
    client_request_id: hexRequestId(),
    refund_amount_e8s: [],
    owner: TEST_OWNER,
    from: [],
    kind: { UserDeposit: null },
    mint_stage: [],
    mint_id: `0x${'aa'.repeat(32)}`,
    pull_block_index: [],
    mint_amount_e8s: 2_499_990_000n,
    updated_at_ns: 1_700_000_000_000_000_000n,
    created_at_ns: 1_700_000_000_000_000_000n,
    amount_e8s: 2_500_000_000n,
    delivered: [],
    refund_block_index: [],
    deposit_fee_e8s: 10_000n,
    ...overrides,
  };
}

export function makeRelease(overrides: Partial<ReleaseView> = {}): ReleaseView {
  return {
    status: { Pending: null },
    destination: [],
    raw_icp_principal: TEST_OWNER.toUint8Array(),
    log_index: 0n,
    release_block_index: [],
    base_sender: '0x1111111111111111111111111111111111111111',
    updated_at_ns: 1_700_000_000_000_000_000n,
    transfer_id: `0x${'ef'.repeat(32)}`,
    created_at_ns: 1_700_000_000_000_000_000n,
    amount_e8s: 100_000_000n,
    release_fee_e8s: 0n,
    ledger_fee_e8s: [],
    base_tx_hash: `0x${'12'.repeat(32)}`,
    base_block_number: 100n,
    amount_to_receive_e8s: [],
    icp_subaccount: new Uint8Array(32),
    sequence: 1n,
    ...overrides,
  };
}

export function makeLane(lane: 'Minter' | 'Admin', overrides: Partial<LaneStatus> = {}): LaneStatus {
  return {
    next_nonce: [0n],
    unfinalized_txs: 0,
    lane: lane === 'Minter' ? { Minter: null } : { Admin: null },
    address: [lane === 'Minter' ? '0x3333333333333333333333333333333333333333' : '0x4444444444444444444444444444444444444444'],
    awaiting_gas: false,
    halted: [],
    unincluded_tx: [],
    ...overrides,
  };
}

export function makeBridgeInfo(overrides: Partial<BridgeInfo> = {}): BridgeInfo {
  return {
    chain_id: 8453n,
    contract: [
      {
        address: '0x2222222222222222222222222222222222222222',
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
      max_outstanding_e8s: 10n ** 15n,
      max_active_per_caller: 3,
      max_active_global: 100,
      window_seconds: 86_400n,
      mint_window_limit_e8s: 10n ** 14n,
      release_window_limit_e8s: 10n ** 14n,
    },
    fees: { deposit_fee_e8s: 10_000n, release_fee_e8s: 0n },
    ledger_fee_e8s: [100_000n],
    icusd_ledger: TEST_LEDGER,
    pauses: { mints: false, observer: false, releases: false, deposits: false },
    halted: [],
    lanes: [makeLane('Minter'), makeLane('Admin')],
    version: 'rumi-base-icusd-bridge-v1',
    ...overrides,
  } as unknown as BridgeInfo;
}

// ---------------------------------------------------------------------------
// EIP-55 fixtures
// ---------------------------------------------------------------------------

/** The four test vectors published in EIP-55. */
export const EIP55_VECTORS = [
  '0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed',
  '0xfB6916095ca1df60bB79Ce92cE3Ea74c37c5d359',
  '0xdbF03B407c01E7cD3CBea99509d93f8DDDC8C6FB',
  '0xD1220A0cf47c7B9Be7A2E6BA89F429762e7b9aDb',
] as const;

/** Mixed case that was never checksummed (its real checksum is 0xabCDeF...). */
export const MIXED_CASE_NO_CHECKSUM = '0xAbCdEf0123456789aBcDeF0123456789AbCdEf01';

/**
 * Keccak-256 digests of the lowercase 40-character hex text (no 0x) of each
 * fixture address, produced offline with @noble/hashes. They let the specs
 * exercise the checksum logic against the official vectors without shipping a
 * hash implementation in the app.
 */
const ORACLE_DIGESTS: Record<string, string> = {
  '5aaeb6053f3e94c9b9a09f33669435e7ef1beaed': 'd385650ce8fdc6db7ee3a091d34814dbc4ce18219ffae52182efff4034d707e5',
  fb6916095ca1df60bb79ce92ce3ea74c37c5d359: '5cfac663f45837b409c4d3dc1cef5f4759734f4989dd53a31b1265734c0b28f4',
  dbf03b407c01e7cd3cbea99509d93f8dddc8c6fb: '75cd3958e251de0c49f54da99b77f79adbef92caed36af8e81f3a7ddbde17bb9',
  d1220a0cf47c7b9be7a2e6ba89f429762e7b9adb: 'c8bc5d10249238b92acb838a86d883bb9253c4b02ceb1b3f927d0c3ec09eef6c',
  abcdef0123456789abcdef0123456789abcdef01: '46fb2b0307483bc5d321b05a192734e27f9e8f89b6af29dd2a71787cb309f558',
};

function digestBytes(hex: string): Uint8Array {
  const out = new Uint8Array(hex.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  return out;
}

/** A table-backed Keccak-256: answers only for the fixture inputs and throws for anything else. */
export const ORACLE: Keccak256 = (data) => {
  const text = String.fromCharCode(...Array.from(data));
  const digest = ORACLE_DIGESTS[text];
  if (!digest) throw new Error(`oracle has no digest for ${JSON.stringify(text)}`);
  return digestBytes(digest);
};

/**
 * Like ORACLE, but answers an all-zero digest for any other input (so every nibble is below 8 and the
 * checksum of an unknown address is its lowercase form). Safe for page specs that display arbitrary addresses.
 */
export const ORACLE_OR_ZERO: Keccak256 = (data) => {
  const digest = ORACLE_DIGESTS[String.fromCharCode(...Array.from(data))];
  return digest ? digestBytes(digest) : new Uint8Array(32);
};

/** Flip the case of the hex character at `index` (0-based, after the 0x). */
export function flipCase(address: string, index: number): string {
  const body = address.slice(2).split('');
  const c = body[index];
  body[index] = c === c.toUpperCase() ? c.toLowerCase() : c.toUpperCase();
  return `0x${body.join('')}`;
}
