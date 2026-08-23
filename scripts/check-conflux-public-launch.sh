#!/usr/bin/env bash
# Read-only Conflux public-launch monitor. It performs query calls only.
set -euo pipefail

BACKEND="tfesu-vyaaa-aaaap-qrd7a-cai"
CHAIN_ID="1030"
NETWORK="ic"
EXPECTATION="disabled"
MIN_CYCLES="5000000000000"
SELF_TEST="0"
EXPECTED_ICUSD_CONTRACT="0x8DdB0a13B26ed28912e4B8cCa99Bc3E8c66Df7Ff"
EXPECTED_EVM_RPC_PRINCIPAL="7hfb6-caaaa-aaaar-qadga-cai"
EXPECTED_ECDSA_KEY_NAME="key_1"
# Principal + network calls need no project configuration. Running them outside
# the checkout with an isolated CLI home avoids both a stale project-pinned
# package and any dependency on the operator's configured signing identities.
ICP_CALL_DIR="${TMPDIR:-/tmp}"

usage() {
  cat <<'USAGE'
Usage: scripts/check-conflux-public-launch.sh [options]

Read-only checks for the Conflux eSpace production launch rail.

Options:
  --expect-disabled       Require chain status Disabled (default)
  --expect-public-active  Require Registered plus all public-open gates
  --backend PRINCIPAL     Backend canister (default: tfesu-vyaaa-aaaap-qrd7a-cai)
  --chain NAT32           EVM chain id (default: 1030)
  --network NETWORK       icp-cli network (default: ic)
  --min-cycles NAT        Additional cycle floor (default: 5000000000000)
  --self-test             Run deterministic parser/verdict fixtures only
  -h, --help              Show this help

This script uses an isolated built-in anonymous identity and never invokes an update method.
USAGE
}

while (($#)); do
  case "$1" in
    --expect-disabled) EXPECTATION="disabled" ;;
    --expect-public-active) EXPECTATION="public-active" ;;
    --backend) BACKEND="${2:?--backend requires a principal}"; shift ;;
    --chain) CHAIN_ID="${2:?--chain requires a nat32}"; shift ;;
    --network) NETWORK="${2:?--network requires a value}"; shift ;;
    --min-cycles) MIN_CYCLES="${2:?--min-cycles requires a nat}"; shift ;;
    --self-test) SELF_TEST="1" ;;
    -h|--help) usage; exit 0 ;;
    *) printf 'ERROR: unknown argument: %s\n' "$1" >&2; usage >&2; exit 2 ;;
  esac
  shift
done

[[ "$CHAIN_ID" =~ ^[0-9]+$ ]] || { printf 'ERROR: --chain must be an unsigned integer\n' >&2; exit 2; }
[[ "$MIN_CYCLES" =~ ^[0-9]+$ ]] || { printf 'ERROR: --min-cycles must be an unsigned integer\n' >&2; exit 2; }

