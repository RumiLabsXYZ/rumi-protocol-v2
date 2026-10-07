#!/usr/bin/env python3
"""Build and compare the reviewed backend Wasm, optionally against live state.

The checked-in provenance manifest is deliberately source-specific. Create it
from a clean, reviewed source commit with --record-manifest, review and commit
the resulting file, then use --phase preinstall or --phase postinstall. The
ICP module hash is SHA-256 of the installed (decompressed) Wasm module.
"""

from __future__ import annotations

import argparse
import gzip
import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
MANIFEST = Path("deploy/provenance/rumi_protocol_backend.json")
ARTIFACT = Path(".icp/cache/artifacts/rumi_protocol_backend")
CANISTER = "rumi_protocol_backend"
ENVIRONMENT = "mainnet-live"
BUILD_COMMAND = ["icp", "build", "--environment", ENVIRONMENT, CANISTER]
MANIFEST_KIND = "rumi-backend-build-provenance-v1"


class VerificationError(Exception):
    pass


def run(args: list[str], *, cwd: Path = ROOT, capture: bool = True) -> str:
    try:
        result = subprocess.run(
            args,
            cwd=cwd,
            check=True,
            text=True,
            stdout=subprocess.PIPE if capture else None,
            stderr=subprocess.PIPE if capture else None,
        )
    except (OSError, subprocess.CalledProcessError) as exc:
        detail = getattr(exc, "stderr", None)
        raise VerificationError(
            f"command failed: {' '.join(args)}" + (f"\n{detail.strip()}" if detail else "")
        ) from exc
    return result.stdout.strip() if capture else ""


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def toolchain() -> dict[str, str]:
    return {
        "icp": run(["icp", "--version"]),
        "rustc": run(["rustc", "--version"]),
        "cargo": run(["cargo", "--version"]),
        "ic_wasm": run(["ic-wasm", "--version"]),
    }


def git(*args: str) -> str:
    return run(["git", *args])


def status_paths() -> list[str]:
    raw = run(["git", "status", "--porcelain=v1", "--untracked-files=all"])
    paths = []
    for line in raw.splitlines():
        if len(line) >= 4:
            paths.append(line[3:])
    return paths


def assert_clean_except_manifest(manifest_rel: str, *, allow_manifest: bool) -> None:
    dirty = status_paths()
    if not allow_manifest:
        exceptions: set[str] = set()
    else:
        exceptions = {manifest_rel}
    unexpected = [path for path in dirty if path not in exceptions]
    if unexpected:
        raise VerificationError(
            "source checkout is dirty; cannot claim a reviewed reproducible build: "
            + ", ".join(unexpected[:8])
        )


def module_bytes(path: Path) -> bytes:
    try:
        data = path.read_bytes()
    except OSError as exc:
        raise VerificationError(f"artifact is unavailable: {path}: {exc}") from exc
    if data.startswith(b"\x1f\x8b"):
        try:
            data = gzip.decompress(data)
        except (OSError, EOFError) as exc:
            raise VerificationError(f"artifact is not valid gzip: {path}") from exc
    if not data.startswith(b"\0asm"):
        raise VerificationError(f"artifact does not contain a Wasm module: {path}")
    return data


def record_manifest(path: Path) -> None:
    rel = path.resolve().relative_to(ROOT).as_posix()
    assert_clean_except_manifest(rel, allow_manifest=True)
    run(BUILD_COMMAND, capture=False)
    module = module_bytes(ROOT / ARTIFACT)
    manifest = {
        "kind": MANIFEST_KIND,
        "canister": CANISTER,
        "source_commit": git("rev-parse", "HEAD"),
        "source_tree": git("rev-parse", "HEAD^{tree}"),
        "build_command": BUILD_COMMAND,
        "cargo_lock_sha256": sha256((ROOT / "Cargo.lock").read_bytes()),
        "icp_manifest_sha256": sha256((ROOT / "icp.yaml").read_bytes()),
        "toolchain": toolchain(),
        "wasm_module_sha256": sha256(module),
    }
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    print(f"Wrote candidate provenance manifest: {path}")
    print("Review and commit this file with the source before using the verifier.")
    print(f"Source commit: {manifest['source_commit']}")
    print(f"Source tree: {manifest['source_tree']}")
    print(f"Wasm module SHA-256: {manifest['wasm_module_sha256']}")


