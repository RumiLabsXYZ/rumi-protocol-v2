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
  baseNetworkId,
  type BridgeConfig,
  type BridgeConfigResult,
  type EvmDestinationChain,
} from './chains';
import type { SolanaBridgeConfig, SolanaConfigResult } from '../solanaBridge/config';
import { SOLANA_MAINNET } from '../solanaBridge/networks';

/**
 * The live configuration of a non-EVM (native) bridge instance. A native instance is NOT an EVM `BridgeConfig` with
 * other field names (it has a mint, a return account and a genesis hash instead of a contract and a chain id), so it has
 * its own field, next to `config`, and the page branches on `family`. Solana is the only native family so far; a later
 * family adds its own member to this union.
 */
export type NativeDestinationConfig = SolanaBridgeConfig;

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
  /**
   * The per-network key: the profile's network id (`base-mainnet`, `ethereum-mainnet`). It is the selector entry
   * and the destination key of saved records, and a mainnet and a testnet never share one.
   */
  id: string;
  /** The profile this network belongs to, a family label (`base`, `ethereum`). The list holds at most one `base` entry. */
  profileId: string;
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
  /** The live configuration of a native (non-EVM) instance. Present only while enabled; absent for every EVM entry. */
  native?: NativeDestinationConfig | null;
}

/** The profile label of the Base bridge. */
export const BASE_PROFILE_ID = 'base';

/** The key of a Base entry whose chain is not known (the canister could not be read, or reports an unsupported chain). */
export const BASE_UNKNOWN_ID = 'base-unknown';

function networkOf(testnet: boolean): DestinationNetwork {
  return testnet ? 'testnet' : 'mainnet';
}

/** The Base entry, from the result of resolving `get_bridge_info`. */
export function baseDestination(result: BridgeConfigResult, actorCanisterId: string | null): DestinationView {
  if (result.ok) {
    const { config } = result;
    return {
      // `config.chain` is a supported Base chain, so it has a key.
      id: baseNetworkId(config.chain.chainId) ?? BASE_UNKNOWN_ID,
      profileId: BASE_PROFILE_ID,
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
    id: (chain && baseNetworkId(chain.chainId)) || BASE_UNKNOWN_ID,
    profileId: BASE_PROFILE_ID,
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
    id: chain.networkId,
    profileId: chain.profileId,
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

/** The static, disabled Solana row. No Solana instance is configured, so it has no actor and cannot be selected. */
export function unavailableSolanaDestination(): DestinationView {
  return {
    id: SOLANA_MAINNET.networkId,
    profileId: 'solana',
    family: 'solana',
    name: SOLANA_MAINNET.name,
    chainId: SOLANA_MAINNET.genesisHash,
    network: 'mainnet',
    status: 'disabled',
    reason: `Not available yet: there is no icUSD bridge to ${SOLANA_MAINNET.name} at the moment.`,
    limits: null,
    fees: null,
    actorCanisterId: null,
    config: null,
    native: null,
  };
}

/** The native (non-EVM) rows that are not offered. */
export function unavailableNativeDestinations(): DestinationView[] {
  return [unavailableSolanaDestination()];
}

/**
 * The Solana entry from the result of resolving the instance's answers. An enabled entry exists only while the
 * canister's own answer passes every check (`resolveSolanaConfig`); otherwise it is disabled with the reason.
 */
export function solanaDestination(result: SolanaConfigResult, actorCanisterId: string | null): DestinationView {
  if (result.ok) {
    const { config } = result;
    return {
      id: config.network.networkId,
      profileId: 'solana',
      family: 'solana',
      name: config.network.name,
      chainId: config.network.genesisHash,
      network: config.network.role === 'mainnet' ? 'mainnet' : 'testnet',
      status: 'enabled',
      reason: null,
      limits: { minDepositE8s: config.minDepositE8s, maxDepositE8s: config.maxDepositE8s, minBridgeOutE8s: 1n },
      fees: { depositFeeE8s: config.depositFeeE8s, releaseFeeE8s: config.releaseFeeE8s, ledgerFeeE8s: null },
      actorCanisterId,
      config: null,
      native: config,
    };
  }
  const network = result.network;
  return {
    id: network?.networkId ?? SOLANA_MAINNET.networkId,
    profileId: 'solana',
    family: 'solana',
    name: network?.name ?? SOLANA_MAINNET.name,
    chainId: network?.genesisHash ?? '',
    network: network && network.role !== 'mainnet' ? 'testnet' : 'mainnet',
    status: 'disabled',
    reason: result.message,
    limits: null,
    fees: null,
    actorCanisterId,
    config: null,
    native: null,
  };
}

/** The Solana row to show when its instance could not be read at all. */
export function unreachableSolanaDestination(message: string, actorCanisterId: string | null): DestinationView {
  return { ...unavailableSolanaDestination(), reason: `The Solana bridge could not be reached: ${message}`, actorCanisterId };
}

/** Put a Solana entry in a list: it replaces the static row of the same network, else it is added after the others. */
export function withSolanaDestination(list: readonly DestinationView[], solana: DestinationView): DestinationView[] {
  return list.some((entry) => entry.id === solana.id) ? list.map((entry) => (entry.id === solana.id ? solana : entry)) : [...list, solana];
}

/** The full list: Base first, then the static disabled entries. */
export function buildDestinations(base: BridgeConfigResult, baseActorCanisterId: string | null): DestinationView[] {
  return [baseDestination(base, baseActorCanisterId), ...unavailableEvmDestinations(), ...unavailableNativeDestinations()];
}

/** The list to show when the bridge canister could not be reached at all: every entry is unavailable. */
export function unreachableDestinations(message: string, baseActorCanisterId: string | null = null): DestinationView[] {
  return [
    {
      id: BASE_UNKNOWN_ID,
      profileId: BASE_PROFILE_ID,
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
    ...unavailableNativeDestinations(),
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
