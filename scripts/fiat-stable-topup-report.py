#!/usr/bin/env python3
"""Read-only preview of the fiat-stable historical points uplift.

The preview reads one immutable prefix of the global point ledger.  It never
calls an update method and never reads principal state to reconstruct history:
the ledger rows are the source of truth for both the population and the
reconciliation.  Historical unmatched rows are adjusted per row with
``floor(points_delta / 3)``.  The result is an approximate recorded-contribution
4x view, not a replay of the discarded snapshot inputs.

The live query path deliberately uses the anonymous identity, ``--query``, the
``ic`` network, and the checked-in current Candid file.  Use ``--cutoff`` to
pin an exclusive ledger offset; when omitted, the length read immediately
before pagination is used as the prefix boundary.
"""

from __future__ import annotations

import argparse
import csv
import json
import re
import subprocess
import sys
from decimal import Decimal, ROUND_HALF_UP
from datetime import datetime, timezone
from pathlib import Path
from typing import Callable, Iterable


MAINNET_POINTS = "bfnu3-6aaaa-aaaab-qhanq-cai"
E8S = 100_000_000
PAGE_SIZE = 1_000
U128_MAX = (1 << 128) - 1
NANOS_PER_DAY = 86_400_000_000_000
# `scale_by_period` saturates the multiplication before dividing by the day
# constant.  Preserve the backend's exact observable sentinel rather than
# treating it as an ordinary historical points amount.
SCALED_SATURATION_SENTINEL = U128_MAX // NANOS_PER_DAY

KNOWN_SOURCES = {
    "Registration",
    "IcUsdDebt",
    "IcUsd3Pool",
    "CkStable3PoolUnmatched",
    "CkStable3PoolMatched",
    "CkStable3PoolFlat4x",
    "CkStable3PoolUnmatchedTopUp",
    "VaultRepayment",
    "IcUsdStabilityPool",
    "ThreeUsdStabilityPool",
    "AmmLp",
}
TOPUP_SOURCE = "CkStable3PoolUnmatchedTopUp"
FLAT_SOURCE = "CkStable3PoolFlat4x"
UNMATCHED_SOURCE = "CkStable3PoolUnmatched"


class ReportError(RuntimeError):
    """A malformed, drifting, or ambiguous read-only report input."""


def _candid_text(raw: str) -> str:
    """Return Candid text from plain output or ``icp --json`` output."""
    try:
        value = json.loads(raw)
    except json.JSONDecodeError:
        return raw
    if isinstance(value, dict):
        text = value.get("response_candid")
        if isinstance(text, str):
            return text
        text = value.get("response_text")
        if isinstance(text, str):
            return text
    raise ReportError("icp JSON output did not contain response_candid/response_text")


def _record_bodies(text: str) -> Iterable[str]:
    """Yield balanced Candid ``record { ... }`` bodies, including nested ones."""
    for match in re.finditer(r"\brecord\s*\{", text):
        depth = 1
        in_string = False
        escaped = False
        i = match.end()
        while i < len(text) and depth:
            char = text[i]
            if in_string:
                if escaped:
                    escaped = False
                elif char == "\\":
                    escaped = True
                elif char == '"':
                    in_string = False
            elif char == '"':
                in_string = True
            elif char == "{":
                depth += 1
            elif char == "}":
                depth -= 1
            i += 1
        if depth == 0:
            yield text[match.end() : i - 1]


def _field_int(body: str, field: str, required: bool = True) -> int | None:
    match = re.search(
        rf'"?{re.escape(field)}"?\s*=\s*([0-9][0-9_]*)\s*:\s*nat(?:8|32|64)?\b',
        body,
    )
    if not match:
        if required:
            raise ReportError(f"ledger record is missing integer field {field!r}")
        return None
    return int(match.group(1).replace("_", ""))


