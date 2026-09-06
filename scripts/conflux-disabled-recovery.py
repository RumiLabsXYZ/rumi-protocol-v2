#!/usr/bin/env python3
"""Fail-closed one-shot recovery for the Disabled Conflux launch rail.

Dry-run/preflight is the default. Both modes require the exact reviewed
commit/file/module-hash binding; update dispatch additionally requires
--execute and the literal one-use execution id. The procedure never resumes a
prior execution id and never retries an update call.
"""

from __future__ import annotations

import argparse
import hashlib
import hmac
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


CANISTER = "tfesu-vyaaa-aaaap-qrd7a-cai"
NETWORK = "ic"
IDENTITY = "rumi_identity"
OPERATOR_PRINCIPAL = "fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae"
CHAIN_ID = 1030
CHAIN_ID_HEX = "0x406"
OLD_CURSOR = 154_966_240
FINALITY = 400
TARGET_LAG = 2_048
CANDIDATE_OFFSET = 1_024
MAX_CURSOR_JUMP = 1_000_000
MAX_HEAD_SKEW = 128
MAX_HEAD_AGE_SECS = 300
MAX_SAMPLE_SECS = 60
MIN_CYCLES = 5_000_000_000_000
PRE_DIGEST_MODULE_HASH = "14d65746d2d801347ecdb24dc54611b12cb3cca8765f5bf80f929751f1eda287"
CONTROLLERS = {
    "cpbhu-5iaaa-aaaad-aalta-cai",
    "mi66c-zqlu4-4kxd6-2gtp7-szg5v-6a62a-geoty-fahu5-4trje-xyfby-wqe",
    "fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae",
}
EVM_RPC_PRINCIPAL = "7hfb6-caaaa-aaaar-qadga-cai"
ECDSA_KEY = "key_1"
ICUSD = "0x8DdB0a13B26ed28912e4B8cCa99Bc3E8c66Df7Ff"
SETTLEMENT = "0x00142f7ee842b171d539ec6053eaf88dd9a1adda"
FACTORY = "0xe2a6f7c0ce4d5d300f97aa7e125455f5cd3342f5"
PAIR = "0x0736b3384531cda2f545f5449e84c6c6bcd6f01b"
WCFX = "0x14b2d3bc65e74dae1030eafd8ac30c533c976a9b"
USDC = "0x6963efed0ab40f6c3d7bda44a05dcf1437c44372"
LIQ_DIGEST = "d7ff0d667b867f4cb3fbccabd57c05911d17eee6888a5df58e81daf8954f4f1d"
BAD_DEBT_THRESHOLD = 10_000_000
EXECUTION_ID = "conflux-disabled-recovery-v1"
PROVIDERS = (
    ("Confura", "https://evm.confluxrpc.com"),
    ("BlockPI", "https://conflux-espace.blockpi.network/v1/rpc/public"),
    ("Unifra", "https://conflux-espace-public.unifra.io"),
)
RPC_ENDPOINT_DIGEST_DOMAIN_V1 = b"rumi.chain-rpc-endpoint-set-digest.v1"
RPC_EFFECTIVE_QUORUM = 2
RPC_ENDPOINT_SET_DIGEST_V1 = "c57af2bf81cf047aeeb94ecd463f612263abbcc3de93f1ec143488f32c951f31"
RUNBOOK = "docs/plans/2026-06-18-conflux-gated-mainnet-launch-runbook.md"
EVIDENCE = "docs/plans/2026-08-26-conflux-disabled-cursor-reseed-evidence.md"
SCRIPT = "scripts/conflux-disabled-recovery.py"
UPDATE_ALLOWLIST = (
    "set_last_observed_block",
    "set_chain_liquidation_config",
    "reconcile_chain_supply",
)
TOOL_PINS = {
    "/Library/Frameworks/Python.framework/Versions/3.11/bin/python3.11": "690ff84223d152000549f124e52752a724c0797a0f88a5c68b2b6fa304e50e36",
    "/usr/local/bin/icp": "6fc707d24d37c00c20e2e0466fb10e2bdd7c7ef4d10a2e945eece9cd60f6cd9b",
    "/Users/robertripley/.cargo/bin/didc": "358b9599583a6d70ea56c8d3db594b40391f52b9dc5a6b8295f9080305fe762a",
    "/usr/bin/git": "179301dcb41ea78accc3fa0048a7e6f6710d891945a751a34addd622020c1818",
    "/usr/bin/curl": "5ab042572ea0e068644e3b8f9e8dd1ad197bfcf33d199316615b46ddc4390a41",
}

PAIR_DATA = (
    "0xe6a43905"
    "000000000000000000000000" + WCFX[2:] +
    "000000000000000000000000" + USDC[2:]
)
TOTAL_SUPPLY_DATA = "0x18160ddd"
HAS_ROLE_SELECTOR = "0x91d14854"
DEFAULT_ADMIN_ROLE = "00" * 32
MINTER_ROLE = "9f2df0fed2c77648de5860a4cc508cd0818c85b8b8a1ab4ceeef8d981c8956a6"

LIQUIDATION_CANDID = """(1030 : nat32, record {
  dex = variant { UniswapV2 };
  router = \"0x62b0873055bf896dd869e172119871ac24aea305\";
  factory = \"0xe2a6f7c0ce4d5d300f97aa7e125455f5cd3342f5\";
  pair = \"0x0736b3384531cda2f545f5449e84c6c6bcd6f01b\";
  collateral_token = \"0x14b2d3bc65e74dae1030eafd8ac30c533c976a9b\";
  settle_stable_token = \"0x6963efed0ab40f6c3d7bda44a05dcf1437c44372\";
  slippage_cap_bps = 250 : nat16;
  restore_target_cr_e4 = 15_500 : nat64;
  enabled = true;
  max_swap_value_e8s = 200_000_000_000 : nat;
  max_price_age_ns = 1_800_000_000_000 : nat64;
  max_dex_oracle_divergence_bps = 500 : nat32;
  fee_bps = 25 : nat16;
  settle_stable_decimals = 18 : nat8;
  deadline_secs = 180 : nat64;
})"""


class Stop(RuntimeError):
    pass


class ProviderConflict(Stop):
    """A successful provider field contradicted the frozen matrix."""

    pass


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def endpoint_set_digest_v1(endpoints: list[str], chain_id: int, effective_quorum: int) -> str:
    """Mirror the backend's exact V1 length-prefixed endpoint-set commitment."""
    unique = sorted(set(endpoints), key=lambda value: value.encode("utf-8"))
    if not (0 <= chain_id <= 0xFFFF_FFFF and 0 <= effective_quorum <= 0xFFFF_FFFF):
        raise Stop("endpoint digest chain/quorum is outside u32")
    if len(unique) > 0xFFFF_FFFF:
        raise Stop("endpoint digest set is outside u32")
    hasher = hashlib.sha256()
    for value in (RPC_ENDPOINT_DIGEST_DOMAIN_V1,):
        hasher.update(len(value).to_bytes(8, "big"))
        hasher.update(value)
    hasher.update(chain_id.to_bytes(4, "big"))
    hasher.update(effective_quorum.to_bytes(4, "big"))
    hasher.update(len(unique).to_bytes(4, "big"))
    for endpoint in unique:
        raw = endpoint.encode("utf-8")
        hasher.update(len(raw).to_bytes(8, "big"))
        hasher.update(raw)
    return hasher.hexdigest()


def canonical_json(value: Any) -> bytes:
    return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()


def atomic_private_json(path: Path, value: Any) -> None:
    data = canonical_json(value)
    tmp = path.with_name(path.name + ".tmp")
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    try:
        with os.fdopen(fd, "wb") as handle:
            handle.write(data)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(tmp, path)
        dir_fd = os.open(path.parent, os.O_RDONLY)
        try:
            os.fsync(dir_fd)
        finally:
            os.close(dir_fd)
    finally:
        if tmp.exists():
            tmp.unlink()


@dataclass(frozen=True)
class Target:
    h: int
    t: int
    f: int
    c: int
    p: int


def select_target(heads: list[int]) -> Target:
    if len(heads) != 3 or any(not isinstance(v, int) or isinstance(v, bool) for v in heads):
        raise Stop("selection requires exactly three integer provider heads")
    if min(heads) <= TARGET_LAG:
        raise Stop("provider head is too small for target arithmetic")
    if max(heads) - min(heads) > MAX_HEAD_SKEW:
        raise Stop("provider head skew exceeds the 128-block bound")
    h = min(heads)
    t = h - TARGET_LAG
    f = t + FINALITY
    c = t + CANDIDATE_OFFSET
    p = c + FINALITY
    if not (OLD_CURSOR < t < f < c < p <= h - 624):
        raise Stop("target arithmetic or monotonic cursor guard failed")
    if t - OLD_CURSOR > MAX_CURSOR_JUMP:
        raise Stop("target exceeds the reviewed 1,000,000-block maximum cursor jump")
    return Target(h, t, f, c, p)


def word_address(address: str) -> str:
    raw = address.lower().removeprefix("0x")
    if not re.fullmatch(r"[0-9a-f]{40}", raw):
        raise Stop("invalid EVM address constant")
    return "0" * 24 + raw


def has_role_data(role: str) -> str:
    if not re.fullmatch(r"[0-9a-f]{64}", role):
        raise Stop("invalid role constant")
    return HAS_ROLE_SELECTOR + role + word_address(SETTLEMENT)


