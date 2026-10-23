#!/usr/bin/env python3
"""Bounded local regressions for durable rejection before financial dispatch.

The parent driver owns real candidate canisters, immutable ingress receipts and
the localhost-only PocketIC lifecycle. No production identity or URL is used.
"""
from __future__ import annotations

import hashlib
import base64
import json
import subprocess
import urllib.request
from typing import Any

from sp_reopening_runtime import account, b64, blob, nat, raw_principal


QUIET_TIMER_SECONDS = 100 * 86400


def _some(value: Any) -> Any:
    return value.get("Some") if isinstance(value, dict) else None


def _bytes(value: Any) -> bytes:
    if isinstance(value, dict) and "blob_hex" in value:
        return bytes.fromhex(value["blob_hex"])
    return bytes(nat(x) for x in value)


def _account(owner: str, subaccount: bytes | None = None) -> str:
    sub = "null" if subaccount is None else "opt " + blob(subaccount)
    return f'record {{owner=principal "{owner}";subaccount={sub}}}'


def _balance(driver: Any, ledger: str, owner: str, subaccount: bytes | None = None) -> int:
    did = driver.args.flaky_did if ledger == driver.flaky_id else driver.args.ledger_did
    return nat(driver.call(ledger, "icrc1_balance_of", f'({_account(owner, subaccount)})',
                           did=did, query=True)[0])


def _pool_money(driver: Any) -> dict[str, Any]:
    status = driver.call(driver.pool_id, "get_pool_status", did=driver.args.threepool_did, query=True)[0]
    return {key: status[key] for key in ("balances", "lp_total_supply", "virtual_price")}


def _snapshot(driver: Any, ids: dict[str, str]) -> dict[str, Any]:
    result: dict[str, Any] = {"pool": _pool_money(driver), "position": driver.pool_position(),
                              "lp_sp": nat(driver.call(driver.pool_id, "get_lp_balance",
                                  f'(principal "{driver.sp}")', did=driver.args.threepool_did, query=True)[0])}
    for name, ledger in ids.items():
        for role, owner in (("user", driver.user), ("sp", driver.sp),
                            ("backend", driver.backend_id), ("treasury", driver.admin)):
            result[name + "_" + role] = _balance(driver, ledger, owner)
        result[name + "_supply"] = nat(driver.call(ledger, "icrc1_total_supply",
                                                  did=driver.args.ledger_did, query=True)[0])
    result["vaults"] = driver.call(driver.backend_id, "get_vaults",
                                   f'(opt principal "{driver.user}")', did=driver.args.backend_did, query=True)[0]
    return result


def _conversion_identity(driver: Any, ledger: str, seq: int, domain: bytes) -> bytes:
    h = hashlib.sha256(domain)
    for owner in (driver.sp, driver.user, ledger):
        raw = raw_principal(owner)
        h.update(bytes([len(raw)])); h.update(raw)
    h.update(seq.to_bytes(8, "big"))
    return h.digest()


def _pool_receipt(driver: Any, intent_id: bytes) -> Any:
    return _some(driver.call(driver.pool_id, "get_liquidity_receipt_v1", f'({blob(intent_id)})',
                             sender=driver.sp, did=driver.args.threepool_did, query=True)[0])


def _pool_request(receipt: dict[str, Any]) -> str:
    request = receipt["request"]
    sub = _some(request["source_subaccount"])
    sub_text = "null" if sub is None else "opt " + blob(_bytes(sub))
    cap = _some(request["max_transfer_fee"])
    cap_text = "null" if cap is None else f"opt {nat(cap)} : opt nat"
    amounts = ";".join(f"{nat(x)} : nat" for x in request["amounts"])
    return (f'(record {{intent_id={blob(_bytes(request["intent_id"]))};amounts=vec {{{amounts}}};'
            f'min_lp={nat(request["min_lp"])} : nat;source_subaccount={sub_text};'
            f'max_transfer_fee={cap_text}}})')


def _fee_curve(driver: Any, params: dict[str, Any]) -> None:
    driver.result_ok(driver.call(driver.pool_id, "set_fee_curve_params",
        f'(record {{min_fee_bps={nat(params["min_fee_bps"])} : nat16;'
        f'max_fee_bps={nat(params["max_fee_bps"])} : nat16;'
        f'imb_saturation={nat(params["imb_saturation"])} : nat64}})', did=driver.args.threepool_did))


