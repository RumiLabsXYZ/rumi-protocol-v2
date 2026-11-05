#!/usr/bin/env python3
"""Opt-in reviewed upgrade: stop, verify stopped, upgrade once, restart once.

This is not a replacement for snapshot, Candid, controller, or release review.
Any uncertain management result is held for manual reconciliation; it never
retries a mutation or automatically restarts after an uncertain upgrade.
"""
import argparse
from contextlib import contextmanager
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import stat
import sys


class Hold(Exception):
    pass


def sha(data):
    return hashlib.sha256(data).hexdigest()


def run(argv, timeout):
    try:
        return subprocess.run(argv, stdin=subprocess.DEVNULL, capture_output=True,
                              text=True, timeout=timeout, check=False)
    except (subprocess.TimeoutExpired, OSError) as exc:
        raise Hold(f"command outcome unknown: {exc}") from exc


@contextmanager
def target_lock(target):
    # Target-only key also serializes aliases for the same canister principal.
    # This is local to this host; release controllers on other hosts must coordinate.
    name = "rumi-drained-upgrade-" + sha(target.encode("utf-8")) + ".lock"
    path = os.path.join("/tmp", name)
    flags = os.O_RDWR | os.O_CREAT | getattr(os, "O_NOFOLLOW", 0)
    fd = os.open(path, flags, 0o600)
    try:
        info = os.fstat(fd)
        if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_nlink != 1 or info.st_mode & 0o077:
            raise Hold("unsafe target lock file")
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as exc:
            raise Hold("another local upgrade for this target is in progress") from exc
        try:
            yield
        finally:
            fcntl.flock(fd, fcntl.LOCK_UN)
    finally:
        os.close(fd)


def execute(a, runner=run):
    if not a.execute:
        raise Hold("review-only: pass --execute after the release is authorized")
    if not a.target:
        raise Hold("explicit target principal required")
    with target_lock(a.target):
        return execute_locked(a, runner)


def execute_locked(a, runner):
    if not all((a.target, a.environment, a.identity)):
        raise Hold("explicit target, environment and identity are required")
    if not 1 <= a.timeout <= 300:
        raise Hold("timeout must be between 1 and 300 seconds")
    for value in (a.current_sha256, a.new_sha256, a.args_sha256):
        if not re.fullmatch(r"[0-9a-fA-F]{64}", value):
            raise Hold("all hashes must be 64 hexadecimal characters")
    v = runner(["icp", "--version"], a.timeout)
    if v.returncode or v.stdout.strip() != "icp 1.3.0":
        raise Hold("status parser is verified only for icp 1.3.0")
    artifact = Path(a.wasm).read_bytes()
    args = Path(a.args_file).read_bytes()
    if sha(artifact) != a.new_sha256.lower() or sha(args) != a.args_sha256.lower():
        raise Hold("artifact or arguments do not match approved hashes")
    args_text = args.decode("utf-8")
    def cmd(verb, *rest):
        return ["icp", "canister", verb, a.target, *rest, "--environment", a.environment, "--identity", a.identity]
    def status():
        result = runner(cmd("status", "--json"), a.timeout)
        if result.returncode:
            raise Hold("controller status request failed")
        try:
            value = json.loads(result.stdout)
        except (ValueError, TypeError) as exc:
            raise Hold("status is not a single JSON object") from exc
        # In ICP CLI 1.3, the public fallback omits status and settings.
        if not isinstance(value, dict) or value.get("id") != a.target or value.get("status") not in ("Running", "Stopping", "Stopped") or not isinstance(value.get("settings"), dict):
            raise Hold("exact controller-authenticated status is unavailable")
        module = value.get("module_hash")
        if not isinstance(module, str) or not re.fullmatch(r"(?:0x)?[0-9a-fA-F]{64}", module):
            raise Hold("module hash is unavailable")
        return value["status"], module.removeprefix("0x").lower()
    old_hash, new_hash = a.current_sha256.lower(), a.new_sha256.lower()
    if status() != ("Running", old_hash):
        raise Hold("running status or current module hash does not match")
    # O_EXCL prevents accidental reuse of this evidence path. It is not a
    # cross-host lock: release operator must ensure exclusive management access.
    fd = os.open(a.journal, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8") as journal:
        def record(event, **data):
            journal.write(json.dumps({"event": event, **data}, sort_keys=True) + "\n")
            journal.flush()
            os.fsync(journal.fileno())
        record("preflight", target=a.target, environment=a.environment, identity=a.identity,
               current_sha256=old_hash, new_sha256=new_hash, args_sha256=a.args_sha256.lower())
        # Freeze the already verified bytes in an exclusive evidence sibling.
        frozen = a.journal + ".wasm"
        wasmfd = os.open(frozen, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(wasmfd, "wb") as output:
            output.write(artifact)
            output.flush()
            os.fsync(output.fileno())
        if sha(Path(frozen).read_bytes()) != new_hash:
            raise Hold("frozen artifact hash mismatch")
        record("stop_dispatched")
        try:
            result = runner(cmd("stop"), a.timeout)
        except Hold as exc:
            record("stop_unknown", detail=str(exc))
            raise Hold("stop outcome unknown; reconcile before any start or upgrade") from exc
        record("stop_result", code=result.returncode, stdout=result.stdout, stderr=result.stderr)
        if result.returncode:
            raise Hold("stop did not return success; reconcile before any start or upgrade")
        if status() != ("Stopped", old_hash):
            raise Hold("stopped status and original hash were not verified; reconcile")
        if sha(Path(frozen).read_bytes()) != new_hash:
            raise Hold("frozen artifact changed; leave stopped for review")
        record("upgrade_dispatched")
        try:
            result = runner(cmd("install", "--mode", "upgrade", "--wasm", frozen, "--args", args_text), a.timeout)
        except Hold as exc:
            record("upgrade_unknown", detail=str(exc))
            raise Hold("upgrade outcome unknown; leave stopped and reconcile") from exc
        record("upgrade_result", code=result.returncode, stdout=result.stdout, stderr=result.stderr)
        if result.returncode:
            raise Hold("upgrade did not return success; leave stopped and reconcile")
        if status() != ("Stopped", new_hash):
            raise Hold("installed hash or stopped state not verified; do not restart")
        record("start_dispatched")
        try:
            result = runner(cmd("start"), a.timeout)
        except Hold as exc:
            record("start_unknown", detail=str(exc))
            raise Hold("start outcome unknown; reconcile manually") from exc
        record("start_result", code=result.returncode, stdout=result.stdout, stderr=result.stderr)
        if result.returncode or status() != ("Running", new_hash):
            raise Hold("running status and module hash not verified")
        record("running_verified", module_hash=new_hash)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ("target", "environment", "identity", "wasm", "args-file", "current-sha256", "new-sha256", "args-sha256", "journal"):
        p.add_argument("--" + name, required=True)
    p.add_argument("--timeout", type=int, default=60)
    p.add_argument("--execute", action="store_true")
    try:
        execute(p.parse_args())
    except (Hold, OSError, UnicodeError) as exc:
        print(f"HOLD: {exc}", file=sys.stderr)
        return 1
    print("Running state and uploaded module hash verified; complete release readback separately.")
    return 0

if __name__ == "__main__":
    sys.exit(main())
