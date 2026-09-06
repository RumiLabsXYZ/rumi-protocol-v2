import type { ChainPublicLaunchStatus } from "./backend";
import { CANARY_CHAIN_ID, CANARY_ICUSD_CONTRACT } from "./config";

const REASONS: Record<string, string> = {
  chain_not_configured: "Chain 1030 is not configured.",
  chain_disabled: "New Conflux writes are disabled by the backend.",
  not_conflux_mainnet_public_chain: "The backend returned a chain other than Conflux eSpace mainnet.",
  conflux_mainnet_config_mismatch: "The stored Conflux configuration does not match the reviewed mainnet shape.",
  debt_config_mismatch: "The live debt configuration does not match the reviewed production configuration.",
  icusd_contract_not_bound: "The production icUSD contract is not bound.",
  icusd_contract_mismatch: "The bound icUSD contract is not the reviewed production contract.",
  rpc_endpoint_configuration_too_large: "The RPC endpoint configuration exceeds its bounded public-readiness limit.",
  rpc_distinct_endpoints_insufficient: "Fewer than two distinct RPC endpoints are configured.",
  rpc_agreement_below_two: "RPC agreement is configured below two responses.",
  rpc_agreement_unsatisfiable: "The configured RPC endpoints cannot meet the required agreement threshold.",
  evm_rpc_principal_mismatch: "The backend is not bound to the reviewed official EVM RPC canister.",
  chains_ecdsa_key_mismatch: "The chain-signing key name does not match the reviewed production key.",
  finality_depth_mismatch: "The Conflux finality depth does not match the reviewed production value.",
  liquidation_config_missing: "Liquidation routing is not configured.",
  liquidation_disabled: "Liquidation routing is disabled.",
  liquidation_config_mismatch: "Liquidation routing does not match the reviewed production configuration.",
  collateral_price_missing: "The CFX collateral price is unavailable.",
  collateral_price_zero: "The CFX collateral price is zero.",
  collateral_price_timestamp_missing: "The CFX price has no observation timestamp.",
  price_freshness_limit_missing: "The price freshness limit is not configured.",
  collateral_price_stale: "The CFX collateral price is stale.",
  protocol_mode_not_general_availability: "The protocol is not in General Availability mode.",
  protocol_frozen: "The protocol is frozen.",
  supply_invariant_halted: "The internal supply invariant has halted chain writes.",
  reorg_halted: "Conflux writes are halted for reorg review.",
  bad_debt_circuit_not_configured: "The bad-debt circuit breaker is not configured.",
  bad_debt_circuit_tripped: "The bad-debt circuit breaker is tripped.",
  burn_cursor_unseeded: "The production burn observer cursor has not been seeded.",
  hot_wallet_balance_unknown: "The settlement hot-wallet balance is unavailable.",
  hot_wallet_balance_low: "The settlement hot wallet is below its configured minimum.",
  hot_wallet_balance_timestamp_unknown: "The settlement hot-wallet balance has no verified refresh time.",
  hot_wallet_balance_stale: "The settlement hot-wallet balance observation is stale.",
};

export function blockingReasonText(reason: string): string {
  return REASONS[reason] ?? `Backend readiness check failed: ${reason.replaceAll("_", " ")}.`;
}

export function variantName(value: Record<string, unknown> | undefined): string {
  return value ? (Object.keys(value)[0] ?? "Unknown") : "Not configured";
}

export function ratioE4(value: [] | [bigint]): number | null {
  return value.length ? Number(value[0]) / 10_000 : null;
}

export function priceE8(value: [] | [bigint]): number | null {
  return value.length ? Number(value[0]) / 100_000_000 : null;
}

export function publicBindingRefusal(status: ChainPublicLaunchStatus | null): string | null {
  if (!status) return "Live backend readiness is unavailable.";
  if (status.chain_id !== CANARY_CHAIN_ID) return "The backend returned readiness for the wrong chain.";
  const bound = status.bound_icusd_contract[0];
  if (!status.icusd_contract_matches_expected || !bound ||
      bound.toLowerCase() !== CANARY_ICUSD_CONTRACT.toLowerCase()) {
    return "The backend is not bound to this build's production icUSD contract.";
  }
  return null;
}

export function publicWriteRefusal(status: ChainPublicLaunchStatus | null): string | null {
  const bindingRefusal = publicBindingRefusal(status);
  if (bindingRefusal) return bindingRefusal;
  status = status!;
  if (!status.evm_rpc_principal_matches_expected) {
    return "The effective EVM RPC canister does not match the reviewed official canister.";
  }
  if (!status.chains_ecdsa_key_matches_expected) {
    return "The chain-signing key name does not match the reviewed production key.";
  }
  if (!status.collateral_config_matches_expected) {
    return "The live collateral configuration does not match the reviewed production configuration.";
  }
  if (!status.debt_config_matches_expected) {
    return "The live debt configuration does not match the reviewed production configuration.";
  }
  if (!status.liquidation_config_matches_expected) {
    return "The live liquidation configuration does not match the reviewed production configuration.";
  }
  const liquidationDigest = status.liquidation_config_digest[0];
  if (!liquidationDigest || !status.expected_liquidation_config_digest ||
      liquidationDigest !== status.expected_liquidation_config_digest) {
    return "The live liquidation configuration digest is missing or does not match the reviewed digest.";
  }
  if (!status.hot_wallet_balance_is_fresh) {
    return "The settlement hot-wallet balance observation is not fresh.";
  }
  if (!status.public_open_ready) {
    return status.blocking_reasons.length
      ? status.blocking_reasons.map(blockingReasonText).join(" ")
      : "The backend has not declared public writes ready.";
  }
  return null;
}

export function publicActionRefusal(
  status: ChainPublicLaunchStatus | null,
  requiresPublicReadiness: boolean,
): string | null {
  return requiresPublicReadiness ? publicWriteRefusal(status) : publicBindingRefusal(status);
}