def _check_pool_rejection(driver: Any, ids: dict[str, str]) -> dict[str, Any]:
    """An actual market quote change rejects a funded configured-SP request."""
    token = ids["ckusdc"]
    amount, budget, fee = 1_000_000, 5_000, 1_000
    old_curve = driver.call(driver.pool_id, "get_fee_curve_params", did=driver.args.threepool_did, query=True)[0]
    _fee_curve(driver, {"min_fee_bps": 0, "max_fee_bps": 0, "imb_saturation": 1_000_000_000})
    quote = driver.result_ok(driver.call(driver.sp, "quote_3usd_conversion",
                                        f'(principal "{token}",{amount} : nat64,0 : nat)'))
    minimum = nat(quote["quoted_lp"])
    before = _snapshot(driver, ids)
    driver.call(token, "set_phantom_approve_failures", "(1 : nat32)", did=driver.args.flaky_did)
    seq, started = driver.start_conversion(token, amount, budget, minimum)
    pending = driver.result_ok(started)
    driver.check("no_admission_pool_funded_approval_reply_lost",
                 "Pending" in pending and "Approving" in pending["Pending"]["phase"])
    intent_id = _conversion_identity(driver, token, seq, b"RUMI-SP-3POOL-INTENT-V1")
    escrow = _conversion_identity(driver, token, seq, b"RUMI-SP-CONVERSION-ESCROW-V1")
    driver.check("no_admission_pool_not_dispatched_before_quote_change", _pool_receipt(driver, intent_id) is None)
    _fee_curve(driver, {"min_fee_bps": 1000, "max_fee_bps": 1000, "imb_saturation": 1_000_000_000})
    lower = driver.result_ok(driver.call(driver.sp, "quote_3usd_conversion",
                                        f'(principal "{token}",{amount} : nat64,0 : nat)'))
    driver.check("no_admission_pool_min_lp_will_fail", nat(lower["quoted_lp"]) < minimum)
    final = driver.settle_conversion(seq)
    receipt = _pool_receipt(driver, intent_id)
    driver.check("no_admission_pool_durable_rejected_receipt",
                 receipt is not None and "Rejected" in receipt["status"])
    driver.check("no_admission_pool_rejection_has_no_dispatched_input_or_mint",
                 all(x is None for x in receipt["input_ids"])
                 and all(x is None for x in receipt["input_transfers"])
                 and receipt["lp_minted"] is None and receipt["mint_event_id"] is None)
    after = _snapshot(driver, ids)
    driver.check("no_admission_pool_sp_returns_principal", "Refunded" in final)
    driver.check("no_admission_pool_exact_four_fee_cost", before["ckusdc_user"] - after["ckusdc_user"] == 4 * fee)
    driver.check("no_admission_pool_no_lp_or_pool_asset_change",
                 before["pool"] == after["pool"] and before["lp_sp"] == after["lp_sp"]
                 and before["position"] == after["position"])
    driver.check("no_admission_pool_all_escrow_returned", _balance(driver, token, driver.sp, escrow) == 0)
    allowance = driver.call(token, "icrc2_allowance",
        f'(record {{account={_account(driver.sp, escrow)};spender={account(driver.pool_id)}}})',
        did=driver.args.ledger_did)[0]
    driver.check("no_admission_pool_allowance_revoked", nat(allowance["allowance"]) == 0)
    _fee_curve(driver, {"min_fee_bps": 0, "max_fee_bps": 0, "imb_saturation": 1_000_000_000})
    replay_quote = driver.result_ok(driver.call(driver.sp, "quote_3usd_conversion",
        f'(principal "{token}",{amount} : nat64,0 : nat)'))
    driver.check("no_admission_pool_delayed_original_min_lp_now_admissible",
                 nat(replay_quote["quoted_lp"]) >= minimum)

    # Make a delayed original financially executable in the absence of its
    # tombstone. This funding and approval are explicit LOCAL fixture actions,
    # performed only after verifying the real user's complete compensation.
    driver.result_ok(driver.call(token, "icrc1_transfer",
        f'(record {{from_subaccount=null;to={_account(driver.sp, escrow)};amount=2000000 : nat;'
        'fee=opt 1000 : opt nat;memo=null;created_at_time=null})', sender=driver.user, did=driver.args.flaky_did))
    driver.result_ok(driver.call(token, "icrc2_approve",
        f'(record {{from_subaccount=opt {blob(escrow)};spender={account(driver.pool_id)};'
        f'amount={amount + fee} : nat;expected_allowance=opt 0 : opt nat;expires_at=null;'
        'fee=opt 1000 : opt nat;memo=null;created_at_time=null})', sender=driver.sp, did=driver.args.flaky_did))
    late_allowance = driver.call(token, "icrc2_allowance",
        f'(record {{account={_account(driver.sp, escrow)};spender={account(driver.pool_id)}}})',
        did=driver.args.ledger_did)[0]
    late_before = _snapshot(driver, ids)
    escrow_before = _balance(driver, token, driver.sp, escrow)
    driver.check("no_admission_pool_delayed_original_has_sufficient_balance_and_allowance",
                 escrow_before >= amount + fee and nat(late_allowance["allowance"]) >= amount + fee)
    late = driver.result_ok(driver.call(driver.pool_id, "add_liquidity_with_receipt_client_v1",
                                        _pool_request(receipt), sender=driver.sp, did=driver.args.threepool_did))
    driver.check("no_admission_pool_delayed_original_replays_rejection", late == receipt)
    driver.check("no_admission_pool_delayed_original_cannot_spend_or_mint",
                 _snapshot(driver, ids) == late_before and _balance(driver, token, driver.sp, escrow) == escrow_before)
    _fee_curve(driver, old_curve)
    return {"intent_seq": seq, "pool_intent_id": intent_id.hex(), "receipt": receipt,
            "final": final, "before": before, "after": after, "late_probe_funded_escrow_raw": escrow_before,
            "local_only": True}


