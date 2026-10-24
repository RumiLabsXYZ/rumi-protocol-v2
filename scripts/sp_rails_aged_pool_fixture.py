#!/usr/bin/env python3
"""Local positive-proof fixtures for 3pool add-liquidity receipts.

The caller is the existing local ``RailsDriver`` from ``sp_rails_runtime``.
This module does not create a fake backend, address a production endpoint, or
resubmit an ambiguous ingress.  Every ledger candidate is read from the local
ICRC-3 log and every update is submitted through ``driver.call`` once.

The fixture discovers the configured token order so the flaky ckUSDC leg is
bound to the exact receipt leg under test.  The refund reply-loss case uses
that single funded leg, allowing admission to perform its full balance and
allowance preflight before the committed input and compensation interruptions.
"""
from __future__ import annotations

import hashlib
import json
from typing import Any


MIN_AGING_SECONDS = 2 * 24 * 60 * 60


def _nat(value: Any) -> int:
    if isinstance(value, dict) and "value" in value:
        value = value["value"]
    return int(str(value).replace("_", ""))


def _blob(value: bytes) -> str:
    return 'blob "' + "".join(f"\\{byte:02x}" for byte in value) + '"'


def _account(owner: str, subaccount: bytes | None = None) -> str:
    if subaccount is None:
        return f'record {{ owner=principal "{owner}"; subaccount=null }}'
    return f'record {{ owner=principal "{owner}"; subaccount=opt {_blob(subaccount)} }}'


def _some(value: Any) -> Any:
    return value.get("Some") if isinstance(value, dict) else None


def _status_name(receipt: dict[str, Any]) -> str:
    status = receipt.get("status")
    if isinstance(status, dict):
        return next(iter(status), "")
    return str(status)


def _variant_name(value: Any) -> str:
    if isinstance(value, dict) and "Some" in value:
        value = value["Some"]
    if isinstance(value, dict):
        return next(iter(value), "")
    return str(value)


def _pool_status(driver: Any) -> dict[str, Any]:
    return driver.call(
        driver.pool_id,
        "get_pool_status",
        did=driver.args.threepool_did,
        query=True,
    )[0]


def _pool_snapshot(driver: Any, ledger: str, flaky: bool = False) -> dict[str, Any]:
    status = _pool_status(driver)
    return {
        "pool_status": status,
        "balances": [_nat(value) for value in status.get("balances", [])],
        "lp_total_supply": _nat(status.get("lp_total_supply", 0)),
        "lp_balance": _nat(
            driver.call(
                driver.pool_id,
                "get_lp_balance",
                f'(principal "{driver.sp}")',
                did=driver.args.threepool_did,
                query=True,
            )[0]
        ),
        "pool_ledger_balance": driver.qbalance(ledger, driver.pool_id, flaky=flaky),
    }


def _ledger_tip(driver: Any, ledger: str, *, flaky: bool) -> int:
    did = driver.args.flaky_did if flaky else driver.args.ledger_did
    value = driver.call(
        ledger,
        "icrc3_get_blocks",
        "(vec {record {start=0 : nat; length=0 : nat}})",
        did=did,
        query=True,
    )[0]
    return _nat(value["log_length"])


def _direct_block(driver: Any, ledger: str, block: int, *, flaky: bool, label: str) -> dict[str, Any]:
    did = driver.args.flaky_did if flaky else driver.args.ledger_did
    value = driver.call(
        ledger,
        "icrc3_get_blocks",
        f"(vec {{record {{start={block} : nat; length=1 : nat}}}})",
        did=did,
        query=True,
    )[0]
    blocks = value.get("blocks", [])
    driver.check(
        label,
        not value.get("archived_blocks")
        and len(blocks) == 1
        and _nat(blocks[0].get("id")) == block,
    )
    return value