def _parse_entry_body(body: str) -> dict[str, int | str] | None:
    # The page itself is also a record containing nested PointEntry records.
    # Only parse the leaf record so the first entry is not counted twice.
    if "record {" in body or "points_delta" not in body or "source" not in body:
        return None
    principal = re.search(
        r'"?principal"?\s*=\s*principal\s+"([a-z0-9-]+)"', body
    )
    source = re.search(r'"?source"?\s*=\s*variant\s*\{\s*([A-Za-z0-9_]+)', body)
    if not principal or not source:
        raise ReportError("ledger entry is missing principal or source")
    source_name = source.group(1)
    if source_name not in KNOWN_SOURCES:
        raise ReportError(f"unknown PointSource variant {source_name!r}")
    epoch = _field_int(body, "epoch_index")
    delta = _field_int(body, "points_delta")
    recorded_at = _field_int(body, "recorded_at_ns")
    assert epoch is not None and delta is not None and recorded_at is not None
    return {
        "principal": principal.group(1),
        "epoch": epoch,
        "delta": delta,
        "source": source_name,
        "recorded_at_ns": recorded_at,
    }


def parse_page(raw: str) -> dict[str, object]:
    """Parse one global ``get_point_entries`` page from Candid or JSON output."""
    text = _candid_text(raw)
    next_offset = _field_int(text, "next_offset")
    if next_offset is None:
        raise ReportError("ledger page is missing next_offset")
    reached_match = re.search(r'"?reached_end"?\s*=\s*(true|false)', text)
    if not reached_match:
        raise ReportError("ledger page is missing reached_end")
    entries = []
    for body in _record_bodies(text):
        entry = _parse_entry_body(body)
        if entry is not None:
            entries.append(entry)
    return {
        "entries": entries,
        "next_offset": next_offset,
        "reached_end": reached_match.group(1) == "true",
    }


def parse_nat(raw: str) -> int:
    """Parse a scalar nat response, including the tuple wrapper from --json."""
    text = _candid_text(raw)
    match = re.search(r"\b([0-9][0-9_]*)\s*:\s*nat(?:8|32|64)?\b", text)
    if not match:
        raise ReportError("icp response did not contain a nat")
    return int(match.group(1).replace("_", ""))


def collect_prefix(
    query_page: Callable[[int, int], str],
    ledger_length: int,
    cutoff: int | None = None,
    page_size: int = PAGE_SIZE,
) -> list[dict[str, int | str]]:
    """Read exactly the immutable prefix ``[0, cutoff)`` with forward cursors."""
    if ledger_length < 0:
        raise ReportError("ledger length cannot be negative")
    boundary = ledger_length if cutoff is None else cutoff
    if boundary < 0 or boundary > ledger_length:
        raise ReportError(
            f"--cutoff must be between 0 and current ledger length {ledger_length}"
        )
    if page_size <= 0 or page_size > PAGE_SIZE:
        raise ReportError(f"page size must be in 1..{PAGE_SIZE}")

    rows: list[dict[str, int | str]] = []
    offset = 0
    while offset < boundary:
        page = parse_page(query_page(offset, page_size))
        entries = page["entries"]
        assert isinstance(entries, list)
        next_offset = page["next_offset"]
        assert isinstance(next_offset, int)
        reached_end = page["reached_end"]
        assert isinstance(reached_end, bool)
        span = next_offset - offset
        if span <= 0:
            raise ReportError(f"ledger cursor did not advance from {offset}")
        if len(entries) != span:
            raise ReportError(
                f"ledger page at {offset} has {len(entries)} rows for cursor span {span}"
            )
        if next_offset > ledger_length:
            raise ReportError(
                f"ledger cursor {next_offset} exceeds observed length {ledger_length}"
            )
        take = min(len(entries), boundary - offset)
        rows.extend(entries[:take])
        offset += take
        if offset == boundary:
            break
        if next_offset != offset:
            raise ReportError("ledger page did not cover the requested contiguous prefix")
        if reached_end:
            raise ReportError(
                f"ledger ended at {next_offset} before requested cutoff {boundary}"
            )
    return rows