def _lifecycle(driver: Any, canister: str, method: str) -> Any:
    assert method in ("stop_canister", "start_canister")
    did = driver.out / "no-admission-management.did"
    if not did.exists():
        did.write_text('service:{stop_canister:(record{canister_id:principal})->();'
                       'start_canister:(record{canister_id:principal})->()}')
    return driver.call("aaaaa-aa", method, f'(record {{canister_id=principal "{canister}"}})',
                       sender=driver.anon, did=did, effective={"CanisterId": b64(raw_principal(canister))})


def _ordinary_status(driver: Any, request_id: bytes) -> dict[str, Any]:
    row = _some(driver.call(driver.sp, "get_ordinary_liquidation_status", f'({blob(request_id)})', query=True)[0])
    assert row is not None
    return row


def _backend_ordinary(driver: Any, request_id: bytes) -> Any:
    return driver.call(driver.backend_id, "stability_pool_liquidation_status", f'({blob(request_id)})',
                       sender=driver.sp, did=driver.args.backend_did, query=True)


def _open(driver: Any, ids: dict[str, str], operation: int) -> int:
    driver.local_collateral_policy(ids["collateral"], False)
    opened = driver.result_ok(driver.call(driver.backend_id, "open_vault_and_borrow_v2",
        f'(record {{collateral_amount=1000000000 : nat64;operation_id={operation} : nat64;'
        f'borrow_amount=5000000000 : nat64;collateral_type=opt principal "{ids["collateral"]}"}})',
        sender=driver.user, did=driver.args.backend_did))
    driver.result_ok(driver.call(driver.backend_id, "ack_vault_operation",
        f'(record {{operation_id={operation} : nat64}})', sender=driver.user, did=driver.args.backend_did))
    driver.local_collateral_policy(ids["collateral"], True)
    driver.vault_operation_seq = operation
    return nat(opened["vault_id"])


