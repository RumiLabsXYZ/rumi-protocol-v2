#!/usr/bin/env python3
"""Fail-closed two-call continuation after the landed Conflux cursor reseed.

Dry-run is the default. Execute mode is a distinct one-use procedure and can
dispatch only the exact liquidation configuration followed by supply
reconciliation. It never resends or reverses the already-landed cursor.
"""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any


# Importing the reviewed base must not create an untracked __pycache__ before
# the clean-tree approval binding runs.
sys.dont_write_bytecode = True

REPO = Path(__file__).resolve().parents[1]
BASE_SCRIPT = "scripts/conflux-disabled-recovery.py"
SCRIPT = "scripts/conflux-disabled-recovery-continuation.py"
RUNBOOK = "docs/plans/2026-06-18-conflux-gated-mainnet-launch-runbook.md"
EVIDENCE = "docs/plans/2026-08-27-conflux-disabled-continuation-evidence.md"


def load_base() -> Any:
    path = REPO / BASE_SCRIPT
    spec = importlib.util.spec_from_file_location("rumi_conflux_recovery_base", path)
    if spec is None or spec.loader is None:
        raise RuntimeError("cannot load reviewed recovery base")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


base = load_base()

EXECUTION_ID = "conflux-disabled-recovery-continuation-v1"
UPDATE_ALLOWLIST = ("set_chain_liquidation_config", "reconcile_chain_supply")
EXPECTED_MODULE_SHA256 = "835bc12041222bc6acd4ffa65746a27b9d16cdec63a0e2bd28d9536c28752c54"
EXPECTED_HOST_SHA256 = "353a2c68134d012cfc89fc69187cb8f121493b08837c815f2e884be75ec432da"
IOREG_SHA256 = "8e0b0dad2abf584925b3f8ddab09353d5db0d4d0f25c5b1b085cdb0b2307e27b"
TARGET = base.Target(h=155_343_278, t=155_341_230, f=155_341_630, c=155_342_254, p=155_342_654)
FROZEN_HEADER_HASHES = {
    155_343_278: "0xf70ceb338aff58c7d0ca4cffd01a399e2c88d8b6b8344d7ff37e96f69a6fdce1",
    155_341_230: "0xd0f809730379ae86c87cfbd2ca773eaa421cc72c409c409d03ab0e83ef73ff6b",
    155_341_630: "0xd8c6ac0faf8d1f6c419ea6a4c81af80348aa482fb5bcd722981c80ea5e56bc56",
    155_342_254: "0xb78c717c7474b0c7b94df278629d6152247c9da8af5954c19b96c78f679cda6a",
    155_342_654: "0x05d7f168c7c496f59298e74ee3d89e4396053f59246706589746d6e7308d0168",
}
PRIOR_EXECUTION_ID = "conflux-disabled-recovery-v1"
PRIOR_COMMIT = "102c99a508fc79163919d343762179c6efef97a4"
PRIOR_MANIFEST_SHA256 = "92b72fd1e9fa62bdc7507522c49e53ad37db5dc3554919f17b569af212c9a955"
PRIOR_TRANSCRIPT_SHA256 = "209d0666b802a06f6f01e4f0d2e5acb029c38970b1ea9434e14ddabf407950bf"
PRIOR_JOURNAL_SHA256 = "7a80b877fe4185b7c4df76522b66e8929106f326f1ec2b16d0c1718d279c53f8"
PRIOR_UPDATE_SHA256 = "67c0d269d550ae7d18dd7003f8b2aa577fa925e5f0680ad8fc82fefc4c170810"
PRIOR_STALE_READBACK_SHA256 = "411df0b66cee858a7f23dc8d207dd76f078ada4fb3cd69383bf4f7f5f1df324d"
PRIOR_CURSOR_ARG_SHA256 = "df40d664035eddbd642a324fa7eaa4c22b97ef4875f395cbe68658521a62f915"
LIQUIDATION_ARG_SHA256 = "a4ccce984ffdaa69f09fe31cf8d7b9ed5c28848ad6b0a979fa2d2ac1eb543060"
RECONCILE_ARG_SHA256 = "ee95dff5c236fb605552cd49583910dc34e9f1260b9e8d7b7829b638d9bec5e8"
PRIOR_RELATIVE_FILES = {
    "journal": ("journal.json", PRIOR_JOURNAL_SHA256),
    "manifest": ("sealed-manifest.json", PRIOR_MANIFEST_SHA256),
    "transcript": ("sanitized-transcript.log", PRIOR_TRANSCRIPT_SHA256),
    "update": ("raw/051-update-1-set_last_observed_block.stdout", PRIOR_UPDATE_SHA256),
    "stale_readback": ("raw/052-phase1-cursor-readback.stdout", PRIOR_STALE_READBACK_SHA256),
}


