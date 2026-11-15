/**
 * Keyless Solana read and broadcast adapter.
 *
 * It holds no key and signs nothing: it asks a JSON-RPC node for a recent blockhash and the block height,
 * lists the holder's token accounts, broadcasts bytes a wallet already signed, and looks a signature or a
 * transaction up. Every flow talks to the `SolanaRpc` interface, so the specs inject a fake and no public
 * network is ever contacted by a test. The fetch-based implementation below is exercised only against a mocked
 * `fetch`.
 *
 * An RPC answer is a hint about the chain, not settlement: the bridge canister settles a return from its
 * own finalized reads. These answers only drive what the page shows and whether the holder may be offered a
 * safe next step.
 */

import type { Bytes } from './bytes';

export type Commitment = 'processed' | 'confirmed' | 'finalized';

export interface BlockhashInfo {
  /** Base58 blockhash. */
  blockhash: string;
  lastValidBlockHeight: bigint;
}

export interface SignatureStatus {
  /** The highest commitment the node reports for the signature. */
  confirmation: Commitment;
  /** The transaction failed in execution (it still landed and paid its fee). */
  failed: boolean;
  slot: bigint;
}

export interface HolderTokenAccount {
  /** Base58 token account address. */
  address: string;
  /** Raw amount in e8s. */
  amountE8s: bigint;
}

export interface SignatureListing {
  signature: string;
  failed: boolean;
}

export interface SolanaRpc {
  getLatestBlockhash(): Promise<BlockhashInfo>;
  getBlockHeight(): Promise<bigint>;
  /** Token accounts of `owner` for `mint` under `tokenProgram` (base58 each). */
  getTokenAccounts(owner: string, mint: string, tokenProgram: string): Promise<HolderTokenAccount[]>;
  /** Broadcast signed wire bytes; resolves to the signature the node reports. */
  sendTransaction(wire: Bytes): Promise<string>;
  /** Null when the node does not know the signature (not landed, or dropped, or too old). */
  getSignatureStatus(signature: string): Promise<SignatureStatus | null>;
  /** Recent signatures that mention `address`, newest first. */
  getSignatures(address: string, limit: number): Promise<SignatureListing[]>;
  /** The wire bytes of a landed transaction, or null. */
  getTransactionWire(signature: string): Promise<Bytes | null>;
}

export class SolanaRpcError extends Error {
  readonly code: number | null;
  constructor(message: string, code: number | null = null) {
    super(message);
    this.name = 'SolanaRpcError';
    this.code = code;
  }
}

type Fetch = (input: string, init: { method: 'POST'; headers: Record<string, string>; body: string }) => Promise<{ ok: boolean; status: number; json(): Promise<unknown> }>;

function toBase64(bytes: Bytes): string {
  let binary = '';
  for (let i = 0; i < bytes.length; i++) binary += String.fromCharCode(bytes[i]);
  return btoa(binary);
}

function fromBase64(text: string): Bytes {
  const binary = atob(text);
  const out = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) out[i] = binary.charCodeAt(i);
  return out;
}