def _request(intent: bytes, amounts: list[int], max_fee: int) -> str:
    encoded = ";".join(f"{amount} : nat" for amount in amounts)
    return (
        f"(record {{source_subaccount=null;min_lp=0 : nat;amounts=vec {{{encoded}}};"
        f"max_transfer_fee=opt {max_fee} : opt nat;intent_id={_blob(intent)}}})"
    )


def _receipt(driver: Any, intent: bytes, *, sender: str, method: str, owner: str | None = None) -> Any:
    owner = owner or driver.user
    return driver.call(
        driver.pool_id,
        method,
        f'(principal "{owner}",{_blob(intent)},2 : nat8,0 : nat64)'
        if method.endswith("_for_v1") and "input" in method
        else f"({_blob(intent)},2 : nat8,0 : nat64)",
        sender=sender,
        did=driver.args.threepool_did,
    )


def _advance(driver: Any) -> dict[str, Any]:
    return driver.advance_local_time(MIN_AGING_SECONDS)


def _assert_receipt_pending(driver: Any, receipt: dict[str, Any], label: str) -> None:
    driver.check(label, _status_name(receipt) == "RecoveryPending")


def _proof_result(driver: Any, result: Any, label: str) -> dict[str, Any]:
    return driver.result_ok(result)


def _error_result(driver: Any, result: Any, label: str) -> Any:
    error = driver.result_err(result, ("RecoveryRequired", "NotFound", "PoolLocked"))
    driver.check(label, error is not None)
    return error


def _token_indices(driver: Any, ids: dict[str, str]) -> dict[str, int]:
    status = _pool_status(driver)
    indices: dict[str, int] = {}
    for index, token in enumerate(status.get("tokens", [])):
        ledger = token.get("ledger_id", {}) if isinstance(token, dict) else {}
        principal = ledger.get("principal") if isinstance(ledger, dict) else ledger
        for name, value in ids.items():
            if value == principal:
                indices[name] = index
    return indices


def _recovery_error_text(receipt: dict[str, Any]) -> str:
    return str(receipt.get("error", "")).lower()


def _canonical_too_old(receipt: dict[str, Any]) -> bool:
    # Candid/debug renderings vary (`TooOld`, `too old`, or punctuation), but
    # the normalized token must be the ledger's exact TooOld disposition.
    compact = "".join(character for character in _recovery_error_text(receipt) if character.isalnum())
    return "tooold" in compact


def _receipt_identity_payload(receipt: dict[str, Any]) -> tuple[Any, ...]:
    return tuple(
        receipt.get(field)
        for field in ("input_ids", "refund_ids", "input_transfers", "refund_transfers")
    )


def _save(driver: Any, name: str, value: Any) -> None:
    path = driver.out / name
    if path.exists():
        raise RuntimeError(f"refusing to overwrite immutable evidence {path}")
    path.write_text(json.dumps(value, indent=2, sort_keys=True))