def _check_ordinary_absent_prepare(driver: Any, ids: dict[str, str]) -> dict[str, Any]:
    vault_id = _open(driver, ids, 1)
    before = _snapshot(driver, ids)
    stable = ids["ckusdt"]
    max_gross = driver.pool_book(stable) - 1000
    _lifecycle(driver, driver.backend_id, "stop_canister")
    try:
        driver.call(driver.sp, "notify_liquidatable_vaults",
            f'(vec {{record {{vault_id={vault_id} : nat64;owner=principal "{driver.user}";'
            f'collateral_type=principal "{ids["collateral"]}";debt_amount=5000000000;'
            'collateral_amount=1000000000;recommended_liquidation_amount=5000000000;'
            'collateral_price_e8s=1000000000}})', sender=driver.backend_id)
        found = driver.find_ordinary_status(vault_id)
        driver.check("no_admission_ordinary_sp_persists_request_before_backend_delivery", found is not None)
        _, request_id, row = found
        driver.check("no_admission_ordinary_expected_payment_binding",
                     "Ckusdt" in row["payment"] and row["stable_ledger"]["principal"] == stable)
        driver.check("no_admission_ordinary_prepare_was_unacknowledged",
                     "BackendSubmitted" in row["phase"] and row["quote"] is None
                     and not row["approval_attempted"] and row["backend_attempted"])
        held_withdraw = driver.call(driver.sp, "withdraw", f'(principal "{stable}",1000000 : nat64)', sender=driver.user)
        driver.result_err(held_withdraw)
    finally:
        _lifecycle(driver, driver.backend_id, "start_canister")
    driver.result_err(_backend_ordinary(driver, request_id))
    driver.check("no_admission_ordinary_rejected_prepare_moves_no_funds", _snapshot(driver, ids) == before)
    # The owner's normal repay/close changes the mutable admission condition;
    # cancellation itself must leave this resulting state unchanged.
    driver.approve(ids["icusd"], driver.backend_id, driver.user)
    driver.result_ok(driver.call(driver.backend_id, "repay_and_close_vault",
        f'(record {{vault_id={vault_id} : nat64;amount=5000000000 : nat64}})',
        sender=driver.user, did=driver.args.backend_did))
    cancel_before = _snapshot(driver, ids)
    driver.call(driver.sp, "retry_ordinary_liquidation", f'({blob(request_id)})')
    final = _ordinary_status(driver, request_id)
    tombstone = driver.result_ok(_backend_ordinary(driver, request_id))
    driver.check("no_admission_ordinary_cancelled_through_real_sp_wrapper",
                 "Cancelled" in final["phase"] and "Cancelled" in tombstone["phase"])
    driver.check("no_admission_ordinary_cancel_changes_no_money_or_debt", _snapshot(driver, ids) == cancel_before)
    exact = (f'(record {{request_id={blob(request_id)};vault_id={vault_id} : nat64;'
             f'payment=variant {{Ckusdt}};max_debt_e8s=5000000000 : nat64;max_gross_raw={max_gross} : nat64}})')
    late = driver.result_ok(driver.call(driver.backend_id, "stability_pool_prepare_liquidation", exact,
                                        sender=driver.sp, did=driver.args.backend_did))
    driver.check("no_admission_ordinary_delayed_exact_prepare_replays_tombstone", late == tombstone)
    driver.check("no_admission_ordinary_delayed_prepare_moves_no_money_or_debt", _snapshot(driver, ids) == cancel_before)
    # Same ledger and same amount rejected under the hold must now work.
    book = driver.pool_book(stable)
    driver.result_ok(driver.call(driver.sp, "withdraw", f'(principal "{stable}",1000000 : nat64)', sender=driver.user))
    driver.check("no_admission_ordinary_guard_released", driver.pool_book(stable) == book - 1000000)
    return {"vault_id": vault_id, "request_id": request_id.hex(), "tombstone": tombstone,
            "final": final, "before": before, "cancel_before": cancel_before, "local_only": True,
            "fault": "Local management stopped backend before prepare delivery; no backend receipt existed."}


def _backend_upgrade(driver: Any, *, setup_reload: bool = False) -> None:
    driver.upgrade(driver.backend_id, driver.args.backend_wasm,
        driver.encode('(variant {Upgrade=record {mode=null;description=opt "local no-admission fixture"}})',
                      driver.args.backend_did, types="(ProtocolArg)"), fixture_skip_pre_upgrade=setup_reload)


def _v2_status(driver: Any, vault: int, absorb: int) -> dict[str, Any]:
    return driver.call(driver.backend_id, "get_stability_pool_liquidate_with_reserves_v2_status",
                       f'({vault} : nat64,{absorb} : nat64)', sender=driver.sp,
                       did=driver.args.backend_did, query=True)[0]


def _v2_args(driver: Any, vault: int, absorb: int, debt: int, amount: int) -> str:
    return (f'({vault} : nat64,{absorb} : nat64,{debt} : nat64,{amount} : nat64,'
            f'principal "{driver.pool_id}")')


def _retire(driver: Any, args: str, sender: str | None = None) -> Any:
    return driver.call(driver.backend_id, "retire_stability_pool_liquidate_with_reserves_v2", args,
                       sender=sender or driver.sp, did=driver.args.backend_did)


