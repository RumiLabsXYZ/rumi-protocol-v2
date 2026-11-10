#!/usr/bin/env python3
"""Offline Season 2 accounting calculator.

Input is an operator-prepared JSON manifest; this script makes no network calls
and never imports receipts into a canister. The required method must be stated
explicitly. ``event_time_integral_model`` is a disclosed constant-balance model,
not the inherited random two-snapshot calculation.

Schema (all times are nanoseconds; all amounts are integer USD-e8s):
{
  "method": "event_time_integral_model" | "inherited_two_snapshot",
  "campaign_start_ns": 1793491200000000000,
  "cutoff_ns": <exclusive end>,
  "eligible_repayment_ledgers": {"ckusdc": "<principal>", "ckusdt": "<principal>"},
  "participants": [{
    "principal_id": "...", "registered_at_ns": <time>,
    "intervals": [{"start_ns": ..., "end_ns": ...,
      "known_categories_mask": <integer bitmask>,
      "inputs": {<SnapshotInputs fields>},
      "a": {<SnapshotInputs fields>}, "b": {<SnapshotInputs fields>}
    }],
    "repayments": [{"source_tag": 0..3, "event_id": <nonnegative int>,
      "asset_ledger": "<configured principal>", "asset_evidence_ref": "<evidence>",
      "amount_usd_e8s": ..., "repaid_at_ns": ..., "window_end_ns": ...}]
  }]
}

The bit positions follow CATEGORIES (bit 0 is icusd_debt, through bit 6
vault_repayment). inputs is required for the event-time method. a and b are required
for the inherited method. The unused fields are rejected. SnapshotInputs keys
match ``accrual.rs``; values may be null only where the corresponding category
is marked unknown. The two 3pool categories share verification scaling, so they
must be known together. A category marked unknown is emitted as null, never 0.
For each interval, each position category and each repayment event is floored
separately; splitting an interval can therefore change the total. A ledger match
and evidence reference are operator attestations, not independently checked proof.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import tempfile
from pathlib import Path
from typing import Any

NANOS_PER_DAY = 86_400_000_000_000
REPAYMENT_WINDOW_NS = 90 * NANOS_PER_DAY
U64_MAX = (1 << 64) - 1
U128_MAX = (1 << 128) - 1
CAMPAIGN_START_NS = 1_793_491_200_000_000_000  # 2026-11-01 00:00 UTC

CATEGORIES = (
    "icusd_debt",
    "icusd_3pool",
    "fiat_stable_3pool",
    "icusd_stability_pool",
    "threeusd_stability_pool",
    "amm_lp",
    "vault_repayment",
)
POSITION_CATEGORIES = CATEGORIES[:-1]
ALL_CATEGORY_MASK = (1 << len(CATEGORIES)) - 1
CATEGORY_BITS = {name: 1 << i for i, name in enumerate(CATEGORIES)}
INPUT_FIELDS = (
    "vault_debt",
    "recorded_3pool_icusd",
    "recorded_3pool_usdc",
    "recorded_3pool_usdt",
    "verified_3usd",
    "sp_icusd",
    "sp_3usd",
    "amm_lp_value",
)


class AccountingError(ValueError):
    """Invalid or incomplete accounting manifest."""


def _is_int(value: Any) -> bool:
    return isinstance(value, int) and not isinstance(value, bool)


def _uint(value: Any, name: str, maximum: int = U128_MAX) -> int:
    if not _is_int(value) or value < 0 or value > maximum:
        raise AccountingError(f"{name} must be an integer in [0, {maximum}]")
    return value


def _sat_add(a: int, b: int) -> int:
    return min(U128_MAX, a + b)


def _sat_mul(a: int, b: int) -> int:
    return min(U128_MAX, a * b)


def _weighted(inputs: dict[str, int | None], include: set[str]) -> dict[str, int]:
    """Reproduce snapshot_weights_for_policy(..., fiat_stable_flat_4x=true)."""
    eff_icusd = eff_usdc = eff_usdt = 0
    if "icusd_3pool" in include or "fiat_stable_3pool" in include:
        pool_raw = [
            inputs["recorded_3pool_icusd"],
            inputs["recorded_3pool_usdc"],
            inputs["recorded_3pool_usdt"],
            inputs["verified_3usd"],
        ]
        if any(v is None for v in pool_raw):
            raise AccountingError("known 3pool categories require every verification input")
        rec_icusd, rec_usdc, rec_usdt, verified = (int(v) for v in pool_raw)
        total = _sat_add(_sat_add(rec_icusd, rec_usdc), rec_usdt)
        cap = _sat_mul(verified, 1005) // 1000
        capped = min(cap, total)
        if total:
            eff_icusd = _sat_mul(rec_icusd, capped) // total
            eff_usdc = _sat_mul(rec_usdc, capped) // total
            eff_usdt = _sat_mul(rec_usdt, capped) // total

    def required(field: str) -> int:
        value = inputs[field]
        if value is None:
            raise AccountingError(f"known category requires {field}")
        return value

    weights = {}
    if "icusd_debt" in include:
        weights["icusd_debt"] = required("vault_debt")
    if "icusd_3pool" in include:
        weights["icusd_3pool"] = eff_icusd
    if "fiat_stable_3pool" in include:
        weights["fiat_stable_3pool"] = _sat_mul(_sat_add(eff_usdc, eff_usdt), 4)
    if "icusd_stability_pool" in include:
        weights["icusd_stability_pool"] = required("sp_icusd")
    if "threeusd_stability_pool" in include:
        weights["threeusd_stability_pool"] = _sat_mul(required("sp_3usd"), 2)
    if "amm_lp" in include:
        weights["amm_lp"] = _sat_mul(required("amm_lp_value"), 2)
    return weights


def _scaled(weights: dict[str, int], duration_ns: int) -> dict[str, int]:
    return {name: _sat_mul(value, duration_ns) // NANOS_PER_DAY for name, value in weights.items()}


def _total(weights: dict[str, int]) -> int:
    total = 0
    for value in weights.values():
        total = _sat_add(total, value)
    return total


def _parse_inputs(raw: Any, context: str) -> dict[str, int | None]:
    if not isinstance(raw, dict) or set(raw) != set(INPUT_FIELDS):
        raise AccountingError(f"{context} must contain exactly SnapshotInputs fields: {', '.join(INPUT_FIELDS)}")
    parsed: dict[str, int | None] = {}
    for key in INPUT_FIELDS:
        value = raw[key]
        parsed[key] = None if value is None else _uint(value, f"{context}.{key}")
    return parsed


def _known_categories(raw: Any, context: str) -> set[str]:
    if not _is_int(raw) or raw < 0 or raw > ALL_CATEGORY_MASK:
        raise AccountingError(f"{context}.known_categories_mask must be an integer in [0, {ALL_CATEGORY_MASK}]")
    known = {name for name, bit in CATEGORY_BITS.items() if raw & bit}
    if ("icusd_3pool" in known) != ("fiat_stable_3pool" in known):
        raise AccountingError("the two 3pool categories must be known together because verification scaling is shared")
    return known


def _validate_interval_inputs(inputs: dict[str, int | None], known: set[str], context: str) -> None:
    requirements = {
        "icusd_debt": ("vault_debt",),
        "icusd_3pool": ("recorded_3pool_icusd", "recorded_3pool_usdc", "recorded_3pool_usdt", "verified_3usd"),
        "fiat_stable_3pool": ("recorded_3pool_icusd", "recorded_3pool_usdc", "recorded_3pool_usdt", "verified_3usd"),
        "icusd_stability_pool": ("sp_icusd",),
        "threeusd_stability_pool": ("sp_3usd",),
        "amm_lp": ("amm_lp_value",),
    }
    for category, fields in requirements.items():
        if category in known:
            for field in fields:
                if inputs[field] is None:
                    raise AccountingError(f"{context}: known {category} has null {field}")


def _period_points(weights: dict[str, int], duration_ns: int) -> dict[str, int]:
    return _scaled(weights, duration_ns)


def _parse_intervals(participant: dict[str, Any], method: str, coverage_start: int, cutoff: int) -> list[dict[str, Any]]:
    intervals = participant.get("intervals")
    if not isinstance(intervals, list):
        raise AccountingError("participant.intervals must be an array")
    if coverage_start == cutoff:
        if intervals:
            raise AccountingError("zero-duration participant must not include interval rows")
        return []
    if not intervals:
        raise AccountingError("missing interval history for a non-empty earning period")

    parsed = []
    cursor = coverage_start
    for i, raw in enumerate(intervals):
        context = f"participant interval {i}"
        if not isinstance(raw, dict):
            raise AccountingError(f"{context} must be an object")
        start = _uint(raw.get("start_ns"), f"{context}.start_ns", U64_MAX)
        end = _uint(raw.get("end_ns"), f"{context}.end_ns", U64_MAX)
        if start < CAMPAIGN_START_NS:
            raise AccountingError(f"{context} starts before the Season 2 campaign; Season 1 dates are not accepted")
        if end <= start or end > cutoff:
            raise AccountingError(f"{context} must have positive duration and end no later than the exclusive cutoff")
        if start != cursor:
            reason = "overlap" if start < cursor else "gap"
            raise AccountingError(f"{context} creates a temporal {reason}; expected start {cursor}")
        known = _known_categories(raw.get("known_categories_mask"), context)
        if method == "event_time_integral_model":
            if "inputs" not in raw or "a" in raw or "b" in raw:
                raise AccountingError(f"{context} requires inputs only for event_time_integral_model")
            inputs = _parse_inputs(raw["inputs"], context + ".inputs")
            _validate_interval_inputs(inputs, known, context)
            parsed.append({"start_ns": start, "end_ns": end, "known": known, "inputs": inputs})
        else:
            if "a" not in raw or "b" not in raw or "inputs" in raw:
                raise AccountingError(f"{context} requires a and b only for inherited_two_snapshot")
            position_known = known & set(POSITION_CATEGORIES)
            if position_known != set(POSITION_CATEGORIES):
                raise AccountingError("inherited_two_snapshot rejects partial unknown snapshot categories because min selects a whole snapshot")
            a = _parse_inputs(raw["a"], context + ".a")
            b = _parse_inputs(raw["b"], context + ".b")
            _validate_interval_inputs(a, set(POSITION_CATEGORIES), context + ".a")
            _validate_interval_inputs(b, set(POSITION_CATEGORIES), context + ".b")
            if any(v is None for v in a.values()) or any(v is None for v in b.values()):
                raise AccountingError("inherited_two_snapshot requires complete a and b SnapshotInputs")
            parsed.append({"start_ns": start, "end_ns": end, "known": known, "a": a, "b": b})
        cursor = end
    if cursor != cutoff:
        raise AccountingError(f"history ends at {cursor}; exact coverage must end at exclusive cutoff {cutoff}")
    return parsed


def _repayment_points(events: Any, effective_start: int, campaign_start: int, cutoff: int, intervals: list[dict[str, Any]], eligible_ledgers: Any, seen: set[tuple[int, int]]) -> int:
    if not isinstance(events, list):
        raise AccountingError("participant.repayments must be an array")
    total = 0
    for i, event in enumerate(events):
        context = f"participant repayment {i}"
        if not isinstance(event, dict):
            raise AccountingError(f"{context} must be an object")
        source = _uint(event.get("source_tag"), f"{context}.source_tag", 3)
        event_id = _uint(event.get("event_id"), f"{context}.event_id", U64_MAX)
        key = (source, event_id)
        if key in seen:
            raise AccountingError(f"{context} repeats source_tag/event_id {key}")
        seen.add(key)
        if source != 0:
            raise AccountingError(f"{context} must reference a backend event (source_tag 0)")
        if (not isinstance(eligible_ledgers, dict) or set(eligible_ledgers) != {"ckusdc", "ckusdt"}
            or any(not isinstance(v, str) or not v.strip() for v in eligible_ledgers.values())
            or eligible_ledgers["ckusdc"] == eligible_ledgers["ckusdt"]):
            raise AccountingError("eligible_repayment_ledgers must name distinct ckUSDC and ckUSDT ledger principals")
        ledger = event.get("asset_ledger")
        evidence = event.get("asset_evidence_ref")
        if not isinstance(ledger, str) or ledger not in eligible_ledgers.values():
            raise AccountingError(f"{context} requires a configured eligible asset_ledger")
        if not isinstance(evidence, str) or not evidence.strip():
            raise AccountingError(f"{context} requires an asset_evidence_ref")
        amount = _uint(event.get("amount_usd_e8s"), f"{context}.amount_usd_e8s")
        repaid_at = _uint(event.get("repaid_at_ns"), f"{context}.repaid_at_ns", U64_MAX)
        window_end = _uint(event.get("window_end_ns"), f"{context}.window_end_ns", U64_MAX)
        if repaid_at < campaign_start:
            raise AccountingError(f"{context} predates the Season 2 campaign; Season 1 repayment dates are not accepted")
        if repaid_at < effective_start or repaid_at >= cutoff:
            raise AccountingError(f"{context} must fall within this participant's covered Season 2 period [start, cutoff)")
        if window_end < repaid_at or window_end > min(U64_MAX, repaid_at + REPAYMENT_WINDOW_NS):
            raise AccountingError(f"{context} has an invalid repayment window")
        for interval in intervals:
            overlap_start = max(repaid_at, interval["start_ns"])
            overlap_end = min(window_end, cutoff, interval["end_ns"])
            overlap_ns = max(0, overlap_end - overlap_start)
            amount_x5 = _sat_mul(_sat_mul(amount, 5), overlap_ns)
            total = _sat_add(total, amount_x5 // NANOS_PER_DAY)
    return total


def calculate(manifest: Any) -> dict[str, Any]:
    if not isinstance(manifest, dict):
        raise AccountingError("manifest must be a JSON object")
    method = manifest.get("method")
    if method not in ("event_time_integral_model", "inherited_two_snapshot"):
        raise AccountingError("method is required and must be event_time_integral_model or inherited_two_snapshot")
    start = _uint(manifest.get("campaign_start_ns"), "campaign_start_ns", U64_MAX)
    cutoff = _uint(manifest.get("cutoff_ns"), "cutoff_ns", U64_MAX)
    if start != CAMPAIGN_START_NS:
        raise AccountingError("campaign_start_ns must be 2026-11-01 00:00 UTC; Season 1 dates are not accepted")
    if cutoff <= start:
        raise AccountingError("cutoff_ns must be after campaign_start_ns")
    participants = manifest.get("participants")
    if not isinstance(participants, list) or not participants:
        raise AccountingError("participants must be a non-empty array; an empty file is not evidence of zero credits")

    ids: set[str] = set()
    repayment_ids: set[tuple[int, int]] = set()
    rows: list[dict[str, Any]] = []
    aggregate_values = {name: 0 for name in CATEGORIES}
    aggregate_known = {name: True for name in CATEGORIES}
    for participant in participants:
        if not isinstance(participant, dict):
            raise AccountingError("each participant must be an object")
        principal_id = participant.get("principal_id")
        if not isinstance(principal_id, str) or not principal_id:
            raise AccountingError("participant.principal_id must be a non-empty string")
        if principal_id in ids:
            raise AccountingError(f"duplicate participant {principal_id}")
        ids.add(principal_id)
        registered = _uint(participant.get("registered_at_ns"), f"{principal_id}.registered_at_ns", U64_MAX)
        if registered >= cutoff:
            raise AccountingError(f"{principal_id} registration must precede the exclusive cutoff")
        if "repayments" not in participant:
            raise AccountingError(f"{principal_id}.repayments is required; use [] only when its history is known empty")
        # Existing Season 1 registrants may participate, but earned intervals
        # begin no earlier than the Season 2 campaign boundary.
        if method == "inherited_two_snapshot" and registered > start and (registered - start) % (7 * NANOS_PER_DAY) != 0:
            raise AccountingError("inherited_two_snapshot cannot infer capture eligibility for a registration inside an epoch")
        effective_start = max(start, registered)
        intervals = _parse_intervals(participant, method, effective_start, cutoff)
        known_all = {name: True for name in CATEGORIES}
        category_totals = {name: 0 for name in CATEGORIES}

        for interval in intervals:
            known = interval["known"]
            for category in CATEGORIES:
                if category not in known:
                    known_all[category] = False
            if method == "event_time_integral_model":
                weights = _weighted(interval["inputs"], known & set(POSITION_CATEGORIES))
            else:
                position_known = set(POSITION_CATEGORIES)
                wa = _weighted(interval["a"], position_known)
                wb = _weighted(interval["b"], position_known)
                weights = wa if _total(wa) <= _total(wb) else wb
            points = _period_points(weights, interval["end_ns"] - interval["start_ns"])
            for category in POSITION_CATEGORIES:
                if category in known and category in points:
                    category_totals[category] = _sat_add(category_totals[category], points[category])

        repayment_known = all("vault_repayment" in i["known"] for i in intervals)
        if not intervals:
            repayment_known = False
        if repayment_known:
            category_totals["vault_repayment"] = _repayment_points(
                participant["repayments"], effective_start, start, cutoff, intervals,
                manifest.get("eligible_repayment_ledgers"), repayment_ids
            )
        else:
            # Validate supplied event rows even when they cannot be credited, so
            # pre-season or duplicated events are never silently carried forward.
            _repayment_points(participant["repayments"], effective_start, start, cutoff, intervals,
                              manifest.get("eligible_repayment_ledgers"), repayment_ids)
        known_all["vault_repayment"] = repayment_known

        out_categories = {
            name: category_totals[name] if known_all[name] else None for name in CATEGORIES
        }
        unresolved_mask = sum(CATEGORY_BITS[name] for name in CATEGORIES if not known_all[name])
        participant_total = None if unresolved_mask else _sum_values(category_totals.values())
        rows.append({
            "principal_id": principal_id,
            "registered_at_ns": registered,
            "earning_start_ns": effective_start,
            "cutoff_ns": cutoff,
            "category_points": out_categories,
            "unresolved_categories_mask": unresolved_mask,
            "total_points": participant_total,
        })
        for category in CATEGORIES:
            if known_all[category]:
                aggregate_values[category] = _sat_add(aggregate_values[category], category_totals[category])
            else:
                aggregate_known[category] = False

    aggregate_categories = {
        name: aggregate_values[name] if aggregate_known[name] else None for name in CATEGORIES
    }
    all_known = all(aggregate_known.values())
    aggregate_total = _sum_values(aggregate_values.values()) if all_known else None
    return {
        "method": method,
        "campaign_start_ns": start,
        "cutoff_ns": cutoff,
        "coverage_complete": True,
        "all_categories_known": all_known,
        "provisional": True,
        "credits_approved": False,
        "receipt_import_performed": False,
        "note": (
            "Offline calculation only. No credits are approved or imported. "
            "event_time_integral_model is a disclosed constant-balance model, "
            "not the inherited random two-snapshot capture. Each interval and repayment event is floored separately."
            if method == "event_time_integral_model"
            else "Offline calculation only: operator-supplied snapshot arithmetic, not proof of captures. "
                 "Correct epoch bounds and actual A/B capture eligibility are input preconditions not verified by this calculator; "
                 "whole-snapshot minimum and per-interval per-event flooring are used. No credits are approved or imported."
        ),
        "participants": rows,
        "aggregate_category_points": aggregate_categories,
        "aggregate_total_points": aggregate_total,
    }


def _sum_values(values: Any) -> int:
    total = 0
    for value in values:
        total = _sat_add(total, value)
    return total


def _write_private(path: Path, result: dict[str, Any]) -> None:
    private_dir = Path(tempfile.gettempdir()) / "rumi-season2-accounting-private"
    if private_dir.is_symlink():
        raise AccountingError(f"private output directory must not be a symlink: {private_dir}")
    private_dir = private_dir.resolve()
    target = path.resolve()
    if target.parent != private_dir:
        raise AccountingError(f"output must be directly inside {private_dir}")
    private_dir.mkdir(parents=True, exist_ok=True, mode=0o700)
    os.chmod(private_dir, 0o700)
    payload = json.dumps(result, indent=2, sort_keys=True) + "\n"
    fd = os.open(target, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8") as stream:
        stream.write(payload)
    os.chmod(target, 0o600)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("input", type=Path, help="operator-prepared JSON manifest")
    parser.add_argument(
        "--output",
        type=Path,
        default=Path(tempfile.gettempdir()) / "rumi-season2-accounting-private" / "provisional.json",
        help="private output file (must be directly inside the task-specific temp directory)",
    )
    args = parser.parse_args(argv)
    try:
        with args.input.open("r", encoding="utf-8") as source:
            manifest = json.load(source)
        result = calculate(manifest)
        _write_private(args.output, result)
    except (OSError, json.JSONDecodeError, AccountingError) as exc:
        print(f"season2_accounting: {exc}", file=sys.stderr)
        return 2
    print(f"Wrote provisional, unapproved calculation to {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