if [[ "$SELF_TEST" == "0" ]]; then
  for dependency in icp jq python3; do
    command -v "$dependency" >/dev/null 2>&1 || {
      printf 'ERROR: required command not found: %s\n' "$dependency" >&2
      exit 2
    }
  done

  ICP_HOME_DIR="$(mktemp -d "$ICP_CALL_DIR/rumi-conflux-monitor.XXXXXX")"
  trap 'rm -r -- "$ICP_HOME_DIR"' EXIT

  if ! STATUS_JSON="$(
    cd "$ICP_CALL_DIR"
    env ICP_HOME="$ICP_HOME_DIR" DO_NOT_TRACK=1 \
      icp canister call "$BACKEND" get_chain_public_launch_status \
        "($CHAIN_ID : nat32)" -n "$NETWORK" --identity anonymous --query --json 2>&1
  )"; then
    printf 'FAIL status query unavailable: %s\n' "$STATUS_JSON" >&2
    exit 1
  fi
  if ! STATUS_CANDID="$(printf '%s' "$STATUS_JSON" | jq -er '.response_candid | strings')"; then
    printf 'FAIL status query returned malformed icp-cli JSON\n' >&2
    exit 1
  fi

  if ! CYCLES_JSON="$(
    cd "$ICP_CALL_DIR"
    env ICP_HOME="$ICP_HOME_DIR" DO_NOT_TRACK=1 \
      icp canister call "$BACKEND" cycles_status '()' \
        -n "$NETWORK" --identity anonymous --query --json 2>&1
  )"; then
    printf 'FAIL cycles query unavailable: %s\n' "$CYCLES_JSON" >&2
    exit 1
  fi
  if ! CYCLES_CANDID="$(printf '%s' "$CYCLES_JSON" | jq -er '.response_candid | strings')"; then
    printf 'FAIL cycles query returned malformed icp-cli JSON\n' >&2
    exit 1
  fi

  if ! SUPPLY_AUDIT_JSON="$(
    cd "$ICP_CALL_DIR"
    env ICP_HOME="$ICP_HOME_DIR" DO_NOT_TRACK=1 \
      icp canister call "$BACKEND" get_supply_audit '()' \
        -n "$NETWORK" --identity anonymous --query --json 2>&1
  )"; then
    printf 'FAIL operator supply audit query unavailable: %s\n' "$SUPPLY_AUDIT_JSON" >&2
    exit 1
  fi
  if ! SUPPLY_AUDIT_CANDID="$(printf '%s' "$SUPPLY_AUDIT_JSON" | jq -er '.response_candid | strings')"; then
    printf 'FAIL operator supply audit returned malformed icp-cli JSON\n' >&2
    exit 1
  fi
else
  STATUS_CANDID=""
  CYCLES_CANDID=""
  SUPPLY_AUDIT_CANDID=""
fi

export EXPECTATION CHAIN_ID MIN_CYCLES SELF_TEST EXPECTED_ICUSD_CONTRACT
export EXPECTED_EVM_RPC_PRINCIPAL EXPECTED_ECDSA_KEY_NAME
export STATUS_CANDID CYCLES_CANDID SUPPLY_AUDIT_CANDID

python3 - <<'PY'
import os
import re
import sys


def launch_status(**overrides):
    values = {
        "chain_id": "1_030 : nat32",
        "configured": "true",
        "status": "opt variant { Disabled }",
        "registered": "false",
        "native_symbol": 'opt "CFX"',
        "effective_evm_rpc_principal": 'principal "7hfb6-caaaa-aaaar-qadga-cai"',
        "evm_rpc_principal_matches_expected": "true",
        "chains_ecdsa_key_name": '"key_1"',
        "chains_ecdsa_key_matches_expected": "true",
        "bound_icusd_contract": 'opt "0x8DdB0a13B26ed28912e4B8cCa99Bc3E8c66Df7Ff"',
        "icusd_contract_matches_expected": "true",
        "rpc_endpoint_count": "3 : nat32",
        "rpc_min_quorum_providers": "2 : nat32",
        "rpc_effective_agreement_requirement": "2 : nat32",
        "rpc_configuration_sufficient": "true",
        "finality_depth": "opt (400 : nat32)",
        "min_cr_e4": "opt (15_000 : nat64)",
        "liquidation_threshold_e4": "opt (13_300 : nat64)",
        "collateral_config_matches_expected": "true",
        "effective_debt_config": "opt record { debt_ceiling_e8s = opt (50_000_000_000 : nat); min_vault_debt_e8s = 10_000_000 : nat }",
        "debt_config_matches_expected": "true",
        "chain_supply_e8s": "0 : nat",
        "chain_reserve_backing_e8s": "0 : nat",
        "chain_pending_burn_e8s": "0 : nat",
        "liquidation_configured": "true",
        "liquidation_enabled": "true",
        "liquidation_config_matches_expected": "true",
        "liquidation_config_digest": 'opt "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"',
        "expected_liquidation_config_digest": '"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"',
        "max_price_age_ns": "opt (1_800_000_000_000 : nat64)",
        "collateral_price_e8": "opt (15_000_000 : nat64)",
        "collateral_price_set_at_ns": "opt (1_000_000_000 : nat64)",
        "collateral_price_age_ns": "opt (10_000_000_000 : nat64)",
        "collateral_price_is_fresh": "true",
        "protocol_mode": "variant { GeneralAvailability }",
        "protocol_frozen": "false",
        "invariant_halted": "false",
        "reorg_halted": "false",
        "bad_debt_e8s": "0 : nat",
        "bad_debt_threshold_e8s": "opt (100_000_000 : nat)",
        "bad_debt_circuit_tripped": "false",
        "bad_debt_tripped_at_ns": "null",
        "burn_cursor": "154_846_297 : nat64",
        "hot_wallet_balance_e18": "opt (200_000_000_000_000_000 : nat)",
        "hot_wallet_min_balance_e18": "100_000_000_000_000_000 : nat",
        "hot_wallet_ready": "opt true",
        "hot_wallet_balance_refreshed_at_ns": "opt (1_000_000_000_000 : nat64)",
        "hot_wallet_balance_age_ns": "opt (10_000_000_000 : nat64)",
        "hot_wallet_balance_max_age_ns": "300_000_000_000 : nat64",
        "hot_wallet_balance_is_fresh": "true",
        "public_open_ready": "false",
        "blocking_reasons": 'vec { "chain_disabled" }',
    }
    values.update(overrides)
    body = ";\n    ".join(f"{key} = {value}" for key, value in values.items())
    return f"(\n  record {{\n    {body};\n  }},\n)"