def prior_run_dir() -> Path:
    return Path.home() / ".local" / "state" / "rumi" / "conflux-disabled-recovery" / PRIOR_EXECUTION_ID


def continuation_state_root() -> Path:
    return Path.home() / ".local" / "state" / "rumi" / "conflux-disabled-recovery"


def exact_hash(path: Path, expected: str, label: str) -> None:
    if not path.is_file() or base.sha256_bytes(path.read_bytes()) != expected:
        raise base.Stop(f"{label} hash binding failed")


def current_host_fingerprint() -> str:
    ioreg = Path("/usr/sbin/ioreg")
    exact_hash(ioreg, IOREG_SHA256, "ioreg tool")
    result = subprocess.run(
        [str(ioreg), "-rd1", "-c", "IOPlatformExpertDevice"],
        capture_output=True,
        check=False,
    )
    if result.returncode != 0:
        raise base.Stop("host hardware binding probe failed")
    match = re.search(rb'"IOPlatformUUID"\s*=\s*"([0-9A-Fa-f-]{36})"', result.stdout)
    if not match:
        raise base.Stop("host hardware binding is malformed")
    return hashlib.sha256(b"rumi.conflux-recovery-host.v1\0" + match.group(1).lower()).hexdigest()


def validate_live_heads(heads: list[int]) -> None:
    if len(heads) != 3 or any(not isinstance(value, int) or isinstance(value, bool) for value in heads):
        raise base.Stop("continuation requires exactly three integer provider heads")
    if max(heads) - min(heads) > base.MAX_HEAD_SKEW:
        raise base.Stop("continuation provider head skew exceeds 128 blocks")
    if min(heads) < TARGET.p:
        raise base.Stop("current providers have not reached the pinned proof block")


