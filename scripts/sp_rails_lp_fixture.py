#!/usr/bin/env python3
"""Small, importable fixture for the local 3USD reserve liquidation rail.

The main PocketIC harness owns server lifecycle and immutable ingress evidence.
This module only drives the 3USD-specific fixture after ``configure()`` has
installed the real candidate Wasms and local ICRC ledgers.  It deliberately
does not contain a fallback fake backend, a test-only liquidation endpoint, or
any production URL.
"""
from __future__ import annotations

from typing import Any


def _nat(value: Any) -> int:
    """Decode the JSON decoder's nat representation without losing precision."""
    if isinstance(value, dict) and "value" in value:
        value = value["value"]
    return int(str(value).replace("_", ""))


def _account(owner: str) -> str:
    return f'record {{ owner=principal "{owner}"; subaccount=null }}'


def _blob(value: bytes) -> str:
    return 'blob "' + "".join(f"\\{byte:02x}" for byte in value) + '"'


def _some(value: Any) -> Any:
    if isinstance(value, dict) and "Some" in value:
        return value["Some"]
    return None


def _owner(account: Any) -> str | None:
    if not isinstance(account, dict):
        return None
    owner = account.get("owner")
    if isinstance(owner, dict):
        return owner.get("principal")
    return owner if isinstance(owner, str) else None


def _book(position: dict[str, Any] | None, ledger: str) -> int:
    if not position:
        return 0
    for row in position.get("stablecoin_balances", []):
        if isinstance(row, dict) and row.get("0", {}).get("principal") == ledger:
            return _nat(row["1"])
    return 0


def _gain(position: dict[str, Any] | None, ledger: str) -> int:
    if not position:
        return 0
    for row in position.get("collateral_gains", []):
        if isinstance(row, dict) and row.get("0", {}).get("principal") == ledger:
            return _nat(row["1"])
    return 0


def _pool_total(driver: Any, ledger: str) -> int:
    status = driver.call(
        driver.sp,
        "get_pool_status",
        did=driver.args.sp_did,
        query=True,
    )[0]
    for row in status.get("stablecoin_balances", []):
        if isinstance(row, dict) and row.get("0", {}).get("principal") == ledger:
            return _nat(row["1"])
    return 0


def _lp_ledger_balance(driver: Any) -> int:
    value = driver.call(
        driver.pool_id,
        "get_lp_balance",
        f'(principal "{driver.sp}")',
        did=driver.args.threepool_did,
        query=True,
    )
    return _nat(value[0])


def _advance_local_time(driver: Any, seconds: int) -> dict[str, int]:
    """Advance only this PocketIC instance so SP's real timers can run."""
    current = driver.http(driver.endpoint("read/get_time"), method="GET")
    before = int(current["nanos_since_epoch"])
    after = before + seconds * 1_000_000_000
    driver.http(driver.endpoint("update/set_time"), {"nanos_since_epoch": after})
    # A timer callback can enqueue an inter-canister call.  Ticking a few
    # times lets that callback and its reply complete without resubmitting an
    # ingress or fabricating a recovery result.
    for _ in range(5):
        driver.http(driver.endpoint("update/tick"), {"blockmakers": None})
    return {"before_nanos": before, "after_nanos": after, "seconds": seconds}


def _direct_icrc3_block(driver: Any, ledger: str, did: Any, block_index: int) -> dict[str, Any]:
    """Read the candidate block directly from the local ledger log."""
    value = driver.call(
        ledger,
        "icrc3_get_blocks",
        f'(vec {{record {{start={block_index} : nat; length=1 : nat}}}})',
        did=did,
        query=True,
    )[0]
    blocks = value.get("blocks", [])
    archived = value.get("archived_blocks", [])
    driver.check(
        "threeusd_direct_icrc3_block",
        _nat(value.get("log_length", 0)) > block_index
        and not archived
        and len(blocks) == 1
        and _nat(blocks[0].get("id")) == block_index,
    )
    return value


