/**
 * Keyless Sui node adapter, built on the official `@mysten/sui` SDK's GraphQL client (`SuiGraphQLClient`).
 *
 * It holds no key and signs nothing: it reads the holder's icUSD coins and SUI balance, the reference gas price and the
 * epoch, broadcasts bytes a wallet already signed, and looks a transaction up by its digest. Constructing the SDK client
 * creates no identity, keystore or wallet and makes no request; a request is made only when a flow calls one of these
 * methods. The SDK is imported lazily, so the disabled path never loads it.
 *
 * The adapter is only built for an endpoint configured in `endpoints.ts` (EMPTY in every build today) and keyed to the
 * expected network AND chain identifier. Before it answers anything it also asks the node for its own genesis checkpoint
 * digest and refuses a node on another network, so a URL pointed at the wrong network can never be trusted for coins,
 * gas or the status of a transfer.
 */

import type { SuiNetworkInfo } from './networks';

export type Fetch = typeof fetch;

export interface SuiCoin {
  objectId: string;
  version: string;
  digest: string;
  /** Raw balance in the coin's own units: e8s for icUSD. */
  balance: bigint;
}

export interface SuiTxStatus {
  /** The transaction executed without aborting. */
  success: boolean;
  /** The checkpoint that includes it. Inclusion in a checkpoint is finality. Null while not yet included. */
  checkpoint: bigint | null;
  epoch: bigint | null;
  /** The abort or execution error, when it failed. */
  error: string | null;
}

export interface SuiExecution extends SuiTxStatus {
  digest: string;
}

export interface SuiRpc {
  /** The node's genesis checkpoint digest, base58. */
  getChainIdentifier(): Promise<string>;
  /** MIST per gas unit (SUI has 9 decimals). */
  getReferenceGasPrice(): Promise<bigint>;
  getEpoch(): Promise<bigint>;
  /** Every coin object of this type the owner holds (bounded). */
  listCoins(owner: string, coinType: string): Promise<SuiCoin[]>;
  /** The owner's total SUI balance in MIST (9 decimals): gas, never icUSD. */
  getSuiBalance(owner: string): Promise<bigint>;
  /** Broadcast signed bytes. Resolves the effects the node reports. */
  executeTransaction(bytes: Uint8Array, signatures: string[]): Promise<SuiExecution>;
  /** Null when the node does not know the digest (not executed, not yet indexed, or pruned). Never proof of absence. */
  getTransactionStatus(digest: string): Promise<SuiTxStatus | null>;
}

export class SuiRpcError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'SuiRpcError';
  }
}

/** One deployment-time endpoint, keyed to the Sui network it serves and that network's full chain identifier. */
export interface SuiNodeEndpoint {
  networkId: string;
  chainIdentifier: string;
  url: string;
}

const MAX_COIN_PAGES = 10;
const COIN_PAGE_SIZE = 50;

function toBigInt(value: unknown, what: string): bigint {
  try {
    return BigInt(value as string);
  } catch {
    throw new SuiRpcError(`The Sui node returned an unreadable ${what}.`);
  }
}

function describe(cause: unknown): string {
  return cause instanceof Error && cause.message ? cause.message : String(cause);
}

function statusOf(tx: { status: { success: boolean; error?: { message?: string } | null }; checkpoint: string | null; epoch: string | null }): SuiTxStatus {
  return {
    success: tx.status.success,
    checkpoint: tx.checkpoint === null || tx.checkpoint === undefined ? null : toBigInt(tx.checkpoint, 'checkpoint'),
    epoch: tx.epoch === null || tx.epoch === undefined ? null : toBigInt(tx.epoch, 'epoch'),
    error: tx.status.success ? null : (tx.status.error?.message ?? 'The transaction failed.'),
  };
}