def validate_prior_structures(journal: dict[str, Any], manifest: dict[str, Any], update_candid: str) -> None:
    current = journal.get("current")
    if not isinstance(current, dict) or current != {
        "phase": "phase1",
        "reason": "cursor readback mismatch",
        "state": "stopped",
        "update_count": 1,
        "updated_at": "2026-08-27T18:52:15+00:00",
    }:
        raise base.Stop("prior journal is not the exact terminal phase1 state")
    history = journal.get("history")
    if not isinstance(history, list):
        raise base.Stop("prior journal history is malformed")
    dispatches = [entry for entry in history if isinstance(entry, dict) and entry.get("state") == "dispatching"]
    if len(dispatches) != 1 or dispatches[0].get("method") != "set_last_observed_block":
        raise base.Stop("prior journal does not contain exactly one cursor dispatch")
    if dispatches[0].get("arg_sha256") != PRIOR_CURSOR_ARG_SHA256 or dispatches[0].get("update_count") != 1:
        raise base.Stop("prior cursor dispatch binding drifted")
    if manifest.get("schema") != "rumi-conflux-disabled-recovery-v1":
        raise base.Stop("prior manifest schema drifted")
    if manifest.get("execution_id") != PRIOR_EXECUTION_ID or manifest.get("post_state") != "pending":
        raise base.Stop("prior manifest execution state drifted")
    bindings = manifest.get("bindings")
    if not isinstance(bindings, dict) or bindings.get("commit") != PRIOR_COMMIT:
        raise base.Stop("prior manifest commit binding drifted")
    selection = manifest.get("selection")
    if not isinstance(selection, dict):
        raise base.Stop("prior manifest selection is malformed")
    for field, value in (("H", TARGET.h), ("T", TARGET.t), ("F", TARGET.f), ("C", TARGET.c), ("P", TARGET.p)):
        if selection.get(field) != value:
            raise base.Stop(f"prior manifest {field} drifted")
    hashes = selection.get("header_hashes")
    expected_hashes = {str(number): block_hash for number, block_hash in FROZEN_HEADER_HASHES.items()}
    if hashes != expected_hashes:
        raise base.Stop("prior manifest header hashes drifted")
    calls = manifest.get("calls")
    expected_calls = [
        (1, "set_last_observed_block", PRIOR_CURSOR_ARG_SHA256),
        (2, "set_chain_liquidation_config", LIQUIDATION_ARG_SHA256),
        (3, "reconcile_chain_supply", RECONCILE_ARG_SHA256),
    ]
    if not isinstance(calls, list) or len(calls) != 3 or not all(isinstance(call, dict) for call in calls) or [
        (call.get("ordinal"), call.get("method"), call.get("arg_sha256"))
        for call in calls
    ] != expected_calls:
        raise base.Stop("prior manifest call sequence drifted")
    if not base.strict_ok(update_candid):
        raise base.Stop("prior cursor update is not an explicit Ok")


def verify_prior_evidence(run_root: Path | None = None) -> dict[str, Any]:
    root = run_root or prior_run_dir()
    for label, (relative, expected_hash) in PRIOR_RELATIVE_FILES.items():
        exact_hash(root / relative, expected_hash, f"prior {label}")
    update_files = sorted((root / "raw").glob("*-update-*.stdout"))
    if [path.name for path in update_files] != ["051-update-1-set_last_observed_block.stdout"]:
        raise base.Stop("prior evidence does not contain exactly one update response")
    journal = json.loads((root / "journal.json").read_text(encoding="utf-8"))
    manifest = json.loads((root / "sealed-manifest.json").read_text(encoding="utf-8"))
    update_obj = json.loads((root / "raw/051-update-1-set_last_observed_block.stdout").read_text(encoding="utf-8"))
    update_candid = update_obj.get("response_candid") if isinstance(update_obj, dict) else None
    if not isinstance(update_candid, str):
        raise base.Stop("prior cursor update response is malformed")
    validate_prior_structures(journal, manifest, update_candid)
    return {
        "execution_id": PRIOR_EXECUTION_ID,
        "journal_sha256": PRIOR_JOURNAL_SHA256,
        "manifest_sha256": PRIOR_MANIFEST_SHA256,
        "transcript_sha256": PRIOR_TRANSCRIPT_SHA256,
        "update_response_sha256": PRIOR_UPDATE_SHA256,
        "stale_readback_sha256": PRIOR_STALE_READBACK_SHA256,
        "landed_cursor": TARGET.t,
    }