def _display_e8s(value: int) -> str:
    whole, fraction = divmod(value, E8S)
    if fraction:
        return f"{whole:,}.{fraction:08d}".rstrip("0")
    return f"{whole:,}.00000000"


def _display_percent(numerator: int, denominator: int) -> str:
    if denominator == 0:
        return "0.000000%"
    value = (Decimal(numerator) * Decimal(100) / Decimal(denominator)).quantize(
        Decimal("0.000001"), rounding=ROUND_HALF_UP
    )
    return f"{value:.6f}%"


def _share_bps(points: int, total: int) -> int:
    return 0 if total == 0 else (points * 10_000) // total


def _population(entries: list[dict[str, int | str]]) -> list[str]:
    """Return registered markers plus principals with positive ledger rows."""
    markers = {
        str(entry["principal"])
        for entry in entries
        if entry["source"] == "Registration"
    }
    positive = {
        str(entry["principal"])
        for entry in entries
        if int(entry["delta"]) > 0
    }
    return sorted(markers | positive)


def build_report(
    entries: list[dict[str, int | str]],
    *,
    ledger_length: int,
    cutoff: int,
    cutover_epoch: int | None = None,
) -> dict[str, object]:
    """Build the exact integer before/uplift/after report from ledger rows."""
    if cutoff < 0 or cutoff > ledger_length or len(entries) != cutoff:
        raise ReportError("entries must be exactly the selected immutable prefix")
    for ledger_offset, entry in enumerate(entries):
        source = entry.get("source")
        if source not in KNOWN_SOURCES:
            raise ReportError(f"unknown PointSource variant {source!r}")
        if source == UNMATCHED_SOURCE and int(entry["delta"]) in {
            U128_MAX,
            SCALED_SATURATION_SENTINEL,
        }:
            raise ReportError(
                "historical preview rejected: saturated original unmatched row "
                f"at ledger offset {ledger_offset}, principal {entry['principal']}, "
                f"epoch {entry['epoch']}, points_delta={entry['delta']} "
                "matches the backend saturation sentinel"
            )

    topups = [e for e in entries if e["source"] == TOPUP_SOURCE]
    flat_rows = [e for e in entries if e["source"] == FLAT_SOURCE]
    if topups:
        raise ReportError(
            "historical preview rejected: the prefix already contains "
            f"{len(topups)} {TOPUP_SOURCE} adjustment row(s); their source rows "
            "cannot be paired safely, so no uplift is re-applied"
        )
    if flat_rows and cutover_epoch is None:
        raise ReportError(
            "historical preview rejected: the prefix contains future flat-4x "
            f"rows (first epoch {min(int(e['epoch']) for e in flat_rows)}), but "
            "no immutable --cutover-epoch was supplied"
        )
    if flat_rows and cutover_epoch is not None:
        earliest_flat = min(int(e["epoch"]) for e in flat_rows)
        if earliest_flat < cutover_epoch:
            raise ReportError(
                f"flat-4x row epoch {earliest_flat} precedes supplied "
                f"--cutover-epoch {cutover_epoch}; preview is ambiguous"
            )

    before_by_user: dict[str, int] = {}
    uplift_by_user: dict[str, int] = {}
    source_totals: dict[str, int] = {source: 0 for source in sorted(KNOWN_SOURCES)}
    for entry in entries:
        principal = str(entry["principal"])
        source = str(entry["source"])
        epoch = int(entry["epoch"])
        delta = int(entry["delta"])
        source_totals[source] += delta
        # Existing ledger points are preserved exactly.  Top-up rows were
        # rejected above; a flat-4x row is an already-recorded future point and
        # therefore remains part of the before total when its cutover is known.
        before_by_user[principal] = before_by_user.get(principal, 0) + delta
        if source == UNMATCHED_SOURCE and (
            cutover_epoch is None or epoch < cutover_epoch
        ):
            uplift_by_user[principal] = uplift_by_user.get(principal, 0) + delta // 3

    population = _population(entries)
    before_total = sum(before_by_user.values())
    uplift_total = sum(uplift_by_user.values())
    after_total = before_total + uplift_total
    if sum(before_by_user.get(p, 0) for p in population) != before_total:
        raise ReportError("before-total reconciliation failed across the population")
    if sum(uplift_by_user.get(p, 0) for p in population) != uplift_total:
        raise ReportError("uplift reconciliation failed across the population")

    users = []
    for principal in population:
        before = before_by_user.get(principal, 0)
        uplift = uplift_by_user.get(principal, 0)
        after = before + uplift
        users.append(
            {
                "principal": principal,
                "before_points_e8s": before,
                "uplift_points_e8s": uplift,
                "after_points_e8s": after,
                "before_share_bps": _share_bps(before, before_total),
                "after_share_bps": _share_bps(after, after_total),
                "before_points_decimal": _display_e8s(before),
                "uplift_points_decimal": _display_e8s(uplift),
                "after_points_decimal": _display_e8s(after),
                "before_share_percent": _display_percent(before, before_total),
                "after_share_percent": _display_percent(after, after_total),
                "correction_points_e8s": 0,
                "current_ledger_points_e8s": before,
                "principal_total_points_e8s": None,
            }
        )

    if flat_rows:
        classification = "mixed_legacy_and_future_flat4x_with_explicit_cutover"
    else:
        classification = "legacy_only_approximate_historic4x"
    return {
        "schema_version": 1,
        "ledger_length_observed": ledger_length,
        "cutoff_exclusive": cutoff,
        "prefix_rows": len(entries),
        "population_count": len(population),
        "classification": classification,
        "method": "per-recorded-unmatched-row-floor-divide-by-3",
        "exact_historic_replay": False,
        "note": (
            "Approximate historic 4x recorded-contribution view; it does not "
            "replay discarded snapshot selection inputs."
        ),
        "cutover_epoch": cutover_epoch,
        "before_total_points_e8s": before_total,
        "uplift_total_points_e8s": uplift_total,
        "after_total_points_e8s": after_total,
        "before_total_points_decimal": _display_e8s(before_total),
        "uplift_total_points_decimal": _display_e8s(uplift_total),
        "after_total_points_decimal": _display_e8s(after_total),
        "source_totals_e8s": source_totals,
        "reconciliation": {
            "original_rows_sum_e8s": before_total,
            "per_user_before_sum_e8s": sum(
                int(user["before_points_e8s"]) for user in users
            ),
            "uplift_rows_sum_e8s": uplift_total,
            "per_user_uplift_sum_e8s": sum(
                int(user["uplift_points_e8s"]) for user in users
            ),
            "before_matches": True,
            "uplift_matches": True,
        },
        "users": users,
    }