def cycles_status(**overrides):
    values = {
        "low_watermark": "5_000_000_000_000 : nat",
        "balance": "11_900_000_000_000 : nat",
        "healthy": "true",
        "freeze_threshold_secs": "2_592_000 : nat64",
    }
    values.update(overrides)
    body = ";\n    ".join(f"{key} = {value}" for key, value in values.items())
    return f"(\n  record {{\n    {body};\n  }},\n)"


def supply_audit(**overrides):
    values = {
        "total_e8s": "0 : nat",
        "chain_supply_e8s": "0 : nat",
        "chain_id": "1_030 : nat32",
    }
    values.update(overrides)
    return f'''(
  record {{
    total_e8s = {values["total_e8s"]};
    per_chain = vec {{
      record {{
        supply_e8s = {values["chain_supply_e8s"]};
        display_name = "ConfluxESpaceMainnet";
        chain_id = {values["chain_id"]};
      }};
    }};
  }},
)'''


def field_expr(text, name):
    match = re.search(rf"(?m)^\s*{re.escape(name)}\s*=\s*(.+?);\s*$", text)
    if not match:
        raise ValueError(f"missing field {name}")
    return match.group(1).strip()


def boolean(text, name):
    raw = field_expr(text, name)
    if raw == "true":
        return True
    if raw == "false":
        return False
    raise ValueError(f"field {name} is not a bool: {raw}")


def optional_boolean(text, name):
    raw = field_expr(text, name)
    if raw == "opt true":
        return True
    if raw == "opt false":
        return False
    if raw == "null":
        return None
    raise ValueError(f"field {name} is not an opt bool: {raw}")


def integer(text, name):
    raw = field_expr(text, name)
    match = re.fullmatch(r"([+-]?[0-9][0-9_]*)(?:\s*:\s*(?:nat|nat32|nat64|int))?", raw)
    if not match:
        raise ValueError(f"field {name} is not an integer: {raw}")
    return int(match.group(1).replace("_", ""))


def optional_integer(text, name):
    null_match = re.search(rf"(?m)^\s*{re.escape(name)}\s*=\s*null\s*;", text)
    if null_match:
        return None
    match = re.search(
        rf"(?ms)^\s*{re.escape(name)}\s*=\s*opt\s*\(?\s*"
        r"(\d[0-9_]*)\s*(?:\:\s*(?:nat|nat32|nat64|int))?\s*\)?\s*;",
        text,
    )
    if not match:
        raise ValueError(f"field {name} is not an optional integer")
    return int(match.group(1).replace("_", ""))


def optional_text(text, name):
    raw = field_expr(text, name)
    if raw == "null":
        return None
    match = re.fullmatch(r'opt\s+"([^"]*)"', raw)
    if not match:
        raise ValueError(f"field {name} is not optional text: {raw}")
    return match.group(1)


def text_value(text, name):
    raw = field_expr(text, name)
    match = re.fullmatch(r'"([^\"]*)"', raw)
    if not match:
        raise ValueError(f"field {name} is not text: {raw}")
    return match.group(1)