def verify_bindings(args: argparse.Namespace) -> dict[str, str]:
    values = {
        "commit": args.approved_commit,
        "script": args.approved_script_sha256,
        "base_script": args.approved_base_script_sha256,
        "runbook": args.approved_runbook_sha256,
        "evidence": args.approved_evidence_sha256,
        "module": args.approved_module_sha256,
        "host": args.approved_host_sha256,
    }
    if not base.valid_hex_binding(values["commit"], 40):
        raise base.Stop("--approved-commit must be exact 40-hex")
    for name in ("script", "base_script", "runbook", "evidence", "module", "host"):
        if not base.valid_hex_binding(values[name], 64):
            raise base.Stop(f"--approved-{name.replace('_', '-')}-sha256 must be exact 64-hex")
    if values["module"] != EXPECTED_MODULE_SHA256:
        raise base.Stop("approved module is not the exact reviewed live module")
    if values["host"] != EXPECTED_HOST_SHA256 or current_host_fingerprint() != values["host"]:
        raise base.Stop("approved hardware-host binding failed")
    head = subprocess.run(["/usr/bin/git", "rev-parse", "HEAD"], cwd=REPO, capture_output=True, text=True, check=True).stdout.strip()
    tree_hash = subprocess.run(["/usr/bin/git", "rev-parse", "HEAD^{tree}"], cwd=REPO, capture_output=True, text=True, check=True).stdout.strip()
    tree = subprocess.run(
        ["/usr/bin/git", "status", "--porcelain=v1", "--untracked-files=all"],
        cwd=REPO, capture_output=True, text=True, check=True,
    ).stdout
    if head != values["commit"] or tree:
        raise base.Stop("continuation requires clean HEAD exactly equal to approved commit")
    paths = {
        "script": SCRIPT,
        "base_script": BASE_SCRIPT,
        "runbook": RUNBOOK,
        "evidence": EVIDENCE,
    }
    for name, relative in paths.items():
        current = (REPO / relative).read_bytes()
        committed = subprocess.run(
            ["/usr/bin/git", "show", f"{values['commit']}:{relative}"],
            cwd=REPO, capture_output=True, check=True,
        ).stdout
        if base.sha256_bytes(current) != values[name] or base.sha256_bytes(committed) != values[name]:
            raise base.Stop(f"approved {name} hash binding failed")
    values["tree"] = tree_hash
    return values


def replicated_phase_state(run: Any, phase: str, expect_row: bool) -> dict[str, Any]:
    cursor = base.replicated_read(run, f"{phase}-replicated-cursor", "get_last_observed_block", "(1030 : nat32)")
    if base.parse_single_nat64(cursor, f"{phase} replicated cursor") != TARGET.t:
        raise base.Stop(f"{phase} replicated cursor is not the landed target")
    row = base.replicated_read(run, f"{phase}-replicated-row", "get_chain_liquidation_config", "(1030 : nat32)")
    if base.exact_liquidation_row(row) != expect_row:
        raise base.Stop(f"{phase} replicated liquidation row phase mismatch")
    launch = base.replicated_read(run, f"{phase}-replicated-launch", "get_chain_public_launch_status", "(1030 : nat32)")
    compact = " ".join(launch.split())
    if not re.search(r"\bstatus\s*=\s*opt variant \{ Disabled \}\s*;", compact):
        raise base.Stop(f"{phase} chain is not Disabled")
    if base.named_bool(launch, "liquidation_configured") != expect_row:
        raise base.Stop(f"{phase} liquidation configured projection mismatch")
    if expect_row:
        if not base.named_bool(launch, "liquidation_enabled"):
            raise base.Stop(f"{phase} liquidation row is not enabled")
        if not re.search(rf'liquidation_config_digest\s*=\s*opt "{base.LIQ_DIGEST}"\s*;', compact):
            raise base.Stop(f"{phase} liquidation digest mismatch")
    elif not re.search(r"liquidation_config_digest\s*=\s*null\s*;", compact):
        raise base.Stop(f"{phase} unexpected liquidation digest")
    supply = base.replicated_read(run, f"{phase}-replicated-supply", "get_supply_audit", "()")
    if base.named_nat(supply, "total_e8s") != 0 or base.named_nat(supply, "supply_e8s") != 0:
        raise base.Stop(f"{phase} replicated supply is nonzero")
    return {"cursor": TARGET.t, "liquidation_row_present": expect_row, "supply_e8s": 0, "status": "Disabled"}