def _seed_pending_backend(driver: Any, ids: dict[str, str], vault: int, absorb: int,
                          debt: int, amount: int) -> dict[str, Any]:
    """Qualified synthetic setup only; no claim of an actual suspended call."""
    helper = getattr(driver.args, "no_admission_fixture_helper", None)
    assert helper is not None, "--no-admission-fixture-helper is required"
    before = _snapshot(driver, ids)
    _backend_upgrade(driver)
    response = driver.http(driver.endpoint("read/get_stable_memory"),
                           {"canister_id": b64(raw_principal(driver.backend_id))})
    saved = base64.b64decode(response["blob"], validate=True)
    path = driver.out / "no-admission-backend-before.stable.bin"
    path.write_bytes(saved)
    out = driver.out / "no-admission-backend-offline"
    command = [str(helper), str(path), driver.sp, str(vault), str(absorb),
               str(debt), str(amount), driver.pool_id, str(out)]
    run = subprocess.run(command, capture_output=True, text=True, timeout=120)
    (driver.out / "no-admission-helper.stdout.txt").write_text(run.stdout)
    (driver.out / "no-admission-helper.stderr.txt").write_text(run.stderr)
    driver._save_json_once(driver.out / "no-admission-helper-command.json", {"argv": command, "exit_code": run.returncode})
    if run.returncode:
        raise AssertionError("offline no-admission adapter failed: " + run.stderr)
    receipt = json.loads((out / "receipt.json").read_text())
    for key in ("synthetic_local_fixture", "only_no_admission_journal_changed",
                "all_other_state_fields_preserved", "non_target_virtual_memories_preserved", "no_ledger_dispatch"):
        driver.check("no_admission_synthetic_" + key, receipt.get(key) is True)
    driver.check("no_admission_synthetic_no_actual_fault_or_migration_claim",
                 receipt["actual_runtime_equivalence_asserted"] is False and receipt["production_migration"] is False)
    patched = (out / "patched-stable.bin").read_bytes()
    request = urllib.request.Request(driver.url + "/blobstore/", data=patched, method="POST",
                                     headers={"Content-Type": "application/octet-stream"})
    with urllib.request.urlopen(request, timeout=120) as upload:
        blob_id = upload.read().decode().strip()
    driver.http(driver.endpoint("update/set_stable_memory"),
                {"canister_id": b64(raw_principal(driver.backend_id)), "blob_id": b64(bytes.fromhex(blob_id))})
    _backend_upgrade(driver, setup_reload=True)
    pending = _v2_status(driver, vault, absorb)
    driver.check("no_admission_synthetic_pending_visible", "AdmissionPending" in pending["status"])
    driver.check("no_admission_synthetic_setup_preserves_all_money_and_debt", _snapshot(driver, ids) == before)
    # This is the actual persistence assertion, separate from the qualified
    # setup reload. It uses ordinary pre_upgrade AND post_upgrade hooks.
    _backend_upgrade(driver)
    driver.check("no_admission_v2_actual_normal_pending_upgrade_preserved", _v2_status(driver, vault, absorb) == pending)
    driver.check("no_admission_v2_actual_normal_pending_upgrade_preserves_money", _snapshot(driver, ids) == before)
    return {"receipt": receipt, "status": pending, "before": before,
            "qualification": "Synthetic AdmissionPending setup; subsequent normal pending upgrade and retirement are runtime observations."}