def check_aged_pool_receipts(driver: Any, ids: dict[str, str]) -> dict[str, Any]:
    """Exercise permissionless exact proof for an aged input and refund.

    ``driver.configure()`` must already have installed and seeded the real
    pool and local ledgers.  The returned evidence is local-only and is also
    persisted by the caller's immutable ingress recorder.
    """
    driver.require_method(driver.args.threepool_did, "recover_liquidity_receipt_for_v1")
    driver.require_method(driver.args.threepool_did, "reconcile_liquidity_receipt_input_for_v1")
    driver.require_method(driver.args.threepool_did, "reconcile_liquidity_receipt_refund_for_v1")
    driver.require_method(driver.args.threepool_did, "get_active_liquidity_receipt_v1")

    ckusdc = ids["ckusdc"]
    icusd = ids["icusd"]
    ckusdt = ids["ckusdt"]
    indices = _token_indices(driver, ids)
    driver.check("aged_pool_token_indices_discovered", len(indices) >= 3)
    flaky_index = indices["ckusdc"]
    input_index = flaky_index

    # Case 1: a committed transfer_from loses its reply.  No recovery path
    # may infer no transfer from the expired retry window; the explicit block
    # proof must be the only route to the exact compensation.
    input_amount = 1_000_000
    input_fee = 1_000
    before_input = _pool_snapshot(driver, ckusdc, flaky=True)
    input_wallet = driver.qbalance(ckusdc, driver.user, flaky=True)
    input_other_wallet = driver.qbalance(ckusdc, driver.other, flaky=True)
    input_tip_before = _ledger_tip(driver, ckusdc, flaky=True)
    input_intent = hashlib.sha256(b"aged-pool-receipt-input-v1").digest()
    driver.call(
        ckusdc,
        "set_phantom_transfer_from_failures",
        "(1 : nat32)",
        did=driver.args.flaky_did,
    )
    pending_input = _proof_result(
        driver,
        driver.call(
            driver.pool_id,
            "add_liquidity_with_receipt_v1",
            _request(input_intent, [input_amount if index == input_index else 0 for index in range(3)], input_fee),
            sender=driver.user,
            did=driver.args.threepool_did,
        ),
        "aged_pool_input_receipt_returned",
    )
    _assert_receipt_pending(driver, pending_input, "aged_pool_input_lost_reply_pending")
    input_tip_after = _ledger_tip(driver, ckusdc, flaky=True)
    driver.check("aged_pool_input_logged_one_block", input_tip_after == input_tip_before + 1)
    input_candidate = input_tip_after - 1
    _direct_block(driver, ckusdc, input_candidate, flaky=True, label="aged_pool_input_direct_block")

    wrong_owner = driver.call(
        driver.pool_id,
        "reconcile_liquidity_receipt_input_for_v1",
        f'(principal "{driver.other}",{_blob(input_intent)},{input_index} : nat8,{input_candidate} : nat64)',
        sender=driver.other,
        did=driver.args.threepool_did,
    )
    _error_result(driver, wrong_owner, "aged_pool_input_wrong_owner_no_access")
    _advance(driver)
    aged_input = _proof_result(
        driver,
        driver.call(
            driver.pool_id,
            "recover_liquidity_receipt_for_v1",
            f'(principal "{driver.user}",{_blob(input_intent)})',
            sender=driver.other,
            did=driver.args.threepool_did,
        ),
        "aged_pool_input_recovery_returned",
    )
    driver.check(
        "aged_pool_input_too_old_held",
        _status_name(aged_input) == "RecoveryPending"
        and _canonical_too_old(aged_input)
        and _receipt_identity_payload(aged_input) == _receipt_identity_payload(pending_input),
    )
    # Upgrade while the durable receipt still owns the pool fence.  The
    # current candidate is installed through the local management canister;
    # the empty Candid tuple is only the upgrade argument and carries no
    # production state.
    driver.upgrade(driver.pool_id, driver.args.threepool_wasm, bytes.fromhex("4449444c0000"))
    active = driver.call(
        driver.pool_id,
        "get_active_liquidity_receipt_v1",
        did=driver.args.threepool_did,
        query=True,
    )[0]
    driver.check("aged_pool_input_upgrade_preserved_fence", _some(active) is not None)
    before_wrong_input_proof = {
        "wallet": driver.qbalance(ckusdc, driver.user, flaky=True),
        "pool": _pool_snapshot(driver, ckusdc, flaky=True),
    }
    wrong_candidate = driver.call(
        driver.pool_id,
        "reconcile_liquidity_receipt_input_for_v1",
        f'(principal "{driver.user}",{_blob(input_intent)},{input_index} : nat8,{input_candidate + 1} : nat64)',
        sender=driver.other,
        did=driver.args.threepool_did,
    )
    _error_result(driver, wrong_candidate, "aged_pool_input_wrong_candidate_held")
    driver.check(
        "aged_pool_input_wrong_candidate_no_movement",
        driver.qbalance(ckusdc, driver.user, flaky=True) == before_wrong_input_proof["wallet"]
        and _pool_snapshot(driver, ckusdc, flaky=True) == before_wrong_input_proof["pool"],
    )
    input_proof_result = driver.call(
        driver.pool_id,
        "reconcile_liquidity_receipt_input_for_v1",
        f'(principal "{driver.user}",{_blob(input_intent)},{input_index} : nat8,{input_candidate} : nat64)',
        sender=driver.other,
        did=driver.args.threepool_did,
    )
    if isinstance(input_proof_result, list) and input_proof_result and "Err" in input_proof_result[0]:
        driver.result_err(input_proof_result, ("RecoveryRequired",))
        retained_input = driver.call(
            driver.pool_id,
            "get_liquidity_receipt_v1",
            f'({_blob(input_intent)})',
            sender=driver.user,
            did=driver.args.threepool_did,
            query=True,
        )[0]
        retained_input = _some(retained_input)
        driver.check(
            "aged_pool_input_proof_confirmed_before_refund_retry",
            _status_name(retained_input) == "RecoveryPending"
            and _variant_name(retained_input["input_status"][input_index]) == "Confirmed"
            and _variant_name(retained_input["refund_status"][input_index]) == "RejectedNoTransfer"
            and _receipt_identity_payload(retained_input) == _receipt_identity_payload(pending_input),
        )
        input_proof = _proof_result(
            driver,
            driver.call(
                driver.pool_id,
                "recover_liquidity_receipt_for_v1",
                f'(principal "{driver.user}",{_blob(input_intent)})',
                sender=driver.other,
                did=driver.args.threepool_did,
            ),
            "aged_pool_input_refund_retry_terminal",
        )
    else:
        input_proof = _proof_result(driver, input_proof_result, "aged_pool_input_exact_proof_compensated")
    input_after = _pool_snapshot(driver, ckusdc, flaky=True)
    driver.check("aged_pool_input_refunded_terminal", _status_name(input_proof) == "Refunded")
    driver.check("aged_pool_input_lp_unchanged", input_after["lp_total_supply"] == before_input["lp_total_supply"] and input_after["lp_balance"] == before_input["lp_balance"])
    driver.check("aged_pool_input_pool_accounting_unchanged", input_after["pool_status"] == before_input["pool_status"])
    driver.check("aged_pool_input_exact_actual_fees", input_wallet - driver.qbalance(ckusdc, driver.user, flaky=True) == 2 * input_fee)
    driver.check("aged_pool_input_no_executor_payout", driver.qbalance(ckusdc, driver.other, flaky=True) == input_other_wallet)
    input_replay = _proof_result(
        driver,
        driver.call(
            driver.pool_id,
            "reconcile_liquidity_receipt_input_for_v1",
            f'(principal "{driver.user}",{_blob(input_intent)},{input_index} : nat8,{input_candidate} : nat64)',
            sender=driver.other,
            did=driver.args.threepool_did,
        ),
        "aged_pool_input_terminal_replay",
    )
    driver.check("aged_pool_input_replay_no_second_movement", input_replay == input_proof and _pool_snapshot(driver, ckusdc, flaky=True) == input_after)

    # Case 2 uses only the flaky leg.  Admission now checks every positive
    # leg's full balance and allowance before reserving a receipt, so a later
    # unfunded leg could never reach the committed input/refund interruption
    # this case is intended to exercise.
    refund_amount = 2_000_000
    refund_before = _pool_snapshot(driver, ckusdc, flaky=True)
    refund_wallet = driver.qbalance(ckusdc, driver.user, flaky=True)
    refund_executor_wallet = driver.qbalance(ckusdc, driver.other, flaky=True)
    refund_tip_before = _ledger_tip(driver, ckusdc, flaky=True)
    refund_intent = hashlib.sha256(b"aged-pool-receipt-refund-v1").digest()
    driver.call(
        ckusdc,
        "set_phantom_transfer_from_failures",
        "(1 : nat32)",
        did=driver.args.flaky_did,
    )
    amounts = [0, 0, 0]
    amounts[flaky_index] = refund_amount
    pending_refund = _proof_result(
        driver,
        driver.call(
            driver.pool_id,
            "add_liquidity_with_receipt_v1",
            _request(refund_intent, amounts, 100_000),
            sender=driver.user,
            did=driver.args.threepool_did,
        ),
        "aged_pool_refund_receipt_returned",
    )
    _assert_receipt_pending(driver, pending_refund, "aged_pool_refund_lost_reply_pending")
    refund_tip_after_input = _ledger_tip(driver, ckusdc, flaky=True)
    driver.check("aged_pool_refund_logged_input_block", refund_tip_after_input == refund_tip_before + 1)
    input_candidate = refund_tip_after_input - 1
    _direct_block(driver, ckusdc, input_candidate, flaky=True, label="aged_pool_refund_input_direct_block")
    # Resolve the committed input by duplicate proof while its exact retry
    # identity remains inside the ordinary deduplication window, then make
    # the compensation transfer lose its reply.  The recovery continuation
    # must persist the confirmed input and the exact refund attempt even when
    # the refund reply is lost.
    driver.call(
        ckusdc,
        "set_phantom_transfer_from_failures",
        "(0 : nat32)",
        did=driver.args.flaky_did,
    )
    driver.call(
        ckusdc,
        "set_phantom_icrc1_failures",
        "(1 : nat32)",
        did=driver.args.flaky_did,
    )
    within_window_refund = _proof_result(
        driver,
        driver.call(
            driver.pool_id,
            "recover_liquidity_receipt_for_v1",
            f'(principal "{driver.user}",{_blob(refund_intent)})',
            sender=driver.other,
            did=driver.args.threepool_did,
        ),
        "aged_pool_refund_within_window_recovery_returned",
    )
    driver.check(
        "aged_pool_refund_reply_lost_pending",
        _status_name(within_window_refund) == "RecoveryPending"
        and _variant_name(within_window_refund["input_status"][flaky_index]) == "Confirmed"
        and _receipt_identity_payload(within_window_refund) == _receipt_identity_payload(pending_refund),
    )
    refund_tip_after = _ledger_tip(driver, ckusdc, flaky=True)
    driver.check("aged_pool_refund_logged_input_and_refund_blocks", refund_tip_after == refund_tip_before + 2)
    refund_candidate = refund_tip_after - 1
    _direct_block(driver, ckusdc, refund_candidate, flaky=True, label="aged_pool_refund_refund_direct_block")
    aged_refund = within_window_refund
    driver.upgrade(driver.pool_id, driver.args.threepool_wasm, bytes.fromhex("4449444c0000"))
    active_after_refund_upgrade = driver.call(
        driver.pool_id,
        "get_active_liquidity_receipt_v1",
        did=driver.args.threepool_did,
        query=True,
    )[0]
    driver.check("aged_pool_refund_upgrade_preserved_fence", _some(active_after_refund_upgrade) is not None)
    _advance(driver)

    # The flaky token is also the refund token in this topology.  The input
    # candidate above is not a payout candidate; submitting that actual input
    # block to refund_for must leave the exact attempt held and pay nobody.
    before_wrong_refund = {
        "wallet": driver.qbalance(ckusdc, driver.user, flaky=True),
        "executor": driver.qbalance(ckusdc, driver.other, flaky=True),
    }
    wrong_refund = driver.call(
        driver.pool_id,
        "reconcile_liquidity_receipt_refund_for_v1",
        f'(principal "{driver.user}",{_blob(refund_intent)},{flaky_index} : nat8,{input_candidate} : nat64)',
        sender=driver.other,
        did=driver.args.threepool_did,
    )
    _error_result(driver, wrong_refund, "aged_pool_refund_wrong_candidate_held")
    driver.check(
        "aged_pool_refund_wrong_candidate_no_executor_pay",
        driver.qbalance(ckusdc, driver.user, flaky=True) == before_wrong_refund["wallet"]
        and driver.qbalance(ckusdc, driver.other, flaky=True) == before_wrong_refund["executor"],
    )
    refund_proof = _proof_result(
        driver,
        driver.call(
            driver.pool_id,
            "reconcile_liquidity_receipt_refund_for_v1",
            f'(principal "{driver.user}",{_blob(refund_intent)},{flaky_index} : nat8,{refund_candidate} : nat64)',
            sender=driver.other,
            did=driver.args.threepool_did,
        ),
        "aged_pool_refund_exact_proof_compensated",
    )
    refund_after = _pool_snapshot(driver, ckusdc, flaky=True)
    driver.check("aged_pool_refund_terminal", _status_name(refund_proof) == "Refunded")
    driver.check("aged_pool_refund_lp_unchanged", refund_after["lp_total_supply"] == refund_before["lp_total_supply"] and refund_after["lp_balance"] == refund_before["lp_balance"])
    driver.check("aged_pool_refund_pool_accounting_unchanged", refund_after["pool_status"] == refund_before["pool_status"])
    driver.check("aged_pool_refund_exact_actual_fees", refund_wallet - driver.qbalance(ckusdc, driver.user, flaky=True) == 2 * 1_000)
    driver.check("aged_pool_refund_no_executor_payout", driver.qbalance(ckusdc, driver.other, flaky=True) == refund_executor_wallet)
    refund_replay = _proof_result(
        driver,
        driver.call(
            driver.pool_id,
            "reconcile_liquidity_receipt_refund_for_v1",
            f'(principal "{driver.user}",{_blob(refund_intent)},{flaky_index} : nat8,{refund_candidate} : nat64)',
            sender=driver.other,
            did=driver.args.threepool_did,
        ),
        "aged_pool_refund_terminal_replay",
    )
    driver.check("aged_pool_refund_replay_no_second_movement", refund_replay == refund_proof and _pool_snapshot(driver, ckusdc, flaky=True) == refund_after)

    # Fence release must leave unrelated pool paths live.  The exact amounts
    # are intentionally small relative to the seeded pool, and the successful
    # calls themselves are the liveness proof after both abandoned receipts.
    driver.approve(icusd, driver.pool_id, driver.user, amount=10**18)
    driver.result_ok(driver.call(
        driver.pool_id,
        "add_liquidity",
        "(vec {1000000 : nat;0 : nat;0 : nat},0 : nat)",
        sender=driver.user,
        did=driver.args.threepool_did,
    ))
    driver.result_ok(driver.call(
        driver.pool_id,
        "swap",
        "(0 : nat8,1 : nat8,1000000 : nat,0 : nat)",
        sender=driver.user,
        did=driver.args.threepool_did,
    ))
    driver.check("aged_pool_unrelated_deposit_and_swap_after_fence", True)

    evidence = {
        "local_only": True,
        "production_calls": 0,
        "token_indices": indices,
        "input": {
            "intent_id_hex": input_intent.hex(),
            "candidate_block": input_candidate,
            "pending": pending_input,
            "aged": aged_input,
            "proof": input_proof,
            "replay": input_replay,
        },
        "refund": {
            "intent_id_hex": refund_intent.hex(),
            "candidate_block": refund_candidate,
            "pending": pending_refund,
            "aged": aged_refund,
            "proof": refund_proof,
            "replay": refund_replay,
        },
        "checks": driver.checks,
    }
    _save(driver, "aged-pool-receipt-proofs.json", evidence)
    return evidence


# Name used by callers that group local receipt fixtures under the aged rail.
check_aged_threepool_receipts = check_aged_pool_receipts

