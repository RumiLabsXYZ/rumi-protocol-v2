/**
 * The deployment-time Solana node endpoints: the one place a public JSON-RPC URL can be configured.
 *
 * EMPTY BY DEFAULT. While no entry matches an enabled instance's network and genesis hash, the keyless adapter is null and
 * a Solana to ICP return cannot be prepared (the page says so); nothing in this build points at any node. An entry is
 * added by the activation step (a held decision), never by a flow:
 *
 *   { networkId: 'solana-mainnet', genesisHash: '<that network genesis hash>', url: 'https://<provider>/<path>' }
 *
 * An entry is keyed to the expected network AND its genesis hash, so a URL written for one cluster is never used for
 * another cluster's instance. The URL must be https (plain http only for a local development node).
 */

import type { SolanaRpcEndpoint } from './rpc';

export const SOLANA_RPC_ENDPOINTS: readonly SolanaRpcEndpoint[] = [];
