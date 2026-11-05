#!/usr/bin/env python3
"""Exercise exact SP Wasm against local PocketIC and local ledger fixtures only.

No production URL, identity, CLI call, or production canister is accepted.
Every update is submitted once and its original ingress ID is polled.
"""
from __future__ import annotations

import argparse
import base64
import binascii
import hashlib
import gzip
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import traceback
import urllib.error
import urllib.request


def principal(raw: bytes) -> str:
    encoded = base64.b32encode(binascii.crc32(raw).to_bytes(4, "big") + raw).decode().lower().rstrip("=")
    return "-".join(encoded[i:i + 5] for i in range(0, len(encoded), 5))


def raw_principal(value: str) -> bytes:
    text = value.replace("-", "").upper()
    decoded = base64.b32decode(text + "=" * (-len(text) % 8))
    raw = decoded[4:]
    assert binascii.crc32(raw).to_bytes(4, "big") == decoded[:4]
    return raw


def b64(value: bytes) -> str:
    return base64.b64encode(value).decode()


def nat(value) -> int:
    return int(str(value).replace("_", ""))


def account(owner: str) -> str:
    return 'record { owner=principal "' + owner + '"; subaccount=null }'


def blob(value: bytes) -> str:
    return 'blob "' + "".join("\\%02x" % x for x in value) + '"'


MGMT_DID = '''
type Settings=record { controllers:opt vec principal; compute_allocation:opt nat;
 memory_allocation:opt nat; freezing_threshold:opt nat };
service:{
 provisional_create_canister_with_cycles:(record {amount:opt nat;settings:opt Settings;
 specified_id:opt principal;sender_canister_version:opt nat64})->(record{canister_id:principal});
 install_code:(record {mode:variant{install;reinstall;upgrade};canister_id:principal;
 wasm_module:blob;arg:blob;sender_canister_version:opt nat64})->();
}
'''