/** A JSON-RPC client over `fetch`. `fetchImpl` is injected by the specs. */
export function createFetchSolanaRpc(url: string, fetchImpl: Fetch = (input, init) => fetch(input, init) as never): SolanaRpc {
  let id = 0;
  async function call<T>(method: string, params: unknown[]): Promise<T> {
    id += 1;
    let response;
    try {
      response = await fetchImpl(url, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ jsonrpc: '2.0', id, method, params }),
      });
    } catch (cause) {
      throw new SolanaRpcError(`The Solana node could not be reached (${cause instanceof Error ? cause.message : String(cause)}).`);
    }
    if (!response.ok) throw new SolanaRpcError(`The Solana node answered HTTP ${response.status}.`);
    const body = (await response.json()) as { result?: T; error?: { code?: number; message?: string } };
    if (body.error) throw new SolanaRpcError(body.error.message ?? 'The Solana node returned an error.', body.error.code ?? null);
    return body.result as T;
  }

  return {
    async getLatestBlockhash() {
      const r = await call<{ value: { blockhash: string; lastValidBlockHeight: number | string } }>('getLatestBlockhash', [{ commitment: 'confirmed' }]);
      return { blockhash: r.value.blockhash, lastValidBlockHeight: BigInt(r.value.lastValidBlockHeight) };
    },
    async getBlockHeight() {
      return BigInt(await call<number>('getBlockHeight', [{ commitment: 'confirmed' }]));
    },
    async getTokenAccounts(owner, mint, tokenProgram) {
      const r = await call<{ value: Array<{ pubkey: string; account: { owner: string; data: { parsed?: { info?: { tokenAmount?: { amount?: string } } } } } }> }>(
        'getTokenAccountsByOwner',
        [owner, { mint }, { encoding: 'jsonParsed', commitment: 'confirmed' }],
      );
      return r.value
        .filter((a) => a.account.owner === tokenProgram)
        .map((a) => ({ address: a.pubkey, amountE8s: BigInt(a.account.data.parsed?.info?.tokenAmount?.amount ?? '0') }));
    },
    async sendTransaction(wire) {
      return call<string>('sendTransaction', [toBase64(wire), { encoding: 'base64', preflightCommitment: 'confirmed' }]);
    },
    async getSignatureStatus(signature) {
      const r = await call<{ value: Array<null | { confirmationStatus?: Commitment; err: unknown; slot: number }> }>('getSignatureStatuses', [
        [signature],
        { searchTransactionHistory: true },
      ]);
      const status = r.value[0];
      if (!status) return null;
      return { confirmation: status.confirmationStatus ?? 'processed', failed: status.err !== null && status.err !== undefined, slot: BigInt(status.slot) };
    },
    async getSignatures(address, limit) {
      const r = await call<Array<{ signature: string; err: unknown }>>('getSignaturesForAddress', [address, { limit, commitment: 'confirmed' }]);
      return r.map((e) => ({ signature: e.signature, failed: e.err !== null && e.err !== undefined }));
    },
    async getTransactionWire(signature) {
      const r = await call<null | { transaction: [string, string] }>('getTransaction', [
        signature,
        { encoding: 'base64', maxSupportedTransactionVersion: 0, commitment: 'confirmed' },
      ]);
      return r ? fromBase64(r.transaction[0]) : null;
    },
  };
}

/** One deployment-time endpoint, keyed to the Solana network it serves and that network's genesis hash. */
export interface SolanaRpcEndpoint {
  networkId: string;
  genesisHash: string;
  url: string;
}

function usableEndpointUrl(raw: string): string | null {
  const text = raw.trim();
  if (text === '') return null;
  let url: URL;
  try {
    url = new URL(text);
  } catch {
    return null;
  }
  const local = url.hostname === 'localhost' || url.hostname === '127.0.0.1' || url.hostname === '[::1]';
  if (url.protocol === 'https:' || (url.protocol === 'http:' && local)) return url.toString();
  return null;
}

/**
 * The keyless adapter for a network, from the deployment-time endpoint list. Null when no entry matches BOTH the network id
 * and its genesis hash, or when the matching entry's URL is empty, malformed or insecure. With the shipped (empty) list it
 * is always null; no node is contacted here, and the adapter only talks when a flow calls it.
 */
export function resolveSolanaRpc(
  network: { networkId: string; genesisHash: string },
  endpoints: readonly SolanaRpcEndpoint[],
  fetchImpl?: Fetch,
): SolanaRpc | null {
  const entry = endpoints.find((e) => e.networkId === network.networkId && e.genesisHash === network.genesisHash);
  if (!entry) return null;
  const url = usableEndpointUrl(entry.url);
  return url ? createFetchSolanaRpc(url, fetchImpl) : null;
}