def parse_principal_total(raw: str) -> int:
    """Parse ``PrincipalState.total_points`` from an opt-record query result."""
    text = _candid_text(raw)
    if re.search(r"\bnull\b", text):
        raise ReportError("principal state query returned null")
    value = _field_int(text, "total_points")
    if value is None:
        raise ReportError("principal state query did not contain total_points")
    return value


def reconcile_current_rows(
    report: dict[str, object],
    current_rows: list[dict[str, int | str]],
    *,
    cutoff: int,
    principal_totals: dict[str, int],
) -> dict[str, object]:
    """Verify appended correction rows against the immutable preview.

    ``current_rows[:cutoff]`` is the original prefix used for the preview;
    only rows at or after that offset may be corrections.  All current rows are
    still summed per principal and compared with the queried PrincipalState
    totals, so future accrual rows appended alongside a correction cannot hide
    a ledger/state mismatch.
    """
    if cutoff < 0 or cutoff > len(current_rows):
        raise ReportError("reconciliation cutoff is outside the current ledger")
    original_rows = current_rows[:cutoff]
    appended_rows = current_rows[cutoff:]
    if any(entry["source"] == TOPUP_SOURCE for entry in original_rows):
        raise ReportError(
            "reconciliation rejected: original cutoff includes a correction row; "
            "choose the fixed pre-migration cutoff"
        )

    correction_by_user: dict[str, int] = {}
    correction_rows = 0
    appended_non_correction_rows = 0
    for entry in appended_rows:
        source = str(entry["source"])
        delta = int(entry["delta"])
        principal = str(entry["principal"])
        if source == TOPUP_SOURCE:
            if delta <= 0:
                raise ReportError("correction rows must have positive points_delta")
            correction_rows += 1
            correction_by_user[principal] = correction_by_user.get(principal, 0) + delta
        else:
            appended_non_correction_rows += 1

    preview_by_user = {
        str(user["principal"]): int(user["uplift_points_e8s"])
        for user in report["users"]
        if int(user["uplift_points_e8s"]) > 0
    }
    if correction_by_user != preview_by_user:
        raise ReportError(
            "correction reconciliation failed: appended per-principal sums "
            f"{correction_by_user} do not equal preview {preview_by_user}"
        )

    ledger_by_user: dict[str, int] = {}
    for entry in current_rows:
        principal = str(entry["principal"])
        ledger_by_user[principal] = ledger_by_user.get(principal, 0) + int(
            entry["delta"]
        )
    if set(principal_totals) != set(ledger_by_user):
        raise ReportError(
            "principal total reconciliation failed: queried principal set does "
            "not match current ledger population"
        )
    mismatches = {
        principal: (ledger_by_user[principal], principal_totals[principal])
        for principal in ledger_by_user
        if principal_totals[principal] != ledger_by_user[principal]
    }
    if mismatches:
        raise ReportError(
            "principal total reconciliation failed for "
            f"{mismatches}"
        )

    user_index = {str(user["principal"]): user for user in report["users"]}
    for principal, current_total in ledger_by_user.items():
        user = user_index.get(principal)
        if user is not None:
            user["correction_points_e8s"] = correction_by_user.get(principal, 0)
            user["current_ledger_points_e8s"] = current_total
            user["principal_total_points_e8s"] = principal_totals[principal]

    current_total = sum(ledger_by_user.values())
    correction_total = sum(correction_by_user.values())
    post = {
        "original_cutoff_exclusive": cutoff,
        "current_ledger_rows": len(current_rows),
        "appended_rows": len(appended_rows),
        "correction_rows": correction_rows,
        "appended_non_correction_rows": appended_non_correction_rows,
        "correction_points_e8s": correction_total,
        "preview_uplift_points_e8s": sum(preview_by_user.values()),
        "correction_by_principal_e8s": correction_by_user,
        "preview_uplift_by_principal_e8s": preview_by_user,
        "corrections_match_preview": True,
        "current_ledger_total_points_e8s": current_total,
        "principal_totals_match_ledger": True,
    }
    report["mode"] = "post_migration_reconciliation"
    report["post_migration_reconciliation"] = post
    return report


