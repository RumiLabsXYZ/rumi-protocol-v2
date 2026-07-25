#!/usr/bin/env python3
"""Admin report over every Season-1 airdrop participant in `rumi_points`.

Joins the leaderboard, each principal's state, and (once the ledger readers are
deployed) the per-source/per-epoch audit ledger into one breakdown.

    scripts/airdrop-report.py                  # full report, mainnet
    scripts/airdrop-report.py --csv out.csv    # also write a per-principal CSV
    scripts/airdrop-report.py --json out.json  # also write the raw joined data
    scripts/airdrop-report.py --canister <id> --network local

Everything it reads is an unauthenticated query, so no identity is needed. The
per-source breakdown depends on `get_principal_point_entries`, which only exists
after the ledger-reader upgrade; without it the script still prints totals,
ranks, shares and positions, and says so.

UNITS: the canister speaks `usd_e8s` (USD x 1e8) everywhere -- `total_points` is
usd_e8s-DAYS, `recorded_value_usd` is usd_e8s. Divide by 1e8. 3USD/LP is 8-dec.
"""

import argparse
import json
import re
import subprocess
import sys
from datetime import datetime, timezone

MAINNET_POINTS = "bfnu3-6aaaa-aaaab-qhanq-cai"
E8S = 100_000_000

# Human labels for the PointSource variants, with the multiplier the accrual
# engine applies to each (see accrual::snapshot_weights).
SOURCE_LABELS = {
    "Registration": ("Registration marker", "-"),
    "IcUsdDebt": ("icUSD vault debt", "1x"),
    "IcUsd3Pool": ("icUSD in 3pool", "1x"),
    "CkStable3PoolMatched": ("ck-stable 3pool (matched)", "10x"),
    "CkStable3PoolUnmatched": ("ck-stable 3pool (unmatched)", "3x"),
    "IcUsdStabilityPool": ("icUSD in stability pool", "1x"),
    "ThreeUsdStabilityPool": ("3USD in stability pool", "2x"),
    "AmmLp": ("AMM LP", "2x"),
    "VaultRepayment": ("ck-stable vault repayment", "5x"),
}


class MethodMissing(Exception):
    """The canister does not export the method (pre-upgrade)."""


def call(canister, method, args="()", network="ic"):
    proc = subprocess.run(
        ["icp", "canister", "call", canister, method, args, "--network", network],
        capture_output=True,
        text=True,
    )
    if proc.returncode != 0:
        err = proc.stderr or proc.stdout
        if "no update method" in err or "no query method" in err or "IC0536" in err:
            raise MethodMissing(method)
        raise SystemExit(f"call {method} failed:\n{err}")
    return proc.stdout


def nums(text, field):
    """All values of `field = 123_456 : natN` in candid text, in order."""
    return [int(m.replace("_", "")) for m in re.findall(rf"{field} = ([\d_]+) : nat", text)]


def one_num(text, field, default=0):
    got = nums(text, field)
    return got[0] if got else default


def principals(text, field='"principal"'):
    return re.findall(rf'{field} = principal "([a-z0-9\-]+)"', text)


def usd(v):
    return f"${v / E8S:,.2f}"


def ts(ns):
    if not ns:
        return "-"
    return datetime.fromtimestamp(ns / 1e9, timezone.utc).strftime("%Y-%m-%d %H:%M UTC")


def parse_leaderboard(text):
    rows = []
    # Each entry is a record; split on the principal field to keep fields paired.
    for chunk in text.split('record {')[1:]:
        p = re.search(r'"principal" = principal "([a-z0-9\-]+)"', chunk)
        pts = re.search(r"total_points = ([\d_]+) : nat", chunk)
        rank = re.search(r"rank = ([\d_]+) : nat32", chunk)
        bps = re.search(r"estimated_share_bps = ([\d_]+) : nat32", chunk)
        if p and pts and rank:
            rows.append(
                {
                    "principal": p.group(1),
                    "total_points": int(pts.group(1).replace("_", "")),
                    "rank": int(rank.group(1).replace("_", "")),
                    "share_bps": int(bps.group(1).replace("_", "")) if bps else 0,
                }
            )
    rows.sort(key=lambda r: r["rank"])
    return rows


