/**
 * Where icUSD can go: the destination list the bridge page shows.
 *
 * One bridge canister instance exists per destination network, and the page picks
 * which instance to talk to. Today only one instance exists (Base), so:
 *  - the Base entry is synthesized from that canister's `get_bridge_info`, through
 *    the same `resolveBridgeConfig` the page has always used (same checks, same
 *    messages, same status handling);
 *  - every other network is a static entry that is DISABLED, with no actor and no
 *    way to select it. An entry never gets an enabled flag from this file: only the
 *    canister's own answer can enable a destination.
 *
 * "Disabled" means not offered. It is not "paused": a paused Base keeps its enabled
 * status and the existing paused-release behavior (see `signingGuard.ts`).
 *
 * This file is pure (no actor, no wallet, no network). `loadDestinations`, which reads
 * the canister, is in `destinationLoader.ts`.
 */

import {
  EVM_DESTINATION_CHAINS,
  type BridgeConfig,
  type BridgeConfigResult,
  type EvmDestinationChain,
} from './chains';

export type DestinationFamily = 'evm' | 'solana' | 'sui' | 'xrpl';
export type DestinationNetwork = 'mainnet' | 'testnet';
export type DestinationStatus = 'enabled' | 'disabled';

export interface DestinationLimits {
  minDepositE8s: bigint;
  maxDepositE8s: bigint;
  /** Contract minimum for a burn on the destination, in e8s. */
  minBridgeOutE8s: bigint;
}

export interface DestinationFees {
  depositFeeE8s: bigint;
  releaseFeeE8s: bigint;
  /** The bridge canister's cached icUSD ledger fee, when it has one. */
  ledgerFeeE8s: bigint | null;
}

export interface DestinationView {
  /** Stable key: the selector entry, and the destination key of saved records. */
  id: string;
  family: DestinationFamily;
  name: string;
  /** The chain identifier as text. Not every family has a number (and an unreadable one is the empty string). */
  chainId: string;
  network: DestinationNetwork;
  status: DestinationStatus;
  /** Plain-language reason. Null while enabled. */
  reason: string | null;
  /** Known only while the destination's canister has answered. */
  limits: DestinationLimits | null;
  fees: DestinationFees | null;
  /** The bridge canister instance for this destination, when one exists. */
  actorCanisterId: string | null;
  /** The live bridge configuration the EVM forms use. Present only while enabled. */
  config: BridgeConfig | null;
}

export const BASE_DESTINATION_ID = 'base';

function networkOf(testnet: boolean): DestinationNetwork {
  return testnet ? 'testnet' : 'mainnet';
}

/** The Base entry, from the result of resolving `get_bridge_info`. */
export function baseDestination(result: BridgeConfigResult, actorCanisterId: string | null): DestinationView {
  if (result.ok) {
    const { config } = result;
    return {
      id: BASE_DESTINATION_ID,
      family: 'evm',
      name: config.chain.name,
      chainId: String(config.chain.chainId),
      network: networkOf(config.chain.testnet),
      status: 'enabled',
      reason: null,
      limits: {
        minDepositE8s: config.minDepositE8s,
        maxDepositE8s: config.maxDepositE8s,
        minBridgeOutE8s: config.minBridgeOutE8s,
      },
      fees: {
        depositFeeE8s: config.depositFeeE8s,
        releaseFeeE8s: config.releaseFeeE8s,
        ledgerFeeE8s: config.ledgerFeeE8s,
      },
      actorCanisterId,
      config,
    };
  }
  const chain = result.chain;
  return {
    id: BASE_DESTINATION_ID,
    family: 'evm',
    name: chain?.name ?? 'Base',
    chainId: chain ? String(chain.chainId) : '',
    network: networkOf(chain?.testnet ?? false),
    status: 'disabled',
    reason: result.message,
    limits: null,
    fees: null,
    actorCanisterId,
    config: null,
  };
}

function staticDestination(chain: EvmDestinationChain): DestinationView {
  return {
    id: chain.destinationId,
    family: 'evm',
    name: chain.name,
    chainId: String(chain.chainId),
    network: 'mainnet',
    status: 'disabled',
    reason: `Not available yet: there is no icUSD bridge to ${chain.name} at the moment.`,
    limits: null,
    fees: null,
    actorCanisterId: null,
    config: null,
  };
}

/** The other EVM networks the bridge canister knows. No instance exists for any of them, so none can be enabled. */
export function unavailableEvmDestinations(): DestinationView[] {
  return EVM_DESTINATION_CHAINS.filter((chain) => !chain.testnet).map(staticDestination);
}

/** The full list: Base first, then the static disabled entries. */
export function buildDestinations(base: BridgeConfigResult, baseActorCanisterId: string | null): DestinationView[] {
  return [baseDestination(base, baseActorCanisterId), ...unavailableEvmDestinations()];
}

/** The list to show when the bridge canister could not be reached at all: every entry is unavailable. */
export function unreachableDestinations(message: string, baseActorCanisterId: string | null = null): DestinationView[] {
  return [
    {
      id: BASE_DESTINATION_ID,
      family: 'evm',
      name: 'Base',
      chainId: '',
      network: 'mainnet',
      status: 'disabled',
      reason: `The bridge could not be reached: ${message}`,
      limits: null,
      fees: null,
      actorCanisterId: baseActorCanisterId,
      config: null,
    },
    ...unavailableEvmDestinations(),
  ];
}

// ---------------------------------------------------------------------------
// Selection
// ---------------------------------------------------------------------------

export function isEnabled(destination: Pick<DestinationView, 'status'>): boolean {
  return destination.status === 'enabled';
}

export function findDestination(list: readonly DestinationView[], id: string | null): DestinationView | null {
  return id === null ? null : (list.find((destination) => destination.id === id) ?? null);
}

/**
 * The destination the page is on: the one the user chose (even if it has since become disabled, so the
 * page can say so), else the first enabled one, else the first entry (which then explains why nothing is on offer).
 */
export function pickSelected(list: readonly DestinationView[], chosenId: string | null): DestinationView | null {
  return findDestination(list, chosenId) ?? list.find(isEnabled) ?? list[0] ?? null;
}

export type SelectorKey = 'ArrowRight' | 'ArrowDown' | 'ArrowLeft' | 'ArrowUp' | 'Home' | 'End';

export function isSelectorKey(key: string): key is SelectorKey {
  return key === 'ArrowRight' || key === 'ArrowDown' || key === 'ArrowLeft' || key === 'ArrowUp' || key === 'Home' || key === 'End';
}

/**
 * Where a navigation key moves the selection. Disabled entries are skipped, so they can never be
 * reached from the keyboard. Null when nothing is enabled.
 */
export function nextEnabledDestination(
  list: readonly DestinationView[],
  currentId: string | null,
  key: SelectorKey,
): DestinationView | null {
  const enabled = list.filter(isEnabled);
  if (enabled.length === 0) return null;
  if (key === 'Home') return enabled[0];
  if (key === 'End') return enabled[enabled.length - 1];
  const forward = key === 'ArrowRight' || key === 'ArrowDown';
  const at = enabled.findIndex((destination) => destination.id === currentId);
  if (at < 0) return forward ? enabled[0] : enabled[enabled.length - 1];
  return enabled[(at + (forward ? 1 : -1) + enabled.length) % enabled.length];
}