def encode_args(run: Any) -> dict[str, str]:
    values = {
        "set_chain_liquidation_config": base.encode_arg(run, "set_chain_liquidation_config", base.LIQUIDATION_CANDID, REPO),
        "reconcile_chain_supply": base.encode_arg(run, "reconcile_chain_supply", "(1030 : nat32)", REPO),
    }
    expected = {
        "set_chain_liquidation_config": LIQUIDATION_ARG_SHA256,
        "reconcile_chain_supply": RECONCILE_ARG_SHA256,
    }
    for method, encoded in values.items():
        if base.sha256_bytes(bytes.fromhex(encoded)) != expected[method]:
            raise base.Stop(f"{method} encoded argument drifted")
    return values


def dispatch_once(run: Any, method: str, arg_hex: str) -> str:
    if getattr(run, "continuation_terminal", False):
        raise base.Stop("continuation is terminal; no further dispatch is permitted")
    if run.update_count >= len(UPDATE_ALLOWLIST) or UPDATE_ALLOWLIST[run.update_count] != method:
        raise base.Stop("attempted method outside literal ordered two-call allowlist")
    run.update_count += 1
    run.write_journal("dispatching", method=method, arg_sha256=base.sha256_bytes(bytes.fromhex(arg_hex)))
    label = f"update-{run.update_count}-{method}"
    try:
        raw = run.command(
            label,
            [
                "/usr/local/bin/icp", "canister", "call", base.CANISTER, method, arg_hex,
                "--args-format", "hex", "-n", base.NETWORK, "--identity", base.IDENTITY, "--json",
            ],
            timeout=240,
        )
        candid = base.parse_candid_response(raw, label)
    except base.Stop as exc:
        run.continuation_terminal = True
        run.write_journal("ambiguous-permanent-stop", method=method, reason=str(exc))
        raise base.Stop(f"{method} ambiguous; continuation permanently stopped") from exc
    return candid


def stop_run(run: Any, phase: str, reason: str) -> None:
    run.continuation_terminal = True
    run.write_journal("stopped", phase=phase, reason=reason)
    raise base.Stop(reason)


def pre_phase(run: Any, phase: str, expect_row: bool, bindings: dict[str, str]) -> dict[str, Any]:
    prior = verify_prior_evidence()
    endpoint = base.bind_live_endpoint_set(run, phase)
    matrix = base.phase_matrix(run, phase, TARGET, FROZEN_HEADER_HASHES)
    backend = base.backend_gate(run, phase, TARGET, bindings["module"])
    replicated = replicated_phase_state(run, phase, expect_row)
    run.log(f"{phase} fixed-matrix/backend/replicated/zero/quiescence gates=PASS")
    return {"prior": prior, "endpoint": endpoint, "matrix": matrix, "backend": backend, "replicated": replicated}


def build_manifest(
    bindings: dict[str, str], tools: dict[str, Any], heads: dict[str, Any], selection: dict[str, Any],
    args_hex: dict[str, str], phase2: dict[str, Any], prior: dict[str, Any], observations: list[dict[str, Any]],
) -> dict[str, Any]:
    return {
        "schema": "rumi-conflux-disabled-recovery-continuation-v1",
        "authority": "distinct procedure-bound authority for remaining two calls; not cursor resend or literal dry-run bytes",
        "execution_id": EXECUTION_ID,
        "created_at": base.utc_now(),
        "canister": base.CANISTER,
        "network": base.NETWORK,
        "identity": base.IDENTITY,
        "module_hash": bindings["module"],
        "bindings": bindings,
        "operator_tools": tools,
        "prior_landed_phase1": prior,
        "frozen_target": {"H": TARGET.h, "T": TARGET.t, "F": TARGET.f, "C": TARGET.c, "P": TARGET.p},
        "frozen_header_hashes": {str(number): value for number, value in FROZEN_HEADER_HASHES.items()},
        "current_heads": heads,
        "selection_matrix": selection,
        "phase2_preconditions": phase2,
        "calls": [
            {
                "ordinal": index + 1,
                "method": method,
                "arg_hex": args_hex[method],
                "arg_sha256": base.sha256_bytes(bytes.fromhex(args_hex[method])),
            }
            for index, method in enumerate(UPDATE_ALLOWLIST)
        ],
        "raw_response_hashes": observations,
        "limits": {"max_updates": 2, "max_head_age_secs": base.MAX_HEAD_AGE_SECS, "max_head_skew": base.MAX_HEAD_SKEW},
        "post_state": "pending",
    }