def parse_principal_state(text):
    if "opt record" not in text:
        return None
    st = {
        "registered_at_ns": one_num(text, "registered_at_ns"),
        "total_points": one_num(text, "total_points"),
        "last_epoch_processed": one_num(text, "last_epoch_processed"),
        "first_action": (
            re.search(r"first_qualifying_action = variant \{ (\w+) \}", text).group(1)
            if re.search(r"first_qualifying_action = variant \{ (\w+) \}", text)
            else "?"
        ),
        "deposits": [],
        "repayments": [],
    }
    # active_deposits: candid emits asset/venue/last_verified_at BEFORE
    # recorded_value_usd inside each record, so split on the value field and read
    # the descriptors out of the PRECEDING chunk (its last occurrence), not the
    # following one.
    parts = text.split("recorded_value_usd = ")
    for i in range(1, len(parts)):
        val = re.match(r"([\d_]+) : nat", parts[i])
        if not val:
            continue
        before = parts[i - 1]
        assets = re.findall(r"asset = variant \{ (\w+) \}", before)
        venues = re.findall(r"venue = variant \{ (\w+) \}", before)
        lastv = re.findall(r"last_verified_at = ([\d_]+) : nat64", before)
        st["deposits"].append(
            {
                "asset": assets[-1] if assets else "?",
                "venue": venues[-1] if venues else "?",
                "value": int(val.group(1).replace("_", "")),
                "last_verified_ns": int(lastv[-1].replace("_", "")) if lastv else 0,
            }
        )
    for chunk in text.split("repaid_at")[1:]:
        amt = re.search(r"amount_usd = ([\d_]+) : nat", chunk[:300])
        if amt:
            st["repayments"].append(int(amt.group(1).replace("_", "")))
    return st