def icp_call(
    canister: str,
    method: str,
    args: str,
    *,
    network: str,
    candid: str,
) -> str:
    command = [
        "icp",
        "canister",
        "call",
        canister,
        method,
        args,
        "--network",
        network,
        "--identity",
        "anonymous",
        "--query",
        "--candid",
        candid,
        "--json",
        "--output",
        "candid",
    ]
    proc = subprocess.run(command, capture_output=True, text=True)
    if proc.returncode:
        detail = proc.stderr.strip() or proc.stdout.strip()
        raise ReportError(f"read-only query {method} failed: {detail}")
    return proc.stdout


def write_outputs(report: dict[str, object], json_path: str | None, csv_path: str | None) -> None:
    if json_path:
        with open(json_path, "w", encoding="utf-8") as handle:
            json.dump(report, handle, indent=2, sort_keys=True)
            handle.write("\n")
    if csv_path:
        fields = [
            "principal",
            "before_points_e8s",
            "uplift_points_e8s",
            "after_points_e8s",
            "correction_points_e8s",
            "current_ledger_points_e8s",
            "principal_total_points_e8s",
            "before_share_bps",
            "after_share_bps",
            "before_points_decimal",
            "uplift_points_decimal",
            "after_points_decimal",
            "before_share_percent",
            "after_share_percent",
        ]
        with open(csv_path, "w", newline="", encoding="utf-8") as handle:
            writer = csv.DictWriter(handle, fieldnames=fields)
            writer.writeheader()
            writer.writerows(report["users"])