def claim_execution_dir(root: Path) -> Path:
    root.mkdir(parents=True, exist_ok=True, mode=0o700)
    os.chmod(root, 0o700)
    run_dir = root / EXECUTION_ID
    try:
        run_dir.mkdir(mode=0o700)
    except FileExistsError as exc:
        raise base.Stop("continuation execution id already has a journal; no resume or resend is permitted") from exc
    return run_dir


def self_test() -> None:
    assert UPDATE_ALLOWLIST == ("set_chain_liquidation_config", "reconcile_chain_supply")
    assert "set_last_observed_block" not in UPDATE_ALLOWLIST
    assert EXPECTED_MODULE_SHA256 == "835bc12041222bc6acd4ffa65746a27b9d16cdec63a0e2bd28d9536c28752c54"
    assert current_host_fingerprint() == EXPECTED_HOST_SHA256
    assert (TARGET.f, TARGET.c, TARGET.p) == (TARGET.t + 400, TARGET.t + 1024, TARGET.t + 1424)
    assert set(FROZEN_HEADER_HASHES) == {TARGET.h, TARGET.t, TARGET.f, TARGET.c, TARGET.p}
    for method in (
        "get_last_observed_block", "get_chain_liquidation_config", "get_chain_public_launch_status", "get_supply_audit",
    ):
        argv = base.replicated_read_argv(method, "(1030 : nat32)" if method != "get_supply_audit" else "()")
        assert "--query" not in argv
        assert argv.count(method) == 1
    stale_ordinary = "(154_966_240 : nat64)"
    authoritative_replicated = "(155_341_230 : nat64)"
    assert base.parse_single_nat64(stale_ordinary, "stale ordinary fixture") != TARGET.t
    assert base.parse_single_nat64(authoritative_replicated, "replicated fixture") == TARGET.t
    validate_live_heads([TARGET.p, TARGET.p + 64, TARGET.p + 128])
    try:
        validate_live_heads([TARGET.p, TARGET.p + 64, TARGET.p + 129])
    except base.Stop:
        pass
    else:
        raise AssertionError("129-block live head skew unexpectedly passed")
    journal = {
        "current": {
            "phase": "phase1", "reason": "cursor readback mismatch", "state": "stopped",
            "update_count": 1, "updated_at": "2026-08-27T18:52:15+00:00",
        },
        "history": [
            {"state": "dispatching", "method": "set_last_observed_block", "arg_sha256": PRIOR_CURSOR_ARG_SHA256, "update_count": 1},
        ],
    }
    manifest = {
        "schema": "rumi-conflux-disabled-recovery-v1",
        "execution_id": PRIOR_EXECUTION_ID,
        "post_state": "pending",
        "bindings": {"commit": PRIOR_COMMIT},
        "selection": {
            "H": TARGET.h, "T": TARGET.t, "F": TARGET.f, "C": TARGET.c, "P": TARGET.p,
            "header_hashes": {str(number): value for number, value in FROZEN_HEADER_HASHES.items()},
        },
        "calls": [
            {"ordinal": 1, "method": "set_last_observed_block", "arg_sha256": PRIOR_CURSOR_ARG_SHA256},
            {"ordinal": 2, "method": "set_chain_liquidation_config", "arg_sha256": LIQUIDATION_ARG_SHA256},
            {"ordinal": 3, "method": "reconcile_chain_supply", "arg_sha256": RECONCILE_ARG_SHA256},
        ],
    }
    validate_prior_structures(journal, manifest, "(variant { Ok })")
    for mutation in (
        lambda value: value["current"].update(update_count=2),
        lambda value: value["history"].append({"state": "dispatching", "method": "set_chain_liquidation_config"}),
    ):
        altered = json.loads(json.dumps(journal))
        mutation(altered)
        try:
            validate_prior_structures(altered, manifest, "(variant { Ok })")
        except base.Stop:
            pass
        else:
            raise AssertionError("altered prior journal unexpectedly passed")
    try:
        validate_prior_structures(journal, manifest, '(variant { Err = "bad" })')
    except base.Stop:
        pass
    else:
        raise AssertionError("non-Ok prior update unexpectedly passed")
    with tempfile.TemporaryDirectory(prefix="rumi-continuation-selftest-") as temporary:
        root = Path(temporary)
        claimed = claim_execution_dir(root)
        assert claimed.name == EXECUTION_ID
        try:
            claim_execution_dir(root)
        except base.Stop:
            pass
        else:
            raise AssertionError("second execution-directory claim unexpectedly passed")
    class AmbiguousRun:
        update_count = 0
        continuation_terminal = False

        def __init__(self) -> None:
            self.states: list[str] = []

        def write_journal(self, state: str, **_extra: Any) -> None:
            self.states.append(state)

        def command(self, *_args: Any, **_kwargs: Any) -> bytes:
            raise base.Stop("fixture transport ambiguity")

    ambiguous_run = AmbiguousRun()
    try:
        dispatch_once(ambiguous_run, "set_chain_liquidation_config", "00")
    except base.Stop:
        pass
    else:
        raise AssertionError("ambiguous update unexpectedly returned")
    assert ambiguous_run.update_count == 1
    assert ambiguous_run.continuation_terminal
    assert ambiguous_run.states == ["dispatching", "ambiguous-permanent-stop"]
    try:
        dispatch_once(ambiguous_run, "reconcile_chain_supply", "00")
    except base.Stop:
        pass
    else:
        raise AssertionError("terminal continuation unexpectedly dispatched again")
    assert len(UPDATE_ALLOWLIST) == 2
    print("PASS self-test: prior phase1 binding, pinned target, replicated critical readbacks, literal two-call order, ambiguity stop, and one-use journal")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--execute", action="store_true")
    parser.add_argument("--execution-id")
    parser.add_argument("--approved-commit")
    parser.add_argument("--approved-script-sha256")
    parser.add_argument("--approved-base-script-sha256")
    parser.add_argument("--approved-runbook-sha256")
    parser.add_argument("--approved-evidence-sha256")
    parser.add_argument("--approved-module-sha256")
    parser.add_argument("--approved-host-sha256")
    parser.add_argument("--self-test", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.self_test:
        self_test()
        return 0
    execute = args.execute
    if execute and args.execution_id != EXECUTION_ID:
        raise base.Stop(f"execute mode requires --execution-id {EXECUTION_ID}")
    bindings = verify_bindings(args)
    base.EXECUTION_ID = EXECUTION_ID
    if execute:
        run_dir = claim_execution_dir(continuation_state_root())
    else:
        run_dir = Path(tempfile.mkdtemp(prefix="rumi-conflux-continuation-preflight-"))
        os.chmod(run_dir, 0o700)
    run = base.EvidenceRun(execute, run_dir)
    run.write_journal("preflight")
    run.log(f"mode={'EXECUTE' if execute else 'DRY-RUN'} execution_id={EXECUTION_ID} canister={base.CANISTER} network={base.NETWORK} identity={base.IDENTITY}")
    run.log("authorization=distinct-two-call-continuation cursor-resend-forbidden")
    prior = verify_prior_evidence()
    tools = base.operator_fingerprints(run)
    run.log(f"prior_phase1_binding=PASS landed_cursor={TARGET.t} operator_principal={tools['principal']} tool_fingerprints=PASS")
    selection_endpoint = base.bind_live_endpoint_set(run, "selection")
    heads, head_evidence = base.select_heads(run)
    validate_live_heads(heads)
    hashes, selection_matrix = base.selection_matrix(run, TARGET)
    if hashes != FROZEN_HEADER_HASHES:
        raise base.Stop("selection header hashes drifted from prior phase1 manifest")
    run.log(f"pinned H={TARGET.h} T={TARGET.t} F={TARGET.f} C={TARGET.c} P={TARGET.p} all3_selection=PASS")
    args_hex = encode_args(run)
    phase2 = pre_phase(run, "phase2", False, bindings)
    manifest = build_manifest(bindings, tools, head_evidence, selection_matrix, args_hex, phase2, prior, run.observations)
    manifest["selection_endpoint_binding"] = selection_endpoint
    manifest_path = run.run_dir / "sealed-manifest.json"
    base.atomic_private_json(manifest_path, manifest)
    manifest_sha = base.sha256_bytes(manifest_path.read_bytes())
    run.write_journal("sealed", manifest_sha256=manifest_sha, target={"h": TARGET.h, "t": TARGET.t, "f": TARGET.f, "c": TARGET.c, "p": TARGET.p})
    run.log(f"manifest_sha256={manifest_sha} exact_remaining_arg_bytes_frozen=true")
    if not execute:
        run.write_journal("dry-run-complete", manifest_sha256=manifest_sha)
        run.log(f"DRY-RUN COMPLETE no updates dispatched transcript={run.transcript_path} raw_private={run.raw_dir}")
        return 0

    verify_prior_evidence()
    candid = dispatch_once(run, "set_chain_liquidation_config", args_hex["set_chain_liquidation_config"])
    if not base.strict_ok(candid):
        stop_run(run, "phase2", "phase2 returned explicit non-Ok; continuation stopped")
    try:
        replicated_phase_state(run, "phase2-post", True)
    except base.Stop as exc:
        stop_run(run, "phase2", f"phase2 replicated readback failed: {exc}")
    run.write_journal("phase2-complete", liquidation_digest=base.LIQ_DIGEST)

    try:
        phase3 = pre_phase(run, "phase3", True, bindings)
    except base.Stop as exc:
        stop_run(run, "phase3", f"phase3 precondition failed: {exc}")
    verify_prior_evidence()
    candid = dispatch_once(run, "reconcile_chain_supply", args_hex["reconcile_chain_supply"])
    try:
        base.require_reconciliation(candid, TARGET)
    except base.Stop as exc:
        stop_run(run, "phase3", f"reconciliation response failed: {exc}")
    try:
        post_replicated = replicated_phase_state(run, "post", True)
        post_backend = base.backend_gate(run, "post", TARGET, bindings["module"])
    except base.Stop as exc:
        stop_run(run, "post", f"postcondition failed: {exc}")
    final = {
        "schema": "rumi-conflux-disabled-recovery-continuation-final-v1",
        "execution_id": EXECUTION_ID,
        "sealed_manifest_sha256": manifest_sha,
        "completed_at": base.utc_now(),
        "phase3_preconditions": phase3,
        "post_replicated": post_replicated,
        "post_backend": post_backend,
        "all_raw_response_hashes": run.observations,
    }
    final_path = run.run_dir / "final-evidence.json"
    base.atomic_private_json(final_path, final)
    final_sha = base.sha256_bytes(final_path.read_bytes())
    run.write_journal("complete", manifest_sha256=manifest_sha, final_evidence_sha256=final_sha, final_state="Disabled")
    run.log(f"PASS two updates complete; chain remains Disabled; final_evidence_sha256={final_sha}")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except base.Stop as exc:
        print(f"STOP: {exc}", file=sys.stderr)
        raise SystemExit(1)
