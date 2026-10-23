#!/usr/bin/env python3
"""Local PocketIC end-to-end rail checks for the restored 3USD/SP paths.

This harness is deliberately a local artifact test.  It accepts every Wasm and
Candid path explicitly, starts one private PocketIC instance, uses only local
ICRC ledgers and a local XRC fixture, and never addresses a production
canister.  It records the exact Candid payload submitted for every ingress and
the exact reply observed for that ingress.  An update is submitted once and is
only polled afterwards, which makes ambiguous callback tests reproducible.

The generated DIDs must come from the same candidate build as the Wasms.  The
script fails before starting PocketIC when an interface does not expose the
restored methods, so a stale checked-in DID cannot silently turn a passing run
into evidence for a different artifact.
"""
from __future__ import annotations

import argparse
import base64
import binascii
import gzip
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import traceback
import urllib.error
import urllib.request

from sp_reopening_runtime import Driver, account, b64, blob, nat, principal, raw_principal


XRC_DID = '''
type AssetClass=variant {Cryptocurrency; FiatCurrency};
type Asset=record {symbol:text; class:AssetClass};
type GetExchangeRateRequest=record {base_asset:Asset; quote_asset:Asset; timestamp:opt nat64};
type ExchangeRateMetadata=record {decimals:nat32; base_asset_num_queried_sources:nat64; base_asset_num_received_rates:nat64; quote_asset_num_queried_sources:nat64; quote_asset_num_received_rates:nat64; standard_deviation:nat64; forex_timestamp:opt nat64};
type ExchangeRate=record {base_asset:Asset; quote_asset:Asset; timestamp:nat64; rate:nat64; metadata:ExchangeRateMetadata};
type OtherError=record {code:nat32; description:text};
type ExchangeRateError=variant {AnonymousPrincipalNotAllowed; Pending; CryptoBaseAssetNotFound; CryptoQuoteAssetNotFound; StablecoinRateNotFound; StablecoinRateTooFewRates; StablecoinRateZeroRate; ForexInvalidTimestamp; ForexBaseAssetNotFound; ForexQuoteAssetNotFound; ForexAssetsNotFound; RateLimited; NotEnoughCycles; InconsistentRatesReceived; Other:OtherError};
type GetExchangeRateResult=variant {Ok:ExchangeRate; Err:ExchangeRateError};
type MockXrcInit=record{rates:vec record{text;nat64}};
service : (MockXrcInit) -> {get_exchange_rate:(GetExchangeRateRequest)->(GetExchangeRateResult);set_exchange_rate:(text,text,nat64)->()};
'''

MGMT_DID = '''
type Settings=record { controllers:opt vec principal; compute_allocation:opt nat;
 memory_allocation:opt nat; freezing_threshold:opt nat };
type ChunkHash=record {hash:blob};
type UpgradeOptions=record {skip_pre_upgrade:opt bool;wasm_memory_persistence:opt variant {keep;replace}};
service:{
 provisional_create_canister_with_cycles:(record {amount:opt nat;settings:opt Settings;
 specified_id:opt principal;sender_canister_version:opt nat64})->(record{canister_id:principal});
 install_code:(record {mode:variant{install;reinstall;upgrade:opt UpgradeOptions};canister_id:principal;
 wasm_module:blob;arg:blob;sender_canister_version:opt nat64})->();
 upload_chunk:(record {canister_id:principal;chunk:blob})->(ChunkHash);
 clear_chunk_store:(record {canister_id:principal})->();
 install_chunked_code:(record {mode:variant{install;reinstall;upgrade:opt UpgradeOptions};
 target_canister:principal;store_canister:opt principal;chunk_hashes_list:vec ChunkHash;
 wasm_module_hash:blob;arg:blob;sender_canister_version:opt nat64})->();
}
'''


class RailsError(RuntimeError):
    pass