def _status(driver: Any, vault_id: int, absorb_id: int) -> dict[str, Any]:
    value = driver.call(
        driver.backend_id,
        "get_stability_pool_liquidate_with_reserves_v2_status",
        f"({vault_id} : nat64,{absorb_id} : nat64)",
        sender=driver.sp,
        did=driver.args.backend_did,
        query=True,
    )
    return value[0]


def _candidate(driver: Any, vault_id: int, absorb_id: int) -> Any:
    value = driver.call(
        driver.backend_id,
        "get_stability_pool_liquidate_with_reserves_v2_payout_candidate",
        f"({vault_id} : nat64,{absorb_id} : nat64)",
        sender=driver.sp,
        did=driver.args.backend_did,
        query=True,
    )
    return _some(value[0])


def _inventory(driver: Any) -> dict[str, Any]:
    value = driver.call(
        driver.sp,
        "admin_get_three_usd_absorb_inventory_page",
        "(null,32 : nat16)",
        sender=driver.admin,
        did=driver.args.sp_did,
        query=True,
    )
    return driver.result_ok(value)


def _vault_debt(driver: Any, vault_id: int) -> int:
    values = driver.call(
        driver.backend_id,
        "get_vaults",
        f'(opt principal "{driver.user}")',
        did=driver.args.backend_did,
        query=True,
    )[0]
    for row in values:
        if _nat(row.get("vault_id")) == vault_id:
            return _nat(row.get("borrowed_icusd_amount", 0))
    return 0


def _pending_for(page: dict[str, Any], vault_id: int) -> dict[str, Any] | None:
    for entry in page.get("entries", []):
        pending = entry.get("Pending") if isinstance(entry, dict) else None
        if pending is not None and _nat(pending.get("vault_id")) == vault_id:
            return pending
    return None


def _drain_non_lp_books(driver: Any, ids: dict[str, str]) -> dict[str, int]:
    """Make the 3USD path the only eligible SP stable rail for this vault."""
    drained: dict[str, int] = {}
    for name in ("icusd", "ckusdt", "ckusdc"):
        ledger = ids[name]
        amount = driver.pool_book(ledger)
        if amount == 0:
            continue
        flaky = name == "ckusdc"
        value = driver.call(
            driver.sp,
            "withdraw",
            f'(principal "{ledger}",{amount} : nat64)',
            sender=driver.user,
            did=driver.args.sp_did,
        )
        driver.result_ok(value)
        driver.check(f"threeusd_drain_{name}", driver.pool_book(ledger) == 0)
        drained[name] = amount
        # ``flaky`` is intentionally only explanatory here.  The regular
        # withdraw call is still decoded by the SP DID; no fixture fault is
        # armed by this helper.
        del flaky
    return drained


def _principal(value):
    return value.get("principal") if isinstance(value,dict) else value


