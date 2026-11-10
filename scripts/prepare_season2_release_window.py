#!/usr/bin/env python3
"""Render a local Season 2 init; never calls a canister or authorizes execution."""
import argparse
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import subprocess

UTC = dt.timezone.utc
EPOCH = dt.datetime(1970, 1, 1, tzinfo=UTC)
START_NS = 1_793_491_200_000_000_000
QUARTER_NS = 900_000_000_000
H0 = "d050af464ea96252b37b21da99eaa7e7dd8291766627aee3eabd05610d0448b8"
ADMIN = "fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae"
WASM_SHA256 = "dc6b6fe48e79571d0bbe371b9fb8854bc77207abf3b5005c0eda7455388a9c17"


def parse_time(value):
    parsed = dt.datetime.fromisoformat(value.replace("Z", "+00:00"))
    if parsed.tzinfo is None:
        raise ValueError("timestamps require an explicit UTC offset")
    return parsed.astimezone(UTC)


def ns(value):
    delta = value - EPOCH
    return (delta.days * 86400 + delta.seconds) * 1_000_000_000 + delta.microseconds * 1000


def iso(value):
    return value.astimezone(UTC).isoformat().replace("+00:00", "Z")


def derive(approved_at, preflight_at):
    if preflight_at < approved_at:
        raise ValueError("preflight precedes approval")
    expiry = approved_at + dt.timedelta(hours=24)
    earliest_ns = ns(preflight_at) + 3_600_000_000_000
    cutoff_ns = ((earliest_ns + QUARTER_NS - 1) // QUARTER_NS) * QUARTER_NS
    if cutoff_ns > ns(expiry):
        raise ValueError("no cutoff with the required lead time remains in the approval window")
    if cutoff_ns <= START_NS or cutoff_ns > (1 << 64) - 1:
        raise ValueError("cutoff outside supported Season 2 time range")
    return {
        "approval_at_utc": iso(approved_at),
        "preflight_at_utc": iso(preflight_at),
        "approval_expires_at_utc": iso(expiry),
        "cutoff_ns": cutoff_ns,
        "cutoff_utc": iso(EPOCH + dt.timedelta(seconds=cutoff_ns // 1_000_000_000)),
        "logical_start_ns": START_NS,
        "credit_policy_status": "unresolved; timing preparation does not approve estimates or compensation",
        "credit_import_authorized_by_this_binding": False,
        "creation_source_debit_cycles": 900_100_000_000,
        "creation_amount_cycles": 900_000_000_000,
        "expected_new_canister_cycles_before_install": 400_000_000_000,
        "wasm_sha256": WASM_SHA256,
        "public_h0": H0,
    }


def init_text(cutoff_ns):
    blob = "".join("\\" + H0[i:i + 2] for i in range(0, len(H0), 2))
    return (
        "// Derived proposal. No execution authority is supplied by this file.\n"
        "(opt record {\n"
        f'  admin = opt principal "{ADMIN}";\n'
        "  excluded_principals = null;\n"
        f"  season_start_ns = opt {cutoff_ns} : opt nat64;\n"
        "  season_end_ns = opt 18446744073709551615 : opt nat64;\n"
        f'  snapshot_seed_commit = opt blob "{blob}";\n'
        "})\n"
    )


def write_new(path, content):
    with os.fdopen(os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "wb") as file:
        file.write(content)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--approved-at", required=True)
    parser.add_argument("--preflight-at")
    parser.add_argument("--permission-ref", default="")
    parser.add_argument("--example", action="store_true", help="non-executable local fixture")
    parser.add_argument("--out-dir", type=Path, required=True, help="new directory; existing files are never replaced")
    args = parser.parse_args()
    now = dt.datetime.now(UTC)
    approved = parse_time(args.approved_at)
    preflight = parse_time(args.preflight_at) if args.preflight_at else now
    if not args.example:
        if not args.permission_ref.strip():
            raise ValueError("a direct approval reference is required; this script cannot verify or grant it")
        if approved > now or abs((preflight - now).total_seconds()) > 60:
            raise ValueError("runtime approval cannot be future-dated and preflight must be current")
    binding = derive(approved, preflight)
    binding.update({
        "example_only": args.example,
        "permission_ref": args.permission_ref or None,
        "authorization_verified_by_script": False,
        "execution_performed": False,
        "retime_after_install_permitted": False,
        "require_current_clock_before_T0_at_begin": True,
        "require_current_clock_before_first_A_at_start": True,
    })
    text = init_text(binding["cutoff_ns"])
    did = Path(__file__).resolve().parents[1] / "src/rumi_points/rumi_points.did"
    encoded = subprocess.run(
        ["didc", "encode", "--types", "(opt InitArgs)", "--defs", str(did)],
        input=text, text=True, capture_output=True, check=True,
    )
    binary = bytes.fromhex(encoded.stdout.strip())
    binding["init_text_sha256"] = hashlib.sha256(text.encode()).hexdigest()
    binding["init_binary_sha256"] = hashlib.sha256(binary).hexdigest()
    binding["init_binary_bytes"] = len(binary)
    args.out_dir.mkdir(mode=0o700, parents=False, exist_ok=False)
    stem = "example-init" if args.example else "season2-init"
    write_new(args.out_dir / f"{stem}.did", text.encode())
    write_new(args.out_dir / f"{stem}.bin", binary)
    write_new(args.out_dir / "window-binding.json", (json.dumps(binding, indent=2) + "\n").encode())
    print(json.dumps(binding, indent=2))


if __name__ == "__main__":
    main()