def principal_value(text, name):
    raw = field_expr(text, name)
    match = re.fullmatch(r'principal\s+"([^\"]+)"', raw)
    if not match:
        raise ValueError(f"field {name} is not a principal: {raw}")
    return match.group(1)


def variant(text, name):
    raw = field_expr(text, name)
    match = re.fullmatch(r"(?:opt\s+)?variant\s*\{\s*([A-Za-z0-9_]+)\s*\}", raw)
    if not match:
        raise ValueError(f"field {name} is not a simple variant: {raw}")
    return match.group(1)


def present_optional(text, name):
    match = re.search(rf"(?m)^\s*{re.escape(name)}\s*=\s*(null|opt\b)", text)
    if not match:
        raise ValueError(f"field {name} is not optional or is missing")
    return match.group(1) == "opt"


def text_vector(text, name):
    match = re.search(
        rf"(?ms)^\s*{re.escape(name)}\s*=\s*vec\s*\{{(.*?)\}}\s*;",
        text,
    )
    if not match:
        raise ValueError(f"field {name} is not a text vector or is missing")
    return re.findall(r'"([^"]*)"', match.group(1))


def parse_supply_audit(audit):
    total = integer(audit, "total_e8s")
    entries = []
    for body in re.findall(r"record\s*\{([^{}]*)\}", audit, re.DOTALL):
        if re.search(r"(?m)^\s*chain_id\s*=", body):
            entries.append((integer(body, "chain_id"), integer(body, "supply_e8s")))
    if not entries:
        raise ValueError("operator supply audit has no per_chain entries")
    return total, entries


