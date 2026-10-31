/**
 * The Sui bridge instance's live configuration: the native counterpart of the EVM `BridgeConfig` and the Solana config.
 *
 * A Sui instance is NOT an EVM config with different field names: it has a package, a shared Gate object, a coin type and
 * a full genesis checkpoint digest instead of a contract and a chain id. This file reads the canister's own answers
 * (`get_config`, `get_roles`, `get_mint_lane`, `get_observed_deployment`) and turns them into one of two results: a usable
 * config, or a plain-language reason the destination is not offered. Only the canister's answer can enable a Sui
 * destination; nothing here enables anything by itself.
 *
 * Two things are kept apart on purpose:
 *  - IDENTITY AVAILABILITY (this file's `ok`): is this the exact, bound and observed deployment? The instance is
 *    initialized; its chain identifier is a full digest this app knows; the binding names a package, a Gate, a mint
 *    cap and an admin cap in strict form, with the coin and Gate types derived from that package and 8 decimals; the
 *    canister's own independent observation of the deployment (`get_observed_deployment`) agrees with the binding, has
 *    seen the published module, and reports 8 decimals; all three signing roles have an address.
 *  - DIRECTION STATE (pauses, halt, mint lane): a paused or not-ready direction does not make the identity unavailable.
 *    `get_mint_lane.ready` and `blocked_by` say whether a deposit can be minted right now; `pauses` and `halted` say what
 *    the operator has stopped. `ConfigView.signing_enabled` is legacy and always false: it is NEVER read here and is
 *    not a readiness signal.
 */

import type { ConfigView, MintLaneView, ObservedDeploymentView, RoleView } from '$declarations/rumi_icusd_sui_bridge/rumi_icusd_sui_bridge.did';
import { suiNetworkByChainIdentifier, type SuiNetworkInfo } from './networks';
import { parseSuiAddress } from './address';

export interface SuiBridgeConfig {
  family: 'sui';
  /** The bridge canister instance for this network. */
  instanceCanisterId: string;
  network: SuiNetworkInfo;
  packageId: string;
  gateId: string;
  /** The Gate's `initial_shared_version`, as the canister observed it (needed to reference the shared object). */
  gateInitialSharedVersion: string;
  /** `0x<package>::rumi_icusd_gate::RUMI_ICUSD_GATE`. */
  coinType: string;
  gateType: string;
  mintCapId: string;
  adminCapId: string;
  /** The three signing roles' addresses: a holder may be none of them, a recipient neither. */
  roleAddresses: string[];
  icusdLedger: string;
  minDepositE8s: bigint;
  maxDepositE8s: bigint;
  /** The smallest return the Gate accepts (`min_bridge_out`), in e8s. */
  minBridgeOutE8s: bigint;
  depositFeeE8s: bigint;
  releaseFeeE8s: bigint;
  /** DIRECTION STATE, separate from the identity above. */
  depositsPaused: boolean;
  /** Mints to Sui paused: a deposit would wait. */
  mintsPaused: boolean;
  releasesPaused: boolean;
  halted: string | null;
  /** Whether the mint lane can run a mint now, and, when it cannot, the canister's own reasons. */
  mintLaneReady: boolean;
  mintLaneBlockedBy: string[];
}

export type SuiConfigResult =
  | { ok: true; config: SuiBridgeConfig }
  | { ok: false; message: string; network: SuiNetworkInfo | null };

const MODULE = 'rumi_icusd_gate';

/** Turn the instance's answers into a config or a reason. Pure. */
export function resolveSuiConfig(
  config: ConfigView,
  roles: readonly RoleView[],
  lane: MintLaneView,
  observed: ObservedDeploymentView | null,
  instanceCanisterId: string,
): SuiConfigResult {
  const network = suiNetworkByChainIdentifier(config.chain_identifier);
  const fail = (message: string): SuiConfigResult => ({ ok: false, message, network });
  if (!config.initialized) return fail('The Sui bridge is not set up yet.');
  if (network === null) return fail('The Sui bridge reports a network this app does not know, so it is not offered.');
  if (('Mainnet' in config.network) !== (network.role === 'mainnet')) return fail('The Sui bridge reports a network that does not match its chain identifier, so it is not offered.');

  const binding = config.binding[0];
  if (!binding) return fail('The Sui bridge has no icUSD Gate bound on Sui yet.');
  const ids = {
    package: parseSuiAddress(binding.package_id),
    gate: parseSuiAddress(binding.gate_id),
    mintCap: parseSuiAddress(binding.mint_cap_id),
    adminCap: parseSuiAddress(binding.admin_cap_id),
  };
  for (const [label, id] of Object.entries(ids)) if (!id.ok) return fail(`The Sui bridge's ${label} id is not a Sui address in strict form, so it is not offered.`);
  if (binding.decimals !== 8) return fail('The Sui bridge reports an icUSD coin that does not have 8 decimals, so it is not offered.');
  const packageId = binding.package_id;
  const coinType = `${packageId}::${MODULE}::RUMI_ICUSD_GATE`;
  const gateType = `${packageId}::${MODULE}::Gate`;
  if (binding.coin_type !== coinType || binding.gate_type !== gateType) return fail('The Sui bridge reports coin and Gate types that do not belong to its package, so it is not offered.');

  if (!observed) return fail('The Sui bridge has not yet observed its deployment on Sui, so it is not offered.');
  if (observed.decimals !== 8) return fail('The observed Sui coin does not have 8 decimals, so the bridge is not offered.');
  if (!observed.module_digest_observed) return fail('The Sui bridge has not matched the published Gate module, so it is not offered.');
  if (observed.mint_cap_id !== binding.mint_cap_id || observed.admin_cap_id !== binding.admin_cap_id) {
    return fail('The deployment the Sui bridge observed differs from the one it is bound to, so it is not offered.');
  }
  if (observed.initial_shared_version === 0n) return fail('The observed Gate has no shared version yet, so the bridge is not offered.');

  const roleAddresses: string[] = [];
  for (const role of ['Treasury', 'Gas', 'UpgradeCustody'] as const) {
    const entry = roles.find((r) => role in r.role);
    const address = entry?.address[0];
    if (!address || !parseSuiAddress(address).ok) return fail('A signing role of the Sui bridge has no address yet, so it is not offered.');
    roleAddresses.push(address);
  }

  return {
    ok: true,
    config: {
      family: 'sui',
      instanceCanisterId,
      network,
      packageId,
      gateId: binding.gate_id,
      gateInitialSharedVersion: observed.initial_shared_version.toString(),
      coinType,
      gateType,
      mintCapId: binding.mint_cap_id,
      adminCapId: binding.admin_cap_id,
      roleAddresses,
      icusdLedger: config.icusd_ledger.toText(),
      minDepositE8s: config.limits.min_deposit_e8s,
      maxDepositE8s: config.limits.max_deposit_e8s,
      minBridgeOutE8s: observed.min_bridge_out,
      depositFeeE8s: config.fees.deposit_fee_e8s,
      releaseFeeE8s: config.fees.release_fee_e8s,
      depositsPaused: config.pauses.deposits,
      mintsPaused: config.pauses.mints,
      releasesPaused: config.pauses.releases,
      halted: config.halted[0] ?? null,
      mintLaneReady: lane.ready,
      mintLaneBlockedBy: [...lane.blocked_by],
    },
  };
}