/** The adapter over the SDK's GraphQL client. `fetchImpl` is injected by the specs. */
export function createGraphqlSuiRpc(options: { url: string; network: SuiNetworkInfo; fetchImpl?: Fetch }): SuiRpc {
  const { url, network } = options;
  const fetchImpl = options.fetchImpl ?? ((input: RequestInfo | URL, init?: RequestInit) => fetch(input, init));
  type Client = import('@mysten/sui/graphql').SuiGraphQLClient;
  let clientPromise: Promise<Client> | null = null;
  let verified: Promise<void> | null = null;

  function client(): Promise<Client> {
    if (!clientPromise) {
      clientPromise = import('@mysten/sui/graphql').then(({ SuiGraphQLClient }) => new SuiGraphQLClient({ url, network: network.role, fetch: fetchImpl }));
    }
    return clientPromise;
  }

  /** The node must be on the expected network before any of its answers is used. */
  async function ready(): Promise<Client> {
    const c = await client();
    if (!verified) {
      verified = (async () => {
        const reported = (await c.getChainIdentifier()).chainIdentifier;
        if (reported !== network.chainIdentifierBase58) {
          throw new SuiRpcError(`The configured Sui node is on another network (chain ${reported}), not ${network.name}. Nothing was read from it.`);
        }
      })();
      verified.catch(() => {
        verified = null;
      });
    }
    await verified;
    return c;
  }

  async function guard<T>(run: (c: Client) => Promise<T>): Promise<T> {
    try {
      return await run(await ready());
    } catch (cause) {
      if (cause instanceof SuiRpcError) throw cause;
      throw new SuiRpcError(`The Sui node could not be read (${describe(cause)}).`);
    }
  }

  return {
    getChainIdentifier: () => guard(async (c) => (await c.getChainIdentifier()).chainIdentifier),
    getReferenceGasPrice: () => guard(async (c) => toBigInt((await c.getReferenceGasPrice()).referenceGasPrice, 'gas price')),
    getEpoch: () => guard(async (c) => toBigInt((await c.getCurrentSystemState()).systemState.epoch, 'epoch')),
    listCoins: (owner, coinType) =>
      guard(async (c) => {
        const coins: SuiCoin[] = [];
        let cursor: string | null | undefined;
        for (let page = 0; page < MAX_COIN_PAGES; page++) {
          const result = await c.listCoins({ owner, coinType, limit: COIN_PAGE_SIZE, cursor });
          for (const coin of result.objects) {
            if (!coin.version || !coin.digest || coin.balance === undefined) throw new SuiRpcError('The Sui node returned an incomplete coin.');
            coins.push({ objectId: coin.objectId, version: String(coin.version), digest: coin.digest, balance: toBigInt(coin.balance, 'coin balance') });
          }
          if (!result.hasNextPage) return coins;
          cursor = result.cursor;
        }
        throw new SuiRpcError('This account holds too many icUSD coin objects to list. Merge some of them first.');
      }),
    getSuiBalance: (owner) => guard(async (c) => toBigInt((await c.getBalance({ owner })).balance.balance, 'balance')),
    executeTransaction: (bytes, signatures) =>
      guard(async (c) => {
        const result = await c.executeTransaction({ transaction: bytes, signatures });
        const tx = result.Transaction ?? result.FailedTransaction;
        if (!tx) throw new SuiRpcError('The Sui node answered without a transaction.');
        return { digest: tx.digest, ...statusOf(tx) };
      }),
    getTransactionStatus: (digest) =>
      guard(async (c) => {
        try {
          const result = await c.getTransaction({ digest });
          const tx = result.Transaction ?? result.FailedTransaction;
          return tx ? statusOf(tx) : null;
        } catch (cause) {
          if ((cause as { reason?: string } | null)?.reason === 'notFound') return null;
          throw cause;
        }
      }),
  };
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
 * and its full chain identifier, or when the matching entry's URL is empty, malformed or insecure. With the shipped (empty)
 * list it is always null; no node is contacted here, and the adapter only talks when a flow calls it.
 */
export function resolveSuiRpc(network: SuiNetworkInfo, endpoints: readonly SuiNodeEndpoint[], fetchImpl?: Fetch): SuiRpc | null {
  const entry = endpoints.find((e) => e.networkId === network.networkId && e.chainIdentifier === network.chainIdentifier);
  if (!entry) return null;
  const url = usableEndpointUrl(entry.url);
  return url ? createGraphqlSuiRpc({ url, network, fetchImpl }) : null;
}