class Driver:
    def __init__(self, args):
        self.args = args
        self.out = args.out.resolve()
        self.out.mkdir(mode=0o700, parents=True, exist_ok=False)
        self.sp_did = args.sp_did.resolve()
        self.ledger_did = args.ledger_did.resolve()
        self.flaky_did = args.flaky_did.resolve()
        self.mgmt_did = self.out / "management.did"
        self.mgmt_did.write_text(MGMT_DID)
        self.admin = principal(b"\x11" * 28 + b"\x02")
        self.user = principal(b"\x22" * 28 + b"\x02")
        self.other = principal(b"\x33" * 28 + b"\x02")
        self.backend = principal(b"\x44" * 28 + b"\x02")
        self.anon = "2vxsx-fae"
        self.calls = []
        self.checks = {}
        self.instance = None
        self.server = None
        self.url = None
        self.serial = 0

    def encode(self, text, did, method=None, types=None):
        command = [str(self.args.didc), "encode", "--defs", str(did)]
        command += ["--method", method] if method else ["--types", types]
        p = subprocess.run(command, input=text, text=True, capture_output=True, timeout=60)
        if p.returncode:
            raise RuntimeError("Candid encode failed: " + p.stderr)
        return bytes.fromhex(p.stdout.strip())

    def decode(self, raw, did, method, name):
        p = subprocess.run([str(self.args.didc), "decode", "--defs", str(did), "--method", method],
                           input=raw.hex(), text=True, capture_output=True, timeout=60)
        assert p.returncode == 0, p.stderr
        reply = self.out / (name + ".reply.txt")
        reply.write_text(p.stdout)
        (self.out / (name + ".reply.bin")).write_bytes(raw)
        target = self.out / (name + ".typed.json")
        p = subprocess.run([str(self.args.decoder), str(did), method, str(reply), str(target)],
                           capture_output=True, text=True, timeout=60)
        assert p.returncode == 0, p.stderr
        return json.loads(target.read_text())

    def http(self, path, body=None, method="POST"):
        assert self.url and self.url.startswith("http://127.0.0.1:")
        url = self.url + path
        payload = json.dumps(body).encode() if method == "POST" else None
        request = urllib.request.Request(url, data=payload, method=method,
                                         headers={"Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(request, timeout=120) as response:
                status = response.status
                raw = response.read()
        except urllib.error.HTTPError as error:
            if error.code == 404:
                raise
            raise RuntimeError("Local HTTP error " + str(error.code) + ": " + error.read().decode()[:10000]) from error
        value = json.loads(raw) if raw else None
        if status == 202:
            # A started computation is polled by ID; never resubmit the update.
            assert set(value) >= {"state_label", "op_id"}, value
            for _ in range(12000):
                time.sleep(.01)
                try:
                    return self.http("/read_graph/" + value["state_label"] + "/" + str(value["op_id"]), method="GET")
                except urllib.error.HTTPError as error:
                    if error.code != 404:
                        raise
            raise TimeoutError("PocketIC operation did not finish")
        return value

    def endpoint(self, suffix):
        assert self.instance is not None
        return "/instances/" + str(self.instance) + "/" + suffix

    def start(self):
        port_file = self.out / "server.port"
        command = [str(self.args.pocketic), "--ip-addr", "127.0.0.1", "--port", "0",
                   "--port-file", str(port_file), "--ttl", "900"]
        self.server_log = (self.out / "server.log").open("wb")
        self.server = subprocess.Popen(command, stdout=self.server_log, stderr=subprocess.STDOUT)
        for _ in range(600):
            if port_file.exists() and port_file.read_text().strip():
                break
            if self.server.poll() is not None:
                raise RuntimeError("Own PocketIC server exited; inspect server.log")
            time.sleep(.05)
        assert port_file.exists() and port_file.read_text().strip(), "PocketIC server did not publish its port"
        self.url = "http://127.0.0.1:" + str(int(port_file.read_text().strip()))
        config = {"subnet_config_set": {"nns": None, "sns": None, "ii": None,
                  "fiduciary": None, "bitcoin": None, "system": [],
                  "application": [{"state_config": "New", "instruction_config": "Production"}],
                  "verified_application": []}, "state_dir": None, "nonmainnet_features": False,
                  "log_level": None, "bitcoind_addr": None}
        result = self.http("/instances", config)
        assert "Created" in result, result
        self.instance = result["Created"]["instance_id"]
        (self.out / "local-server.json").write_text(json.dumps({"command": command,
            "pid": self.server.pid, "url": self.url, "instance": self.instance}, indent=2))

    def call(self, target, method, text="()", *, sender=None, did=None, query=False, effective=None):
        did = did or self.sp_did
        self.serial += 1
        name = "%03d-%s" % (self.serial, method)
        payload = self.encode(text, did, method)
        raw_target = raw_principal(target)
        request = {"sender": b64(raw_principal(sender or self.admin)),
                   "canister_id": b64(raw_target), "method": method, "payload": b64(payload),
                   "effective_principal": effective or {"CanisterId": b64(raw_target)}}
        began = time.time()
        if query:
            result = self.http(self.endpoint("read/query"), request)
        else:
            submitted = self.http(self.endpoint("update/submit_ingress_message"), request)
            assert "Ok" in submitted, submitted
            msg = submitted["Ok"]
            result = None
            for _ in range(300):
                self.http(self.endpoint("update/tick"), {"blockmakers": None})
                result = self.http(self.endpoint("read/ingress_status"),
                                   {"raw_message_id": msg, "raw_caller": None})
                if result is not None:
                    break
            assert result is not None, "Local ingress did not terminate"
        record = {"method": method, "target": target, "sender": sender or self.admin,
                  "local_url": self.url, "query": query, "elapsed_seconds": time.time() - began,
                  "result": result}
        self.calls.append(record)
        (self.out / (name + ".receipt.json")).write_text(json.dumps(record, indent=2))
        assert "Ok" in result, result
        return self.decode(base64.b64decode(result["Ok"]), did, method, name)

    def create(self):
        result = self.call("aaaaa-aa", "provisional_create_canister_with_cycles",
            "(record { amount=opt 0; settings=null; specified_id=null; sender_canister_version=null })",
            sender=self.anon, did=self.mgmt_did, effective="None")
        cid = result[0]["canister_id"]["principal"]
        self.http(self.endpoint("update/add_cycles"), {"canister_id": b64(raw_principal(cid)), "amount": 10**14})
        return cid

    def install(self, cid, wasm, args):
        module = wasm.read_bytes()
        if len(module) > 1500000 and not module.startswith(b"\x1f\x8b"):
            module = gzip.compress(module, compresslevel=9, mtime=0)
        return self.call("aaaaa-aa", "install_code", "(record { mode=variant {install}; canister_id=principal \"" +
            cid + "\"; wasm_module=" + blob(module) + "; arg=" + blob(args) +
            "; sender_canister_version=null })", sender=self.anon, did=self.mgmt_did,
            effective={"CanisterId": b64(raw_principal(cid))})

    def check(self, key, condition):
        self.checks[key] = bool(condition)
        assert condition, key
        print("PASS", key, flush=True)

    def ok(self, result):
        assert len(result) == 1 and "Ok" in result[0], result

    def err(self, result, expected=None):
        assert len(result) == 1 and "Err" in result[0], result
        if expected:
            assert expected in result[0]["Err"], result

    def balance(self, ledger, owner, flaky=False):
        return nat(self.call(ledger, "icrc1_balance_of", "(" + account(owner) + ")",
                   did=self.flaky_did if flaky else self.ledger_did, query=True)[0])

    def position(self, owner=None):
        value = self.call(self.sp, "get_user_position", '(opt principal "' + (owner or self.user) + '")', query=True)[0]
        return value["Some"] if value is not None else None

    def book(self, token, owner=None):
        p = self.position(owner)
        if p is None:
            return 0
        return next((nat(x["1"]) for x in p["stablecoin_balances"] if x["0"]["principal"] == token), 0)

    def register(self, token, symbol, decimals, priority=1):
        config = '(record { ledger_id=principal "' + token + '"; symbol="' + symbol + '"; decimals=' + str(decimals) + "; priority=" + str(priority) + "; is_active=true; transfer_fee=opt 1; is_lp_token=null; underlying_pool=null })"
        self.ok(self.call(self.sp, "register_stablecoin", config))

    def approve(self, token, owner, flaky=False):
        text = '(record { spender=' + account(self.sp) + '; amount=1000000000000; expected_allowance=null; expires_at=null; fee=null; memo=null; from_subaccount=null; created_at_time=null })'
        self.ok(self.call(token, "icrc2_approve", text, sender=owner,
                          did=self.flaky_did if flaky else self.ledger_did))

    def deposit(self, token, amount, seq, owner=None):
        return self.call(self.sp, "deposit_with_intent", '(' + str(seq) + ' : nat64, principal "' + token + '", ' + str(amount) + ' : nat64)', sender=owner or self.user)

    def withdraw(self, token, amount, owner=None):
        return self.call(self.sp, "withdraw", '(principal "' + token + '", ' + str(amount) + ' : nat64)', sender=owner or self.user)

    def pending_withdrawals(self):
        assert "get_my_pending_withdrawals" in self.sp_did.read_text(), "Updated Candid required for pending withdrawal assertions"
        return self.call(self.sp, "get_my_pending_withdrawals", sender=self.user, query=True)[0]

    def run(self):
        self.start()
        self.sp = self.create()
        self.install(self.sp, self.args.sp_wasm, self.encode('(record { protocol_canister_id=principal "' + self.backend + '"; authorized_admins=vec {principal "' + self.admin + '"} })', self.sp_did, types="(StabilityPoolInitArgs)"))
        self.real = self.create()
        initial = '; '.join('record {' + account(p) + '; 1000000000000 : nat}' for p in [self.user, self.other, self.backend])
        ledger_init = '(variant { Init=record { minting_account=' + account(self.backend) + '; fee_collector_account=null; transfer_fee=10000; decimals=opt 8; max_memo_length=opt 32; token_name="Local icUSD"; token_symbol="icUSD"; metadata=vec {}; initial_balances=vec {' + initial + '}; feature_flags=opt record {icrc2=true}; maximum_number_of_accounts=null; accounts_overflow_trim_quantity=null; archive_options=record {num_blocks_to_archive=10000; max_transactions_per_response=opt 1000; trigger_threshold=20000; max_message_size_bytes=null; cycles_for_archive_creation=null; node_max_memory_size_bytes=null; controller_id=principal "' + self.admin + '"; more_controller_ids=null} } })'
        self.install(self.real, self.args.ledger_wasm, self.encode(ledger_init, self.ledger_did, types="(LedgerArg)"))
        self.register(self.real, "icUSD", 8)
        self.flaky = self.create()
        self.install(self.flaky, self.args.flaky_wasm, bytes.fromhex("4449444c0000"))
        self.call(self.flaky, "set_fee", "(10000 : nat)", did=self.flaky_did)
        self.call(self.flaky, "mint", "(" + account(self.user) + ",1000000000 : nat)", did=self.flaky_did)
        self.register(self.flaky, "ckUSDT", 6, 2)

        self.err(self.call(self.sp, "resume_operations", sender=self.other), "Unauthorized")
        self.ok(self.call(self.sp, "emergency_pause"))
        before = self.balance(self.real, self.user)
        self.err(self.deposit(self.real, 100000000, 1), "EmergencyPaused")
        self.check("paused_deposit_no_token_movement", self.balance(self.real, self.user) == before)
        self.ok(self.call(self.sp, "resume_operations"))
        self.check("admin_resume_clears_pause", not self.call(self.sp, "get_pool_status", query=True)[0]["emergency_paused"])
        self.err(self.call(self.sp, "deposit", '(principal "' + self.real + '",1000000 : nat64)', sender=self.user), "SystemBusy")
        self.check("legacy_deposit_remains_disabled_before_pull", self.balance(self.real, self.user) == before)
        self.approve(self.real, self.user)
        if not self.args.baseline_expect_unsafe_withdrawal:
            self.err(self.withdraw(self.real, 1000000, self.anon), "Unauthorized")
        self.ok(self.deposit(self.real, 100000000, 1))
        deposited_wallet = self.balance(self.real, self.user)
        self.check("real_ledger_deposit_exact", self.book(self.real) == 100000000 and self.balance(self.real, self.sp) == 100000000)
        self.ok(self.deposit(self.real, 100000000, 1))
        self.check("deposit_intent_replay_no_second_pull", self.balance(self.real, self.user) == deposited_wallet and self.book(self.real) == 100000000)
        self.err(self.deposit(self.real, 100000001, 1), "DepositIntentPayloadMismatch")
        rejected = self.withdraw(self.real, 100000001)
        self.err(rejected)
        assert set(rejected[0]["Err"]) & {"InsufficientBalance", "InsufficientPoolBalance"}, rejected
        self.check("oversized_withdrawal_preserves_balance", self.book(self.real) == 100000000)
        wallet = self.balance(self.real, self.user)
        self.ok(self.withdraw(self.real, 40000000))
        self.check("confirmed_withdrawal_exact_net_fee", self.balance(self.real, self.user) - wallet == 39990000 and self.book(self.real) == 60000000 and self.balance(self.real, self.sp) == 60000000)

        # Test collateral payout with the real ICRC ledger, separate from deposits.
        self.ok(self.call(self.sp, "register_collateral", '(record {ledger_id=principal "' + self.real + '"; symbol="TEST"; decimals=8; status=variant {Active}})'))
        self.ok(self.call(self.real, "icrc1_transfer", '(record {to=' + account(self.sp) + '; amount=5000000; fee=null; from_subaccount=null; memo=null; created_at_time=null})', sender=self.backend, did=self.ledger_did))
        self.ok(self.call(self.sp, "admin_correct_collateral_gain", '(principal "' + self.user + '", principal "' + self.real + '", 5000000 : nat64)'))
        wallet = self.balance(self.real, self.user)
        self.ok(self.call(self.sp, "claim_collateral", '(principal "' + self.real + '")', sender=self.user))
        self.check("collateral_claim_exact_net_fee", self.balance(self.real, self.user) - wallet == 4990000)
        wallet = self.balance(self.real, self.user)
        self.ok(self.call(self.sp, "claim_collateral", '(principal "' + self.real + '")', sender=self.user))
        self.check("claim_repeat_no_second_payout", self.balance(self.real, self.user) == wallet)

        # Fault fixture setup is explicit. Older cached flaky ledgers lack
        # ICRC-3, so seed local backing/books instead of claiming their intent
        # deposit completed. The real ledger above proves the deposit path.
        self.call(self.flaky, "mint", "(" + account(self.sp) + ",10000000 : nat)", did=self.flaky_did)
        self.ok(self.call(self.sp, "admin_correct_balance", '(principal "' + self.user + '",principal "' + self.flaky + '",10000000 : nat64)'))
        self.check("local_fault_fixture_backing_matches_seeded_book", self.balance(self.flaky, self.sp, True) == self.book(self.flaky) == 10000000)
        self.call(self.flaky, "set_fee", "(20000 : nat)", did=self.flaky_did)
        wallet = self.balance(self.flaky, self.user, True)
        self.ok(self.withdraw(self.flaky, 1000000))
        self.check("withdrawal_uses_live_fee_after_change", self.balance(self.flaky, self.user, True) - wallet == 980000 and self.book(self.flaky) == 9000000)

        wallet = self.balance(self.flaky, self.user, True)
        book = self.book(self.flaky)
        self.call(self.flaky, "set_bad_fee_failures", "(1 : nat32)", did=self.flaky_did)
        self.err(self.withdraw(self.flaky, 1000000))
        self.check("proven_no_effect_bad_fee_restores_book", self.book(self.flaky) == book and self.balance(self.flaky, self.user, True) == wallet)

        # The fixture commits the transfer but returns GenericError once.
        self.call(self.flaky, "set_phantom_failures", "(1 : nat32)", did=self.flaky_did)
        wallet = self.balance(self.flaky, self.user, True)
        old_book = self.book(self.flaky)
        result = self.withdraw(self.flaky, 1000000)
        paid = self.balance(self.flaky, self.user, True) - wallet
        held = old_book - self.book(self.flaky)
        self.check("lost_reply_fixture_really_committed", paid == 980000)
        self.checks["lost_reply_keeps_debit"] = held == 1000000
        if self.args.baseline_expect_unsafe_withdrawal:
            self.check("old_artifact_withdrawal_bug_reproduced", held == 0)
            self.baseline_regression = {"already_paid_net": paid, "book_debit_after_error": held, "reply": result}
        else:
            self.check("lost_reply_keeps_debit", held == 1000000)
            pending = self.pending_withdrawals()
            self.check("lost_reply_retains_exact_withdrawal_tuple", len(pending) == 1 and nat(pending[0]["gross_amount"]) == 1000000 and nat(pending[0]["transfer_fee"]) == 20000 and nat(pending[0]["transfer_amount"]) == 980000 and pending[0]["recipient"]["owner"]["principal"] == self.user)
            self.check("pending_withdrawal_inventory_is_caller_scoped", self.call(self.sp, "get_my_pending_withdrawals", sender=self.other, query=True)[0] == [])
            self.err(self.call(self.sp, "reconcile_withdrawal", '(principal "' + self.flaky + '",principal "' + self.user + '",0 : nat64)', sender=self.other), "Unauthorized")
            self.check("unauthorized_withdrawal_reconciliation_retains_tuple", self.pending_withdrawals() == pending)
            retry_book = self.book(self.flaky)
            self.err(self.withdraw(self.flaky, 2000000))
            self.check("pending_withdrawal_rejects_changed_amount", self.book(self.flaky) == retry_book and self.pending_withdrawals() == pending)
            retry_wallet = self.balance(self.flaky, self.user, True)
            retried = self.withdraw(self.flaky, 1000000)
            self.check("lost_reply_retry_does_not_pay_twice", self.balance(self.flaky, self.user, True) == retry_wallet and self.book(self.flaky) == old_book - 1000000)
            if self.args.require_fault_proof_completion:
                self.ok(retried)
                self.check("lost_reply_duplicate_receipt_proven_and_cleared", self.pending_withdrawals() == [])

        # New ordinary interest is supported; duplicate source keys are inert.
        self.ok(self.call(self.real, "icrc1_transfer", '(record {to=' + account(self.sp) + '; amount=100000; fee=null; from_subaccount=null; memo=null; created_at_time=null})', sender=self.backend, did=self.ledger_did))
        old_book = self.book(self.real)
        interest = '(principal "' + self.real + '", 100000 : nat64, opt principal "' + self.real + '", 9000000000 : nat64)'
        self.err(self.call(self.sp, "receive_interest_revenue_v2", interest, sender=self.other), "Unauthorized")
        self.check("unauthorized_interest_does_not_credit", self.book(self.real) == old_book)
        self.ok(self.call(self.sp, "receive_interest_revenue_v2", interest, sender=self.backend))
        after_interest = self.book(self.real)
        self.ok(self.call(self.sp, "receive_interest_revenue_v2", interest, sender=self.backend))
        self.check("ordinary_interest_and_duplicate_exact_once", after_interest == old_book + 100000 and self.book(self.real) == after_interest)
        self.ok(self.call(self.sp, "emergency_pause"))
        wallet = self.balance(self.real, self.user)
        self.err(self.withdraw(self.real, 1000000), "EmergencyPaused")
        self.check("repaused_withdrawal_no_movement", self.balance(self.real, self.user) == wallet)
        self.err(self.call(self.sp, "deposit_as_3usd", '(principal "' + self.real + '",1000000 : nat64)', sender=self.user), "SystemBusy")
        self.check("disabled_conversion_no_movement", self.balance(self.real, self.user) == wallet)
        self.ok(self.call(self.sp, "resume_operations"))
        self.ok(self.call(self.sp, "validate_pool_state", query=True))
        reconciliation = self.call(self.sp, "get_ledger_reconciliation")[0]["Ok"]
        deltas = {row["symbol"]: nat(row["delta_e8s"]) for row in reconciliation}
        if self.args.baseline_expect_unsafe_withdrawal:
            self.check("old_artifact_paid_credit_restoration_causes_shortfall", deltas["ckUSDT"] == -1000000)
        else:
            self.check("all_registered_ledger_balances_equal_books", len(reconciliation) == 2 and all(nat(row["delta_e8s"]) == 0 and row["healthy"] for row in reconciliation))

    def close(self):
        if self.instance is not None:
            try:
                self.http("/instances/" + str(self.instance), method="DELETE")
            except Exception as error:
                (self.out / "cleanup-error.txt").write_text(str(error))
        if self.server is not None:
            self.server.terminate()
            try:
                self.server.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.server.kill()
                self.server.wait(timeout=10)
            self.server_log.close()


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--sp-wasm", type=Path, required=True)
    p.add_argument("--sp-did", type=Path, required=True)
    p.add_argument("--ledger-wasm", type=Path, required=True)
    p.add_argument("--ledger-did", type=Path, required=True)
    p.add_argument("--flaky-wasm", type=Path, required=True)
    p.add_argument("--flaky-did", type=Path, required=True)
    p.add_argument("--decoder", type=Path, required=True)
    p.add_argument("--didc", type=Path, default=Path.home()/".cargo/bin/didc")
    p.add_argument("--pocketic", type=Path, default=Path.home()/".cache/pocketic/9.0.3/pocket-ic")
    p.add_argument("--out", type=Path, required=True)
    p.add_argument("--baseline-expect-unsafe-withdrawal", action="store_true")
    p.add_argument("--require-fault-proof-completion", action="store_true",
                   help="Require a fault fixture with ICRC-3 support and successful proof-verified retry completion")
    args = p.parse_args()
    for key in ["sp_wasm", "sp_did", "ledger_wasm", "ledger_did", "flaky_wasm", "flaky_did", "decoder", "didc", "pocketic"]:
        # The cached server requires its executable basename to be pocket-ic.
        # Preserve that symlink instead of resolving to pocket-ic-arm64-darwin.
        value = getattr(args,key).absolute() if key == "pocketic" else getattr(args,key).resolve()
        assert value.is_file(), value
        setattr(args,key,value)
    version = subprocess.run([str(args.pocketic), "--version"], capture_output=True, text=True, check=True).stdout.strip()
    assert version == "pocket-ic-server 9.0.3", version
    driver = Driver(args)
    failure = None
    try:
        driver.run()
    except Exception:
        failure = traceback.format_exc()
        print(failure, file=sys.stderr)
        (driver.out / "failure.txt").write_text(failure)
    finally:
        driver.close()
        report = {"passed": failure is None, "test_mode": "old_artifact_regression" if args.baseline_expect_unsafe_withdrawal else "repaired_artifact_readiness",
                  "local_runtime_gate_passed": failure is None and not args.baseline_expect_unsafe_withdrawal and all(driver.checks.values()),
                  "production_readiness_claimed": False,
                  "local_only": True, "production_calls": 0,
                  "checks": driver.checks, "baseline_regression": getattr(driver,"baseline_regression",None),
                  "server_version": version, "calls": len(driver.calls),
                  "command": sys.argv,
                  "harness_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
                  "inputs": {key: {"path": str(getattr(args,key)), "sha256": hashlib.sha256(getattr(args,key).read_bytes()).hexdigest()} for key in ["sp_wasm","sp_did","ledger_wasm","ledger_did","flaky_wasm","flaky_did","decoder"]},
                  "limits": ["Local real-ledger and controlled-fault fixtures; no production execution.",
                             "Fresh-init fixture does not reproduce production completed-recovery state or native liquidation/network flows.",
                             *([] if args.require_fault_proof_completion else ["Fault-proof completion was not required in this mode; held-debit/no-duplicate assertions alone do not prove final proof acknowledgment."])]}
        (driver.out / "verification.json").write_text(json.dumps(report, indent=2))
        print(json.dumps({k:v for k,v in report.items() if k not in ["inputs"]}, indent=2))
    return 0 if failure is None else 1


if __name__ == "__main__":
    sys.exit(main())