class EvidenceRun:
    def __init__(self, execute: bool, run_dir: Path):
        self.execute = execute
        self.run_dir = run_dir
        self.raw_dir = run_dir / "raw"
        self.raw_dir.mkdir(mode=0o700)
        self.transcript_path = run_dir / "sanitized-transcript.log"
        self.transcript_path.touch(mode=0o600)
        os.chmod(self.transcript_path, 0o600)
        self.observations: list[dict[str, Any]] = []
        self.update_count = 0
        self.journal_path = run_dir / "journal.json"
        self.journal_history: list[dict[str, Any]] = []

    def log(self, message: str) -> None:
        line = f"{utc_now()} {message}"
        print(line, flush=True)
        with self.transcript_path.open("a", encoding="utf-8") as handle:
            handle.write(line + "\n")
            handle.flush()
            os.fsync(handle.fileno())

    def capture(self, label: str, stdout: bytes, stderr: bytes, rc: int) -> dict[str, Any]:
        stamp = utc_now()
        safe = re.sub(r"[^A-Za-z0-9_.-]", "_", label)
        out_path = self.raw_dir / f"{len(self.observations):03d}-{safe}.stdout"
        err_path = self.raw_dir / f"{len(self.observations):03d}-{safe}.stderr"
        for path, data in ((out_path, stdout), (err_path, stderr)):
            fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            with os.fdopen(fd, "wb") as handle:
                handle.write(data)
                handle.flush()
                os.fsync(handle.fileno())
        item = {
            "label": label,
            "timestamp": stamp,
            "rc": rc,
            "stdout_sha256": sha256_bytes(stdout),
            "stderr_sha256": sha256_bytes(stderr),
        }
        self.observations.append(item)
        return item

    def command(self, label: str, argv: list[str], timeout: int = 60) -> bytes:
        try:
            result = subprocess.run(argv, capture_output=True, timeout=timeout, check=False)
        except subprocess.TimeoutExpired as exc:
            stdout = exc.stdout or b""
            stderr = exc.stderr or b""
            self.capture(label, stdout, stderr, 124)
            raise Stop(f"{label} timed out; no retry permitted") from exc
        self.capture(label, result.stdout, result.stderr, result.returncode)
        if result.returncode != 0:
            raise Stop(f"{label} failed with exit {result.returncode}; no retry permitted")
        return result.stdout

    def write_journal(self, state: str, **extra: Any) -> None:
        entry = {
            "state": state,
            "updated_at": utc_now(),
            "update_count": self.update_count,
            **extra,
        }
        self.journal_history.append(entry)
        value = {
            "execution_id": EXECUTION_ID,
            "current": entry,
            "history": self.journal_history,
        }
        atomic_private_json(self.journal_path, value)


def parse_json_object(raw: bytes, label: str) -> dict[str, Any]:
    try:
        value = json.loads(raw)
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise Stop(f"{label} returned malformed JSON") from exc
    if not isinstance(value, dict):
        raise Stop(f"{label} JSON is not an object")
    return value


def parse_candid_response(raw: bytes, label: str) -> str:
    obj = parse_json_object(raw, label)
    candid = obj.get("response_candid")
    if not isinstance(candid, str) or not candid.strip():
        raise Stop(f"{label} lacks a non-empty response_candid")
    return candid


def strict_ok(candid: str) -> bool:
    compact = " ".join(candid.split())
    return bool(re.fullmatch(r"\(\s*variant \{ Ok(?: = null)? \}\s*,?\s*\)", compact))


def named_nat(text: str, name: str) -> int:
    match = re.search(rf"\b{re.escape(name)}\s*=\s*([0-9_]+)(?:\s*:\s*nat(?:8|16|32|64)?)?\s*;", text)
    if not match:
        raise Stop(f"missing numeric field {name}")
    return int(match.group(1).replace("_", ""))


def optional_named_nat(text: str, name: str) -> int | None:
    match = re.search(
        rf"\b{re.escape(name)}\s*=\s*(null|opt\s*\(([0-9_]+)(?:\s*:\s*nat(?:8|16|32|64)?)?\))\s*;",
        text,
    )
    if not match:
        raise Stop(f"missing optional numeric field {name}")
    return None if match.group(1) == "null" else int(match.group(2).replace("_", ""))


def named_bool(text: str, name: str) -> bool:
    match = re.search(rf"\b{re.escape(name)}\s*=\s*(true|false)\s*;", text)
    if not match:
        raise Stop(f"missing boolean field {name}")
    return match.group(1) == "true"


def query(run: EvidenceRun, label: str, method: str, args: str = "()") -> str:
    raw = run.command(
        label,
        [
            "/usr/local/bin/icp", "canister", "call", CANISTER, method, args,
            "-n", NETWORK, "--identity", IDENTITY, "--query", "--json",
        ],
    )
    return parse_candid_response(raw, label)


def replicated_read(run: EvidenceRun, label: str, method: str, args: str) -> str:
    """Invoke a read-only query method through replicated ingress execution."""
    raw = run.command(
        label,
        replicated_read_argv(method, args),
        timeout=240,
    )
    return parse_candid_response(raw, label)


def replicated_read_argv(method: str, args: str) -> list[str]:
    return [
        "/usr/local/bin/icp", "canister", "call", CANISTER, method, args,
        "-n", NETWORK, "--identity", IDENTITY, "--json",
    ]


def parse_endpoint_digest_result(candid: str, label: str) -> dict[str, Any]:
    compact = " ".join(candid.split())
    outer = re.fullmatch(r"\(\s*variant \{ Ok = record \{ (.*) \} \}\s*,?\s*\)", compact)
    if not outer:
        raise Stop(f"{label} did not return an anchored Ok endpoint-digest record")
    pieces = [piece.strip() for piece in outer.group(1).split(";") if piece.strip()]
    if len(pieces) != 4:
        raise Stop(f"{label} endpoint-digest record has an unexpected field count")
    fields: dict[str, str] = {}
    for piece in pieces:
        match = re.fullmatch(r"([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.+)", piece)
        if not match or match.group(1) in fields:
            raise Stop(f"{label} endpoint-digest record has malformed/duplicate fields")
        fields[match.group(1)] = match.group(2).strip()
    expected_names = {
        "digest_sha256", "effective_min_quorum_providers", "endpoint_count", "chain_id",
    }
    if set(fields) != expected_names:
        raise Stop(f"{label} endpoint-digest record has unexpected fields")

    def scalar_nat(name: str, kind: str) -> int:
        match = re.fullmatch(rf"([0-9_]+)(?:\s*:\s*{kind})?", fields[name])
        if not match:
            raise Stop(f"{label} endpoint-digest field {name} is malformed")
        return int(match.group(1).replace("_", ""))

    digest_match = re.fullmatch(r'"([0-9a-f]{64})"', fields["digest_sha256"])
    if not digest_match:
        raise Stop(f"{label} endpoint digest is not lowercase unprefixed 64-hex")
    return {
        "chain_id": scalar_nat("chain_id", "nat32"),
        "endpoint_count": scalar_nat("endpoint_count", "nat32"),
        "effective_min_quorum_providers": scalar_nat("effective_min_quorum_providers", "nat32"),
        "digest_sha256": digest_match.group(1),
    }


def require_endpoint_digest_binding(value: dict[str, Any], phase: str) -> None:
    if value["chain_id"] != CHAIN_ID:
        raise Stop(f"{phase} endpoint digest chain is not 1030")
    if value["endpoint_count"] != len(PROVIDERS):
        raise Stop(f"{phase} endpoint digest count is not three")
    if value["effective_min_quorum_providers"] != RPC_EFFECTIVE_QUORUM:
        raise Stop(f"{phase} endpoint digest quorum is not two")
    if not hmac.compare_digest(value["digest_sha256"], RPC_ENDPOINT_SET_DIGEST_V1):
        raise Stop(f"{phase} live endpoint-set digest mismatches reviewed Confura/BlockPI/Unifra set")


def bind_live_endpoint_set(run: EvidenceRun, phase: str) -> dict[str, Any]:
    candid = replicated_read(
        run,
        f"{phase}-replicated-rpc-endpoint-digest",
        "get_chain_rpc_endpoint_set_digest",
        "(1030 : nat32)",
    )
    value = parse_endpoint_digest_result(candid, phase)
    require_endpoint_digest_binding(value, phase)
    run.log(
        f"{phase} replicated_endpoint_binding=PASS chain_id={CHAIN_ID} "
        f"endpoint_count={len(PROVIDERS)} effective_quorum={RPC_EFFECTIVE_QUORUM} "
        f"digest_sha256={RPC_ENDPOINT_SET_DIGEST_V1}"
    )
    return value