def fetch_entries(canister, principal, network):
    """Page the audit ledger for one principal. Raises MethodMissing pre-upgrade."""
    out, offset = [], 0
    while True:
        txt = call(
            canister,
            "get_principal_point_entries",
            f'(principal "{principal}", {offset}:nat64, 1000:nat32)',
            network,
        )
        for chunk in txt.split("record {")[1:]:
            src = re.search(r"source = variant \{ (\w+) \}", chunk)
            delta = re.search(r"points_delta = ([\d_]+) : nat", chunk)
            epoch = re.search(r"epoch_index = ([\d_]+) : nat64", chunk)
            if src and delta and epoch:
                out.append(
                    {
                        "source": src.group(1),
                        "delta": int(delta.group(1).replace("_", "")),
                        "epoch": int(epoch.group(1).replace("_", "")),
                    }
                )
        nxt = re.search(r"next_offset = ([\d_]+) : nat64", txt)
        done = "reached_end = true" in txt
        if done or not nxt or int(nxt.group(1).replace("_", "")) == offset:
            break
        offset = int(nxt.group(1).replace("_", ""))
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--canister", default=MAINNET_POINTS)
    ap.add_argument("--network", default="ic")
    ap.add_argument("--csv")
    ap.add_argument("--json")
    a = ap.parse_args()

    cfg = call(a.canister, "get_points_config", "()", a.network)
    epoch = call(a.canister, "get_epoch_status", "()", a.network)
    # The excluded list is a bare `vec { principal "..." }` with no field name.
    excluded = re.findall(
        r'principal "([a-z0-9\-]+)"',
        call(a.canister, "get_excluded_principals", "()", a.network),
    )

    season_start = one_num(cfg, "season_start_ns")
    season_end = one_num(cfg, "season_end_ns")
    admin = principals(cfg, "admin")

    print("=" * 78)
    print("RUMI SEASON-1 AIRDROP -- PARTICIPANT REPORT")
    print("=" * 78)
    print(f"canister        {a.canister} ({a.network})")
    print(f"generated       {datetime.now(timezone.utc).strftime('%Y-%m-%d %H:%M UTC')}")
    print(f"admin           {admin[0] if admin else '?'}")
    print(f"season          {ts(season_start)}  ->  {ts(season_end)}")
    print(f"current epoch   {one_num(cfg, 'current_epoch_index')}"
          f"   driver={'on' if 'driver_enabled = true' in epoch else 'OFF'}")
    print(f"registered      {one_num(cfg, 'registered_count')}")
    print(f"excluded        {len(excluded)} principals (protocol canisters)")

    board = parse_leaderboard(call(a.canister, "get_leaderboard", "(0:nat32, 1000:nat32)", a.network))
    total_all = sum(r["total_points"] for r in board)

    print()
    print("-" * 78)
    print("RANKING")
    print("-" * 78)
    print(f"{'#':>3}  {'principal':<64} {'points (USD-days)':>18} {'share':>8}")
    for r in board:
        print(f"{r['rank']:>3}  {r['principal']:<64} {r['total_points']/E8S:>18,.2f}"
              f" {r['share_bps']/100:>7.2f}%")
    print(f"{'':>3}  {'TOTAL':<64} {total_all/E8S:>18,.2f}")

    ledger_ok = True
    joined = []
    for r in board:
        st = parse_principal_state(
            call(a.canister, "get_principal_state", f'(principal "{r["principal"]}")', a.network)
        )
        entries = []
        if ledger_ok:
            try:
                entries = fetch_entries(a.canister, r["principal"], a.network)
            except MethodMissing:
                ledger_ok = False
        joined.append({**r, "state": st, "entries": entries})

    print()
    print("-" * 78)
    print("PER-PARTICIPANT BREAKDOWN")
    print("-" * 78)
    for j in joined:
        st = j["state"] or {}
        print()
        print(f"#{j['rank']}  {j['principal']}")
        print(f"     points        {j['total_points']/E8S:,.2f} USD-days"
              f"   ({j['share_bps']/100:.2f}% of season)")
        print(f"     registered    {ts(st.get('registered_at_ns', 0))}"
              f"   via {st.get('first_action','?')}")
        print(f"     last epoch    {st.get('last_epoch_processed','?')}")
        deps = st.get("deposits", [])
        if deps:
            print("     recorded positions (NOT live balances -- see caveat below):")
            for d in deps:
                print(f"       {d['venue']:<14} {d['asset']:<8} {usd(d['value']):>14}"
                      f"   verified {ts(d['last_verified_ns'])}")
        else:
            print("     recorded positions: none")
        if st.get("repayments"):
            tot = sum(st["repayments"])
            print(f"     repayments    {len(st['repayments'])} event(s), {usd(tot)} total")
        if j["entries"]:
            by_src = {}
            for e in j["entries"]:
                by_src[e["source"]] = by_src.get(e["source"], 0) + e["delta"]
            print("     points by source:")
            for src, tot in sorted(by_src.items(), key=lambda kv: -kv[1]):
                label, mult = SOURCE_LABELS.get(src, (src, "?"))
                pct = (tot / j["total_points"] * 100) if j["total_points"] else 0
                print(f"       {label:<30} {mult:>4}  {tot/E8S:>14,.2f}  {pct:>5.1f}%")
            epochs = sorted({e["epoch"] for e in j["entries"]})
            print(f"     accrued over  {len(epochs)} epoch(s): {epochs}")

    if not ledger_ok:
        print()
        print("!" * 78)
        print("PER-SOURCE BREAKDOWN UNAVAILABLE")
        print("  The canister does not export `get_principal_point_entries`, so the")
        print("  per-source / per-epoch decomposition could not be read. The audit")
        print("  rows ARE being written to POINT_LEDGER; they just have no reader")
        print("  until the ledger-reader upgrade is deployed.")
        print("!" * 78)

    print()
    print("-" * 78)
    print("CAVEATS")
    print("-" * 78)
    print("  * `recorded positions` are the ingest-tracked 3pool composition, NOT")
    print("    live balances. Known bug (2026-07-25): a 3pool `RemoveOneCoin` only")
    print("    debits the coin withdrawn, so the untouched ck-stable legs ratchet")
    print("    upward and never come down. Treat these as an upper bound.")
    print("  * Accrual applies a verification cap: recorded 3pool value is scaled")
    print("    to min(1.005 * live 3USD holdings, recorded) before points, which")
    print("    partly absorbs the above -- but only when the cap binds.")
    print("  * All figures are usd_e8s / 1e8. Points are USD-DAYS, not dollars.")

    if a.csv:
        import csv as _csv

        with open(a.csv, "w", newline="") as fh:
            w = _csv.writer(fh)
            w.writerow(["rank", "principal", "points_usd_days", "share_pct",
                        "registered_utc", "first_action", "last_epoch", "recorded_positions_usd"])
            for j in joined:
                st = j["state"] or {}
                w.writerow([
                    j["rank"], j["principal"], f"{j['total_points']/E8S:.2f}",
                    f"{j['share_bps']/100:.2f}", ts(st.get("registered_at_ns", 0)),
                    st.get("first_action", "?"), st.get("last_epoch_processed", ""),
                    f"{sum(d['value'] for d in st.get('deposits', []))/E8S:.2f}",
                ])
        print(f"\nCSV written to {a.csv}")

    if a.json:
        with open(a.json, "w") as fh:
            json.dump({"generated_utc": datetime.now(timezone.utc).isoformat(),
                       "canister": a.canister, "participants": joined,
                       "excluded": excluded, "ledger_readable": ledger_ok}, fh, indent=2)
        print(f"JSON written to {a.json}")


if __name__ == "__main__":
    sys.exit(main())