def check_threeusd_reserve_liquidation(driver: Any, ids: dict[str,str]) -> dict[str,Any]:
    """Exercise a real V2 LP pull, debt commit, collateral proof and exact replay.

    A normal direct LP deposit isolates liquidation from conversion failures.
    Local admin risk setters make a 200%-CR vault unsafe at the real cached
    XRC $10 price. PocketIC supplies the configured backend caller identity;
    this tests the SP notification handler, not the backend scan scheduler.
    """
    collateral=ids["collateral"]
    drained=_drain_non_lp_books(driver,ids)
    if driver.pool_book(driver.pool_id)<2_000_000_000:
        driver.result_ok(driver.call(driver.pool_id,"icrc2_approve",f'(record {{spender={_account(driver.sp)};amount=2000000000 : nat;expected_allowance=null;expires_at=null;fee=null;memo=null;from_subaccount=null;created_at_time=null}})',sender=driver.user,did=driver.args.threepool_did))
        seq=getattr(driver,"deposit_seq",2)+1
        driver.result_ok(driver.call(driver.sp,"deposit_with_intent",f'({seq} : nat64,principal "{driver.pool_id}",2000000000 : nat64)',sender=driver.user))
        driver.deposit_seq=seq
    driver.local_collateral_policy(collateral,False)
    driver.result_ok(driver.call(driver.backend_id,"set_min_icusd_amount","(10000000 : nat64)",did=driver.args.backend_did))
    operation_id=getattr(driver,"vault_operation_seq",0)+1
    opened=driver.result_ok(driver.call(driver.backend_id,"open_vault_and_borrow_v2",f'(record {{collateral_amount=1000000000 : nat64;operation_id={operation_id} : nat64;borrow_amount=5000000000 : nat64;collateral_type=opt principal "{collateral}"}})',sender=driver.user,did=driver.args.backend_did))
    vault_id=_nat(opened["vault_id"])
    driver.vault_operation_seq=operation_id
    driver.result_ok(driver.call(driver.backend_id,"ack_vault_operation",f'(record {{operation_id={operation_id} : nat64}})',sender=driver.user,did=driver.args.backend_did))
    driver.local_collateral_policy(collateral,True)
    def reserve_book():
        return _nat(driver.call(driver.backend_id,"get_protocol_3usd_reserves",did=driver.args.backend_did,query=True)[0])
    def lp(owner):
        return _nat(driver.call(driver.pool_id,"get_lp_balance",f'(principal "{owner}")',did=driver.args.threepool_did,query=True)[0])
    before={"position":driver.pool_position(),"sp_lp":lp(driver.sp),"backend_lp":lp(driver.backend_id),"reserve":reserve_book(),"debt":_vault_debt(driver,vault_id),"collateral":driver.qbalance(collateral,driver.sp),"total":_pool_total(driver,driver.pool_id)}
    info=f'(vec {{record {{vault_id={vault_id};collateral_type=principal "{collateral}";debt_amount=5000000000;collateral_amount=1000000000;recommended_liquidation_amount=5000000000;collateral_price_e8s=1000000000}}}})'
    notification=driver.call(driver.sp,"notify_liquidatable_vaults",info,sender=driver.backend_id)
    driver.check("threeusd_notification_reached_sp",bool(notification[0]))
    wrong=driver.call(driver.sp,"notify_liquidatable_vaults",info,sender=driver.other)
    driver.check("threeusd_wrong_caller_rejected",wrong==[[]])
    inventory=driver.result_ok(driver.call(driver.backend_id,"get_three_usd_reserve_inventory_page","(null,32 : nat16)",did=driver.args.backend_did,query=True))
    ingress=next((entry["Ingress"] for entry in inventory["entries"] if "Ingress" in entry and _nat(entry["Ingress"]["key"]["vault_id"])==vault_id),None)
    driver.check("threeusd_backend_ingress_journal_retained",ingress is not None)
    if ingress is None: raise RuntimeError("no exact backend LP ingress journal")
    absorb_id=_nat(ingress["key"]["absorb_id"])
    request=ingress["row"]["request"]
    amount=_nat(request["three_usd_amount_e8s"])
    debt_target=_nat(request["icusd_debt_covered_e8s"])
    status=_status(driver,vault_id,absorb_id)
    driver.check("threeusd_status_bound_to_intent",_nat(status["vault_id"])==vault_id and _nat(status["absorb_id"])==absorb_id and _principal(status["stability_pool"])==driver.sp)
    steps=[]
    for _ in range(12):
        position=driver.pool_position()
        if "Absorbed" in status["status"] and _book(position,driver.pool_id)<_book(before["position"],driver.pool_id):break
        steps.append(_advance_local_time(driver,31))
        status=_status(driver,vault_id,absorb_id)
    driver.check("threeusd_absorbed_status_terminal","Absorbed" in status["status"])
    absorbed=status["status"]["Absorbed"]
    result=absorbed["result"]
    liquidated=_nat(result["liquidated_debt"])
    driver.check("threeusd_positive_debt_commit",0<liquidated<=before["debt"] and liquidated<=debt_target)
    realized=min(amount,amount*liquidated//debt_target) if liquidated<debt_target else amount
    ingress_fee=_nat(absorbed["ingress_fee_e8s"])
    refund=_some(absorbed["proportional_refund"])
    expected_debit=realized+(0 if refund else ingress_fee)
    position=driver.pool_position()
    after={"position":position,"sp_lp":lp(driver.sp),"backend_lp":lp(driver.backend_id),"reserve":reserve_book(),"debt":_vault_debt(driver,vault_id),"collateral":driver.qbalance(collateral,driver.sp),"total":_pool_total(driver,driver.pool_id)}
    driver.check("threeusd_sp_user_debit_exact",_book(before["position"],driver.pool_id)-_book(position,driver.pool_id)==expected_debit)
    driver.check("threeusd_sp_aggregate_debit_exact",before["total"]-after["total"]==expected_debit)
    driver.check("threeusd_lp_physical_pull_exact",before["sp_lp"]-after["sp_lp"]==expected_debit)
    driver.check("threeusd_backend_physical_received_exact",after["backend_lp"]-before["backend_lp"]==realized)
    driver.check("threeusd_backend_reserve_book_exact",after["reserve"]-before["reserve"]==realized)
    driver.check("threeusd_backend_debt_exact",before["debt"]-after["debt"]==liquidated)
    driver.check("threeusd_pending_intent_cleared",_pending_for(_inventory(driver),vault_id) is None)
    candidate=_candidate(driver,vault_id,absorb_id)
    driver.check("threeusd_payout_candidate_retained",candidate is not None)
    net=_nat(candidate["net_e8s"])
    driver.check("threeusd_payout_tuple_exact",_owner(candidate["source"])==driver.backend_id and _owner(candidate["destination"])==driver.sp and _principal(candidate["ledger"])==collateral and net+_nat(candidate["expected_fee_e8s"])==_nat(candidate["gross_e8s"])==_nat(result["collateral_received"]))
    driver.check("threeusd_collateral_receipt_exact",after["collateral"]-before["collateral"]==net and net>0)
    driver.check("threeusd_collateral_gain_exact",_gain(position,collateral)-_gain(before["position"],collateral)==net)
    block=_some(candidate["candidate_block_index"])
    driver.check("threeusd_payout_block_recorded",block is not None)
    payout_block=_direct_icrc3_block(driver,collateral,driver.args.ledger_did,_nat(block))
    refund_block=None
    if refund:
        tuple_value=refund["tuple"]
        driver.check("threeusd_proportional_refund_exact",_nat(tuple_value["amount_e8s"])==amount-realized+ingress_fee and _nat(tuple_value["charged_fee_e8s"])==0 and _principal(tuple_value["source_owner"])==driver.backend_id and _owner(tuple_value["destination"])==driver.sp)
        refund_block=_direct_icrc3_block(driver,driver.pool_id,driver.args.threepool_did,_nat(refund["block_index"]))
    replay=driver.call(driver.backend_id,"stability_pool_liquidate_with_reserves_v2",f'({vault_id} : nat64,{absorb_id} : nat64,{debt_target} : nat64,{amount} : nat64,principal "{driver.pool_id}")',sender=driver.sp,did=driver.args.backend_did)
    driver.result_ok(replay)
    driver.check("threeusd_exact_id_replay_no_second_lp",lp(driver.sp)==after["sp_lp"] and lp(driver.backend_id)==after["backend_lp"] and reserve_book()==after["reserve"])
    driver.check("threeusd_exact_id_replay_no_second_debt_or_payout",_vault_debt(driver,vault_id)==after["debt"] and driver.qbalance(collateral,driver.sp)==after["collateral"] and driver.pool_position()==position)
    return {"vault_id":vault_id,"absorb_id":absorb_id,"ingress":ingress,"status":status,"candidate":candidate,"before":before,"after":after,"payout_block":payout_block,"refund":refund,"refund_block":refund_block,"time_steps":steps,"notification":notification,"replay":replay,"drained":drained}