def load_reviewed_manifest(path: Path) -> dict[str, Any]:
    rel = path.resolve().relative_to(ROOT).as_posix()
    assert_clean_except_manifest(rel, allow_manifest=False)
    run(["git", "ls-files", "--error-unmatch", rel])
    try:
        manifest = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as exc:
        raise VerificationError(f"cannot read provenance manifest {path}: {exc}") from exc
    if not isinstance(manifest, dict) or manifest.get("kind") != MANIFEST_KIND:
        raise VerificationError("missing or unsupported reviewed provenance manifest")
    if manifest.get("canister") != CANISTER:
        raise VerificationError("provenance manifest names a different canister")
    if manifest.get("build_command") != BUILD_COMMAND:
        raise VerificationError("build command differs from the reviewed manifest")

    source_commit = manifest.get("source_commit")
    if not isinstance(source_commit, str) or not re.fullmatch(r"[0-9a-f]{40,64}", source_commit):
        raise VerificationError("manifest source_commit is missing or malformed")
    run(["git", "merge-base", "--is-ancestor", source_commit, "HEAD"])
    source_tree = git("rev-parse", f"{source_commit}^{{tree}}")
    if source_tree != manifest.get("source_tree"):
        raise VerificationError("manifest source tree does not match its source commit")
    diff = subprocess.run(
        ["git", "diff", "--quiet", source_commit, "HEAD", "--", ".", f":(exclude){rel}"],
        cwd=ROOT,
    )
    if diff.returncode != 0:
        raise VerificationError("HEAD contains source changes after the reviewed manifest source commit")

    for key, current in (
        ("cargo_lock_sha256", sha256((ROOT / "Cargo.lock").read_bytes())),
        ("icp_manifest_sha256", sha256((ROOT / "icp.yaml").read_bytes())),
    ):
        if manifest.get(key) != current:
            raise VerificationError(f"{key} differs from the reviewed manifest")
    if manifest.get("toolchain") != toolchain():
        raise VerificationError("build toolchain differs from the reviewed manifest")
    return manifest


def extract_live_module_hash(value: Any) -> str:
    found: list[Any] = []

    def visit(node: Any) -> None:
        if isinstance(node, dict):
            for key, child in node.items():
                if key == "module_hash":
                    found.append(child)
                else:
                    visit(child)
        elif isinstance(node, list):
            for child in node:
                visit(child)

    visit(value)
    if len(found) != 1:
        raise VerificationError("status response did not contain exactly one module_hash")
    raw = found[0]
    if raw is None:
        raise VerificationError("live canister has no installed module")
    if isinstance(raw, list) and all(type(byte) is int and 0 <= byte <= 255 for byte in raw):
        result = bytes(raw).hex()
    elif isinstance(raw, str):
        result = raw.removeprefix("0x").lower()
    else:
        raise VerificationError("live module_hash has an unsupported representation")
    if not re.fullmatch(r"[0-9a-f]{64}", result):
        raise VerificationError("live module_hash is malformed")
    return result


def verify(phase: str, manifest_path: Path) -> None:
    manifest = load_reviewed_manifest(manifest_path)
    run(BUILD_COMMAND, capture=False)
    module = module_bytes(ROOT / ARTIFACT)
    actual_hash = sha256(module)
    expected_hash = manifest.get("wasm_module_sha256")
    if not isinstance(expected_hash, str) or not re.fullmatch(r"[0-9a-f]{64}", expected_hash):
        raise VerificationError("manifest Wasm module hash is missing or malformed")
    if actual_hash != expected_hash:
        raise VerificationError(
            f"rebuilt Wasm hash differs from reviewed source: expected {expected_hash}, got {actual_hash}"
        )
    print(f"PASS reviewed source commit: {manifest['source_commit']}")
    print(f"PASS candidate Wasm module SHA-256: {actual_hash}")

    if phase == "preinstall":
        print("PREINSTALL: live module hash was not queried; this verifies only the candidate artifact.")
        return

    status = run(
        [
            "icp",
            "canister",
            "status",
            CANISTER,
            "--environment",
            ENVIRONMENT,
            "--public",
            "--json",
        ]
    )
    try:
        live_hash = extract_live_module_hash(json.loads(status))
    except json.JSONDecodeError as exc:
        raise VerificationError("icp status did not return valid JSON") from exc
    if live_hash != actual_hash:
        raise VerificationError(
            f"live module hash differs from reviewed build: expected {actual_hash}, got {live_hash}"
        )
    print(f"PASS live module hash: {live_hash}")


def self_test() -> None:
    # This is the critical distinction: the network module hash is over raw
    # Wasm bytes. The gzip transport hash is a different value.
    wasm = b"\0asm\x01\0\0\0fixture"
    compressed = gzip.compress(wasm, mtime=0)
    assert sha256(compressed) != sha256(wasm)
    assert module_bytes_from_data(compressed) == wasm
    assert extract_live_module_hash({"module_hash": list(bytes.fromhex("ab" * 32))}) == "ab" * 32
    assert extract_live_module_hash({"module_hash": "0x" + "cd" * 32}) == "cd" * 32
    for bad in ({"module_hash": None}, {}, {"module_hash": "12"}):
        try:
            extract_live_module_hash(bad)
        except VerificationError:
            pass
        else:
            raise AssertionError(f"accepted invalid live module hash: {bad!r}")
    print("PASS raw-Wasm versus gzip identity and fail-closed status parsing")


def module_bytes_from_data(data: bytes) -> bytes:
    return gzip.decompress(data) if data.startswith(b"\x1f\x8b") else data


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--phase", choices=("preinstall", "postinstall"))
    group.add_argument("--record-manifest", action="store_true")
    group.add_argument("--self-test", action="store_true")
    parser.add_argument("--manifest", type=Path, default=ROOT / MANIFEST)
    args = parser.parse_args()
    if not args.manifest.is_absolute():
        args.manifest = ROOT / args.manifest
    try:
        if args.self_test:
            self_test()
        elif args.record_manifest:
            record_manifest(args.manifest)
        else:
            verify(args.phase, args.manifest)
    except VerificationError as exc:
        print(f"FAIL {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
