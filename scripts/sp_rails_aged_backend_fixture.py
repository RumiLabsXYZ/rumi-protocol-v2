#!/usr/bin/env python3
"""Local aged ordinary-liquidation proof scenarios for ``sp_rails_runtime``.

The owner harness supplies real candidate Wasms, a private PocketIC instance,
normal vault administration and two separately installed flaky ICRC ledgers.
This module never creates a production URL or uses a production identity.
It does not substitute a fake backend or manufacture a receipt. All candidate
blocks are read from the configured ledger after its exact transfer landed.
"""
from __future__ import annotations

import hashlib
from typing import Any

from sp_reopening_runtime import account, blob, nat, raw_principal


AGED_SECONDS = 2 * 24 * 60 * 60
QUIET_TIMER_SECONDS = 100 * 24 * 60 * 60


def _some(value: Any) -> Any:
    return value.get("Some") if isinstance(value, dict) else None


def _blob_bytes(value: Any) -> bytes:
    if isinstance(value, dict) and "blob_hex" in value:
        return bytes.fromhex(value["blob_hex"])
    if isinstance(value, list):
        return bytes(nat(byte) for byte in value)
    raise AssertionError(f"unsupported typed blob: {value!r}")


def _generic(value: Any) -> Any:
    """Decode only the typed ICRC-3 values used in this bounded fixture."""
    if not isinstance(value, dict) or len(value) != 1:
        raise AssertionError(f"malformed generic value: {value!r}")
    tag, inner = next(iter(value.items()))
    if tag == "Map":
        if isinstance(inner, dict):
            return {key: _generic(item) for key, item in inner.items()}
        return {row["0"]: _generic(row["1"]) for row in inner}
    if tag == "Array":
        return [_generic(item) for item in inner]
    if tag == "Blob":
        return _blob_bytes(inner)
    if tag in ("Nat", "Nat64", "Int"):
        return nat(inner)
    if tag == "Text":
        return inner
    raise AssertionError(f"unsupported generic tag: {tag}")


def _is_account(value: Any, owner: str) -> bool:
    return isinstance(value, list) and value == [raw_principal(owner)]


def _tip(driver: Any, ledger: str) -> int:
    reply = driver.call(ledger, "icrc3_get_blocks", "(vec {})",
                        did=driver.args.flaky_did, query=True)[0]
    assert not reply["blocks"] and not reply["archived_blocks"]
    return nat(reply["log_length"])


def _blocks(driver: Any, ledger: str, start: int, end: int) -> list[dict[str, Any]]:
    blocks = []
    cursor = start
    while cursor < end:
        length = min(100, end - cursor)
        result = driver.call(ledger, "icrc3_get_blocks",
                             f'(vec {{record {{start={cursor} : nat;length={length} : nat}}}})',
                             did=driver.args.flaky_did, query=True)[0]
        assert not result["archived_blocks"], "local fixture unexpectedly archived history"
        rows = result["blocks"]
        assert [nat(row["id"]) for row in rows] == list(range(cursor, cursor + length))
        blocks.extend({"id": nat(row["id"]), "value": _generic(row["block"])} for row in rows)
        cursor += length
    return blocks


def _exact_transfer(driver: Any, label: str, ledger: str, start: int,
                    source: str, destination: str, amount: int, fee: int,
                    delegated: bool) -> dict[str, Any]:
    end = _tip(driver, ledger)
    candidates = []
    for row in _blocks(driver, ledger, start, end):
        block = row["value"]
        tx = block.get("tx", {})
        if tx.get("op") != "xfer" or not _is_account(tx.get("from"), source):
            continue
        if not _is_account(tx.get("to"), destination):
            continue
        if tx.get("amt") != amount or tx.get("fee", block.get("fee")) != fee:
            continue
        if delegated != ("spender" in tx):
            continue
        if delegated and not _is_account(tx.get("spender"), driver.backend_id):
            continue
        assert isinstance(tx.get("memo"), bytes) and len(tx["memo"]) == 16
        assert isinstance(tx.get("ts"), int) and tx["ts"] > 0
        assert block.get("btype") == ("2xfer" if delegated else "1xfer")
        candidates.append(row)
    driver.check(label + "_one_exact_positive_ledger_transfer", len(candidates) == 1)
    return candidates[0]


