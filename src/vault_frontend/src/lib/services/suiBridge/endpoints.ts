/**
 * The deployment-time Sui node endpoints: the one place a public GraphQL URL can be configured.
 *
 * EMPTY BY DEFAULT. While no entry matches an enabled instance's network and chain identifier, the keyless node adapter
 * is null and a Sui to ICP return cannot be prepared (the page says so); nothing in this build points at any node. An
 * entry is added by the activation step (a held decision), never by a flow:
 *
 *   { networkId: 'sui-mainnet', chainIdentifier: '<that network's chain identifier, 64 hex digits>', url: 'https://<provider>/graphql' }
 *
 * An entry is keyed to the expected network AND its full chain identifier, so a URL written for one network is never used
 * for another network's instance. The URL must be https (plain http only for a local development node). At use the
 * adapter also compares the node's own chain identifier with the expected one and refuses a node on another network.
 */

import type { SuiNodeEndpoint } from './rpc';

export const SUI_NODE_ENDPOINTS: readonly SuiNodeEndpoint[] = [];