def run(args: argparse.Namespace) -> dict[str, object]:
    default_candid = Path(__file__).resolve().parents[1] / "src/rumi_points/rumi_points.did"
    candid = str(args.candid or default_candid)
    query = lambda method, call_args: icp_call(
        args.canister,
        method,
        call_args,
        network=args.network,
        candid=candid,
    )
    ledger_length = parse_nat(query("get_point_ledger_len", "()"))
    if args.reconcile and args.cutoff is None:
        raise ReportError("--reconcile requires an explicit original --cutoff")
    cutoff = ledger_length if args.cutoff is None else args.cutoff
    query_page = lambda offset, limit: query(
        "get_point_entries", f"({offset}:nat64, {limit}:nat32)"
    )
    if args.reconcile:
        current_rows = collect_prefix(query_page, ledger_length, ledger_length)
        if cutoff > ledger_length:
            raise ReportError(
                f"--cutoff must be between 0 and current ledger length {ledger_length}"
            )
        rows = current_rows[:cutoff]
    else:
        rows = collect_prefix(query_page, ledger_length, cutoff)
    report = build_report(
        rows,
        ledger_length=ledger_length,
        cutoff=cutoff,
        cutover_epoch=args.cutover_epoch,
    )
    if args.reconcile:
        current_population = _population(current_rows)
        principal_totals = {
            principal: parse_principal_total(
                query(
                    "get_principal_state",
                    f'(principal "{principal}")',
                )
            )
            for principal in current_population
        }
        reconcile_current_rows(
            report,
            current_rows,
            cutoff=cutoff,
            principal_totals=principal_totals,
        )
    report.update(
        {
            "generated_utc": datetime.now(timezone.utc).isoformat(),
            "canister": args.canister,
            "network": args.network,
            "identity": "anonymous",
            "query_only": True,
            "candid": candid,
            "mode": "post_migration_reconciliation" if args.reconcile else "preview",
        }
    )
    write_outputs(report, args.json, args.csv)
    return report


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--canister", default=MAINNET_POINTS)
    parser.add_argument("--network", default="ic")
    parser.add_argument(
        "--candid",
        help="Candid file for decoding (defaults to src/rumi_points/rumi_points.did)",
    )
    parser.add_argument(
        "--cutoff",
        type=int,
        help="exclusive global ledger offset; defaults to the observed ledger length",
    )
    parser.add_argument(
        "--cutover-epoch",
        type=int,
        help="immutable first flat-4x epoch; omit only for a legacy-only prefix",
    )
    parser.add_argument(
        "--reconcile",
        action="store_true",
        help=(
            "verify appended correction rows after the explicit --cutoff and "
            "compare every current principal total to the global ledger"
        ),
    )
    parser.add_argument("--json", help="write the full integer-safe JSON report")
    parser.add_argument("--csv", help="write per-user before/uplift/after CSV")
    parsed = parser.parse_args(argv)
    try:
        report = run(parsed)
    except ReportError as exc:
        print(f"fiat-stable-topup-report: ERROR: {exc}", file=sys.stderr)
        return 2
    print(
        f"read-only prefix {report['cutoff_exclusive']} rows / "
        f"{report['population_count']} users; "
        f"before {_display_e8s(int(report['before_total_points_e8s']))}, "
        f"uplift {_display_e8s(int(report['uplift_total_points_e8s']))}, "
        f"after {_display_e8s(int(report['after_total_points_e8s']))}"
    )
    if parsed.json:
        print(f"JSON written to {parsed.json}")
    if parsed.csv:
        print(f"CSV written to {parsed.csv}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