def _debt(driver: Any, vault_id: int) -> int:
    vaults = driver.call(driver.backend_id, "get_vaults",
                         f'(opt principal "{driver.user}")',
                         did=driver.args.backend_did, query=True)[0]
    return next((nat(row["borrowed_icusd_amount"]) for row in vaults
                 if nat(row["vault_id"]) == vault_id), 0)


def _backend_status(driver: Any, request_id: bytes) -> dict[str, Any]:
    return driver.result_ok(driver.call(
        driver.backend_id, "stability_pool_liquidation_status", f'({blob(request_id)})',
        sender=driver.sp, did=driver.args.backend_did, query=True))


def _sp_status(driver: Any, request_id: bytes) -> dict[str, Any]:
    response = driver.call(driver.sp, "get_ordinary_liquidation_status",
                           f'({blob(request_id)})', query=True)[0]
    result = _some(response)
    assert result is not None
    return result


def _supply(driver: Any, ledger: str) -> int:
    # Both official and flaky ledgers expose the standard Nat wire shape.
    return nat(driver.call(ledger, "icrc1_total_supply", did=driver.args.ledger_did,
                           query=True)[0])


def _money(driver: Any, ids: dict[str, str]) -> dict[str, int]:
    stable, collateral = ids["ckusdc"], ids["collateral"]
    position = driver.pool_position() or {}
    gains = next((nat(row["1"]) for row in position.get("collateral_gains", [])
                  if row["0"]["principal"] == collateral), 0)
    return {
        "stable_sp": driver.qbalance(stable, driver.sp, flaky=True),
        "stable_backend": driver.qbalance(stable, driver.backend_id, flaky=True),
        "stable_treasury": driver.qbalance(stable, driver.admin, flaky=True),
        "stable_supply": _supply(driver, stable),
        "stable_book": driver.pool_book(stable),
        "collateral_sp": driver.qbalance(collateral, driver.sp, flaky=True),
        "collateral_backend": driver.qbalance(collateral, driver.backend_id, flaky=True),
        "collateral_treasury": driver.qbalance(collateral, driver.admin, flaky=True),
        "collateral_supply": _supply(driver, collateral),
        "collateral_gain_book": gains,
        "icusd_supply": _supply(driver, ids["icusd"]),
    }


def _upgrade_pending(driver: Any, label: str, request_id: bytes) -> None:
    before_backend = _backend_status(driver, request_id)
    before_sp = _sp_status(driver, request_id)
    driver.upgrade(driver.backend_id, driver.args.backend_wasm,
                   driver.encode('(variant {Upgrade=record {mode=null;description=opt "local aged proof fixture"}})',
                                 driver.args.backend_did, types="(ProtocolArg)"))
    driver.upgrade(driver.sp, driver.args.sp_wasm,
                   driver.encode(f'(record {{protocol_canister_id=principal "{driver.backend_id}";'
                                 f'authorized_admins=vec {{principal "{driver.admin}"}}}})',
                                 driver.args.sp_did, types="(StabilityPoolInitArgs)"))
    after_backend = _backend_status(driver, request_id)
    after_sp = _sp_status(driver, request_id)
    driver.check(label + "_backend_upgrade_retains_exact_receipt", before_backend == after_backend)
    for field in ("request_id", "vault_id", "payment", "stable_ledger", "quote", "approval"):
        driver.check(label + "_sp_upgrade_retains_" + field, before_sp[field] == after_sp[field])


def _open(driver: Any, ids: dict[str, str], operation: int) -> int:
    driver.local_collateral_policy(ids["collateral"], False)
    opened = driver.result_ok(driver.call(
        driver.backend_id, "open_vault_and_borrow_v2",
        f'(record {{collateral_amount=1000000000 : nat64;operation_id={operation} : nat64;'
        f'borrow_amount=5000000000 : nat64;collateral_type=opt principal "{ids["collateral"]}"}})',
        sender=driver.user, did=driver.args.backend_did))
    driver.result_ok(driver.call(driver.backend_id, "ack_vault_operation",
                                 f'(record {{operation_id={operation} : nat64}})',
                                 sender=driver.user, did=driver.args.backend_did))
    driver.local_collateral_policy(ids["collateral"], True)
    return nat(opened["vault_id"])