def validate(
    status,
    cycle_status,
    audit,
    expectation,
    expected_chain,
    expected_contract,
    expected_rpc_principal,
    expected_ecdsa_key,
    minimum_cycles,
    quiet=False,
):
    failures = []
    warnings = []
    metrics = {}

    def require(condition, message):
        if not condition:
            failures.append(message)

    try:
        require(integer(status, "chain_id") == expected_chain, "wrong chain id")
        require(boolean(status, "configured"), "chain is not configured")
        require(optional_text(status, "native_symbol") == "CFX", "native symbol is not CFX")
        actual_rpc_principal = principal_value(status, "effective_evm_rpc_principal")
        actual_ecdsa_key = text_value(status, "chains_ecdsa_key_name")
        metrics.update(
            evm_rpc_principal=actual_rpc_principal,
            chains_ecdsa_key_name=actual_ecdsa_key,
        )
        require(
            actual_rpc_principal == expected_rpc_principal,
            "effective EVM-RPC canister principal is not the official principal",
        )
        require(
            boolean(status, "evm_rpc_principal_matches_expected"),
            "backend reports EVM-RPC canister principal mismatch",
        )
        require(
            actual_ecdsa_key == expected_ecdsa_key,
            "chains threshold-ECDSA key is not key_1",
        )
        require(
            boolean(status, "chains_ecdsa_key_matches_expected"),
            "backend reports chains threshold-ECDSA key mismatch",
        )
        actual_status = variant(status, "status")
        blocking_reasons = text_vector(status, "blocking_reasons")
        expected_status = "Disabled" if expectation == "disabled" else "Registered"
        require(actual_status == expected_status, f"status is {actual_status}, expected {expected_status}")

        endpoints = integer(status, "rpc_endpoint_count")
        floor = integer(status, "rpc_min_quorum_providers")
        agreement = integer(status, "rpc_effective_agreement_requirement")
        metrics.update(rpc_endpoint_count=endpoints, rpc_floor=floor, rpc_agreement=agreement)
        require(endpoints >= 2, "fewer than two distinct RPC endpoint URLs")
        require(floor > 0, "RPC floor is zero")
        require(agreement >= 2, "effective RPC agreement is below two")
        require(agreement >= floor, "effective RPC agreement is below configured floor")
        require(endpoints >= agreement, "distinct RPC endpoints are below effective agreement")
        require(boolean(status, "rpc_configuration_sufficient"), "RPC configuration is insufficient")
        require(optional_integer(status, "finality_depth") == 400, "finality depth is not exactly 400")
        min_cr = optional_integer(status, "min_cr_e4")
        liquidation_cr = optional_integer(status, "liquidation_threshold_e4")
        require(
            min_cr is not None and liquidation_cr is not None and min_cr > liquidation_cr,
            "collateral-ratio configuration is missing or unsafe",
        )
        require(
            boolean(status, "collateral_config_matches_expected"),
            "collateral configuration does not match the reviewed production shape",
        )
        require(present_optional(status, "effective_debt_config"), "effective debt config is missing")
        require(
            boolean(status, "debt_config_matches_expected"),
            "debt configuration does not match the reviewed production shape",
        )

        bound_contract = optional_text(status, "bound_icusd_contract")
        require(
            bound_contract is not None and bound_contract.lower() == expected_contract.lower(),
            "bound IcUSD contract does not match production contract",
        )
        require(boolean(status, "icusd_contract_matches_expected"), "backend reports IcUSD contract mismatch")

        price = optional_integer(status, "collateral_price_e8")
        price_age = optional_integer(status, "collateral_price_age_ns")
        metrics.update(collateral_price_e8=price, collateral_price_age_ns=price_age)
        require(price not in (None, 0), "CFX price is missing/zero")
        require(optional_integer(status, "collateral_price_set_at_ns") is not None, "CFX price timestamp is missing")
        require(price_age is not None, "CFX price age is unavailable")
        require(boolean(status, "collateral_price_is_fresh"), "CFX price is stale")

        require(not boolean(status, "protocol_frozen"), "protocol is frozen")
        require(variant(status, "protocol_mode") == "GeneralAvailability", "protocol mode is not GeneralAvailability")
        require(not boolean(status, "invariant_halted"), "supply invariant is halted")
        require(not boolean(status, "reorg_halted"), "chain reorg breaker is halted")
        require(not boolean(status, "bad_debt_circuit_tripped"), "bad-debt circuit is tripped")
        require(present_optional(status, "bad_debt_threshold_e8s"), "bad-debt circuit threshold is missing")
        burn_cursor = integer(status, "burn_cursor")
        require(burn_cursor > 0, "burn cursor is unseeded")
        chain_supply = integer(status, "chain_supply_e8s")
        audit_total, audit_entries = parse_supply_audit(audit)
        audit_sum = sum(supply for _, supply in audit_entries)
        target_entries = [supply for chain, supply in audit_entries if chain == expected_chain]
        metrics.update(
            finality_depth=optional_integer(status, "finality_depth"),
            burn_cursor=burn_cursor,
            chain_supply_e8s=chain_supply,
            reserve_backing_e8s=integer(status, "chain_reserve_backing_e8s"),
            pending_burn_e8s=integer(status, "chain_pending_burn_e8s"),
            operator_supply_total_e8s=audit_total,
        )
        require(audit_sum == audit_total, "operator supply audit total does not equal per-chain sum")
        require(len(target_entries) == 1, "operator supply audit does not contain exactly one chain entry")
        require(bool(target_entries) and target_entries[0] == chain_supply, "operator supply audit disagrees with chain status")

        hot_wallet = optional_boolean(status, "hot_wallet_ready")
        hot_wallet_refreshed_at = optional_integer(status, "hot_wallet_balance_refreshed_at_ns")
        hot_wallet_age = optional_integer(status, "hot_wallet_balance_age_ns")
        hot_wallet_max_age = integer(status, "hot_wallet_balance_max_age_ns")
        metrics.update(
            hot_wallet_refreshed_at_ns=hot_wallet_refreshed_at,
            hot_wallet_age_ns=hot_wallet_age,
            hot_wallet_max_age_ns=hot_wallet_max_age,
        )
        require(hot_wallet is True, "settlement hot-wallet readiness is false/unknown")
        require(
            hot_wallet_refreshed_at is not None and hot_wallet_refreshed_at > 0,
            "settlement hot-wallet refresh timestamp is missing",
        )
        require(hot_wallet_age is not None, "settlement hot-wallet balance age is unavailable")
        require(hot_wallet_max_age > 0, "settlement hot-wallet freshness limit is invalid")
        require(
            hot_wallet_age is not None and hot_wallet_age <= hot_wallet_max_age,
            "settlement hot-wallet balance evidence is stale",
        )
        require(
            boolean(status, "hot_wallet_balance_is_fresh"),
            "backend reports settlement hot-wallet balance evidence stale",
        )
        require(boolean(status, "liquidation_configured"), "liquidation config is missing")
        require(boolean(status, "liquidation_enabled"), "liquidation config is not enabled")
        require(
            boolean(status, "liquidation_config_matches_expected"),
            "liquidation config does not match the reviewed production shape",
        )
        liquidation_digest = optional_text(status, "liquidation_config_digest")
        expected_liquidation_digest = text_value(status, "expected_liquidation_config_digest")
        require(
            liquidation_digest is not None,
            "liquidation config digest is missing",
        )
        require(
            bool(re.fullmatch(r"[0-9a-f]{64}", expected_liquidation_digest)),
            "expected liquidation config digest is malformed",
        )
        require(
            liquidation_digest == expected_liquidation_digest,
            "liquidation config digest does not match the expected digest",
        )

        cycle_balance = integer(cycle_status, "balance")
        low_watermark = integer(cycle_status, "low_watermark")
        effective_cycle_floor = max(low_watermark, minimum_cycles)
        metrics.update(cycles=cycle_balance, cycles_floor=effective_cycle_floor)
        require(boolean(cycle_status, "healthy"), "cycles_status is unhealthy")
        require(
            cycle_balance >= effective_cycle_floor,
            f"cycles balance {cycle_balance} is below required {effective_cycle_floor}",
        )

        if expectation == "public-active":
            require(boolean(status, "registered"), "chain is not registered")
            require(boolean(status, "public_open_ready"), "backend public_open_ready is false")
            require(not blocking_reasons, "backend reports public launch blockers")
        else:
            require(not boolean(status, "registered"), "disabled chain unexpectedly reports registered")
            require(not boolean(status, "public_open_ready"), "disabled chain unexpectedly reports public ready")
            require(
                blocking_reasons == ["chain_disabled"],
                "disabled staging has blockers other than chain_disabled",
            )
    except ValueError as error:
        failures.append(f"malformed readiness response: {error}")

    if not quiet:
        if metrics:
            print("INFO " + " ".join(f"{key}={value}" for key, value in metrics.items()))
        for message in warnings:
            print(f"WARN {message}")
        for message in failures:
            print(f"FAIL {message}")
        if not failures:
            print(
                f"PASS Conflux chain {expected_chain}: expectation={expectation}, "
                "trust-anchors/contract/config-digests/RPC/finality/cursor/price/operator-audit/"
                "breakers/hot-wallet-freshness/cycles checks green"
            )
    return failures