def rpc_once(run: EvidenceRun, provider: str, url: str, label: str, payload: list[dict[str, Any]]) -> list[dict[str, Any]]:
    raw = run.command(
        f"rpc-{provider}-{label}",
        [
            "/usr/bin/curl", "-sS", "--fail-with-body", "--max-time", "30",
            "-H", "Content-Type: application/json", "--data", json.dumps(payload, separators=(",", ":")), url,
        ],
        timeout=35,
    )
    try:
        value = json.loads(raw)
    except (UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise Stop(f"{provider} {label} returned malformed JSON") from exc
    if not isinstance(value, list):
        raise Stop(f"{provider} {label} did not return a JSON-RPC batch")
    return value


def response_map(items: list[dict[str, Any]], label: str) -> dict[int, dict[str, Any]]:
    result: dict[int, dict[str, Any]] = {}
    for item in items:
        if not isinstance(item, dict) or not isinstance(item.get("id"), int):
            raise Stop(f"{label} contains malformed JSON-RPC response")
        if item["id"] in result:
            raise Stop(f"{label} contains duplicate JSON-RPC id")
        result[item["id"]] = item
    return result


def require_result(item: dict[str, Any], label: str) -> Any:
    if item.get("error") is not None or "result" not in item or item.get("result") is None:
        raise Stop(f"{label} lacks a successful JSON-RPC result")
    return item["result"]


def select_heads(run: EvidenceRun) -> tuple[list[int], dict[str, Any]]:
    started = time.monotonic()
    heads: list[int] = []
    evidence: dict[str, Any] = {}
    now = int(time.time())
    for provider, url in PROVIDERS:
        items = rpc_once(run, provider, url, "head", [
            {"jsonrpc": "2.0", "id": 1, "method": "eth_chainId", "params": []},
            {"jsonrpc": "2.0", "id": 2, "method": "eth_getBlockByNumber", "params": ["latest", False]},
        ])
        by_id = response_map(items, f"{provider} head")
        if require_result(by_id.get(1, {}), f"{provider} chainId") != CHAIN_ID_HEX:
            raise Stop(f"{provider} chainId is not 1030")
        block = require_result(by_id.get(2, {}), f"{provider} latest block")
        if not isinstance(block, dict):
            raise Stop(f"{provider} latest block is malformed")
        try:
            number = int(block["number"], 16)
            timestamp = int(block["timestamp"], 16)
            block_hash = block["hash"].lower()
        except (KeyError, TypeError, ValueError) as exc:
            raise Stop(f"{provider} latest block fields are malformed") from exc
        if now - timestamp < -30 or now - timestamp > MAX_HEAD_AGE_SECS:
            raise Stop(f"{provider} head timestamp is outside the 300-second age bound")
        if not re.fullmatch(r"0x[0-9a-f]{64}", block_hash):
            raise Stop(f"{provider} latest block hash is malformed")
        heads.append(number)
        evidence[provider] = {"head": number, "hash": block_hash, "timestamp": timestamp}
    elapsed = time.monotonic() - started
    if elapsed > MAX_SAMPLE_SECS:
        raise Stop("three-provider head sample exceeded 60 seconds")
    evidence["sample_elapsed_ms"] = round(elapsed * 1000)
    return heads, evidence


def matrix_payload(target: Target) -> list[dict[str, Any]]:
    blocks = (target.h, target.t, target.f, target.c, target.p)
    payload: list[dict[str, Any]] = [
        {"jsonrpc": "2.0", "id": 1, "method": "eth_chainId", "params": []},
    ]
    for idx, block in enumerate(blocks, start=2):
        payload.append({"jsonrpc": "2.0", "id": idx, "method": "eth_getBlockByNumber", "params": [hex(block), False]})
    payload.extend([
        {"jsonrpc": "2.0", "id": 7, "method": "eth_call", "params": [{"to": FACTORY, "data": PAIR_DATA}, hex(target.t)]},
        {"jsonrpc": "2.0", "id": 8, "method": "eth_call", "params": [{"to": ICUSD, "data": TOTAL_SUPPLY_DATA}, hex(target.t)]},
        {"jsonrpc": "2.0", "id": 9, "method": "eth_call", "params": [{"to": FACTORY, "data": PAIR_DATA}, hex(target.c)]},
        {"jsonrpc": "2.0", "id": 10, "method": "eth_call", "params": [{"to": ICUSD, "data": has_role_data(DEFAULT_ADMIN_ROLE)}, hex(target.t)]},
        {"jsonrpc": "2.0", "id": 11, "method": "eth_call", "params": [{"to": ICUSD, "data": has_role_data(MINTER_ROLE)}, hex(target.t)]},
    ])
    return payload


def matrix_once(
    run: EvidenceRun,
    provider: str,
    url: str,
    label: str,
    target: Target,
    expected_hashes: dict[int, str] | None = None,
) -> list[dict[str, Any]]:
    # BlockPI's public path rejects large batches. Keep every no-retry request
    # at five entries or fewer while treating the four responses as one bounded
    # matrix sample. IDs remain unique across the assembled matrix.
    payload = matrix_payload(target)
    chunks = (payload[0:4], payload[4:6], payload[6:9], payload[9:11])
    assembled: list[dict[str, Any]] = []
    for index, chunk in enumerate(chunks, start=1):
        assembled.extend(rpc_once(run, provider, url, f"{label}-part{index}", chunk))
        if expected_hashes is not None:
            # Validate each successful field immediately. A later transport or
            # state error must not erase an already-observed contradiction.
            validate_successful_matrix_fields(
                response_map(assembled, f"{provider} {label} partial matrix"),
                target,
                expected_hashes,
                provider,
            )
    return assembled


def validate_successful_matrix_fields(
    by_id: dict[int, dict[str, Any]],
    target: Target,
    expected_hashes: dict[int, str] | None,
    provider: str,
) -> dict[int, str]:
    """Reject every observed successful conflict, even in a partial response."""
    chain_item = by_id.get(1)
    if chain_item is not None and chain_item.get("error") is None and chain_item.get("result") is not None:
        if chain_item["result"] != CHAIN_ID_HEX:
            raise ProviderConflict(f"{provider} chainId is not 1030")
    hashes: dict[int, str] = {}
    for idx, block_number in zip(range(2, 7), (target.h, target.t, target.f, target.c, target.p)):
        item = by_id.get(idx)
        if item is None or item.get("error") is not None or item.get("result") is None:
            continue
        block = item["result"]
        if not isinstance(block, dict) or block.get("number") != hex(block_number):
            raise ProviderConflict(f"{provider} returned the wrong block for {block_number}")
        block_hash = str(block.get("hash", "")).lower()
        if not re.fullmatch(r"0x[0-9a-f]{64}", block_hash):
            raise ProviderConflict(f"{provider} returned a malformed hash for {block_number}")
        if expected_hashes is not None and block_hash != expected_hashes[block_number]:
            raise ProviderConflict(f"{provider} header mismatch at {block_number}")
        hashes[block_number] = block_hash
    pair_word = "0x" + "0" * 24 + PAIR[2:]
    zero_word = "0x" + "0" * 64
    true_word = "0x" + "0" * 63 + "1"
    expected_results = {7: pair_word, 8: zero_word, 9: pair_word, 10: true_word, 11: true_word}
    for idx, expected in expected_results.items():
        item = by_id.get(idx)
        if item is None or item.get("error") is not None or item.get("result") is None:
            continue
        actual = str(item["result"]).lower()
        if actual != expected:
            raise ProviderConflict(f"{provider} state id {idx} mismatched the frozen expected value")
    return hashes


def parse_matrix(items: list[dict[str, Any]], target: Target, expected_hashes: dict[int, str] | None, provider: str) -> dict[str, Any]:
    by_id = response_map(items, f"{provider} matrix")
    required_ids = set(range(1, 12))
    if set(by_id) != required_ids or any(
        item.get("error") is not None or item.get("result") is None for item in by_id.values()
    ):
        raise Stop(f"{provider} matrix is incomplete")
    hashes = validate_successful_matrix_fields(by_id, target, expected_hashes, provider)
    return {"hashes": hashes, "state": "canonical-pair/zero-supply/admin+minter"}


def classify_phase_provider(
    items: list[dict[str, Any]], target: Target, expected_hashes: dict[int, str], provider: str,
) -> dict[str, Any] | None:
    by_id = response_map(items, f"{provider} phase matrix")
    # This validation deliberately runs before availability classification.
    # A provider cannot hide a successful conflict behind another unavailable
    # field while the remaining two providers satisfy the positive floor.
    validate_successful_matrix_fields(by_id, target, expected_hashes, provider)
    required_ids = set(range(1, 12))
    if set(by_id) != required_ids or any(
        item.get("error") is not None or item.get("result") is None for item in by_id.values()
    ):
        return None
    return parse_matrix(items, target, expected_hashes, provider)


def selection_matrix(run: EvidenceRun, target: Target) -> tuple[dict[int, str], dict[str, Any]]:
    started = time.monotonic()
    parsed: dict[str, Any] = {}
    hashes: dict[int, str] | None = None
    for provider, url in PROVIDERS:
        value = parse_matrix(matrix_once(run, provider, url, "selection-matrix", target), target, hashes, provider)
        if hashes is None:
            hashes = value["hashes"]
        parsed[provider] = value
    if time.monotonic() - started > MAX_SAMPLE_SECS:
        raise Stop("selection matrix sample exceeded 60 seconds")
    assert hashes is not None
    return hashes, parsed


def phase_matrix(run: EvidenceRun, phase: str, target: Target, hashes: dict[int, str]) -> dict[str, Any]:
    started = time.monotonic()
    successes: dict[str, Any] = {}
    failures: dict[str, str] = {}
    for provider, url in PROVIDERS:
        try:
            items = matrix_once(run, provider, url, f"{phase}-matrix", target, hashes)
        except ProviderConflict:
            raise
        except Stop as exc:
            failures[provider] = str(exc)
            run.log(f"{phase} provider={provider} state_matrix=unavailable")
            continue
        value = classify_phase_provider(items, target, hashes, provider)
        if value is None:
            failures[provider] = "one or more required results unavailable"
            run.log(f"{phase} provider={provider} state_matrix=unavailable")
            continue
        # A complete but conflicting response is not an unavailable provider;
        # it is evidence drift and invalidates the run even if the other two
        # happen to agree.
        successes[provider] = value
    elapsed = time.monotonic() - started
    if elapsed > MAX_SAMPLE_SECS:
        raise Stop(f"{phase} provider matrix exceeded 60 seconds")
    require_phase_floor(len(successes), phase)
    return {"successes": sorted(successes), "failures": failures, "sample_elapsed_ms": round(elapsed * 1000)}


def require_phase_floor(success_count: int, phase: str) -> None:
    if success_count < 2:
        raise Stop(f"{phase} has only {success_count}/3 exact positive provider agreements")


def archive_supply_zero(run: EvidenceRun, label: str) -> None:
    provider, url = PROVIDERS[0]
    items = rpc_once(run, provider, url, label, [
        {"jsonrpc": "2.0", "id": 1, "method": "eth_chainId", "params": []},
        {"jsonrpc": "2.0", "id": 2, "method": "eth_call", "params": [{"to": ICUSD, "data": TOTAL_SUPPLY_DATA}, hex(OLD_CURSOR)]},
    ])
    by_id = response_map(items, f"{provider} {label}")
    if require_result(by_id.get(1, {}), "archive chainId") != CHAIN_ID_HEX:
        raise Stop("archive path chainId is not 1030")
    if str(require_result(by_id.get(2, {}), "archive old-cursor supply")).lower() != "0x" + "0" * 64:
        raise Stop("IcUSD supply at the old cursor is nonzero")


def expect_fields(text: str, fields: dict[str, str], label: str) -> None:
    compact = " ".join(text.split())
    for name, pattern in fields.items():
        if not re.search(rf"\b{re.escape(name)}\s*=\s*{pattern}\s*;", compact):
            raise Stop(f"{label} field {name} failed its exact expectation")


def exact_liquidation_row(text: str) -> bool:
    compact = " ".join(text.split())
    if compact == "(null)":
        return False
    patterns = {
        "dex": r"variant \{ UniswapV2 \}",
        "router": r'"0x62b0873055bf896dd869e172119871ac24aea305"',
        "factory": rf'"{FACTORY}"',
        "pair": rf'"{PAIR}"',
        "collateral_token": rf'"{WCFX}"',
        "settle_stable_token": rf'"{USDC}"',
        "slippage_cap_bps": r"250(?:\s*:\s*nat16)?",
        "restore_target_cr_e4": r"15_500(?:\s*:\s*nat64)?",
        "enabled": r"true",
        "max_swap_value_e8s": r"200_000_000_000(?:\s*:\s*nat)?",
        "max_price_age_ns": r"1_800_000_000_000(?:\s*:\s*nat64)?",
        "max_dex_oracle_divergence_bps": r"500(?:\s*:\s*nat32)?",
        "fee_bps": r"25(?:\s*:\s*nat16)?",
        "settle_stable_decimals": r"18(?:\s*:\s*nat8)?",
        "deadline_secs": r"180(?:\s*:\s*nat64)?",
    }
    try:
        expect_fields(compact, patterns, "liquidation row")
    except Stop:
        return False
    return True


def parse_single_nat64(text: str, label: str) -> int:
    match = re.fullmatch(r"\(\s*([0-9_]+)\s*:\s*nat64\s*\)", text.strip())
    if not match:
        raise Stop(f"{label} returned malformed nat64 Candid")
    return int(match.group(1).replace("_", ""))


def phase_expected_state(phase: str, target: Target) -> tuple[int, bool]:
    if phase == "phase1":
        return OLD_CURSOR, False
    if phase == "phase2":
        return target.t, False
    if phase in {"phase3", "post"}:
        return target.t, True
    raise Stop(f"unknown recovery phase {phase}")


def backend_gate(run: EvidenceRun, phase: str, target: Target, expected_module_hash: str) -> dict[str, Any]:
    status = run.command(
        f"{phase}-canister-status",
        ["/usr/local/bin/icp", "canister", "status", CANISTER, "-n", NETWORK, "--identity", IDENTITY],
    ).decode("utf-8", "strict")
    if "Status: Running" not in status or f"Module hash: 0x{expected_module_hash}" not in status:
        raise Stop(f"{phase} backend status/module mismatch")
    controller_match = re.search(r"Controllers:\s*([^\n]+)", status)
    if not controller_match or {v.strip() for v in controller_match.group(1).split(",")} != CONTROLLERS:
        raise Stop(f"{phase} controller set mismatch")
    cycles_match = re.search(r"\n\s*Cycles:\s*([0-9_]+)", status)
    if not cycles_match or int(cycles_match.group(1).replace("_", "")) < MIN_CYCLES:
        raise Stop(f"{phase} canister cycles below floor")

    launch = query(run, f"{phase}-launch", "get_chain_public_launch_status", "(1030 : nat32)")
    expect_fields(launch, {
        "status": r"opt variant \{ Disabled \}",
        "registered": r"false",
        "public_open_ready": r"false",
        "configured": r"true",
        "chain_id": r"1_030(?:\s*:\s*nat32)?",
        "chain_supply_e8s": r"0(?:\s*:\s*nat)?",
        "chain_reserve_backing_e8s": r"0(?:\s*:\s*nat)?",
        "chain_pending_burn_e8s": r"0(?:\s*:\s*nat)?",
        "bad_debt_e8s": r"0(?:\s*:\s*nat)?",
        "bad_debt_threshold_e8s": r"opt \(10_000_000(?:\s*:\s*nat)?\)",
        "bad_debt_circuit_tripped": r"false",
        "invariant_halted": r"false",
        "reorg_halted": r"false",
        "protocol_frozen": r"false",
        "protocol_mode": r"variant \{ GeneralAvailability \}",
        "effective_evm_rpc_principal": rf'principal "{EVM_RPC_PRINCIPAL}"',
        "evm_rpc_principal_matches_expected": r"true",
        "chains_ecdsa_key_name": rf'"{ECDSA_KEY}"',
        "chains_ecdsa_key_matches_expected": r"true",
        "bound_icusd_contract": rf'opt "{ICUSD}"',
        "icusd_contract_matches_expected": r"true",
        "rpc_endpoint_count": r"3(?:\s*:\s*nat32)?",
        "rpc_min_quorum_providers": r"2(?:\s*:\s*nat32)?",
        "rpc_effective_agreement_requirement": r"2(?:\s*:\s*nat32)?",
        "rpc_configuration_sufficient": r"true",
        "finality_depth": r"opt \(400(?:\s*:\s*nat32)?\)",
        "collateral_config_matches_expected": r"true",
        "debt_config_matches_expected": r"true",
        "expected_liquidation_config_digest": rf'"{LIQ_DIGEST}"',
    }, f"{phase} launch")

    cycles = query(run, f"{phase}-cycles", "cycles_status")
    if not named_bool(cycles, "healthy") or named_nat(cycles, "balance") < MIN_CYCLES or named_nat(cycles, "low_watermark") != MIN_CYCLES:
        raise Stop(f"{phase} cycles_status failed")
    supply = query(run, f"{phase}-supply-audit", "get_supply_audit")
    if named_nat(supply, "total_e8s") != 0 or named_nat(supply, "supply_e8s") != 0 or named_nat(supply, "chain_id") != CHAIN_ID:
        raise Stop(f"{phase} internal supply audit is nonzero/malformed")
    if "(0 : nat)" not in query(run, f"{phase}-global-supply", "get_global_icusd_supply"):
        raise Stop(f"{phase} global supply is nonzero")
    cursor = parse_single_nat64(
        query(run, f"{phase}-cursor", "get_last_observed_block", "(1030 : nat32)"),
        f"{phase} cursor",
    )
    expected_cursor, expect_row = phase_expected_state(phase, target)
    if cursor != expected_cursor:
        raise Stop(f"{phase} cursor {cursor} != expected {expected_cursor}")
    row = query(run, f"{phase}-liquidation-row", "get_chain_liquidation_config", "(1030 : nat32)")
    if exact_liquidation_row(row) != expect_row:
        raise Stop(f"{phase} liquidation row phase mapping failed")
    if named_bool(launch, "liquidation_configured") != expect_row:
        raise Stop(f"{phase} liquidation configured projection mismatch")
    compact_launch = " ".join(launch.split())
    if expect_row:
        if not named_bool(launch, "liquidation_enabled") or not named_bool(launch, "liquidation_config_matches_expected"):
            raise Stop(f"{phase} liquidation readiness projection mismatch")
        if not re.search(rf'liquidation_config_digest\s*=\s*opt "{LIQ_DIGEST}"\s*;', compact_launch):
            raise Stop(f"{phase} liquidation digest mismatch")
    else:
        if not re.search(r"liquidation_config_digest\s*=\s*null\s*;", compact_launch):
            raise Stop(f"{phase} unexpected liquidation digest")

    bad_debt = query(run, f"{phase}-bad-debt", "get_chain_bad_debt_circuit_status", "(1030 : nat32)")
    if named_bool(bad_debt, "tripped") or named_nat(bad_debt, "bad_debt_e8s") != 0 or optional_named_nat(bad_debt, "threshold_e8s") != BAD_DEBT_THRESHOLD:
        raise Stop(f"{phase} bad-debt circuit gate failed")
    vaults = query(run, f"{phase}-vault-page", "list_chain_vaults_page", "(1030 : nat32, null, 100 : nat16)")
    expect_fields(vaults, {
        "done": r"true", "scanned_count": r"1(?:\s*:\s*nat16)?", "next_start_after": r"null",
        "status": r"variant \{ Closed \}", "vault_id": r"1(?:\s*:\s*nat64)?",
        "debt_e8s": r"0(?:\s*:\s*nat)?", "collateral_amount_e18": r"0(?:\s*:\s*nat)?",
        "pending_mint_e8s": r"0(?:\s*:\s*nat)?", "pending_interest_mint_e8s": r"0(?:\s*:\s*nat)?",
        "pending_liquidation": r"null",
    }, f"{phase} vault inventory")
    if query(run, f"{phase}-active-settlement", "chain_has_active_settlement_op", "(1030 : nat32)").strip() != "(false)":
        raise Stop(f"{phase} active settlement operation exists")
    proofs = " ".join(query(run, f"{phase}-settlement-proofs", "get_settlement_proof_ids", "(opt (1030 : nat32))").split())
    if proofs != "(record { pending = vec {}; reserve = vec {} })":
        raise Stop(f"{phase} settlement proof inventory is nonempty")
    if " ".join(query(run, f"{phase}-pending-burns", "get_pending_chain_burn_aging").split()) != "(vec {})":
        raise Stop(f"{phase} pending burn inventory is nonempty")
    archive_supply_zero(run, f"{phase}-old-cursor-supply")
    return {"cursor": cursor, "liquidation_row_present": expect_row, "cycles": named_nat(cycles, "balance")}


def encode_arg(run: EvidenceRun, method: str, candid: str, repo: Path) -> str:
    raw = run.command(
        f"encode-{method}",
        ["/Users/robertripley/.cargo/bin/didc", "encode", "-d", str(repo / "src/rumi_protocol_backend/rumi_protocol_backend.did"), "-m", method, candid],
    )
    value = raw.decode().strip().lower()
    if not re.fullmatch(r"[0-9a-f]+", value) or not value.startswith("4449444c"):
        raise Stop(f"didc produced malformed Candid bytes for {method}")
    return value


def operator_fingerprints(run: EvidenceRun) -> dict[str, Any]:
    tools: dict[str, Any] = {}
    for path_text, expected_hash in TOOL_PINS.items():
        path = Path(path_text)
        if not path.is_file():
            raise Stop(f"pinned operator tool is missing: {path_text}")
        actual = sha256_bytes(path.read_bytes())
        if actual != expected_hash:
            raise Stop(f"operator tool fingerprint drift: {path_text}")
        tools[path_text] = actual
    resolved = {
        "python": str(Path(sys.executable).resolve()),
        "icp": shutil.which("icp"),
        "didc": shutil.which("didc"),
        "git": shutil.which("git"),
        "curl": shutil.which("curl"),
    }
    expected_resolved = {
        "python": "/Library/Frameworks/Python.framework/Versions/3.11/bin/python3.11",
        "icp": "/usr/local/bin/icp",
        "didc": "/Users/robertripley/.cargo/bin/didc",
        "git": "/usr/bin/git",
        "curl": "/usr/bin/curl",
    }
    if resolved != expected_resolved:
        raise Stop("operator PATH resolves a pinned tool to an unexpected binary")
    principal = run.command(
        "operator-identity-principal",
        ["/usr/local/bin/icp", "identity", "principal", "--identity", IDENTITY],
    ).decode("utf-8", "strict").strip()
    if principal != OPERATOR_PRINCIPAL or principal not in CONTROLLERS:
        raise Stop("rumi_identity does not resolve to the reviewed controller principal")
    versions = {
        "python": sys.version,
        "icp": run.command("tool-version-icp", ["/usr/local/bin/icp", "--version"]).decode().strip(),
        "didc": run.command("tool-version-didc", ["/Users/robertripley/.cargo/bin/didc", "--version"]).decode().strip(),
        "git": run.command("tool-version-git", ["/usr/bin/git", "--version"]).decode().strip(),
        "curl": run.command("tool-version-curl", ["/usr/bin/curl", "--version"]).decode().splitlines()[0].strip(),
    }
    return {"principal": principal, "binary_sha256": tools, "versions": versions}


def update_once(run: EvidenceRun, method: str, arg_hex: str) -> tuple[str | None, str | None]:
    if method not in UPDATE_ALLOWLIST:
        raise Stop("attempted method outside literal three-call allowlist")
    if run.update_count >= 3:
        raise Stop("three-update maximum exceeded")
    run.update_count += 1
    run.write_journal("dispatching", method=method, arg_sha256=sha256_bytes(bytes.fromhex(arg_hex)))
    label = f"update-{run.update_count}-{method}"
    try:
        raw = run.command(
            label,
            [
                "/usr/local/bin/icp", "canister", "call", CANISTER, method, arg_hex,
                "--args-format", "hex", "-n", NETWORK, "--identity", IDENTITY, "--json",
            ],
            timeout=240,
        )
    except Stop as exc:
        run.write_journal("ambiguous-stop", method=method, reason=str(exc))
        return None, str(exc)
    try:
        candid = parse_candid_response(raw, label)
    except Stop as exc:
        run.write_journal("ambiguous-stop", method=method, reason=str(exc))
        return None, str(exc)
    return candid, None


def verify_bindings(args: argparse.Namespace, repo: Path) -> dict[str, str]:
    values = {
        "commit": args.approved_commit,
        "script": args.approved_script_sha256,
        "runbook": args.approved_runbook_sha256,
        "evidence": args.approved_evidence_sha256,
        "module": args.approved_module_sha256,
    }
    if not valid_hex_binding(values["commit"], 40):
        raise Stop("--approved-commit must be an exact 40-hex commit")
    for key in ("script", "runbook", "evidence", "module"):
        if not valid_hex_binding(values[key], 64):
            raise Stop(f"--approved-{key}-sha256 must be exact 64-hex")
    if values["module"] == PRE_DIGEST_MODULE_HASH:
        raise Stop("approved module is the pre-digest production module")
    head = subprocess.run(["/usr/bin/git", "rev-parse", "HEAD"], cwd=repo, capture_output=True, text=True, check=True).stdout.strip()
    tree_hash = subprocess.run(["/usr/bin/git", "rev-parse", "HEAD^{tree}"], cwd=repo, capture_output=True, text=True, check=True).stdout.strip()
    tree = subprocess.run(["/usr/bin/git", "status", "--porcelain=v1", "--untracked-files=all"], cwd=repo, capture_output=True, text=True, check=True).stdout
    if head != values["commit"] or tree:
        raise Stop("execute mode requires clean HEAD exactly equal to the approved commit")
    for key, path in (("script", SCRIPT), ("runbook", RUNBOOK), ("evidence", EVIDENCE)):
        current = sha256_bytes((repo / path).read_bytes())
        committed = subprocess.run(["/usr/bin/git", "show", f"{values['commit']}:{path}"], cwd=repo, capture_output=True, check=True).stdout
        if current != values[key] or sha256_bytes(committed) != values[key]:
            raise Stop(f"approved {key} hash binding failed")
    values["tree"] = tree_hash
    return values


def valid_hex_binding(value: str | None, length: int) -> bool:
    return re.fullmatch(rf"[0-9a-f]{{{length}}}", value or "") is not None


def ambiguity_disposition(method: str, readback_matches: bool) -> str:
    if method in {"set_last_observed_block", "set_chain_liquidation_config"}:
        return "LANDED_NO_RETRY" if readback_matches else "STOP_UNRESOLVED"
    if method == "reconcile_chain_supply":
        return "PERMANENT_STOP_UNRESOLVED"
    raise Stop("ambiguity disposition requested for non-allowlisted method")


def build_manifest(bindings: dict[str, str], tool_fingerprints: dict[str, Any], target: Target, head_evidence: dict[str, Any], hashes: dict[int, str], args_hex: dict[str, str], observations: list[dict[str, Any]]) -> dict[str, Any]:
    provider_config = [{"name": name, "url": url} for name, url in PROVIDERS]
    return {
        "schema": "rumi-conflux-disabled-recovery-v1",
        "authority": "procedure-bound delegated authority; not literal pre-approved per-run bytes",
        "execution_id": EXECUTION_ID,
        "created_at": utc_now(),
        "canister": CANISTER,
        "network": NETWORK,
        "identity": IDENTITY,
        "operator_principal": OPERATOR_PRINCIPAL,
        "operator_tools": tool_fingerprints,
        "module_hash": bindings["module"],
        "providers": provider_config,
        "provider_config_sha256": sha256_bytes(canonical_json(provider_config)),
        "rpc_endpoint_set_digest_v1": {
            "chain_id": CHAIN_ID,
            "endpoint_count": len(PROVIDERS),
            "effective_min_quorum_providers": RPC_EFFECTIVE_QUORUM,
            "digest_sha256": RPC_ENDPOINT_SET_DIGEST_V1,
            "readback_transport": "replicated-ingress-no-query-flag",
        },
        "bindings": bindings,
        "selection": {"H": target.h, "T": target.t, "F": target.f, "C": target.c, "P": target.p, "heads": head_evidence, "header_hashes": {str(k): v for k, v in hashes.items()}},
        "calls": [
            {"ordinal": idx + 1, "method": method, "arg_hex": args_hex[method], "arg_sha256": sha256_bytes(bytes.fromhex(args_hex[method]))}
            for idx, method in enumerate(UPDATE_ALLOWLIST)
        ],
        "raw_response_hashes": observations,
        "limits": {"max_updates": 3, "max_cursor_jump": MAX_CURSOR_JUMP, "max_head_skew": MAX_HEAD_SKEW, "max_head_age_secs": MAX_HEAD_AGE_SECS},
    }


def require_reconciliation(candid: str, target: Target) -> None:
    compact = " ".join(candid.split())
    match = re.fullmatch(r"\(\s*variant \{ Ok = record \{ ([^{}]*) \} \}\s*,?\s*\)", compact)
    if not match:
        raise Stop("reconcile_chain_supply did not return anchored Ok(record)")
    body = match.group(1)
    expected = {
        "chain_id": CHAIN_ID,
        "finalized_block": target.t,
        "onchain_total_supply_e8s": 0,
        "recorded_supply_e8s": 0,
        "in_flight_mint_e8s": 0,
        "reserve_backing_e8s": 0,
        "pending_chain_burn_e8s": 0,
        "reserve_usdc_native": 0,
    }
    expected_names = set(expected) | {"unbacked_excess", "gap_e8s"}
    stripped_body = body.strip()
    if not stripped_body.endswith(";"):
        raise Stop("reconcile_chain_supply returned a malformed record body")
    field_pieces = stripped_body[:-1].split(";")
    if len(field_pieces) != len(expected_names) or any(not piece.strip() for piece in field_pieces):
        raise Stop("reconcile_chain_supply returned a malformed field count")
    field_values: dict[str, str] = {}
    for piece in field_pieces:
        field_match = re.fullmatch(r"\s*([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(\S(?:.*\S)?)\s*", piece)
        if not field_match:
            raise Stop("reconcile_chain_supply returned a malformed field assignment")
        name, value = field_match.groups()
        if name in field_values:
            raise Stop("reconcile_chain_supply returned duplicate fields")
        field_values[name] = value
    if set(field_values) != expected_names:
        raise Stop("reconcile_chain_supply returned duplicate, missing, or unexpected fields")
    expected_types = {
        "chain_id": "nat32",
        "finalized_block": "nat64",
        "onchain_total_supply_e8s": "nat",
        "recorded_supply_e8s": "nat",
        "in_flight_mint_e8s": "nat",
        "reserve_backing_e8s": "nat",
        "pending_chain_burn_e8s": "nat",
        "reserve_usdc_native": "nat",
    }
    for name, value in expected.items():
        value_match = re.fullmatch(rf"([0-9_]+)\s*:\s*{expected_types[name]}", field_values[name])
        if not value_match or int(value_match.group(1).replace("_", "")) != value:
            raise Stop(f"reconciliation field {name} != {value}")
    if not re.fullmatch(r"false", field_values["unbacked_excess"]):
        raise Stop("reconciliation reported unbacked_excess")
    if not re.fullmatch(r"(?:\+)?0\s*:\s*int", field_values["gap_e8s"]):
        raise Stop("reconciliation gap is nonzero/malformed")


def self_test() -> None:
    def must_stop(callable_obj: Any, message: str) -> None:
        try:
            callable_obj()
        except Stop:
            return
        raise AssertionError(message)

    target = select_target([155_300_002, 155_300_000, 155_300_001])
    assert target == Target(155_300_000, 155_297_952, 155_298_352, 155_298_976, 155_299_376)
    test_payload = matrix_payload(target)
    test_chunks = (test_payload[0:4], test_payload[4:6], test_payload[6:9], test_payload[9:11])
    assert [len(chunk) for chunk in test_chunks] == [4, 2, 3, 2]
    assert max(map(len, test_chunks)) <= 5
    calculated_digest = endpoint_set_digest_v1(
        [url for _, url in PROVIDERS], CHAIN_ID, RPC_EFFECTIVE_QUORUM,
    )
    assert calculated_digest == RPC_ENDPOINT_SET_DIGEST_V1
    assert endpoint_set_digest_v1(
        [url for _, url in reversed(PROVIDERS)] + [PROVIDERS[0][1]],
        CHAIN_ID,
        RPC_EFFECTIVE_QUORUM,
    ) == RPC_ENDPOINT_SET_DIGEST_V1
    assert endpoint_set_digest_v1(
        [url for _, url in PROVIDERS], CHAIN_ID, 3,
    ) != RPC_ENDPOINT_SET_DIGEST_V1
    replicated_argv = replicated_read_argv("get_chain_rpc_endpoint_set_digest", "(1030 : nat32)")
    assert "--query" not in replicated_argv
    assert replicated_argv.count("get_chain_rpc_endpoint_set_digest") == 1
    assert replicated_argv[-1] == "--json"
    for method in (
        "get_last_observed_block",
        "get_chain_liquidation_config",
        "get_chain_public_launch_status",
    ):
        critical_argv = replicated_read_argv(method, "(1030 : nat32)")
        assert "--query" not in critical_argv
        assert critical_argv.count(method) == 1
    digest_candid = f'''(variant {{ Ok = record {{
      digest_sha256 = "{RPC_ENDPOINT_SET_DIGEST_V1}";
      effective_min_quorum_providers = 2 : nat32;
      endpoint_count = 3 : nat32;
      chain_id = 1_030 : nat32;
    }} }})'''
    assert parse_endpoint_digest_result(digest_candid, "fixture") == {
        "chain_id": CHAIN_ID,
        "endpoint_count": 3,
        "effective_min_quorum_providers": 2,
        "digest_sha256": RPC_ENDPOINT_SET_DIGEST_V1,
    }
    icp_1_3_singleton_tuple = "(\n  " + digest_candid[1:-1] + ",\n)"
    assert parse_endpoint_digest_result(icp_1_3_singleton_tuple, "icp 1.3 singleton fixture") == {
        "chain_id": CHAIN_ID,
        "endpoint_count": 3,
        "effective_min_quorum_providers": 2,
        "digest_sha256": RPC_ENDPOINT_SET_DIGEST_V1,
    }
    for malformed in (
        "(variant { Err = variant { Unauthorized } })",
        digest_candid.replace("c57a", "C57a"),
        digest_candid.replace("chain_id = 1_030 : nat32;", "chain_id = 1_030 : nat32; extra = 1 : nat32;"),
        digest_candid.replace("endpoint_count = 3", "endpoint_count = nope"),
        digest_candid[:-1] + ", 0 : nat32)",
        digest_candid.replace(
            "chain_id = 1_030 : nat32;",
            "endpoint_count = 3 : nat32;",
        ),
        digest_candid.replace(
            "chain_id = 1_030 : nat32;",
            'endpoint_url = "https://credential.example";',
        ),
    ):
        must_stop(
            lambda malformed=malformed: parse_endpoint_digest_result(malformed, "malformed digest fixture"),
            "malformed endpoint digest unexpectedly passed",
        )
    require_endpoint_digest_binding(parse_endpoint_digest_result(digest_candid, "fixture"), "fixture")
    for drift in (
        digest_candid.replace("chain_id = 1_030", "chain_id = 999"),
        digest_candid.replace("endpoint_count = 3", "endpoint_count = 2"),
        digest_candid.replace("effective_min_quorum_providers = 2", "effective_min_quorum_providers = 3"),
        digest_candid.replace(RPC_ENDPOINT_SET_DIGEST_V1, "f" * 64),
    ):
        must_stop(
            lambda drift=drift: require_endpoint_digest_binding(
                parse_endpoint_digest_result(drift, "drift fixture"), "drift fixture",
            ),
            "endpoint digest drift unexpectedly passed",
        )
    for bad in (
        [1, 2],
        [155_300_000, 155_300_129, 155_300_001],
        [OLD_CURSOR + 2048, OLD_CURSOR + 2048, OLD_CURSOR + 2048],
        [OLD_CURSOR + MAX_CURSOR_JUMP + TARGET_LAG + 1] * 3,
    ):
        try:
            select_target(list(bad))
        except Stop:
            pass
        else:
            raise AssertionError(f"bad selection unexpectedly passed: {bad}")
    assert strict_ok("(variant { Ok })")
    assert strict_ok("(\n  variant { Ok },\n)")
    assert strict_ok("(variant { Ok = null },)")
    assert not strict_ok("(variant { Err = \"bad\" })")
    assert not strict_ok("(record { Ok = true })")
    assert not strict_ok("(variant { Ok }, 0 : nat32)")
    try:
        parse_single_nat64("(malformed)", "fixture cursor")
    except Stop:
        pass
    else:
        raise AssertionError("malformed cursor unexpectedly passed")
    assert exact_liquidation_row("(null)") is False
    assert exact_liquidation_row(LIQUIDATION_CANDID)
    fixture_hashes = {block: "0x" + f"{idx:064x}" for idx, block in enumerate((target.h, target.t, target.f, target.c, target.p), start=1)}
    fixture_items: list[dict[str, Any]] = [{"jsonrpc": "2.0", "id": 1, "result": CHAIN_ID_HEX}]
    for idx, block in enumerate((target.h, target.t, target.f, target.c, target.p), start=2):
        fixture_items.append({"jsonrpc": "2.0", "id": idx, "result": {"number": hex(block), "hash": fixture_hashes[block]}})
    pair_word = "0x" + "0" * 24 + PAIR[2:]
    fixture_items.extend([
        {"jsonrpc": "2.0", "id": 7, "result": pair_word},
        {"jsonrpc": "2.0", "id": 8, "result": "0x" + "0" * 64},
        {"jsonrpc": "2.0", "id": 9, "result": pair_word},
        {"jsonrpc": "2.0", "id": 10, "result": "0x" + "0" * 63 + "1"},
        {"jsonrpc": "2.0", "id": 11, "result": "0x" + "0" * 63 + "1"},
    ])
    assert parse_matrix(fixture_items, target, fixture_hashes, "fixture")["hashes"] == fixture_hashes
    mismatched = json.loads(json.dumps(fixture_items))
    next(item for item in mismatched if item["id"] == 4)["result"]["hash"] = "0x" + "f" * 64
    try:
        parse_matrix(mismatched, target, fixture_hashes, "mismatch fixture")
    except Stop:
        pass
    else:
        raise AssertionError("mismatched provider header unexpectedly passed")
    mixed_conflict = json.loads(json.dumps(fixture_items))
    next(item for item in mixed_conflict if item["id"] == 4)["result"]["hash"] = "0x" + "e" * 64
    next(item for item in mixed_conflict if item["id"] == 8).pop("result")
    next(item for item in mixed_conflict if item["id"] == 8)["error"] = {"code": -32016, "message": "state is not ready"}
    try:
        classify_phase_provider(mixed_conflict, target, fixture_hashes, "mixed conflict fixture")
    except ProviderConflict:
        pass
    else:
        raise AssertionError("successful header conflict was masked by an unavailable state field")
    merely_unavailable = json.loads(json.dumps(fixture_items))
    next(item for item in merely_unavailable if item["id"] == 8).pop("result")
    next(item for item in merely_unavailable if item["id"] == 8)["error"] = {"code": -32016, "message": "state is not ready"}
    assert classify_phase_provider(merely_unavailable, target, fixture_hashes, "unavailable fixture") is None
    require_phase_floor(2, "fixture")
    try:
        require_phase_floor(1, "fixture")
    except Stop:
        pass
    else:
        raise AssertionError("one-of-three provider floor unexpectedly passed")
    try:
        expect_fields("record { chain_id = 999 : nat32; }", {"chain_id": r"1_030(?:\s*:\s*nat32)?"}, "wrong-live-state fixture")
    except Stop:
        pass
    else:
        raise AssertionError("wrong live state unexpectedly passed")
    good_projections = "record { collateral_config_matches_expected = true; debt_config_matches_expected = true; }"
    projection_patterns = {
        "collateral_config_matches_expected": r"true",
        "debt_config_matches_expected": r"true",
    }
    expect_fields(good_projections, projection_patterns, "config projections")
    for field in projection_patterns:
        must_stop(
            lambda field=field: expect_fields(
                good_projections.replace(f"{field} = true", f"{field} = false"),
                projection_patterns,
                f"false {field} fixture",
            ),
            f"false {field} unexpectedly passed",
        )
    try:
        parse_candid_response(b'{"response_candid": 7}', "fixture")
    except Stop:
        pass
    else:
        raise AssertionError("malformed icp JSON unexpectedly passed")
    assert phase_expected_state("phase1", target) == (OLD_CURSOR, False)
    assert phase_expected_state("phase2", target) == (target.t, False)
    assert phase_expected_state("phase3", target) == (target.t, True)
    assert phase_expected_state("post", target) == (target.t, True)
    must_stop(lambda: phase_expected_state("resume", target), "unknown phase unexpectedly passed")
    reconciliation_fixture = f'''(variant {{ Ok = record {{
      chain_id = 1_030 : nat32; finalized_block = {target.t} : nat64;
      onchain_total_supply_e8s = 0 : nat; recorded_supply_e8s = 0 : nat;
      in_flight_mint_e8s = 0 : nat; reserve_backing_e8s = 0 : nat;
      pending_chain_burn_e8s = 0 : nat; reserve_usdc_native = 0 : nat;
      unbacked_excess = false; gap_e8s = 0 : int;
    }} }})'''
    require_reconciliation(reconciliation_fixture, target)
    reconciliation_icp_1_3_singleton_tuple = "(\n  " + reconciliation_fixture[1:-1] + ",\n)"
    require_reconciliation(reconciliation_icp_1_3_singleton_tuple, target)
    must_stop(
        lambda: require_reconciliation(reconciliation_fixture.replace("gap_e8s = 0", "gap_e8s = 1"), target),
        "nonzero reconciliation gap unexpectedly passed",
    )
    must_stop(
        lambda: require_reconciliation(reconciliation_fixture[:-1] + ", 0 : nat32)", target),
        "second reconciliation tuple item unexpectedly passed",
    )
    must_stop(
        lambda: require_reconciliation(
            reconciliation_fixture[:-1] + ', variant { Ok = record { junk = 1 : nat; } })', target,
        ),
        "record-shaped second reconciliation tuple item unexpectedly passed",
    )
    must_stop(
        lambda: require_reconciliation(
            reconciliation_fixture.replace(
                "chain_id = 1_030 : nat32;",
                "chain_id = 1_030 : nat32; chain_id = 1_030 : nat32;",
            ),
            target,
        ),
        "duplicate reconciliation field unexpectedly passed",
    )
    must_stop(
        lambda: require_reconciliation(
            reconciliation_fixture.replace(
                "chain_id = 1_030 : nat32;",
                "chain_id = 1_030 : nat32; unexpected = 0 : nat;",
            ),
            target,
        ),
        "unexpected reconciliation field unexpectedly passed",
    )
    must_stop(
        lambda: require_reconciliation(
            reconciliation_fixture.replace(
                "chain_id = 1_030 : nat32;",
                "chain_id = 1_030 : nat32; 42 = 0 : nat;",
            ),
            target,
        ),
        "numeric reconciliation field label unexpectedly passed",
    )
    must_stop(
        lambda: require_reconciliation(
            reconciliation_fixture.replace(
                "chain_id = 1_030 : nat32;",
                "chain_id = 1_030 : nat32; junk;",
            ),
            target,
        ),
        "malformed reconciliation field piece unexpectedly passed",
    )
    for malformed_type in (
        reconciliation_fixture.replace("chain_id = 1_030 : nat32", "chain_id = 1_030 : nat64"),
        reconciliation_fixture.replace("chain_id = 1_030 : nat32", "chain_id = 1_030"),
        reconciliation_fixture.replace(
            f"finalized_block = {target.t} : nat64", f"finalized_block = {target.t} : nat32",
        ),
        reconciliation_fixture.replace(
            f"finalized_block = {target.t} : nat64", f"finalized_block = {target.t}",
        ),
        reconciliation_fixture.replace("onchain_total_supply_e8s = 0 : nat", "onchain_total_supply_e8s = 0 : nat64"),
        reconciliation_fixture.replace("onchain_total_supply_e8s = 0 : nat", "onchain_total_supply_e8s = 0"),
        reconciliation_fixture.replace("unbacked_excess = false", "unbacked_excess = false : bool"),
        reconciliation_fixture.replace("gap_e8s = 0 : int", "gap_e8s = 0 : nat"),
        reconciliation_fixture.replace("gap_e8s = 0 : int", "gap_e8s = 0"),
    ):
        must_stop(
            lambda malformed_type=malformed_type: require_reconciliation(malformed_type, target),
            "wrong or missing reconciliation field type unexpectedly passed",
        )
    must_stop(
        lambda: require_reconciliation(reconciliation_fixture + " trailing", target),
        "trailing reconciliation data unexpectedly passed",
    )
    # Model the authorization state machine: dry-run never dispatches; any
    # pre-existing journal blocks execute; phase-3 ambiguity never maps to safe
    # resend because reconciliation has no durable landed marker.
    def dispatch_allowed(execute: bool, journal_exists: bool, binding_ok: bool) -> bool:
        return execute and not journal_exists and binding_ok
    assert not dispatch_allowed(False, False, True)
    assert not dispatch_allowed(True, True, True)
    assert not dispatch_allowed(True, False, False)
    assert dispatch_allowed(True, False, True)
    assert valid_hex_binding("a" * 40, 40)
    assert not valid_hex_binding("A" * 40, 40)
    assert not valid_hex_binding("a" * 39, 40)
    assert ambiguity_disposition("set_last_observed_block", True) == "LANDED_NO_RETRY"
    assert ambiguity_disposition("set_chain_liquidation_config", False) == "STOP_UNRESOLVED"
    assert ambiguity_disposition("reconcile_chain_supply", True) == "PERMANENT_STOP_UNRESOLVED"
    assert UPDATE_ALLOWLIST == ("set_last_observed_block", "set_chain_liquidation_config", "reconcile_chain_supply")
    assert len(UPDATE_ALLOWLIST) == 3
    print("PASS self-test: endpoint digest/parser, selection arithmetic, partial-conflict hard stop, config/state phases, exact row/reconciliation, no-execute binding, literal 3-call allowlist, and ambiguity no-resend policy")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--execute", action="store_true", help="dispatch the exact three-call procedure; default is read-only preflight")
    parser.add_argument("--execution-id", default=None, help=f"execute mode requires the literal one-use id {EXECUTION_ID}")
    parser.add_argument("--approved-commit")
    parser.add_argument("--approved-script-sha256")
    parser.add_argument("--approved-runbook-sha256")
    parser.add_argument("--approved-evidence-sha256")
    parser.add_argument(
        "--approved-module-sha256",
        help="exact deployed digest-bearing backend module hash; required in dry-run and execute mode",
    )
    parser.add_argument("--self-test", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.self_test:
        self_test()
        return 0
    repo = Path(__file__).resolve().parents[1]
    execute = args.execute
    if not valid_hex_binding(args.approved_module_sha256, 64):
        raise Stop("--approved-module-sha256 must bind the exact deployed digest-bearing module")
    if args.approved_module_sha256 == PRE_DIGEST_MODULE_HASH:
        raise Stop("production still reports the pre-digest module; recovery is source-ready but not live-ready")
    if execute and args.execution_id != EXECUTION_ID:
        raise Stop(f"execute mode requires --execution-id {EXECUTION_ID}")
    bindings = verify_bindings(args, repo)
    if execute:
        state_root = Path.home() / ".local" / "state" / "rumi" / "conflux-disabled-recovery"
        state_root.mkdir(parents=True, exist_ok=True, mode=0o700)
        os.chmod(state_root, 0o700)
        run_dir = state_root / EXECUTION_ID
        try:
            run_dir.mkdir(mode=0o700)
        except FileExistsError as exc:
            raise Stop("execution id already has a journal; no resume or resend is permitted") from exc
    else:
        run_dir = Path(tempfile.mkdtemp(prefix="rumi-conflux-recovery-preflight-"))
        os.chmod(run_dir, 0o700)
    run = EvidenceRun(execute, run_dir)
    run.write_journal("preflight")
    run.log(f"mode={'EXECUTE' if execute else 'DRY-RUN'} execution_id={EXECUTION_ID} canister={CANISTER} network={NETWORK} identity={IDENTITY}")
    run.log("authorization=procedure-bound-delegated-authority not-literal-preapproved-call-bytes")
    tool_fingerprints = operator_fingerprints(run)
    run.log(f"operator_principal={tool_fingerprints['principal']} tool_fingerprints=PASS")

    selection_endpoint_binding = bind_live_endpoint_set(run, "selection")
    heads, head_evidence = select_heads(run)
    target = select_target(heads)
    hashes, _selection = selection_matrix(run, target)
    run.log(f"frozen H={target.h} T={target.t} F={target.f} C={target.c} P={target.p} all3_selection=PASS")
    archive_supply_zero(run, "selection-old-cursor-supply")

    args_hex = {
        "set_last_observed_block": encode_arg(run, "set_last_observed_block", f"(1030 : nat32, {target.t} : nat64)", repo),
        "set_chain_liquidation_config": encode_arg(run, "set_chain_liquidation_config", LIQUIDATION_CANDID, repo),
        "reconcile_chain_supply": encode_arg(run, "reconcile_chain_supply", "(1030 : nat32)", repo),
    }
    phase1_endpoint_binding = bind_live_endpoint_set(run, "phase1")
    phase1_matrix = phase_matrix(run, "phase1", target, hashes)
    starting_state = backend_gate(run, "phase1", target, bindings["module"])
    run.log("phase1 fixed-matrix/backend/zero/quiescence gates=PASS")
    manifest = build_manifest(bindings, tool_fingerprints, target, head_evidence, hashes, args_hex, run.observations)
    manifest["starting_state"] = starting_state
    manifest["phase1_matrix"] = phase1_matrix
    manifest["endpoint_binding_readbacks"] = {
        "selection": selection_endpoint_binding,
        "phase1": phase1_endpoint_binding,
    }
    manifest["post_state"] = "pending"
    manifest_path = run.run_dir / "sealed-manifest.json"
    atomic_private_json(manifest_path, manifest)
    manifest_sha = sha256_bytes(manifest_path.read_bytes())
    run.write_journal("sealed", manifest_sha256=manifest_sha, target=target.__dict__)
    run.log(f"manifest_sha256={manifest_sha} exact_arg_bytes_frozen=true")
    if not execute:
        run.write_journal("dry-run-complete", manifest_sha256=manifest_sha)
        run.log(f"DRY-RUN COMPLETE no updates dispatched transcript={run.transcript_path} raw_private={run.raw_dir}")
        return 0

    candid, ambiguous = update_once(run, "set_last_observed_block", args_hex["set_last_observed_block"])
    cursor = replicated_read(run, "phase1-cursor-readback", "get_last_observed_block", "(1030 : nat32)")
    landed = parse_single_nat64(cursor, "phase1 cursor readback") == target.t
    if not landed:
        run.write_journal("stopped", phase="phase1", reason="cursor readback mismatch")
        raise Stop("phase1 cursor readback mismatch; no retry/reversal authorized")
    if candid is not None and not strict_ok(candid):
        run.write_journal("stopped", phase="phase1", reason="explicit non-Ok result")
        raise Stop("phase1 returned explicit non-Ok despite landed readback; stop")
    run.write_journal(
        "phase1-complete", ambiguous_reconciled=ambiguous is not None, cursor=target.t,
        endpoint_set_digest=phase1_endpoint_binding["digest_sha256"],
    )

    phase2_endpoint_binding = bind_live_endpoint_set(run, "phase2")
    phase_matrix(run, "phase2", target, hashes)
    backend_gate(run, "phase2", target, bindings["module"])
    run.log("phase2 fixed-matrix/backend/zero/quiescence gates=PASS")
    candid, ambiguous = update_once(run, "set_chain_liquidation_config", args_hex["set_chain_liquidation_config"])
    row = replicated_read(run, "phase2-row-readback", "get_chain_liquidation_config", "(1030 : nat32)")
    launch = replicated_read(run, "phase2-launch-readback", "get_chain_public_launch_status", "(1030 : nat32)")
    landed = exact_liquidation_row(row) and re.search(rf'liquidation_config_digest\s*=\s*opt "{LIQ_DIGEST}"\s*;', " ".join(launch.split())) is not None
    if not landed:
        run.write_journal("stopped", phase="phase2", reason="row/digest readback mismatch")
        raise Stop("phase2 row/digest readback mismatch; no retry permitted")
    if candid is not None and not strict_ok(candid):
        run.write_journal("stopped", phase="phase2", reason="explicit non-Ok result")
        raise Stop("phase2 returned explicit non-Ok despite matching readback; stop")
    run.write_journal(
        "phase2-complete", ambiguous_reconciled=ambiguous is not None,
        liquidation_digest=LIQ_DIGEST, endpoint_set_digest=phase2_endpoint_binding["digest_sha256"],
    )

    phase3_endpoint_binding = bind_live_endpoint_set(run, "phase3")
    phase_matrix(run, "phase3", target, hashes)
    backend_gate(run, "phase3", target, bindings["module"])
    run.log("phase3 fixed-matrix/backend/zero/quiescence gates=PASS")
    candid, ambiguous = update_once(run, "reconcile_chain_supply", args_hex["reconcile_chain_supply"])
    if ambiguous is not None:
        backend_gate(run, "phase3", target, bindings["module"])
        run.write_journal("ambiguous-permanent-stop", phase="phase3", reason=ambiguous)
        raise Stop("phase3 outcome is ambiguous; zero-state readback recorded but landed status is unprovable, so this execution may never resume or resend")
    assert candid is not None
    require_reconciliation(candid, target)
    run.write_journal(
        "phase3-complete", finalized_block=target.t, onchain_supply=0,
        recorded_supply=0, in_flight=0, gap=0,
        endpoint_set_digest=phase3_endpoint_binding["digest_sha256"],
    )

    post_endpoint_binding = bind_live_endpoint_set(run, "post")
    post_matrix = phase_matrix(run, "post", target, hashes)
    post_state = backend_gate(run, "post", target, bindings["module"])
    final_evidence = {
        "schema": "rumi-conflux-disabled-recovery-final-v1",
        "execution_id": EXECUTION_ID,
        "sealed_manifest_sha256": manifest_sha,
        "completed_at": utc_now(),
        "post_state": post_state,
        "post_matrix": post_matrix,
        "post_endpoint_binding": post_endpoint_binding,
        "all_raw_response_hashes": run.observations,
    }
    final_path = run.run_dir / "final-evidence.json"
    atomic_private_json(final_path, final_evidence)
    final_sha = sha256_bytes(final_path.read_bytes())
    run.write_journal("complete", manifest_sha256=manifest_sha, final_evidence_sha256=final_sha, final_state="Disabled")
    run.log(f"final_evidence_sha256={final_sha} post_state=Disabled")
    run.log(f"PASS three updates complete; chain remains Disabled; transcript={run.transcript_path} raw_private={run.raw_dir}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Stop as exc:
        print(f"STOP: {exc}", file=sys.stderr)
        raise SystemExit(1)