def _finish(driver: Any, request_id: bytes, label: str) -> dict[str, Any]:
    for _ in range(8):
        row = _sp_status(driver, request_id)
        if "Completed" in row["phase"]:
            return row
        driver.call(driver.sp, "retry_ordinary_liquidation", f'({blob(request_id)})')
    row = _sp_status(driver, request_id)
    driver.check(label + "_completes_through_normal_sp_retry", "Completed" in row["phase"])
    return row


def check_aged_backend_proofs(driver: Any, ids: dict[str, str]) -> dict[str, Any]:
    """Run all three aged-receipt scenarios through the normal SP book path.

    The normal interest and health timers are moved beyond this fixture's six
    virtual days before any vault exists. This isolates transfer recovery; it
    does not claim a nonzero-interest accrual test. After the committed lost
    reply, the ordinary SP emergency pause contains its recovery timers while
    the tuple ages. The backend must then retain the unknown on exact TooOld;
    positive proof is accepted during the pause, and normal SP retry completes
    after the pause is lifted.
    """
    assert getattr(driver.args, "flaky_collateral", False), "requires fresh flaky collateral fixture"
    for method in ("set_interest_treasury_tick_interval_secs", "set_vault_check_tick_interval_secs"):
        driver.result_ok(driver.call(driver.backend_id, method,
                                     f'({QUIET_TIMER_SECONDS} : nat64)', did=driver.args.backend_did))
    for leg in ("payment", "protocol_fee", "payout"):
        driver.require_method(driver.args.backend_did, "stability_pool_reconcile_liquidation_" + leg)
    driver.deposit_seq += 1
    driver.result_ok(driver.call(driver.sp, "deposit_with_intent",
                                 f'({driver.deposit_seq} : nat64,principal "{ids["ckusdc"]}",10000000000 : nat64)',
                                 sender=driver.user))
    records = []
    for operation, leg in enumerate(("payment", "protocol_fee", "payout"), start=1):
        label = "aged_backend_" + leg
        # The payout case deliberately has no protocol cut, so its first
        # collateral transfer is the SP payout whose reply is being lost.
        share = "0.0" if leg == "payout" else "0.03"
        driver.result_ok(driver.call(driver.backend_id, "set_liquidation_protocol_share",
                                     f'({share} : float64)', did=driver.args.backend_did))
        vault_id = _open(driver, ids, operation)
        before_debt = _debt(driver, vault_id)
        before = _money(driver, ids)
        payment_start = _tip(driver, ids["ckusdc"])
        collateral_start = _tip(driver, ids["collateral"])
        fault_ledger = ids["ckusdc"] if leg == "payment" else ids["collateral"]
        fault = "set_phantom_transfer_from_failures" if leg == "payment" else "set_phantom_icrc1_failures"
        driver.call(fault_ledger, fault, "(1 : nat32)", did=driver.args.flaky_did)
        info = (f'(vec {{record {{vault_id={vault_id};collateral_type=principal "{ids["collateral"]}";'
                'debt_amount=5000000000;collateral_amount=1000000000;'
                'recommended_liquidation_amount=5000000000;collateral_price_e8s=1000000000}})')
        driver.call(driver.sp, "notify_liquidatable_vaults", info, sender=driver.backend_id)
        found = driver.find_ordinary_status(vault_id)
        driver.check(label + "_durable_sp_request_admitted", found is not None)
        _, request_id, row = found
        driver.check(label + "_uses_ckusdc_payment", row["stable_ledger"]["principal"] == ids["ckusdc"])
        driver.check(label + "_pending_after_lost_reply", "Completed" not in row["phase"])
        quote = _some(row["quote"])
        assert quote is not None
        receipt = _backend_status(driver, request_id)
        gross = nat(quote["gross_payment_raw"])
        principal = nat(quote["principal_raw"])
        surcharge = nat(quote["surcharge_raw"])
        payment_fee = nat(quote["pull_fee_raw"])
        approval_fee = 1000
        collateral_fee = nat(quote["collateral_fee_raw"])
        protocol_gross = nat(quote["collateral_gross_raw"]) - nat(quote["collateral_net_raw"])
        payout_net = nat(quote["collateral_net_raw"]) - collateral_fee
        if leg == "payment":
            positive = _exact_transfer(driver, label, ids["ckusdc"], payment_start,
                                       driver.sp, driver.backend_id, principal + surcharge,
                                       payment_fee, True)
        elif leg == "protocol_fee":
            positive = _exact_transfer(driver, label, ids["collateral"], collateral_start,
                                       driver.backend_id, driver.admin, protocol_gross - collateral_fee,
                                       collateral_fee, False)
        else:
            positive = _exact_transfer(driver, label, ids["collateral"], collateral_start,
                                       driver.backend_id, driver.sp, payout_net,
                                       collateral_fee, False)
        first_debt = _debt(driver, vault_id)
        expected_first = 0 if leg == "payment" else nat(quote["debt_e8s"])
        driver.check(label + "_debt_commit_at_expected_stage", before_debt - first_debt == expected_first)
        driver.result_ok(driver.call(driver.sp, "emergency_pause"))
        pause_money = _money(driver, ids)
        driver.advance_local_time(AGED_SECONDS)
        _upgrade_pending(driver, label, request_id)
        driver.check(label + "_pause_age_and_upgrade_preserve_money", _money(driver, ids) == pause_money)
        aged = driver.call(driver.backend_id, "stability_pool_execute_liquidation",
                           f'({blob(request_id)})', sender=driver.sp, did=driver.args.backend_did)
        error = driver.result_err(aged)
        if leg == "payout":
            # The backend deliberately exposes a stable generic held-payout
            # error rather than echoing each ledger error. Observe TooOld
            # directly from the configured local ledger using the original
            # persisted tuple; no timestamp, memo, fee or recipient changes.
            held = _backend_status(driver, request_id)
            payout = _some(held["payout_tuple"])
            assert payout is not None
            driver.check(label + "_backend_keeps_exact_unknown_payout",
                         "PayoutNoEffect" in held["phase"]
                         and held["request_id"] == receipt["request_id"]
                         and held["payout_tuple"] == receipt["payout_tuple"])
            driver.check(label + "_typed_retry_uses_exact_frozen_accounts",
                         payout["ledger"]["principal"] == ids["collateral"]
                         and payout["source"]["owner"]["principal"] == driver.backend_id
                         and payout["source"]["subaccount"] is None
                         and payout["destination"]["owner"]["principal"] == driver.sp
                         and payout["destination"]["subaccount"] is None)
            before_ledger_retry = _money(driver, ids)
            debt_before_retry = _debt(driver, vault_id)
            memo = _blob_bytes(payout["memo"])
            ledger_retry = driver.call(
                ids["collateral"], "icrc1_transfer",
                f'(record {{from_subaccount=null;to={account(driver.sp)};'
                f'amount={nat(payout["amount_raw"])} : nat;'
                f'fee=opt {nat(payout["ledger_fee_raw"])} : opt nat;'
                f'memo=opt {blob(memo)};'
                f'created_at_time=opt {nat(payout["created_at_time_ns"])} : opt nat64}})',
                sender=driver.backend_id, did=driver.args.flaky_did)
            driver.result_err(ledger_retry, ("TooOld",))
            driver.check(label + "_exact_retry_observed_too_old", True)
            driver.check(label + "_typed_too_old_retry_changes_no_money",
                         _money(driver, ids) == before_ledger_retry)
            driver.check(label + "_typed_too_old_retry_changes_no_debt",
                         _debt(driver, vault_id) == debt_before_retry)
            driver.check(label + "_typed_too_old_retry_changes_no_journal",
                         _backend_status(driver, request_id) == held)
            normal_retry = driver.call(
                driver.backend_id, "stability_pool_execute_liquidation",
                f'({blob(request_id)})', sender=driver.sp, did=driver.args.backend_did)
            driver.result_err(normal_retry)
            driver.check(label + "_normal_retry_retains_exact_held_journal",
                         _backend_status(driver, request_id) == held)
            driver.check(label + "_normal_retry_after_too_old_changes_no_money_or_debt",
                         _money(driver, ids) == before_ledger_retry
                         and _debt(driver, vault_id) == debt_before_retry)
        else:
            driver.check(label + "_exact_retry_observed_too_old", "TooOld" in str(error))
        before_bad = _money(driver, ids)
        before_bad_receipt = _backend_status(driver, request_id)
        proof_method = "stability_pool_reconcile_liquidation_" + leg
        # Block zero is fixture mint/approval history, never this immutable
        # backend operation. A failed candidate must not pin any journal key.
        wrong = driver.call(driver.backend_id, proof_method,
                            f'({blob(request_id)},0 : nat64)', did=driver.args.backend_did)
        driver.result_err(wrong)
        driver.check(label + "_wrong_candidate_keeps_journal", _backend_status(driver, request_id) == before_bad_receipt)
        driver.check(label + "_wrong_candidate_keeps_money", _money(driver, ids) == before_bad)
        proof = driver.result_ok(driver.call(driver.backend_id, proof_method,
                                            f'({blob(request_id)},{positive["id"]} : nat64)',
                                            did=driver.args.backend_did))
        if leg == "payment":
            driver.check(label + "_positive_block_adopted", nat(_some(proof["payment_block_index"])) == positive["id"])
        elif leg == "payout":
            driver.check(label + "_positive_block_adopted", nat(_some(proof["payout_block_index"])) == positive["id"])
        driver.result_ok(driver.call(driver.sp, "resume_operations"))
        final = _finish(driver, request_id, label)
        driver.check(label + "_normal_sp_completes", "Completed" in final["phase"])
        after = _money(driver, ids)
        driver.check(label + "_frozen_request_retained", final["request_id"] == row["request_id"])
        driver.check(label + "_debt_reduced_exactly_once", before_debt - _debt(driver, vault_id) == nat(quote["debt_e8s"]))
        driver.check(label + "_payment_quote_gross_exact", gross == principal + surcharge + payment_fee)
        driver.check(label + "_sp_physical_payment_once", before["stable_sp"] - after["stable_sp"] == gross + approval_fee)
        driver.check(label + "_sp_book_payment_once", before["stable_book"] - after["stable_book"] == gross + approval_fee)
        driver.check(label + "_backend_receives_exact_principal_and_surcharge", after["stable_backend"] - before["stable_backend"] == principal + surcharge)
        driver.check(label + "_no_unexpected_stable_treasury_movement", after["stable_treasury"] == before["stable_treasury"])
        driver.check(label + "_stable_fees_burn_once", before["stable_supply"] - after["stable_supply"] == approval_fee + payment_fee)
        driver.check(label + "_sp_collateral_received_once", after["collateral_sp"] - before["collateral_sp"] == payout_net)
        driver.check(label + "_depositor_collateral_gain_credited_once", after["collateral_gain_book"] - before["collateral_gain_book"] == payout_net)
        treasury_net = protocol_gross - collateral_fee if protocol_gross else 0
        driver.check(label + "_protocol_cut_received_once", after["collateral_treasury"] - before["collateral_treasury"] == treasury_net)
        fee_total = collateral_fee * (2 if protocol_gross else 1)
        driver.check(label + "_collateral_fees_once", before["collateral_supply"] - after["collateral_supply"] == fee_total)
        driver.check(label + "_collateral_gross_conserved", before["collateral_backend"] - after["collateral_backend"] == payout_net + treasury_net + fee_total)
        driver.check(label + "_no_unexpected_icusd_mint", before["icusd_supply"] == after["icusd_supply"])
        # Same proof and same SP recovery identity are idempotent after the
        # financial result; neither is allowed to produce another movement.
        driver.result_ok(driver.call(driver.backend_id, proof_method,
                                     f'({blob(request_id)},{positive["id"]} : nat64)',
                                     did=driver.args.backend_did))
        driver.call(driver.sp, "retry_ordinary_liquidation", f'({blob(request_id)})')
        driver.check(label + "_terminal_proof_and_retry_no_second_movement", _money(driver, ids) == after)
        records.append({"leg": leg, "vault_id": vault_id, "request_id_sha256": hashlib.sha256(request_id).hexdigest(),
                        "positive_block": positive["id"], "aged_seconds": AGED_SECONDS,
                        "debt_e8s": nat(quote["debt_e8s"]), "principal_raw": principal,
                        "surcharge_raw": surcharge, "gross_payment_raw": gross,
                        "payout_net_raw": payout_net, "protocol_gross_raw": protocol_gross,
                        "before": before, "after": after, "local_only": True})
    return {"passed": True, "local_only": True, "production_calls": 0, "cases": records,
            "limits": ["Controlled lost replies use local flaky ICRC ledgers, not a production ledger.",
                       "Normal interest and health timers are outside the six-day fixture window; nonzero interest is not covered.",
                       "Each receipt is real local ledger history for the candidate's persisted exact transfer."]}