class RailsDriver(Driver):
    """Driver with immutable ingress evidence and explicit local-only guards."""

    # Keep each upload ingress comfortably below PocketIC's 2 MiB HTTP request
    # limit after Candid and JSON/base64 overhead are applied.
    INSTALL_CHUNK_SIZE = 700_000

    def __init__(self, args):
        # Driver creates the private evidence directory and sets deterministic
        # test principals.  It does not know about any production identity.
        super().__init__(args)
        # The base harness has the legacy two-method management DID.  Replace
        # only this run's private definition with the chunk-store methods used
        # for large local candidate Wasms.
        self.mgmt_did.write_text(MGMT_DID)
        self.ingress = self.out / "ingress"
        self.ingress.mkdir()
        self.xrc_did = self.out / "xrc.did"
        self.xrc_did.write_text(XRC_DID)
        self.backend_id = None
        self.pool_id = None
        self.flaky_id = None
        self.xrc_id = None
        self.ledgers = {}
        self.sequence = 100
        self._write_input_manifest()

    def _write_input_manifest(self):
        names = ["backend_wasm", "backend_did", "threepool_wasm", "threepool_did",
                 "sp_wasm", "sp_did", "ledger_wasm", "ledger_did", "flaky_wasm",
                 "flaky_did", "xrc_wasm", "decoder", "didc", "pocketic"]
        if self.args.recovery_fixture_helper:
            names += ["recovery_fixture_helper", "recovery_fixture_source"]
        if self.args.no_admission_fixture_helper:
            names.append("no_admission_fixture_helper")
        inputs = {}
        for name in names:
            value = Path(getattr(self.args, name))
            if not value.is_file():
                raise RailsError(f"missing explicit input {name}: {value}")
            digest = hashlib.sha256(value.read_bytes()).hexdigest()
            inputs[name] = {"path": str(value), "sha256": digest, "size": value.stat().st_size}
        harness_sources = {}
        for source_name in ["sp_rails_runtime.py", "sp_reopening_runtime.py", "sp_rails_lp_fixture.py", "sp_rails_aged_backend_fixture.py", "sp_rails_aged_pool_fixture.py", "sp_rails_no_admission_fixture.py"]:
            source_path=Path(__file__).with_name(source_name)
            if not source_path.is_file():
                raise RailsError(f"missing required harness source: {source_path}")
            source_bytes=source_path.read_bytes()
            archive_path=self.out/(source_name+".source")
            archive_path.write_bytes(source_bytes)
            harness_sources[source_name]={"path":str(source_path.resolve()),"sha256":hashlib.sha256(source_bytes).hexdigest(),"size":len(source_bytes),"archive":str(archive_path)}
        self.manifest = {
            "claim": "R-05a",
            "harness_sources": harness_sources,
            "local_only": True,
            "production_calls": 0,
            "server": "127.0.0.1 only",
            "pocketic": str(self.args.pocketic),
            "inputs": inputs,
            "harness_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        }
        (self.out / "inputs.json").write_text(json.dumps(self.manifest, indent=2))

    def verify_harness_sources_unchanged(self):
        observed={}
        mismatches=[]
        for name,pin in self.manifest["harness_sources"].items():
            path=Path(pin["path"])
            if not path.is_file():
                observed[name]={"missing":True}
                mismatches.append(name)
                continue
            value=path.read_bytes()
            current={"path":str(path.resolve()),"size":len(value),"sha256":hashlib.sha256(value).hexdigest()}
            observed[name]=current
            if current["size"]!=pin["size"] or current["sha256"]!=pin["sha256"]:
                mismatches.append(name)
        self._save_json_once(self.out/"harness-sources-final.json",{"unchanged":not mismatches,"mismatches":mismatches,"observed":observed})
        self.checks["harness_sources_unchanged"]=not mismatches
        if mismatches:
            raise RailsError("harness source changed during runtime: "+", ".join(mismatches))

    def _safe_name(self, method):
        return "".join(ch if ch.isalnum() or ch in "._-" else "_" for ch in method)

    def _save_json_once(self, path, value):
        if path.exists():
            raise RailsError(f"evidence path already exists; refusing overwrite: {path}")
        path.write_text(json.dumps(value, indent=2, sort_keys=True))

    def call(self, target, method, text="()", *, sender=None, did=None, query=False, effective=None):
        """Submit one ingress, save raw request/reply, and never retry it."""
        did = did or self.sp_did
        if not self.url or not self.url.startswith("http://127.0.0.1:"):
            raise RailsError(f"non-local PocketIC URL: {self.url!r}")
        self.serial += 1
        stem = f"{self.serial:05d}-{self._safe_name(method)}"
        payload = self.encode(text, did, method)
        raw_target = raw_principal(target)
        request = {"sender": b64(raw_principal(sender or self.admin)),
                   "canister_id": b64(raw_target), "method": method,
                   "payload": b64(payload),
                   "payload_hex": payload.hex(),
                   "effective_principal": effective or {"CanisterId": b64(raw_target)}}
        request_meta = {"query": query, "target": target, "sender": sender or self.admin,
                        "method": method, "local_url": self.url,
                        "request": request, "request_text": text,
                        "did": str(did)}
        self._save_json_once(self.ingress / f"{stem}.request.json", request_meta)
        started = time.time()
        if query:
            result = self.http(self.endpoint("read/query"), request)
        else:
            submitted = self.http(self.endpoint("update/submit_ingress_message"), request)
            self._save_json_once(self.ingress / f"{stem}.submit.json", submitted)
            if "Ok" not in submitted:
                self._save_json_once(self.ingress / f"{stem}.transport-reject.json", submitted)
                raise RailsError(f"local ingress submission rejected: {submitted}")
            message_id = submitted["Ok"]
            result = None
            for _ in range(600):
                self.http(self.endpoint("update/tick"), {"blockmakers": None})
                result = self.http(self.endpoint("read/ingress_status"),
                                   {"raw_message_id": message_id, "raw_caller": None})
                if result is not None:
                    break
            if result is None:
                raise RailsError(f"local ingress did not terminate: {stem}")
        status_path = self.ingress / f"{stem}.status.json"
        self._save_json_once(status_path, result)
        if "Ok" not in result:
            # A rejected ingress has no Candid reply bytes.  The PocketIC
            # ingress-status object is its exact local reply; retain it in the
            # same immutable receipt accounting as successful calls.
            record = {"serial": self.serial, "method": method, "target": target,
                      "sender": sender or self.admin, "query": query,
                      "elapsed_seconds": time.time() - started,
                      "request_file": str(self.ingress / f"{stem}.request.json"),
                      "reply_file": None, "status_file": str(status_path),
                      "accepted": False}
            self.calls.append(record)
            self._save_json_once(self.ingress / f"{stem}.receipt.json", record)
            raise RailsError(f"local query/update failed: {method}: {result}")
        raw_reply = base64.b64decode(result["Ok"])
        reply_path = self.ingress / f"{stem}.reply.bin"
        if reply_path.exists():
            raise RailsError(f"reply evidence already exists: {reply_path}")
        reply_path.write_bytes(raw_reply)
        decoded = self._decode_reply(raw_reply, did, method, stem)
        record = {"serial": self.serial, "method": method, "target": target,
                  "sender": sender or self.admin, "query": query,
                  "elapsed_seconds": time.time() - started, "accepted": True,
                  "request_file": str(self.ingress / f"{stem}.request.json"),
                  "reply_file": str(reply_path), "status_file": str(self.ingress / f"{stem}.status.json")}
        self.calls.append(record)
        self._save_json_once(self.ingress / f"{stem}.receipt.json", record)
        return decoded

    def _decode_reply(self, raw, did, method, stem):
        text_path = self.ingress / f"{stem}.reply.txt"
        p = subprocess.run([str(self.args.didc), "decode", "--defs", str(did), "--method", method],
                           input=raw.hex(), text=True, capture_output=True, timeout=60)
        if p.returncode:
            raise RailsError(f"Candid decode failed for {method}: {p.stderr}")
        text_path.write_text(p.stdout)
        typed_path = self.ingress / f"{stem}.typed.json"
        p = subprocess.run([str(self.args.decoder), str(did), method, str(text_path), str(typed_path)],
                           capture_output=True, text=True, timeout=60)
        if p.returncode:
            raise RailsError(f"typed decode failed for {method}: {p.stderr}")
        return json.loads(typed_path.read_text())

    def require_method(self, did, method):
        if method not in did.read_text():
            raise RailsError(f"{did} does not expose required generated method {method}")

    def _install_chunked(self, cid, wasm, args, mode="install"):
        """Install a raw Wasm module through the local management chunk store."""
        module = wasm.read_bytes()
        self.call("aaaaa-aa", "clear_chunk_store",
                  f'(record {{canister_id=principal "{cid}"}})', sender=self.anon,
                  did=self.mgmt_did,
                  effective={"CanisterId": b64(raw_principal(cid))})
        hashes = []
        for offset in range(0, len(module), self.INSTALL_CHUNK_SIZE):
            chunk = module[offset:offset + self.INSTALL_CHUNK_SIZE]
            value = self.call(
                "aaaaa-aa", "upload_chunk",
                f'(record {{canister_id=principal "{cid}";chunk={blob(chunk)}}})',
                sender=self.anon, did=self.mgmt_did,
                effective={"CanisterId": b64(raw_principal(cid))})
            try:
                digest_hex = value[0]["hash"]["blob_hex"]
                digest = bytes.fromhex(digest_hex)
            except (IndexError, KeyError, TypeError, ValueError) as error:
                raise RailsError(f"unexpected upload_chunk reply: {value}") from error
            hashes.append(digest)
        chunk_list = "vec {" + ";".join("record {hash=" + blob(digest) + "}" for digest in hashes) + "}"
        install_text = (
            f'(record {{mode=variant {{{mode}}};target_canister=principal "{cid}";'
            f'store_canister=opt principal "{cid}";chunk_hashes_list={chunk_list};'
            f'wasm_module_hash={blob(hashlib.sha256(module).digest())};arg={blob(args)};'
            'sender_canister_version=null})')
        return self.call("aaaaa-aa", "install_chunked_code", install_text,
                         sender=self.anon, did=self.mgmt_did,
                         effective={"CanisterId": b64(raw_principal(cid))})

    def install(self, cid, wasm, args):
        module_size = wasm.stat().st_size
        if module_size > self.INSTALL_CHUNK_SIZE:
            return self._install_chunked(cid, wasm, args)
        return super().install(cid, wasm, args)

    def upgrade(self, cid, wasm, args, *, fixture_skip_pre_upgrade=False):
        """Upgrade one local candidate while a durable rail is pending."""
        mode = "upgrade=opt record {skip_pre_upgrade=opt true;wasm_memory_persistence=null}" if fixture_skip_pre_upgrade else "upgrade=null"
        if fixture_skip_pre_upgrade and not (cid == self.sp or (self.args.no_admission and cid == self.backend_id)):
            raise RailsError("synthetic fixture skip_pre_upgrade is restricted to local SP setup or isolated no-admission backend setup")
        if wasm.stat().st_size > self.INSTALL_CHUNK_SIZE:
            return self._install_chunked(cid, wasm, args, mode=mode)
        module = wasm.read_bytes()
        if len(module) > 1500000 and not module.startswith(b"\x1f\x8b"):
            module = gzip.compress(module, compresslevel=9, mtime=0)
        text = ('(record { mode=variant {' + mode + '}; canister_id=principal "' + cid +
                '"; wasm_module=' + blob(module) + '; arg=' + blob(args) +
                '; sender_canister_version=null })')
        return self.call("aaaaa-aa", "install_code", text, sender=self.anon,
                         did=self.mgmt_did,
                         effective={"CanisterId": b64(raw_principal(cid))})

    def seed_completed_recovery_control_fixture(self, ids):
        """Synthetic local setup only; subsequent pending upgrades run normally.

        Import completed recovery CONTROL from an inspected actual state. Never
        replay packets or transplant production balances, receipts or journals.
        The offline helper proves all other local fields and virtual memories.
        """
        def observation():
            return {"user_position": self.pool_position(),
                    "pool_status": self.call(self.sp,"get_pool_status",query=True),
                    "physical": {token:{owner:self.qbalance(token,owner,token==ids["ckusdc"] or (token==ids["collateral"] and self.args.flaky_collateral))
                        for owner in [self.user,self.sp,self.backend_id,self.admin]} for token in ids.values()}}
        before=observation()
        sp_args=self.encode(f'(record {{protocol_canister_id=principal "{self.backend_id}";authorized_admins=vec {{principal "{self.admin}"}}}})',self.args.sp_did,types="(StabilityPoolInitArgs)")
        self.upgrade(self.sp,self.args.sp_wasm,sp_args)
        response=self.http(self.endpoint("read/get_stable_memory"),{"canister_id":b64(raw_principal(self.sp))})
        original=base64.b64decode(response["blob"],validate=True)
        original_path=self.out/"recovery-fixture-local-before.stable.bin"
        original_path.write_bytes(original)
        helper_out=self.out/"recovery-fixture-offline"
        run=subprocess.run([str(self.args.recovery_fixture_helper),str(original_path),str(self.args.recovery_fixture_source),str(helper_out)],capture_output=True,text=True,timeout=120)
        (self.out/"recovery-fixture-helper.stdout.txt").write_text(run.stdout)
        (self.out/"recovery-fixture-helper.stderr.txt").write_text(run.stderr)
        if run.returncode: raise RailsError("offline recovery fixture helper failed: "+run.stderr)
        receipt=json.loads((helper_out/"receipt.json").read_text())
        for key in ["only_recovery_control_changed","non_snapshot_virtual_memories_preserved","source_recovery_completed_live"]:
            self.check("synthetic_recovery_fixture_"+key,receipt.get(key) is True)
        patched=(helper_out/"patched-stable.bin").read_bytes()
        request=urllib.request.Request(self.url+"/blobstore/",data=patched,method="POST",headers={"Content-Type":"application/octet-stream"})
        with urllib.request.urlopen(request,timeout=120) as upload:
            blob_id=upload.read().decode().strip()
        self.http(self.endpoint("update/set_stable_memory"),{"canister_id":b64(raw_principal(self.sp)),"blob_id":b64(bytes.fromhex(blob_id))})
        self.upgrade(self.sp,self.args.sp_wasm,sp_args,fixture_skip_pre_upgrade=True)
        recovery=self.result_ok(self.call(self.sp,"get_interest_recovery_status",query=True))
        self.check("synthetic_recovery_fixture_live",bool(recovery and recovery["Some"]["live_interest_only"]))
        after=observation()
        # get_pool_status includes the deliberate recovery control change on
        # some source versions. Financial state is independently exact below.
        self.check("synthetic_recovery_fixture_position_physical_unchanged",before["user_position"]==after["user_position"] and before["physical"]==after["physical"])
        self._save_json_once(self.out/"synthetic-recovery-fixture.json",{"synthetic_local_fixture":True,"production_state_imported_only_for_recovery_control":True,"no_recovery_replay":True,"setup_upgrades":2,"actual_pending_upgrades_skip_pre_upgrade":False,"helper_receipt":receipt,"before":before,"after":after,"recovery_status":recovery,"original_stable_sha256":hashlib.sha256(original).hexdigest(),"patched_stable_sha256":hashlib.sha256(patched).hexdigest()})

    def result_ok(self, value):
        if not (isinstance(value, list) and len(value) == 1 and "Ok" in value[0]):
            raise RailsError(f"expected Ok result, got {value}")
        return value[0]["Ok"]

    def result_err(self, value, names=()):
        if not (isinstance(value, list) and len(value) == 1 and "Err" in value[0]):
            raise RailsError(f"expected Err result, got {value}")
        if names and not any(name in value[0]["Err"] for name in names):
            raise RailsError(f"unexpected error {value}; expected one of {names}")
        return value[0]["Err"]

    def qbalance(self, ledger, owner, flaky=False):
        did = self.args.flaky_did if flaky else self.args.ledger_did
        value = self.call(ledger, "icrc1_balance_of", f"({account(owner)})", did=did, query=True)
        return nat(value[0])

    def pool_position(self, owner=None):
        value = self.call(self.sp, "get_user_position",
                          f'(opt principal "{owner or self.user}")', query=True)
        return value[0]["Some"] if value[0] is not None else None

    def pool_book(self, ledger, owner=None):
        position = self.pool_position(owner)
        if not position:
            return 0
        for item in position.get("stablecoin_balances", []):
            if item["0"]["principal"] == ledger:
                return nat(item["1"])
        return 0

    def approve(self, ledger, spender, owner, flaky=False, amount=10**18):
        did = self.args.flaky_did if flaky else self.args.ledger_did
        text = (f'(record {{spender={account(spender)}; amount={amount}; '
                'expected_allowance=null; expires_at=null; fee=null; memo=null; '
                'from_subaccount=null; created_at_time=null})')
        return self.result_ok(self.call(ledger, "icrc2_approve", text, sender=owner, did=did))

    def install_ledger(self, name, ledger, symbol, decimals, fee, minting, flaky=False):
        did = self.args.flaky_did if flaky else self.args.ledger_did
        if flaky:
            self.install(ledger, self.args.flaky_wasm, bytes.fromhex("4449444c0000"))
            self.call(ledger, "set_fee", f"({fee} : nat)", did=did)
            self.call(ledger, "mint", f"({account(self.user)},{10**12} : nat)", did=did)
            self.ledgers[name] = {"id": ledger, "symbol": symbol, "decimals": decimals,
                                  "fee": fee, "flaky": True}
            return
        # The official ICRC-1 ledger rejects an initial balance for its
        # minting account.  Seed only ordinary accounts; the minting account
        # is independently configured for each token custody model.
        initial = "; ".join("record {" + account(owner) + "; " + str(10**14) + " : nat}"
                            for owner in [self.user, self.other, self.admin] if owner != minting)
        text = ('variant { Init=record { minting_account=' + account(minting) +
                '; fee_collector_account=null; transfer_fee=' + str(fee) +
                '; decimals=opt ' + str(decimals) + '; max_memo_length=opt 32; token_name="Local ' + symbol +
                '"; token_symbol="' + symbol + '"; metadata=vec {}; initial_balances=vec {' + initial +
                '}; feature_flags=opt record {icrc2=true}; maximum_number_of_accounts=null; '
                'accounts_overflow_trim_quantity=null; archive_options=record {num_blocks_to_archive=10000; '
                'max_transactions_per_response=opt 1000; trigger_threshold=20000; max_message_size_bytes=null; '
                'cycles_for_archive_creation=null; node_max_memory_size_bytes=null; controller_id=principal "' +
                self.admin + '"; more_controller_ids=null} } }')
        text = "(" + text + ")"
        self.install(ledger, self.args.ledger_wasm,
                     self.encode(text, self.args.ledger_did, types="(LedgerArg)"))
        self.ledgers[name] = {"id": ledger, "symbol": symbol, "decimals": decimals,
                              "fee": fee, "flaky": False}

    def register_stable(self, ledger, symbol, decimals, priority, lp=False, pool=None, fee=None):
        transfer_fee = "null" if fee is None else f"opt {fee}"
        is_lp = "opt true" if lp else "null"
        underlying = f'opt principal "{pool}"' if pool else "null"
        text = (f'(record {{ledger_id=principal "{ledger}"; symbol="{symbol}"; decimals={decimals}; '
                f'priority={priority}; is_active=true; transfer_fee={transfer_fee}; '
                f'is_lp_token={is_lp}; underlying_pool={underlying}}})')
        return self.result_ok(self.call(self.sp, "register_stablecoin", text))

    def start_conversion(self, token, principal_raw, budget, min_lp=0, seq=None, owner=None):
        seq = self.next_conversion_seq(owner) if seq is None else seq
        request = (f'(record {{intent_seq={seq} : nat64; token_ledger=principal "{token}"; '
                   f'principal_raw={principal_raw} : nat64; max_fee_budget_raw={budget} : nat64; '
                   f'min_lp={min_lp} : nat}})')
        value = self.call(self.sp, "start_3usd_conversion", request, sender=owner or self.user)
        return seq, value

    def next_conversion_seq(self, owner=None):
        value = self.call(self.sp, "get_3usd_conversion_next_seq", sender=owner or self.user, query=True)
        return nat(value[0]["Ok"] if isinstance(value[0], dict) and "Ok" in value[0] else value[0])

    def conversion_status(self, seq, owner=None):
        value = self.call(self.sp, "get_3usd_conversion_status", f"({seq} : nat64)", query=True,
                          sender=owner or self.user)
        return self.result_ok(value)

    def resume_conversion(self, seq, owner=None):
        return self.call(self.sp, "resume_3usd_conversion", f"({seq} : nat64)", sender=owner or self.user)

    def settle_conversion(self, seq, owner=None, limit=20):
        status = self.conversion_status(seq, owner)
        for _ in range(limit):
            if not status or "Pending" not in status:
                return status
            self.resume_conversion(seq, owner)
            status = self.conversion_status(seq, owner)
        raise RailsError(f"conversion {seq} remained pending: {status}")

    def advance_local_time(self,seconds):
        before=self.http(self.endpoint("read/get_time"),method="GET")
        after={"nanos_since_epoch":int(before["nanos_since_epoch"])+seconds*1_000_000_000}
        self.http(self.endpoint("update/set_time"),after)
        for _ in range(10):self.http(self.endpoint("update/tick"),{"blockmakers":None})
        seq=getattr(self,"time_step_seq",0)+1
        self.time_step_seq=seq
        self._save_json_once(self.out/f"local-time-{seq:03d}.json",{"before":before,"after":after,"seconds":seconds,"local_only":True})
        return {"before":before,"after":after,"seconds":seconds}

    def ordinary_request_id(self, vault_id, nonce):
        return hashlib.sha256(
            b"RUMI-SP-ORDINARY-LIQUIDATION-V1" + vault_id.to_bytes(8, "big") + nonce.to_bytes(8, "big")
        ).digest()

    def find_ordinary_status(self, vault_id, max_nonce=32):
        """Find the durable request for a vault without assuming a retry count."""
        for nonce in range(1, max_nonce + 1):
            request_id = self.ordinary_request_id(vault_id, nonce)
            value = self.call(self.sp, "get_ordinary_liquidation_status",
                              f"({blob(request_id)})", query=True)[0]
            if value is not None and "Some" in value and nat(value["Some"]["vault_id"]) == vault_id:
                return nonce, request_id, value["Some"]
        return None

    def configure(self):
        """Create/install all candidate canisters and activate V2 on fresh state."""
        self.xrc_id = self.create()
        icusd = self.create(); ckusdt = self.create(); flaky = self.create(); collateral = self.create()
        self.pool_id = self.create(); self.backend_id = self.create(); self.sp = self.create()
        self.flaky_id = flaky
        xrc_init = ('(record {rates=vec {record {"ICP/USD"; 1000000000 : nat64}; '
                    'record {"USDT/USD"; 100000000 : nat64}; record {"USDC/USD"; 100000000 : nat64}}})')
        self.install(self.xrc_id, self.args.xrc_wasm,
                     self.encode(xrc_init, self.xrc_did, types="(MockXrcInit)"))
        self.install_ledger("icUSD", icusd, "icUSD", 8, 100000, self.backend_id)
        self.install_ledger("ckUSDT", ckusdt, "ckUSDT", 6, 1000, self.other)
        # The flaky ICRC ledger is the third pool coin and is only used for
        # controlled callback loss.  icUSD and ckUSDT remain official ledger
        # Wasms, so the successful rail always includes real ICRC ledgers.
        self.install_ledger("ckUSDC", flaky, "ckUSDC", 6, 1000, self.backend_id, flaky=True)
        self.install_ledger("ICP", collateral, "ICP", 8, 10000, self.other, flaky=self.args.flaky_collateral)
        pool_tokens=[(icusd,"icUSD",8),(ckusdt,"ckUSDT",6),(flaky,"ckUSDC",6)]
        if self.args.aged_pool:
            # An abandoned refund requires a committed flaky input before a
            # later official-token leg whose allowance can fail definitively.
            pool_tokens=[pool_tokens[0],pool_tokens[2],pool_tokens[1]]
        token_configs=";".join(f'record {{ledger_id=principal "{ledger}";symbol="{symbol}";decimals={decimals};precision_mul={10**(18-decimals)}}}' for ledger,symbol,decimals in pool_tokens)
        pool_init=f'(record {{tokens=vec {{{token_configs}}};initial_a=100;swap_fee_bps=4;admin_fee_bps=0;admin=principal "{self.admin}"}})'
        self.install(self.pool_id, self.args.threepool_wasm,
                     self.encode(pool_init, self.args.threepool_did, types="(ThreePoolInitArgs)"))
        backend_init = ('variant {Init=record {xrc_principal=principal "' + self.xrc_id +
                        '"; icusd_ledger_principal=principal "' + icusd +
                        '"; icp_ledger_principal=principal "' + collateral +
                        '"; fee_e8s=1000000; developer_principal=principal "' + self.admin +
                        '"; treasury_principal=opt principal "' + self.admin +
                        '"; stability_pool_principal=opt principal "' + self.sp +
                        '"; ckusdt_ledger_principal=opt principal "' + ckusdt +
                        '"; ckusdc_ledger_principal=opt principal "' + flaky + '"}}')
        backend_init = "(" + backend_init + ")"
        self.install(self.backend_id, self.args.backend_wasm,
                     self.encode(backend_init, self.args.backend_did, types="(ProtocolArg)"))
        sp_init = (f'(record {{protocol_canister_id=principal "{self.backend_id}"; '
                   f'authorized_admins=vec {{principal "{self.admin}"}}}})')
        self.install(self.sp, self.args.sp_wasm,
                     self.encode(sp_init, self.args.sp_did, types="(StabilityPoolInitArgs)"))
        self.result_ok(self.call(self.pool_id,"set_liquidity_receipt_client",f'(opt principal "{self.sp}")',did=self.args.threepool_did))
        receipt_client=self.call(self.pool_id,"get_liquidity_receipt_client",did=self.args.threepool_did,query=True)[0]
        self.check("threepool_trusted_conversion_client_configured",receipt_client is not None and receipt_client["Some"]["principal"]==self.sp)
        self.result_ok(self.call(self.backend_id, "set_three_pool_canister",
                                 f'(principal "{self.pool_id}")', did=self.args.backend_did))
        self.result_ok(self.call(self.backend_id, "set_vault_check_tick_interval_secs", "(86400 : nat64)", did=self.args.backend_did))
        self.register_stable(icusd, "icUSD", 8, 1, fee=100000)
        self.register_stable(ckusdt, "ckUSDT", 6, 2, fee=1000)
        self.register_stable(flaky, "ckUSDC", 6, 3, fee=1000)
        self.register_stable(self.pool_id, "3USD", 8, 4, lp=True, pool=self.pool_id, fee=0)
        self.result_ok(self.call(self.sp, "register_collateral",
                                 f'(record {{ledger_id=principal "{collateral}"; symbol="ICP"; decimals=8; status=variant {{Active}}}})'))
        self.result_ok(self.call(self.sp, "update_pool_configuration",
                                 f'(record {{min_deposit_e8s=10000000; max_liquidations_per_batch=32; '
                                 f'emergency_pause=false; authorized_admins=vec {{principal "{self.admin}"}}}})'))
        # V2 activation is proven from a fresh NoLegacyExposure state by the
        # registered SP acknowledgement followed by the admin enable call.
        # The SP admin wrapper performs the real inter-canister callback to the
        # backend.  An ingress whose sender is merely the SP principal would
        # not exercise that production call boundary.
        self.result_ok(self.call(self.sp, "admin_acknowledge_three_usd_reserve_v2_client",
                                 sender=self.admin))
        self.result_ok(self.call(self.backend_id, "set_three_usd_reserve_ingress_enabled",
                                 "(true)", did=self.args.backend_did))
        enabled = self.call(self.backend_id, "get_three_usd_reserve_ingress_enabled",
                            did=self.args.backend_did, query=True)
        self.check("fresh_v2_activation", bool(enabled[0]))
        for info in self.ledgers.values():
            self.approve(info["id"], self.sp, self.user, flaky=info["flaky"])
            self.approve(info["id"], self.pool_id, self.user, flaky=info["flaky"])
        self.approve(collateral, self.backend_id, self.user)
        # Seed all three pool coins via the real pool ingress; this gives the
        # conversion quote a nonempty invariant and an independently observed LP supply.
        pool_amounts = f"vec {{100000000000 : nat; 1000000000 : nat; 1000000000 : nat}}"
        self.result_ok(self.call(self.pool_id, "add_liquidity", f"({pool_amounts},0 : nat)", sender=self.user,
                                 did=self.args.threepool_did))
        self.check("pool_seeded", nat(self.call(self.pool_id, "get_pool_status", did=self.args.threepool_did, query=True)[0]["lp_total_supply"]) > 0)
        # The LP registry is added after SP initialization. Its normal 300s
        # virtual-price timer must observe the now-seeded pool before LP-only
        # coverage can be valued. No vault exists yet, so this advances no debt.
        self.advance_local_time(301)
        # Keep ordinary liquidation separate from conversion escrow: seed a
        # caller position through the durable deposit ingress so the SP has
        # spendable icUSD and ckUSDT books when the backend notifies it.
        self.result_ok(self.call(self.sp, "deposit_with_intent",
                                 f'(1 : nat64, principal "{icusd}", 10000000000 : nat64)', sender=self.user))
        self.result_ok(self.call(self.sp, "deposit_with_intent",
                                 f'(2 : nat64, principal "{ckusdt}", 10000000000 : nat64)', sender=self.user))
        self.check("ordinary_books_seeded", self.pool_book(icusd) >= 10000000000 and
                   self.pool_book(ckusdt) >= 10000000000)
        self.deposit_seq=2
        return {"icusd": icusd, "ckusdt": ckusdt, "ckusdc": flaky, "collateral": collateral}

    def check_threepool_receipt_admission(self, ids):
        """Invalid public requests must leave no receipt, active row or spend."""
        def pool_status():
            return self.call(self.pool_id,"get_pool_status",did=self.args.threepool_did,query=True)[0]
        def absent(owner,intent,label):
            row=self.call(self.pool_id,"get_liquidity_receipt_v1",f"({blob(intent)})",sender=owner,did=self.args.threepool_did,query=True)[0]
            active=self.call(self.pool_id,"get_active_liquidity_receipt_v1",sender=owner,did=self.args.threepool_did,query=True)[0]
            self.check(label+"_no_durable_or_active_receipt",row is None and active is None)
        before=pool_status()
        wallet=self.qbalance(ids["icusd"],self.admin)
        intent=hashlib.sha256(b"public-no-allowance-runtime").digest()
        request=f'(record {{source_subaccount=null;min_lp=0 : nat;amounts=vec {{100000000 : nat;0 : nat;0 : nat}};max_transfer_fee=opt 100000 : opt nat;intent_id={blob(intent)}}})'
        rejected=self.call(self.pool_id,"add_liquidity_with_receipt_v1",request,sender=self.admin,did=self.args.threepool_did)
        self.result_err(rejected,("InvalidRequest",))
        absent(self.admin,intent,"threepool_no_allowance")
        self.check("threepool_no_allowance_no_value_movement",pool_status()==before and self.qbalance(ids["icusd"],self.admin)==wallet)
        empty_owner=principal(bytes([85])*28+bytes([2]))
        empty_intent=hashlib.sha256(b"public-no-funds-runtime").digest()
        empty_request=request.replace(blob(intent),blob(empty_intent))
        empty=self.call(self.pool_id,"add_liquidity_with_receipt_v1",empty_request,sender=empty_owner,did=self.args.threepool_did)
        self.result_err(empty,("InvalidRequest",))
        absent(empty_owner,empty_intent,"threepool_no_funds")
        self.check("threepool_no_funds_no_value_movement",pool_status()==before and self.qbalance(ids["icusd"],empty_owner)==0)
        self.result_ok(self.call(self.pool_id,"add_liquidity","(vec {10000000 : nat;0 : nat;0 : nat},0 : nat)",sender=self.user,did=self.args.threepool_did))
        self.result_ok(self.call(self.pool_id,"swap","(0 : nat8,1 : nat8,10000000 : nat,0 : nat)",sender=self.user,did=self.args.threepool_did))
        self.check("threepool_invalid_requests_leave_unrelated_add_and_swap_live",True)
        # Read-only admission checks every nonzero leg before the first pull.
        self.approve(ids["icusd"],self.pool_id,self.admin)
        before=pool_status();wallet=self.qbalance(ids["icusd"],self.admin)
        intent2=hashlib.sha256(b"public-later-leg-no-allowance-runtime").digest()
        request2=f'(record {{source_subaccount=null;min_lp=0 : nat;amounts=vec {{100000000 : nat;1000000 : nat;0 : nat}};max_transfer_fee=opt 100000 : opt nat;intent_id={blob(intent2)}}})'
        partial=self.call(self.pool_id,"add_liquidity_with_receipt_v1",request2,sender=self.admin,did=self.args.threepool_did)
        self.result_err(partial,("InvalidRequest",))
        absent(self.admin,intent2,"threepool_later_no_allowance")
        self.check("threepool_later_invalid_leg_never_pulls_first_leg",pool_status()==before and self.qbalance(ids["icusd"],self.admin)==wallet)
        # The invalid request did not pin its ID. Once funded it can execute
        # normally under the same payload and receipt identity.
        self.approve(ids["ckusdt"],self.pool_id,self.admin)
        completed=self.result_ok(self.call(self.pool_id,"add_liquidity_with_receipt_v1",request2,sender=self.admin,did=self.args.threepool_did))
        self.check("threepool_preflight_failure_did_not_pin_intent_id","Completed" in completed["status"])
        before_replay=pool_status()
        replay=self.result_ok(self.call(self.pool_id,"recover_liquidity_receipt_for_v1",f'(principal "{self.admin}",{blob(intent2)})',sender=self.other,did=self.args.threepool_did))
        self.check("threepool_permissionless_recovery_same_terminal_receipt",replay==completed and pool_status()==before_replay)

    def check_conversion(self, ids):
        icusd, ckusdt, ckusdc = ids["icusd"], ids["ckusdt"], ids["ckusdc"]
        self.require_method(self.args.sp_did, "start_3usd_conversion")
        self.require_method(self.args.sp_did, "resume_3usd_conversion")
        self.require_method(self.args.sp_did, "get_3usd_conversion_status")
        quote = self.call(self.sp, "quote_3usd_conversion",
                          f'(principal "{icusd}",2000000000 : nat64,0 : nat)')
        quote_ok = self.result_ok(quote)
        quoted_lp = nat(quote_ok["quoted_lp"])
        self.check("quote_8_decimal_real_ledger", quoted_lp > 0 and
                   nat(quote_ok["live_transfer_fee_raw"]) == 100000)
        too_high = self.call(self.sp, "quote_3usd_conversion",
                             f'(principal "{icusd}",2000000000 : nat64,{quoted_lp + 1} : nat)')
        self.result_err(too_high, ("InvalidRequest", "AmountTooLow"))
        self.check("min_lp_early_reject", True)
        rejected_seq=self.next_conversion_seq()
        rejected_wallet=self.qbalance(icusd,self.user)
        _,bad_start=self.start_conversion(icusd,2000000000,500000,quoted_lp+1,seq=rejected_seq)
        self.result_err(bad_start,("InvalidRequest","AmountTooLow"))
        self.result_err(self.call(self.sp,"get_3usd_conversion_status",f"({rejected_seq} : nat64)",sender=self.user,query=True),("NotFound",))
        self.check("conversion_bad_quote_does_not_reserve_sequence_or_pull",self.next_conversion_seq()==rejected_seq and self.qbalance(icusd,self.user)==rejected_wallet)
        for invalid_owner,label in [(principal(bytes([85])*28+bytes([2])),"no_funds"),(self.admin,"no_sp_allowance")]:
            invalid_seq=self.next_conversion_seq(invalid_owner)
            before_invalid=self.qbalance(icusd,invalid_owner)
            _,unfunded=self.start_conversion(icusd,2000000000,500000,0,seq=invalid_seq,owner=invalid_owner)
            self.result_err(unfunded,("InvalidRequest",))
            self.result_err(self.call(self.sp,"get_3usd_conversion_status",f"({invalid_seq} : nat64)",sender=invalid_owner,query=True),("NotFound",))
            active=self.call(self.sp,"get_active_3usd_conversion_v1",sender=invalid_owner,query=True)[0]
            self.check("conversion_"+label+"_no_capacity_or_fund_reservation",active is None and self.next_conversion_seq(invalid_owner)==invalid_seq and self.qbalance(icusd,invalid_owner)==before_invalid)
        before_wallet = self.qbalance(icusd, self.user)
        before_lp_book = self.pool_book(self.pool_id)
        seq, _ = self.start_conversion(icusd, 2000000000, 500000, quoted_lp)
        final = self.settle_conversion(seq)
        self.check("conversion_8_decimal_completed", "Completed" in final)
        after_wallet = self.qbalance(icusd, self.user)
        self.check("conversion_8_decimal_exact_five_fees", before_wallet - after_wallet == 2000000000 + 5 * 100000)
        self.check("conversion_8_decimal_exact_lp_credit", self.pool_book(self.pool_id) - before_lp_book == nat(final["Completed"]["lp_minted"]))
        replay = self.start_conversion(icusd, 2000000000, 500000, quoted_lp, seq=seq)[1]
        self.check("conversion_duplicate_identity",
                   self.result_ok(replay) == self.conversion_status(seq) and self.qbalance(icusd, self.user) == after_wallet)
        quote6 = self.result_ok(self.call(self.sp, "quote_3usd_conversion",
                                           f'(principal "{ckusdt}",20000000 : nat64,0 : nat)'))
        self.check("quote_6_decimal_nonzero_fee", nat(quote6["live_transfer_fee_raw"]) == 1000 and nat(quote6["quoted_lp"]) > 0)
        before_wallet = self.qbalance(ckusdt, self.user)
        seq6, _ = self.start_conversion(ckusdt, 20000000, 5000, nat(quote6["quoted_lp"]))
        final6 = self.settle_conversion(seq6)
        self.check("conversion_6_decimal_completed", "Completed" in final6)
        self.check("conversion_6_decimal_exact_five_fees", before_wallet - self.qbalance(ckusdt, self.user) == 20000000 + 5 * 1000)
        # A very concentrated quote is a preflight rejection.  No escrow or
        # ledger call can occur before this check, so the wallet balance is the
        # no-movement oracle even though the endpoint is an update.
        before = self.qbalance(icusd, self.user)
        concentrated = self.call(self.sp, "quote_3usd_conversion",
                                 f'(principal "{icusd}",1000000000000000000 : nat64,0 : nat)')
        self.result_err(concentrated, ("InvalidRequest", "AmountTooLow"))
        self.check("concentration_early_reject_no_pull", self.qbalance(icusd, self.user) == before)
        wrong = self.call(self.sp, "get_3usd_conversion_status", f"({seq} : nat64)", sender=self.other, query=True)
        self.result_err(wrong, ("NotFound",))
        anonymous=self.call(self.sp,"get_3usd_conversion_status",f"({seq} : nat64)",sender=self.anon,query=True)
        self.result_err(anonymous,("Unauthorized",))
        self.check("conversion_other_owner_private_and_anonymous_rejected",True)
        conflict = self.start_conversion(icusd, 2000000001, 500000, 0, seq=seq)[1]
        self.result_err(conflict, ("IntentConflict",))
        self.check("conversion_request_conflict_rejected", True)
        # Fixture fault controls are exercised on the third pool coin.  Each
        # control is armed before exactly one attempt and the same sequence is
        # resumed; no fresh intent is allocated after an unknown reply.
        # The caller ingress is an ICRC-2 transfer_from from the SP.  Arm the
        # transfer_from-specific fault so the first pull commits and loses its
        # reply; resume must use this same sequence and escrow identity.
        self.call(ckusdc, "set_phantom_transfer_from_failures", "(1 : nat32)", did=self.args.flaky_did)
        seqf, _ = self.start_conversion(ckusdc, 1000000, 5000, 0)
        finalf = self.settle_conversion(seqf)
        self.check("lost_user_pull_retry_same_identity", "Completed" in finalf or "Refunded" in finalf)
        self.call(ckusdc, "set_phantom_approve_failures", "(1 : nat32)", did=self.args.flaky_did)
        seqa, _ = self.start_conversion(ckusdc, 1000000, 5000, 0)
        finala = self.settle_conversion(seqa)
        self.check("lost_approval_retry_same_identity", "Completed" in finala or "Refunded" in finala)
        # Stop at the ambiguous approval first, then arm transfer_from only
        # after the user pull landed. The next transfer_from is the pool pull.
        self.call(ckusdc, "set_phantom_approve_failures", "(1 : nat32)", did=self.args.flaky_did)
        seqp, pending_pool = self.start_conversion(ckusdc, 1000000, 5000, 0)
        self.check("pool_pull_fault_armed_after_user_pull", "Pending" in self.result_ok(pending_pool))
        self.call(ckusdc, "set_phantom_transfer_from_failures", "(1 : nat32)", did=self.args.flaky_did)
        self.resume_conversion(seqp)
        finalp = self.settle_conversion(seqp)
        self.check("lost_pool_pull_retry_same_identity", "Completed" in finalp or "Refunded" in finalp)
        # Force the compensation leg: the pool rejects after the SP ingress
        # pull, while the fixture commits the refund transfer and drops its
        # reply.  Unpause only after the original ingress has returned so the
        # retry can reconcile the exact refund identity.
        self.require_method(self.args.threepool_did, "set_paused")
        self.result_ok(self.call(self.pool_id, "set_paused", "(true)", did=self.args.threepool_did))
        self.call(ckusdc, "set_phantom_icrc1_failures", "(1 : nat32)", did=self.args.flaky_did)
        refund_wallet = self.qbalance(ckusdc, self.user, flaky=True)
        refund_lp_before=self.pool_book(self.pool_id)
        seqr, refund_started = self.start_conversion(ckusdc, 1000000, 5000, 0)
        # The configured client persists its exact Rejected receipt even when
        # the paused pool fails preflight. Compensation has therefore already
        # reached the actual refund callback loss before this start returns.
        refund_pending=self.result_ok(refund_started)
        self.check("lost_refund_reply_staged_before_upgrade","Pending" in refund_pending and "Refunding" in refund_pending["Pending"]["phase"])
        if self.args.upgrade_pending:
            self.upgrade(self.sp, self.args.sp_wasm, self.encode(f'(record {{protocol_canister_id=principal "{self.backend_id}";authorized_admins=vec {{principal "{self.admin}"}}}})', self.args.sp_did, types="(StabilityPoolInitArgs)"))
            retained=self.conversion_status(seqr)
            self.check("upgrade_pending_conversion_phase","Pending" in retained and "Refunding" in retained["Pending"]["phase"])
        self.result_ok(self.call(self.pool_id, "set_paused", "(false)", did=self.args.threepool_did))
        finalr = self.settle_conversion(seqr)
        self.check("lost_refund_reply_compensation_retry_same_identity", "Refunded" in finalr)
        self.check("refund_exact_once_including_actual_four_fees", refund_wallet - self.qbalance(ckusdc, self.user, flaky=True) == 4*1000)
        self.check("refund_compensation_no_lp_credit",self.pool_book(self.pool_id)==refund_lp_before)
        refund_after = self.qbalance(ckusdc, self.user, flaky=True)
        refund_lp_after=self.pool_book(self.pool_id)
        replay_refund = self.start_conversion(ckusdc, 1000000, 5000, 0, seq=seqr)[1]
        self.check("refund_terminal_replay_no_second_movement", self.result_ok(replay_refund) == finalr and self.qbalance(ckusdc, self.user, flaky=True) == refund_after and self.pool_book(self.pool_id)==refund_lp_after)

    def check_conversion_aged_proofs(self,ids):
        """Adopt real aged ICRC-3 blocks; never replace ambiguous identities."""
        token=ids["ckusdc"]; principal_raw=1000000;budget=5000;fee=1000
        results=[]
        def tip():
            return nat(self.call(token,"icrc3_get_blocks","(vec {record {start=0 : nat;length=0 : nat}})",did=self.args.flaky_did,query=True)[0]["log_length"])
        def money():
            return {"wallet":self.qbalance(token,self.user,True),"lp_book":self.pool_book(self.pool_id),"lp_physical":nat(self.call(self.pool_id,"get_lp_balance",f'(principal "{self.sp}")',did=self.args.threepool_did,query=True)[0]),"ledger_tip":tip()}
        def phase(status):
            return next(iter(status["Pending"]["phase"])) if "Pending" in status else next(iter(status))
        for leg in ["pull","approval","revoke","refund"]:
            before=money()
            control="set_phantom_transfer_from_failures" if leg=="pull" else "set_phantom_icrc1_failures" if leg=="refund" else "set_phantom_approve_failures"
            self.call(token,control,"(1 : nat32)",did=self.args.flaky_did)
            seq,started=self.start_conversion(token,principal_raw,budget,0)
            pending=self.result_ok(started)
            if leg=="revoke":
                self.check("aged_revoke_staged_at_approval",phase(pending)=="Approving")
                self.call(token,"set_phantom_approve_failures","(1 : nat32)",did=self.args.flaky_did)
                self.resume_conversion(seq)
                pending=self.conversion_status(seq)
            expected_phase={"pull":"Pulling","approval":"Approving","revoke":"Revoking","refund":"Refunding"}[leg]
            self.check("aged_"+leg+"_landed_reply_loss_observed",phase(pending)==expected_phase)
            candidate=tip()-1
            direct_before=self.call(token,"icrc3_get_blocks",f'(vec {{record {{start={candidate} : nat;length=1 : nat}}}})',did=self.args.flaky_did,query=True)[0]
            direct_blocks=direct_before["blocks"]
            self.check("aged_"+leg+"_direct_candidate_block_id",len(direct_blocks)==1 and nat(direct_blocks[0]["id"])==candidate)
            self._save_json_once(self.out/("aged-"+leg+"-candidate-before.json"),{"leg":leg,"intent_seq":seq,"owner":self.user,"token_ledger":token,"principal_raw":principal_raw,"fee_budget_raw":budget,"candidate_block":candidate,"direct_response":direct_before})
            self.upgrade(self.sp,self.args.sp_wasm,self.encode(f'(record {{protocol_canister_id=principal "{self.backend_id}";authorized_admins=vec {{principal "{self.admin}"}}}})',self.args.sp_did,types="(StabilityPoolInitArgs)"))
            self.check("aged_"+leg+"_pending_upgrade_preserved",phase(self.conversion_status(seq))==expected_phase)
            # Capture the exact persisted intent tuple from the ordinary
            # pre_upgrade snapshot; do not patch or reinterpret its identity.
            frozen_response=self.http(self.endpoint("read/get_stable_memory"),{"canister_id":b64(raw_principal(self.sp))})
            frozen_state=base64.b64decode(frozen_response["blob"],validate=True)
            frozen_path=self.out/("aged-"+leg+"-persisted-SP.stable.bin")
            frozen_path.write_bytes(frozen_state)
            frozen_pin={"path":str(frozen_path),"bytes":len(frozen_state),"sha256":hashlib.sha256(frozen_state).hexdigest(),"contains_exact_persisted_intent_and_transfer_tuples":True,"offline_only":True}
            self._save_json_once(self.out/("aged-"+leg+"-persisted-SP-pin.json"),frozen_pin)
            self.advance_local_time(2*86400)
            direct_after=self.call(token,"icrc3_get_blocks",f'(vec {{record {{start={candidate} : nat;length=1 : nat}}}})',did=self.args.flaky_did,query=True)[0]
            self.check("aged_"+leg+"_direct_frozen_block_unchanged",direct_after["blocks"]==direct_blocks)
            aged_reply=self.resume_conversion(seq)
            aged_status=self.conversion_status(seq)
            self.check("aged_"+leg+"_too_old_observed",phase(aged_status)==expected_phase and "TooOld" in str(aged_status))
            unchanged=money()
            method="reconcile_3usd_conversion_"+leg+"_v1"
            self.require_method(self.args.sp_did,method)
            anonymous=self.call(self.sp,method,f'({seq} : nat64,{candidate} : nat64)',sender=self.anon)
            self.result_err(anonymous,("Unauthorized",))
            wrong=self.call(self.sp,method,f'({seq} : nat64,0 : nat64)',sender=self.user)
            self.check("aged_"+leg+"_wrong_candidate_no_financial_effect",phase(self.conversion_status(seq))==expected_phase and money()==unchanged)
            adopted=self.call(self.sp,method,f'({seq} : nat64,{candidate} : nat64)',sender=self.user)
            final=self.settle_conversion(seq)
            if leg=="approval":
                self.check("aged_approval_expiry_returns_principal","Refunded" in final)
                self.check("aged_approval_exact_actual_fees",before["wallet"]-self.qbalance(token,self.user,True)==4*fee)
                self.check("aged_approval_no_lp_credit",self.pool_book(self.pool_id)==before["lp_book"])
            else:
                self.check("aged_"+leg+"_completed","Completed" in final)
                self.check("aged_"+leg+"_exact_five_fees",before["wallet"]-self.qbalance(token,self.user,True)==principal_raw+5*fee)
                minted=nat(final["Completed"]["lp_minted"])
                self.check("aged_"+leg+"_lp_credited_once",self.pool_book(self.pool_id)-before["lp_book"]==minted)
            after=money()
            replay=self.start_conversion(token,principal_raw,budget,0,seq=seq)[1]
            self.check("aged_"+leg+"_terminal_replay_no_movement",self.result_ok(replay)==final and money()==after)
            results.append({"leg":leg,"seq":seq,"candidate_block":candidate,"direct_candidate_block":direct_blocks[0],"frozen_SP_state":frozen_pin,"pending":pending,"aged_reply":aged_reply,"wrong_candidate_reply":wrong,"proof_reply":adopted,"final":final,"before":before,"after":after})
        (self.out/"aged-conversion-proofs.json").write_text(json.dumps(results,indent=2,sort_keys=True))

    def total_supply(self, ledger):
        return nat(self.call(ledger, "icrc1_total_supply", did=self.args.ledger_did, query=True)[0])

    def ordinary_snapshot(self, ids, stable, flaky=False):
        return {"approval_fee": nat(self.call(stable,"icrc1_fee",did=self.args.flaky_did if flaky else self.args.ledger_did,query=True)[0]),
                "sp_payment": self.qbalance(stable,self.sp,flaky),
                "backend_payment": self.qbalance(stable,self.backend_id,flaky),
                "book": self.pool_book(stable),
                "payment_supply": None if flaky else self.total_supply(stable),
                "sp_collateral": self.qbalance(ids["collateral"],self.sp),
                "backend_collateral": self.qbalance(ids["collateral"],self.backend_id),
                "treasury_collateral": self.qbalance(ids["collateral"],self.admin),
                "collateral_supply": self.total_supply(ids["collateral"])}

    def assert_ordinary_conservation(self, label, ids, stable, before, row, flaky=False):
        self.check(label+"_completed", "Completed" in row["phase"])
        quote = row["quote"]["Some"]
        # Completed status compacts away the approval tuple. The exact live
        # fee observed immediately before this fixture's execution remains
        # independently checked against physical, book, and supply deltas.
        approval_fee = nat(row["approval"]["Some"]["fee_raw"]) if row.get("approval") else before["approval_fee"]
        gross = nat(quote["gross_payment_raw"])
        principal_raw = nat(quote["principal_raw"])
        surcharge = nat(quote["surcharge_raw"])
        pullfee = nat(quote["pull_fee_raw"])
        net = nat(quote["payout_net_raw"]["Some"])
        after = self.ordinary_snapshot(ids,stable,flaky)
        self.check(label+"_gross_quote_exact", gross == principal_raw+surcharge+pullfee)
        self.check(label+"_physical_payment_exact", before["sp_payment"]-after["sp_payment"] == gross+approval_fee)
        self.check(label+"_book_debit_exact", before["book"]-after["book"] == gross+approval_fee)
        if stable == ids["icusd"]:
            self.check(label+"_icusd_burn_exact", before["payment_supply"]-after["payment_supply"] == principal_raw+surcharge+pullfee+approval_fee and after["backend_payment"] == before["backend_payment"])
        else:
            self.check(label+"_ckstable_principal_and_surcharge_received", after["backend_payment"]-before["backend_payment"] == principal_raw+surcharge)
            if not flaky:
                self.check(label+"_ckstable_fee_burn_exact", before["payment_supply"]-after["payment_supply"] == pullfee+approval_fee)
        self.check(label+"_collateral_receipt_exact", net > 0 and after["sp_collateral"]-before["sp_collateral"] == net)
        protocolcut = nat(quote["collateral_gross_raw"])-nat(quote["collateral_net_raw"])
        fee = nat(quote["collateral_fee_raw"])
        treasury_net = protocolcut-fee if protocolcut>fee else 0
        self.check(label+"_protocol_cut_net_exact", after["treasury_collateral"]-before["treasury_collateral"] == treasury_net)
        fee_burn = before["collateral_supply"]-after["collateral_supply"]
        self.check(label+"_collateral_fee_burn_exact", fee_burn == fee+(fee if protocolcut>fee else 0))
        self.check(label+"_collateral_physical_conserved", before["backend_collateral"]-after["backend_collateral"] == net+treasury_net+fee_burn)
        return after

    def local_collateral_policy(self, collateral, unsafe):
        """Use normal local admin setters to switch a 200%-CR vault unsafe.

        The observed backend price remains the installed XRC fixture's $10.
        This avoids pretending a mock setter invalidates the backend cache.
        """
        methods = [("set_collateral_borrow_threshold", 3.0),
                   ("set_collateral_liquidation_ratio", 2.5)] if unsafe else [
                   ("set_collateral_liquidation_ratio", 1.1),
                   ("set_collateral_borrow_threshold", 1.5)]
        for method, ratio in methods:
            self.result_ok(self.call(self.backend_id, method,
                           f'(principal "{collateral}",{ratio} : float64)', did=self.args.backend_did))

    def check_ordinary_liquidation(self, ids):
        """Run an ordinary liquidation through backend notification and SP.

        The backend owns the vault and price checks.  The test sends the
        notification with the actual backend canister principal, then inspects
        the SP's durable request/result and the official ledger deltas.  The
        local XRC fixture is the only price source; there is no test-only
        endpoint in the candidate Wasms.
        """
        collateral = ids["collateral"]
        self.require_method(self.args.sp_did, "notify_liquidatable_vaults")
        self.require_method(self.args.backend_did, "open_vault_and_borrow_v2")
        # User collateral is pulled by the backend and debt is minted to the
        # user; only the local official ledger is involved.
        open_arg = (f'(record {{collateral_amount=1000000000 : nat64; operation_id=1 : nat64; '
                    f'borrow_amount=5000000000 : nat64; collateral_type=opt principal "{collateral}"}})')
        opened = self.result_ok(self.call(self.backend_id, "open_vault_and_borrow_v2", open_arg,
                                          sender=self.user, did=self.args.backend_did))
        vault_id = nat(opened["vault_id"])
        self.vault_operation_seq=1
        self.result_ok(self.call(self.backend_id,"ack_vault_operation", "(record {operation_id=1 : nat64})", sender=self.user,did=self.args.backend_did))
        self.local_collateral_policy(collateral, True)
        self.check("local_policy_makes_200_percent_vault_unsafe", True)
        info = (f'vec {{record {{vault_id={vault_id}; collateral_type=principal "{collateral}"; '
                'debt_amount=5000000000; collateral_amount=1000000000; '
                'recommended_liquidation_amount=5000000000; collateral_price_e8s=1000000000}}')
        before_vaults = self.call(self.backend_id, "get_vaults",
                                  f'(opt principal "{self.user}")',
                                  did=self.args.backend_did, query=True)[0]
        before_debt = next(nat(v["borrowed_icusd_amount"]) for v in before_vaults
                           if nat(v["vault_id"]) == vault_id)
        money_before = self.ordinary_snapshot(ids,ids["ckusdt"])
        results = self.call(self.sp, "notify_liquidatable_vaults", "("+info+")", sender=self.backend_id)
        self.check("ordinary_liquidation_admitted", bool(results[0]) and results[0][0].get("success", False))
        wrong_caller = self.call(self.sp, "notify_liquidatable_vaults", "("+info+")", sender=self.other)
        self.check("ordinary_wrong_caller_rejected", wrong_caller == [[]])
        found = self.find_ordinary_status(vault_id)
        self.check("ordinary_durable_status_recorded", found is not None)
        if found is not None:
            _, request_id, row = found
            self.check("ordinary_selected_ckusdt", row["stable_ledger"]["principal"] == ids["ckusdt"])
            money_after = self.assert_ordinary_conservation("ordinary_ckusdt",ids,ids["ckusdt"],money_before,row)
            quote = row.get("quote")
            self.check("ordinary_quote_frozen", quote is not None and "Some" in quote and
                       nat(quote["Some"]["debt_e8s"]) > 0 and
                       nat(quote["Some"]["gross_payment_raw"]) >= nat(quote["Some"]["principal_raw"]))
            self.check("ordinary_capped_leg_quote_bounded", nat(quote["Some"]["debt_e8s"]) <= 5000000000)
            unknown_request = blob(bytes([255]) * 32)
            unknown_retry = self.call(self.sp, "retry_ordinary_liquidation",
                                      f"({unknown_request})", sender=self.other)
            self.result_err(unknown_retry, ("LiquidationFailed", "SystemBusy"))
            self.check("ordinary_unknown_request_rejected", True)
        after_vaults = self.call(self.backend_id, "get_vaults",
                                 f'(opt principal "{self.user}")',
                                 did=self.args.backend_did, query=True)[0]
        after_debt = next((nat(v["borrowed_icusd_amount"]) for v in after_vaults
                           if nat(v["vault_id"]) == vault_id), 0)
        self.check("ordinary_backend_debt_reduced_exactly", before_debt-after_debt == nat(found[2]["quote"]["Some"]["debt_e8s"]))
        # Retry the exact retained request, rather than allocating a new
        # liquidation notification against any remaining partial debt.
        retry_result = self.call(self.sp, "retry_ordinary_liquidation", f"({blob(request_id)})")
        self.result_ok(retry_result)
        self.check("ordinary_retained_id_replay_no_payment_or_payout", self.ordinary_snapshot(ids,ids["ckusdt"]) == money_after)
        replay_vaults = self.call(self.backend_id,"get_vaults",f'(opt principal "{self.user}")',did=self.args.backend_did,query=True)[0]
        replay_debt = next((nat(v["borrowed_icusd_amount"]) for v in replay_vaults if nat(v["vault_id"]) == vault_id),0)
        self.check("ordinary_retained_id_replay_no_second_debt_commit",replay_debt == after_debt)
        # Drain the selected ckUSDT book and open a second, sub-dust vault.
        # This forces the ordinary selector onto the 8-decimal icUSD book and
        # exercises the full-close dust branch with a distinct request ID.
        remaining_usdt = self.pool_book(ids["ckusdt"])
        if remaining_usdt:
            self.result_ok(self.call(self.sp, "withdraw",
                                     f'(principal "{ids["ckusdt"]}",{remaining_usdt} : nat64)',
                                     sender=self.user))
        self.check("ckstable_book_drained_for_icusd_leg", self.pool_book(ids["ckusdt"]) == 0)
        self.local_collateral_policy(collateral, False)
        open_arg2 = (f'(record {{collateral_amount=10000000 : nat64; operation_id=2 : nat64; '
                     f'borrow_amount=50000000 : nat64; collateral_type=opt principal "{collateral}"}})')
        opened2 = self.result_ok(self.call(self.backend_id, "open_vault_and_borrow_v2", open_arg2,
                                           sender=self.user, did=self.args.backend_did))
        vault_id2 = nat(opened2["vault_id"])
        self.vault_operation_seq=2
        self.result_ok(self.call(self.backend_id,"ack_vault_operation", "(record {operation_id=2 : nat64})", sender=self.user,did=self.args.backend_did))
        self.result_ok(self.call(self.backend_id, "set_min_icusd_amount", "(100000000 : nat64)", did=self.args.backend_did))
        self.local_collateral_policy(collateral, True)
        info2 = (f'vec {{record {{vault_id={vault_id2}; collateral_type=principal "{collateral}"; '
                 'debt_amount=50000000; collateral_amount=10000000; '
                 'recommended_liquidation_amount=50000000; collateral_price_e8s=1000000000}}')
        dust_before = self.ordinary_snapshot(ids,ids["icusd"])
        second_result = self.call(self.sp, "notify_liquidatable_vaults", "("+info2+")", sender=self.backend_id)
        self.check("ordinary_icusd_dust_leg_admitted", bool(second_result[0]) and second_result[0][0].get("success", False))
        found2 = self.find_ordinary_status(vault_id2)
        self.check("ordinary_icusd_quote_uses_8_decimals", found2 is not None and
                   found2[2]["stable_ledger"]["principal"] == ids["icusd"])
        if found2 is not None and found2[2].get("quote"):
            self.assert_ordinary_conservation("ordinary_icusd",ids,ids["icusd"],dust_before,found2[2])
            quote2 = found2[2]["quote"]["Some"]
            self.check("ordinary_dust_full_close_quote", nat(quote2["debt_e8s"]) == 50000000 and
                       nat(quote2["principal_raw"]) >= 50000000)

    def check_ordinary_fault_matrix(self, ids):
        """Exercise exact retry identity for payment and approval reply loss.

        The fixture's third coin is registered as ckUSDC and backed by a real
        ICRC-2 implementation with narrowly controlled phantom failures.  The
        backend still performs the ordinary quote and debt transition; only
        the ledger callback is made ambiguous.  Payout/debt-response faults
        require a separate inter-canister fault injector and are reported as
        an explicit limitation in the final report.
        """
        self.require_method(self.args.sp_did, "retry_ordinary_liquidation")
        ckusdc = ids["ckusdc"]
        self.result_ok(self.call(self.sp, "deposit_with_intent",
                                 f'(3 : nat64, principal "{ckusdc}", 10000000000 : nat64)', sender=self.user))

        self.deposit_seq=3

        def open_fault_vault(operation_id, debt):
            self.local_collateral_policy(ids["collateral"], False)
            arg = (f'(record {{collateral_amount=400000000 : nat64; operation_id={operation_id} : nat64; '
                   f'borrow_amount={debt} : nat64; collateral_type=opt principal "{ids["collateral"]}"}})')
            result = self.result_ok(self.call(self.backend_id, "open_vault_and_borrow_v2", arg,
                                              sender=self.user, did=self.args.backend_did))
            self.vault_operation_seq=operation_id
            self.result_ok(self.call(self.backend_id,"ack_vault_operation", f"(record {{operation_id={operation_id} : nat64}})", sender=self.user,did=self.args.backend_did))
            self.local_collateral_policy(ids["collateral"], True)
            return nat(result["vault_id"])

        def notify(vault_id, debt):
            info = (f'vec {{record {{vault_id={vault_id}; collateral_type=principal "{ids["collateral"]}"; '
                    f'debt_amount={debt}; collateral_amount=400000000; '
                    f'recommended_liquidation_amount={debt}; collateral_price_e8s=1000000000}}}}')
            return self.call(self.sp, "notify_liquidatable_vaults", "("+info+")", sender=self.backend_id)

        def settle_same_request(vault_id, request_id):
            for _ in range(8):
                current = self.find_ordinary_status(vault_id)
                if current and "Completed" in current[2]["phase"]:
                    return current[2]
                self.call(self.sp, "retry_ordinary_liquidation", f"({blob(request_id)})")
            raise RailsError("ordinary fault request did not settle under its retained identity")

        def vault_debt(vault_id):
            rows=self.call(self.backend_id,"get_vaults",f'(opt principal "{self.user}")',did=self.args.backend_did,query=True)[0]
            return next((nat(v["borrowed_icusd_amount"]) for v in rows if nat(v["vault_id"]) == vault_id),0)

        # The first transfer_from is a backend payment; normal vault ingress
        # uses a separate official collateral ledger and cannot consume it.
        vault_payment = open_fault_vault(3, 2000000000)
        before_payment = self.ordinary_snapshot(ids,ckusdc,True)
        debt_before = vault_debt(vault_payment)
        self.call(ckusdc, "set_phantom_transfer_from_failures", "(1 : nat32)", did=self.args.flaky_did)
        notify(vault_payment, 2000000000)
        found = self.find_ordinary_status(vault_payment)
        self.check("ordinary_payment_reply_loss_admitted", found is not None)
        _, request_id, pending = found
        self.check("ordinary_payment_reply_loss_observed_pending", "Completed" not in pending["phase"] and pending["backend_attempted"])
        pending_quote=pending["quote"]["Some"]
        self.check("ordinary_payment_landed_before_retry", self.qbalance(ckusdc,self.backend_id,True)-before_payment["backend_payment"] == nat(pending_quote["principal_raw"])+nat(pending_quote["surcharge_raw"]))
        if self.args.upgrade_pending:
            self.upgrade(self.sp,self.args.sp_wasm,self.encode(f'(record {{protocol_canister_id=principal "{self.backend_id}";authorized_admins=vec {{principal "{self.admin}"}}}})',self.args.sp_did,types="(StabilityPoolInitArgs)"))
            self.upgrade(self.backend_id,self.args.backend_wasm,self.encode('(variant {Upgrade=record {mode=null;description=null}})',self.args.backend_did,types="(ProtocolArg)"))
            self.check("upgrade_pending_ordinary_journals", self.find_ordinary_status(vault_payment)[1] == request_id)
        settled=settle_same_request(vault_payment,request_id)
        payment_after=self.assert_ordinary_conservation("ordinary_payment_reply_loss",ids,ckusdc,before_payment,settled,True)
        self.check("ordinary_payment_reply_loss_debt_once", debt_before-vault_debt(vault_payment) == nat(settled["quote"]["Some"]["debt_e8s"]))
        self.call(self.sp,"retry_ordinary_liquidation",f"({blob(request_id)})")
        self.check("ordinary_payment_reply_loss_terminal_replay", self.ordinary_snapshot(ids,ckusdc,True) == payment_after)

        # Approval commits and drops its reply before any backend payment.
        vault_approval = open_fault_vault(4, 2000000000)
        approval_before=self.ordinary_snapshot(ids,ckusdc,True)
        approval_debt_before=vault_debt(vault_approval)
        self.call(ckusdc, "set_phantom_approve_failures", "(1 : nat32)", did=self.args.flaky_did)
        notify(vault_approval, 2000000000)
        found = self.find_ordinary_status(vault_approval)
        self.check("ordinary_approval_reply_loss_admitted", found is not None)
        _, request_id, pending=found
        self.check("ordinary_approval_reply_loss_observed_pending", "Completed" not in pending["phase"] and pending["approval_attempted"])
        self.check("ordinary_approval_no_payment_before_retry", self.qbalance(ckusdc,self.backend_id,True) == approval_before["backend_payment"])
        settled=settle_same_request(vault_approval,request_id)
        approval_after=self.assert_ordinary_conservation("ordinary_approval_reply_loss",ids,ckusdc,approval_before,settled,True)
        self.check("ordinary_approval_reply_loss_debt_once", approval_debt_before-vault_debt(vault_approval) == nat(settled["quote"]["Some"]["debt_e8s"]))
        self.call(self.sp,"retry_ordinary_liquidation",f"({blob(request_id)})")
        self.check("ordinary_approval_reply_loss_terminal_replay", self.ordinary_snapshot(ids,ckusdc,True) == approval_after)

    def preflight_interfaces(self):
        for method in ["set_liquidity_receipt_client","get_liquidity_receipt_client","add_liquidity_with_receipt_client_v1"]:
            self.require_method(self.args.threepool_did,method)
        if self.args.no_admission:
            self.require_method(self.args.backend_did,"retire_stability_pool_liquidate_with_reserves_v2")
            self.require_method(self.args.backend_did,"stability_pool_cancel_liquidation_bound")
        for method in ["deposit_with_intent","admin_acknowledge_three_usd_reserve_v2_client","get_ordinary_liquidation_status"]:
            self.require_method(self.args.sp_did,method)
        if self.args.aged_conversion:
            for leg in ["pull","approval","revoke","refund"]:
                self.require_method(self.args.sp_did,"reconcile_3usd_conversion_"+leg+"_v1")
        if self.args.aged_pool:
            for method in ["recover_liquidity_receipt_for_v1","reconcile_liquidity_receipt_input_for_v1","reconcile_liquidity_receipt_refund_for_v1","get_active_liquidity_receipt_v1"]:
                self.require_method(self.args.threepool_did,method)
        if self.args.aged_backend:
            for leg in ["payment","protocol_fee","payout"]:
                self.require_method(self.args.backend_did,"stability_pool_reconcile_liquidation_"+leg)
        if not any([self.args.skip_receipt_regressions,self.args.aged_backend,self.args.aged_conversion,self.args.aged_pool]):
            self.require_method(self.args.threepool_did,"recover_liquidity_receipt_for_v1")

    def run(self):
        self.preflight_interfaces()
        self.start()
        ids = self.configure()
        if self.args.recovery_fixture_helper:
            self.seed_completed_recovery_control_fixture(ids)
        if self.args.no_admission:
            from sp_rails_no_admission_fixture import check_no_admission
            result=check_no_admission(self,ids)
            (self.out/"no-admission-proofs.json").write_text(json.dumps(result,indent=2,sort_keys=True))
            return
        if self.args.aged_backend:
            from sp_rails_aged_backend_fixture import check_aged_backend_proofs
            result=check_aged_backend_proofs(self,ids)
            (self.out/"aged-backend-proofs.json").write_text(json.dumps(result,indent=2,sort_keys=True))
            return
        if self.args.aged_pool:
            from sp_rails_aged_pool_fixture import check_aged_pool_receipts
            check_aged_pool_receipts(self,ids)
            return
        if self.args.aged_conversion:
            self.check_conversion_aged_proofs(ids)
            return
        if not self.args.skip_receipt_regressions:
            self.check_threepool_receipt_admission(ids)
        if not self.args.skip_conversion:
            self.check_conversion(ids)
        if not self.args.skip_ordinary:
            self.check_ordinary_liquidation(ids)
            if self.args.ordinary_fault_matrix:
                self.check_ordinary_fault_matrix(ids)
            from sp_rails_lp_fixture import check_threeusd_reserve_liquidation
            lp_result=check_threeusd_reserve_liquidation(self,ids)
            (self.out/"threeusd-liquidation.json").write_text(json.dumps(lp_result,indent=2,sort_keys=True))


def parse_args():
    p = argparse.ArgumentParser(description=__doc__)
    for name in ["backend", "threepool", "sp", "ledger", "flaky"]:
        p.add_argument(f"--{name}-wasm", type=Path, required=True)
        p.add_argument(f"--{name}-did", type=Path, required=True)
    p.add_argument("--xrc-wasm", type=Path, required=True)
    p.add_argument("--decoder", type=Path, required=True)
    p.add_argument("--didc", type=Path, default=Path.home() / ".cargo/bin/didc")
    p.add_argument("--pocketic", type=Path, default=Path.home() / ".cache/pocketic/9.0.3/pocket-ic")
    p.add_argument("--out", type=Path, required=True)
    p.add_argument("--recovery-fixture-helper",type=Path,help="Offline control-only stable State adapter; synthetic local upgrade fixture only.")
    p.add_argument("--recovery-fixture-source",type=Path,help="Inspected actual State Candid with completed recovery control.")
    p.add_argument("--flaky-collateral",action="store_true",help="Separate local fault fixture using mock ICP collateral.")
    p.add_argument("--aged-pool",action="store_true",help="Run independent aged pool proof cases only, with flaky token before an official token.")
    p.add_argument("--aged-conversion",action="store_true",help="Run independent aged conversion proof cases only.")
    p.add_argument("--aged-backend",action="store_true",help="Run independent aged backend proof cases only; requires --flaky-collateral.")
    p.add_argument("--no-admission",action="store_true",help="Isolated request-bound no-admission retirement regressions; setup-only synthetic journals are explicitly qualified.")
    p.add_argument("--no-admission-fixture-helper",type=Path,help="Offline backend AdmissionPending fixture adapter, accepted only with --no-admission.")
    p.add_argument("--skip-ordinary", action="store_true")
    p.add_argument("--skip-conversion", action="store_true",
                   help="Debug ordinary rails independently; this does not establish conversion readiness.")
    p.add_argument("--skip-receipt-regressions",action="store_true",help="Focused debug only: omit known old-candidate public receipt regression; never a full gate.")
    p.add_argument("--ordinary-fault-matrix", action="store_true",
                   help="Run local ckUSDC payment/approval reply-loss retries.")
    p.add_argument("--upgrade-pending", action="store_true",
                   help="Upgrade the SP candidate during a pending refund phase.")
    return p.parse_args()


def main():
    args = parse_args()
    for name in ["backend_wasm", "backend_did", "threepool_wasm", "threepool_did",
                 "sp_wasm", "sp_did", "ledger_wasm", "ledger_did", "flaky_wasm",
                 "flaky_did", "xrc_wasm", "decoder", "didc", "pocketic"]:
        path = Path(getattr(args, name)).absolute() if name == "pocketic" else Path(getattr(args, name)).resolve()
        if not path.is_file():
            raise SystemExit(f"missing input {name}: {path}")
        setattr(args, name, path)
    if bool(args.recovery_fixture_helper) != bool(args.recovery_fixture_source):
        raise SystemExit("both recovery fixture helper and source are required")
    if args.recovery_fixture_helper:
        for name in ["recovery_fixture_helper","recovery_fixture_source"]:
            value=Path(getattr(args,name)).resolve()
            if not value.is_file(): raise SystemExit("missing recovery fixture input "+name)
            setattr(args,name,value)
    if bool(args.no_admission_fixture_helper) != bool(args.no_admission):
        raise SystemExit("--no-admission requires its explicit fixture helper; helper is forbidden in other modes")
    if args.no_admission_fixture_helper:
        value=Path(args.no_admission_fixture_helper).resolve()
        if not value.is_file(): raise SystemExit("missing no-admission fixture helper")
        args.no_admission_fixture_helper=value
    if sum([args.aged_backend,args.aged_conversion,args.aged_pool,args.no_admission])>1:
        raise SystemExit("choose exactly one isolated aged suite per local instance")
    if args.aged_backend and not args.flaky_collateral:
        raise SystemExit("--aged-backend requires --flaky-collateral")
    version = subprocess.run([str(args.pocketic), "--version"], capture_output=True, text=True, check=True).stdout.strip()
    if version != "pocket-ic-server 9.0.3":
        raise SystemExit(f"expected local PocketIC 9.0.3, got {version}")
    driver = RailsDriver(args)
    failure = None
    try:
        driver.run()
    except Exception:
        failure = traceback.format_exc()
        print(failure, file=sys.stderr)
        (driver.out / "failure.txt").write_text(failure)
    finally:
        driver.close()
        try:
            driver.verify_harness_sources_unchanged()
        except Exception:
            changed_source=traceback.format_exc()
            failure=(failure+"\n" if failure else "")+changed_source
            with (driver.out/"failure.txt").open("a") as source_failure:
                source_failure.write(changed_source)
        report = {"passed": failure is None,
                  "claim": "R-05a",
                  "local_runtime_gate_passed": failure is None and all(driver.checks.values()),
                  "full_standard_runtime_coverage_passed": failure is None and all(driver.checks.values()) and not any([args.skip_conversion,args.skip_ordinary,args.skip_receipt_regressions,args.aged_backend,args.aged_conversion,args.aged_pool,args.no_admission]) and args.ordinary_fault_matrix and args.upgrade_pending,
                  "production_readiness_claimed": False, "local_only": True,
                  "production_calls": 0, "checks": driver.checks,
                  "server_version": version, "calls": len(driver.calls),
                  "coverage": {"no_admission_run":args.no_admission,
                               "synthetic_completed_recovery_control_seeded":bool(args.recovery_fixture_helper),
                               "actual_pending_upgrades_use_normal_pre_and_post_upgrade":True,
                               "aged_backend_run":args.aged_backend,
                               "aged_pool_run":args.aged_pool,
                               "aged_conversion_run":args.aged_conversion,
                               "flaky_collateral":args.flaky_collateral,
                               "public_receipt_regressions_run": not args.skip_receipt_regressions and not args.aged_backend and not args.aged_conversion and not args.aged_pool and not args.no_admission,
                               "conversion_run": not args.skip_conversion and not args.aged_backend and not args.aged_conversion and not args.aged_pool and not args.no_admission,
                               "ordinary_run": not args.skip_ordinary and not args.aged_backend and not args.aged_conversion and not args.aged_pool and not args.no_admission,
                               "ordinary_fault_matrix_requested": args.ordinary_fault_matrix,
                               "pending_conversion_upgrade_checked": driver.checks.get("upgrade_pending_conversion_phase", False)},
                  "command": sys.argv, "manifest": driver.manifest,
                  "limits": ["Candidate Wasms and generated DIDs are required inputs.",
                             "--skip-receipt-regressions is partial debug coverage only.",
                             "--skip-conversion is a partial ordinary-only debug run, never the full restoration gate.",
                             "Real local ICRC ledgers and a local XRC fixture prove local behavior only.",
                             "When recovery-fixture arguments are present, configure imports only completed recovery control into local state; this is synthetic fixture evidence, distinct from actual saved-state migration proof.",
                             "No deployed canister, network price, settlement, admission, or signing proof is produced.",
                             "Ordinary payment/approval reply-loss legs run only with --ordinary-fault-matrix.",
                             "Standard-suite pending upgrades run with --upgrade-pending; isolated aged proof suites include their own upgrade checks.",
                             "Standard ordinary faults cover payment and approval; isolated --aged-backend also covers protocol-fee and collateral-payout reply loss.",
                             "Whole-backend response loss after debt commit is not injected by the standard suite."]}
        (driver.out / "verification.json").write_text(json.dumps(report, indent=2))
        print(json.dumps(report, indent=2))
    return 0 if failure is None else 1


if __name__ == "__main__":
    sys.exit(main())