def self_test():
    disabled = launch_status()
    good_cycles = cycles_status()
    good_audit = supply_audit()
    expected_contract = "0x8DdB0a13B26ed28912e4B8cCa99Bc3E8c66Df7Ff"
    expected_rpc_principal = "7hfb6-caaaa-aaaar-qadga-cai"
    expected_ecdsa_key = "key_1"
    assert not validate(disabled, good_cycles, good_audit, "disabled", 1030, expected_contract, expected_rpc_principal, expected_ecdsa_key, 5_000_000_000_000, quiet=True)

    active = launch_status(
        status="opt variant { Registered }",
        registered="true",
        public_open_ready="true",
        blocking_reasons="vec {}",
    )
    assert not validate(active, good_cycles, good_audit, "public-active", 1030, expected_contract, expected_rpc_principal, expected_ecdsa_key, 5_000_000_000_000, quiet=True)

    cases = [
        (launch_status(collateral_price_is_fresh="false"), good_cycles, good_audit, "disabled", "stale price"),
        (launch_status(rpc_endpoint_count="1 : nat32", rpc_configuration_sufficient="false"), good_cycles, good_audit, "disabled", "RPC floor"),
        (launch_status(invariant_halted="true"), good_cycles, good_audit, "disabled", "breaker"),
        (launch_status(hot_wallet_ready="null"), good_cycles, good_audit, "disabled", "unknown hot wallet"),
        (launch_status(hot_wallet_balance_is_fresh="false", hot_wallet_balance_age_ns="opt (300_000_000_001 : nat64)", blocking_reasons='vec { "chain_disabled"; "hot_wallet_balance_stale" }'), good_cycles, good_audit, "disabled", "stale hot-wallet evidence"),
        (launch_status(hot_wallet_balance_refreshed_at_ns="null"), good_cycles, good_audit, "disabled", "missing hot-wallet refresh timestamp"),
        (launch_status(effective_evm_rpc_principal='principal "aaaaa-aa"', evm_rpc_principal_matches_expected="false", blocking_reasons='vec { "chain_disabled"; "evm_rpc_principal_mismatch" }'), good_cycles, good_audit, "disabled", "wrong EVM-RPC principal"),
        (launch_status(chains_ecdsa_key_name='"test_key_1"', chains_ecdsa_key_matches_expected="false", blocking_reasons='vec { "chain_disabled"; "chains_ecdsa_key_mismatch" }'), good_cycles, good_audit, "disabled", "wrong ECDSA key"),
        (launch_status(bad_debt_threshold_e8s="null"), good_cycles, good_audit, "disabled", "missing bad-debt threshold"),
        (launch_status(bound_icusd_contract='opt "0x0000000000000000000000000000000000000000"', icusd_contract_matches_expected="false", blocking_reasons='vec { "chain_disabled"; "icusd_contract_mismatch" }'), good_cycles, good_audit, "disabled", "wrong contract"),
        (launch_status(finality_depth="opt (399 : nat32)"), good_cycles, good_audit, "disabled", "wrong finality"),
        (launch_status(burn_cursor="0 : nat64"), good_cycles, good_audit, "disabled", "unseeded cursor"),
        (launch_status(collateral_config_matches_expected="false", blocking_reasons='vec { "chain_disabled"; "conflux_mainnet_config_mismatch" }'), good_cycles, good_audit, "disabled", "collateral mismatch"),
        (launch_status(debt_config_matches_expected="false", blocking_reasons='vec { "chain_disabled"; "debt_config_mismatch" }'), good_cycles, good_audit, "disabled", "debt mismatch"),
        (launch_status(liquidation_enabled="false"), good_cycles, good_audit, "disabled", "liquidation not enabled while disabled"),
        (launch_status(liquidation_config_matches_expected="false", blocking_reasons='vec { "chain_disabled"; "liquidation_config_mismatch" }'), good_cycles, good_audit, "disabled", "liquidation mismatch"),
        (launch_status(liquidation_config_digest="null"), good_cycles, good_audit, "disabled", "missing liquidation digest"),
        (launch_status(liquidation_config_digest='opt "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"'), good_cycles, good_audit, "disabled", "wrong liquidation digest"),
        (launch_status(blocking_reasons='vec { "chain_disabled"; "conflux_mainnet_config_mismatch" }'), good_cycles, good_audit, "disabled", "hidden backend blocker"),
        (disabled, good_cycles, supply_audit(total_e8s="1 : nat"), "disabled", "operator supply audit mismatch"),
        (active, cycles_status(balance="4_999_999_999_999 : nat"), good_audit, "public-active", "low cycles"),
        (active.replace("public_open_ready = true;", ""), good_cycles, good_audit, "public-active", "malformed response"),
    ]
    for status, cycle_status, audit, expectation, label in cases:
        failures = validate(status, cycle_status, audit, expectation, 1030, expected_contract, expected_rpc_principal, expected_ecdsa_key, 5_000_000_000_000, quiet=True)
        assert failures, f"negative fixture unexpectedly passed: {label}"
    print(f"PASS self-test: 2 positive and {len(cases)} negative fixtures")


if os.environ["SELF_TEST"] == "1":
    self_test()
    sys.exit(0)

failures = validate(
    os.environ["STATUS_CANDID"],
    os.environ["CYCLES_CANDID"],
    os.environ["SUPPLY_AUDIT_CANDID"],
    os.environ["EXPECTATION"],
    int(os.environ["CHAIN_ID"]),
    os.environ["EXPECTED_ICUSD_CONTRACT"],
    os.environ["EXPECTED_EVM_RPC_PRINCIPAL"],
    os.environ["EXPECTED_ECDSA_KEY_NAME"],
    int(os.environ["MIN_CYCLES"]),
)
sys.exit(1 if failures else 0)
PY