def _check_v2_retirement(driver: Any, ids: dict[str, str], closed_vault: int) -> dict[str, Any]:
    debt, amount = 100_000_000, 100_000_000
    absorb = 9001
    seeded = _seed_pending_backend(driver, ids, closed_vault, absorb, debt, amount)
    args = _v2_args(driver, closed_vault, absorb, debt, amount)
    before = _snapshot(driver, ids)
    driver.result_err(_retire(driver, args, sender=driver.other))
    driver.check("no_admission_v2_wrong_caller_cannot_retire", _v2_status(driver, closed_vault, absorb) == seeded["status"])
    driver.result_err(_retire(driver, _v2_args(driver, closed_vault, absorb, debt + 1, amount)))
    driver.check("no_admission_v2_pending_mismatch_cannot_retire", _v2_status(driver, closed_vault, absorb) == seeded["status"])
    retired = driver.result_ok(_retire(driver, args))
    driver.check("no_admission_v2_pending_retires_durably", "PreTransferRejected" in retired["status"])
    driver.check("no_admission_v2_pending_retirement_moves_no_money_or_debt", _snapshot(driver, ids) == before)
    driver.check("no_admission_v2_pending_retirement_idempotent", driver.result_ok(_retire(driver, args)) == retired)
    driver.result_err(driver.call(driver.backend_id, "stability_pool_liquidate_with_reserves_v2", args,
                                  sender=driver.sp, did=driver.args.backend_did))
    driver.check("no_admission_v2_retired_pending_delayed_original_held",
                 _v2_status(driver, closed_vault, absorb) == retired and _snapshot(driver, ids) == before)

    # This second identity is genuinely submitted to a stopped local backend.
    # The recorded ingress rejection is not interpreted as financial proof;
    # the eventual exact retirement tombstone is the no-dispatch authority.
    unseen_id = 9002
    unseen_args = _v2_args(driver, closed_vault, unseen_id, debt, amount)
    driver.check("no_admission_v2_unseen_identity_initially_absent", "Unseen" in _v2_status(driver, closed_vault, unseen_id)["status"])
    _lifecycle(driver, driver.backend_id, "stop_canister")
    rejected = None
    try:
        try:
            driver.call(driver.backend_id, "stability_pool_liquidate_with_reserves_v2", unseen_args,
                        sender=driver.sp, did=driver.args.backend_did)
        except RuntimeError as error:
            rejected = str(error)
        driver.check("no_admission_v2_local_transport_rejected_before_admission",
                     rejected is not None and "stopped" in rejected.lower())
    finally:
        _lifecycle(driver, driver.backend_id, "start_canister")
    driver.check("no_admission_v2_rejected_delivery_still_unseen", "Unseen" in _v2_status(driver, closed_vault, unseen_id)["status"])
    unseen_before = _snapshot(driver, ids)
    unseen_retired = driver.result_ok(_retire(driver, unseen_args))
    driver.check("no_admission_v2_unseen_retires_durably", "PreTransferRejected" in unseen_retired["status"])
    driver.result_err(driver.call(driver.backend_id, "stability_pool_liquidate_with_reserves_v2", unseen_args,
                                  sender=driver.sp, did=driver.args.backend_did))
    driver.check("no_admission_v2_retired_unseen_delayed_original_cannot_dispatch",
                 _v2_status(driver, closed_vault, unseen_id) == unseen_retired and _snapshot(driver, ids) == unseen_before)

    # Use an actually settled candidate-pool transfer for the monetary phase
    # guard. Submitted/TransferConfirmed phase-only guards remain pure source
    # tests; this fixture does not fabricate positive blocks for those phases.
    from sp_rails_lp_fixture import check_threeusd_reserve_liquidation
    actual = check_threeusd_reserve_liquidation(driver, ids)
    actual_vault, actual_absorb = actual["vault_id"], actual["absorb_id"]
    request = actual["ingress"]["row"]["request"]
    actual_args = _v2_args(driver, actual_vault, actual_absorb,
                           nat(request["icusd_debt_covered_e8s"]), nat(request["three_usd_amount_e8s"]))
    absorbed_before = _v2_status(driver, actual_vault, actual_absorb)
    money_before = _snapshot(driver, ids)
    retirement = driver.result_ok(_retire(driver, actual_args))
    driver.check("no_admission_v2_actual_absorbed_cannot_be_retired",
                 "Absorbed" in retirement["status"] and retirement == absorbed_before)
    driver.check("no_admission_v2_actual_absorbed_retirement_moves_no_money_or_debt", _snapshot(driver, ids) == money_before)
    return {"synthetic_pending": seeded, "pending_retirement": retired,
            "unseen_transport_rejection": rejected, "unseen_retirement": unseen_retired,
            "actual_absorbed": actual, "actual_absorbed_retirement": retirement, "local_only": True}


def check_no_admission(driver: Any, ids: dict[str, str]) -> dict[str, Any]:
    assert not getattr(driver.args, "flaky_collateral", False), "real LP fixture requires standard official collateral ledger"
    for method in ("set_interest_treasury_tick_interval_secs", "set_vault_check_tick_interval_secs"):
        driver.result_ok(driver.call(driver.backend_id, method, f'({QUIET_TIMER_SECONDS} : nat64)',
                                     did=driver.args.backend_did))
    pool = _check_pool_rejection(driver, ids)
    ordinary = _check_ordinary_absent_prepare(driver, ids)
    v2 = _check_v2_retirement(driver, ids, ordinary["vault_id"])
    return {"local_only": True, "production_calls": 0, "pool_rejection": pool,
            "ordinary_absent_prepare": ordinary, "v2_retirement": v2,
            "limits": ["AdmissionPending is synthetic setup only, not a genuine suspended-call fault claim.",
                       "No SP pending V2 intent is injected; its timer integration is covered by source tests.",
                       "Submitted/TransferConfirmed non-retirement guards are pure production source regressions.",
                       "The Absorbed non-retirement guard uses a real candidate-pool transfer and settlement.",
                       "All local fault injection, normal pending upgrades and financial observations are isolated from production."]}
