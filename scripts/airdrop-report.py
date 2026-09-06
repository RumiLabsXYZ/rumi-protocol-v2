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
import contextlib
import io
import json
import os
import re
import subprocess
import sys
import webbrowser
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


SOURCE_TAGS = {
    0: "rumi_protocol_backend",
    1: "rumi_3pool",
    2: "rumi_stability_pool",
    3: "rumi_amm",
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


def parse_epoch_history(text):
    rows = []
    for chunk in text.split("record {")[1:]:
        idx = re.search(r"epoch_index = ([\d_]+) : nat64", chunk)
        if not idx:
            continue
        g = lambda f, t="nat": (
            int(m.group(1).replace("_", ""))
            if (m := re.search(rf"{f} = ([\d_]+) : {t}", chunk))
            else 0
        )
        rows.append(
            {
                "epoch": int(idx.group(1).replace("_", "")),
                "start": g("epoch_start_ns", "nat64"),
                "end": g("epoch_end_ns", "nat64"),
                "snap_a": g("snapshot_a_ns", "nat64"),
                "snap_b": g("snapshot_b_ns", "nat64"),
                "accrued": g("points_accrued_this_epoch"),
                "cumulative": g("total_points_all"),
                "active": g("active_principals", "nat64"),
                "registered": g("registered_principals", "nat64"),
            }
        )
    rows.sort(key=lambda r: r["epoch"])
    return rows


def parse_sources(text):
    out = []
    for chunk in text.split("record {")[1:]:
        tag = re.search(r"tag = (\d+) : nat8", chunk)
        cur = re.search(r"cursor = ([\d_]+) : nat64", chunk)
        can = re.search(r'canister = principal "([a-z0-9\-]+)"', chunk)
        if tag and cur and can:
            out.append(
                {
                    "tag": int(tag.group(1)),
                    "cursor": int(cur.group(1).replace("_", "")),
                    "canister": can.group(1),
                }
            )
    return out


def esc(s):
    return (
        str(s).replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")
    )


def render_html(ctx):
    """Self-contained dashboard. No external resources, opens with file://."""
    p = []
    w = p.append
    board = ctx["board"]
    total_all = sum(r["total_points"] for r in board) or 1

    w("<!doctype html><html lang='en'><head><meta charset='utf-8'>")
    w("<meta name='viewport' content='width=device-width,initial-scale=1'>")
    w(f"<title>Airdrop Admin · Season 1 · {esc(ctx['generated'])}</title><style>")
    w("""
:root{--bg:#0b0f14;--card:#131a23;--line:#1f2a37;--fg:#e5e7eb;--dim:#9ca3af;
--faint:#6b7280;--teal:#2dd4bf;--amber:#fbbf24;--green:#34d399}
*{box-sizing:border-box}
body{margin:0;background:var(--bg);color:var(--fg);
font:14px/1.5 ui-sans-serif,-apple-system,'Segoe UI',sans-serif;padding:24px}
.wrap{max-width:1200px;margin:0 auto;display:flex;flex-direction:column;gap:16px}
h1{font-size:20px;margin:0}h2{font-size:14px;margin:0 0 10px;color:var(--dim);font-weight:600}
.sub{color:var(--faint);font-size:12px;margin-top:2px}
.card{background:var(--card);border:1px solid var(--line);border-radius:12px;padding:16px}
.grid{display:grid;gap:12px}
.g4{grid-template-columns:repeat(auto-fit,minmax(210px,1fr))}
.k{font-size:11px;color:var(--faint)}.v{font-size:18px;margin-top:2px}
.vs{font-size:13px;margin-top:2px}
table{width:100%;border-collapse:collapse;font-size:13px}
th{text-align:left;font-size:11px;color:var(--faint);font-weight:600;
padding:6px 8px;border-bottom:1px solid var(--line)}
td{padding:8px;border-bottom:1px solid rgba(255,255,255,.04);vertical-align:top}
.r{text-align:right}.num{font-variant-numeric:tabular-nums}
.mono{font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:11px}
.teal{color:var(--teal)}.amber{color:var(--amber)}.green{color:var(--green)}
.dim{color:var(--dim)}.faint{color:var(--faint)}
details>summary{cursor:pointer;color:var(--dim);font-size:13px}
.bar{height:7px;background:#0a0e13;border-radius:4px;overflow:hidden;flex:1}
.bar>span{display:block;height:100%;background:var(--teal);opacity:.65}
.brow{display:flex;align-items:center;gap:10px;font-size:12px;margin:3px 0}
.blabel{width:230px;flex:none;color:var(--dim)}
.bval{width:110px;text-align:right}.bpct{width:52px;text-align:right;color:var(--faint)}
.chip{display:inline-block;padding:2px 7px;margin:2px 3px 0 0;border-radius:5px;
background:#0a0e13;border:1px solid var(--line);font-size:11px}
.warn{border-color:#78350f;background:#1c1207;color:#fcd34d;font-size:12px;
padding:10px 12px;border-radius:8px;border-width:1px;border-style:solid}
.detail{background:#0e141c}
.scroll{overflow-x:auto}
""")
    w("</style></head><body><div class='wrap'>")

    # Header
    w("<div><h1>Airdrop Admin — Season 1</h1><div class='sub'>")
    w(f"{esc(ts(ctx['season_start']))} → {esc(ts(ctx['season_end']))}")
    w(f" · generated {esc(ctx['generated'])} · {esc(ctx['canister'])} ({esc(ctx['network'])})")
    w("</div></div>")

    # Engine strip
    w("<div class='grid g4'>")
    w(f"<div class='card'><div class='k'>Current epoch</div><div class='v num'>{ctx['epoch_index']}</div>"
      f"<div class='k' style='margin-top:6px'>{esc(ctx['open_epoch_note'])}</div></div>")
    w(f"<div class='card'><div class='k'>Snapshots (open epoch)</div>"
      f"<div class='vs'>A: {esc(ctx['snap_a'])}</div><div class='vs'>B: {esc(ctx['snap_b'])}</div></div>")
    dcl = "green" if ctx["driver_on"] else "amber"
    w(f"<div class='card'><div class='k'>Epoch driver</div>"
      f"<div class='vs {dcl}'>{'on' if ctx['driver_on'] else 'OFF'} · every {ctx['driver_secs']}s</div>"
      f"<div class='k' style='margin-top:6px'>seed {'committed' if ctx['seed'] else 'MISSING'} · "
      f"{ctx['revealed']} revealed</div></div>")
    pcl = "green" if ctx["poll_on"] else "amber"
    w(f"<div class='card'><div class='k'>Ingest poller</div>"
      f"<div class='vs {pcl}'>{'on' if ctx['poll_on'] else 'OFF'} · every {ctx['poll_secs']}s</div>"
      f"<div class='k' style='margin-top:6px'>{len(board)} registered · {len(ctx['excluded'])} excluded · "
      f"{ctx['ledger_note']}</div></div>")
    w("</div>")

    # Sources
    if ctx["sources"]:
        w("<div class='card'><h2>Event sources</h2><div class='grid g4'>")
        for s in ctx["sources"]:
            name = SOURCE_TAGS.get(s["tag"], f"source {s['tag']}")
            w(f"<div><div class='k'>{esc(name)}</div>"
              f"<div class='mono faint'>{esc(s['canister'])}</div>"
              f"<div class='num dim' style='font-size:12px;margin-top:3px'>cursor {s['cursor']:,}</div></div>")
        w("</div></div>")

    # Season by source
    if ctx["season_sources"]:
        st_total = sum(v for _, v in ctx["season_sources"]) or 1
        w("<div class='card'><h2>Season points by source</h2>")
        for src, val in ctx["season_sources"]:
            label, mult = SOURCE_LABELS.get(src, (src, "?"))
            w("<div class='brow'>")
            w(f"<span class='blabel'>{esc(label)} <span class='faint'>({esc(mult)})</span></span>")
            w(f"<span class='bar'><span style='width:{val/st_total*100:.2f}%'></span></span>")
            w(f"<span class='bval num'>{val/E8S:,.2f}</span>")
            w(f"<span class='bpct num'>{val/st_total*100:.1f}%</span>")
            w("</div>")
        w("</div>")

    # Participants
    w("<div class='card'><h2>Participants "
      f"<span class='faint' style='font-weight:400'>· {len(board)} registered · "
      f"{sum(r['total_points'] for r in board)/E8S:,.2f} USD-days total</span></h2>")
    w("<div class='scroll'><table><thead><tr><th>#</th><th>Principal</th>"
      "<th class='r'>Points (USD-days)</th><th class='r'>Share</th>"
      "<th>Registered</th><th>First action</th></tr></thead><tbody>")
    for j in board:
        st = j.get("state") or {}
        w(f"<tr><td class='num dim'>#{j['rank']}</td>")
        w(f"<td class='mono teal'>{esc(j['principal'])}</td>")
        w(f"<td class='r num'>{j['total_points']/E8S:,.2f}</td>")
        w(f"<td class='r num dim'>{j['share_bps']/100:.2f}%</td>")
        w(f"<td class='dim' style='font-size:12px'>{esc(ts(st.get('registered_at_ns',0)))}</td>")
        w(f"<td class='dim' style='font-size:12px'>{esc(st.get('first_action','—'))}</td></tr>")

        # Detail row
        w("<tr class='detail'><td colspan='6'>")
        deps = st.get("deposits", [])
        if deps:
            w("<div class='k' style='margin-bottom:4px'>Recorded 3pool composition "
              "<span class='faint'>(ingest-tracked, not a live balance)</span></div>")
            for d in deps:
                w(f"<div style='font-size:12px' class='dim'>{esc(d['venue'])} · {esc(d['asset'])} "
                  f"<span class='num'>{esc(usd(d['value']))}</span> "
                  f"<span class='faint'>· verified {esc(ts(d['last_verified_ns']))}</span></div>")
        else:
            w("<div class='k'>No recorded 3pool position</div>")
        if st.get("repayments"):
            w(f"<div class='k' style='margin-top:6px'>{len(st['repayments'])} repayment window(s), "
              f"{esc(usd(sum(st['repayments'])))} total</div>")
        entries = j.get("entries") or []
        if entries:
            by_src, by_ep = {}, {}
            for e in entries:
                by_src[e["source"]] = by_src.get(e["source"], 0) + e["delta"]
                by_ep[e["epoch"]] = by_ep.get(e["epoch"], 0) + e["delta"]
            tot = sum(by_src.values()) or 1
            w("<div class='k' style='margin-top:8px;margin-bottom:4px'>Points by source</div>")
            for src, val in sorted(by_src.items(), key=lambda kv: -kv[1]):
                if val <= 0:
                    continue
                label, mult = SOURCE_LABELS.get(src, (src, "?"))
                w("<div class='brow'>")
                w(f"<span class='blabel'>{esc(label)} <span class='faint'>({esc(mult)})</span></span>")
                w(f"<span class='bar'><span style='width:{val/tot*100:.2f}%'></span></span>")
                w(f"<span class='bval num'>{val/E8S:,.2f}</span>")
                w(f"<span class='bpct num'>{val/tot*100:.1f}%</span></div>")
            w("<div class='k' style='margin-top:8px'>Accrual by epoch</div><div>")
            for ep in sorted(by_ep):
                w(f"<span class='chip num'>E{ep}: {by_ep[ep]/E8S:,.2f}</span>")
            w("</div>")
        elif not ctx["ledger_ok"]:
            w("<div class='k amber' style='margin-top:8px'>Per-source breakdown needs the "
              "ledger-reader canister upgrade (get_principal_point_entries).</div>")
        w("</td></tr>")
    w("</tbody></table></div></div>")

    # Epoch history
    if ctx["epochs"]:
        w("<div class='card'><h2>Epoch history</h2><div class='scroll'><table><thead><tr>"
          "<th>Epoch</th><th>Window</th><th>Snapshots (A / B)</th><th class='r'>Accrued</th>"
          "<th class='r'>Cumulative</th><th class='r'>Active / Reg.</th></tr></thead><tbody>")
        for e in ctx["epochs"]:
            w(f"<tr><td class='num'>E{e['epoch']}</td>"
              f"<td class='dim' style='font-size:12px'>{esc(ts(e['start']))} → {esc(ts(e['end']))}</td>"
              f"<td class='dim' style='font-size:12px'>{esc(ts(e['snap_a']))} / {esc(ts(e['snap_b']))}</td>"
              f"<td class='r num'>{e['accrued']/E8S:,.2f}</td>"
              f"<td class='r num'>{e['cumulative']/E8S:,.2f}</td>"
              f"<td class='r num dim'>{e['active']} / {e['registered']}</td></tr>")
        w("</tbody></table></div></div>")

    # Excluded
    w(f"<details class='card'><summary>Excluded principals ({len(ctx['excluded'])})</summary>"
      "<div style='margin-top:8px'>")
    for e in ctx["excluded"]:
        w(f"<div class='mono faint'>{esc(e)}</div>")
    w("</div></details>")

    # Caveats
    w("<div class='card'><h2>Caveats</h2><div class='warn'>")
    w("<b>Recorded positions are ingest-tracked, not live balances.</b> The pre-fix code "
      "under-debited <code>RemoveOneCoin</code> withdrawals; records written before that "
      "upgrade stay inflated until <code>admin_rebuild_3pool_recorded</code> runs. "
      "Treat them as an upper bound.")
    w("</div><div class='k' style='margin-top:8px'>Accrual applies a verification cap: recorded "
      "3pool value is scaled to min(1.005 × live 3USD holdings, recorded) before points. "
      "All figures are usd_e8s ÷ 1e8; points are USD-DAYS, not dollars.</div></div>")

    w("</div></body></html>")
    return "\n".join(p)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--canister", default=MAINNET_POINTS)
    ap.add_argument("--network", default="ic")
    ap.add_argument("--csv")
    ap.add_argument("--json")
    ap.add_argument("--html", nargs="?", const="airdrop-report.html",
                    help="write a self-contained HTML dashboard (default: airdrop-report.html)")
    ap.add_argument("--open", action="store_true", help="open the --html file in a browser")
    ap.add_argument("--quiet", action="store_true", help="suppress the text report")
    a = ap.parse_args()
    # --quiet swallows only the text report; the "written to" lines below go to
    # the real stdout so the user still learns where the file landed.
    if a.quiet:
        with contextlib.redirect_stdout(io.StringIO()):
            return run(a)
    return run(a)


def run(a):
    cfg = call(a.canister, "get_points_config", "()", a.network)
    epoch = call(a.canister, "get_epoch_status", "()", a.network)
    ingest = call(a.canister, "get_ingest_status", "()", a.network)
    epoch_hist = parse_epoch_history(
        call(a.canister, "get_epoch_history", "(0:nat32, 500:nat32)", a.network)
    )
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
    print("    live balances. The pre-fix code under-debited `RemoveOneCoin`")
    print("    withdrawals (fixed on feat/airdrop-admin-participant-view); records")
    print("    written before that deploy stay inflated until the admin runs")
    print("    `admin_rebuild_3pool_recorded`. Treat them as an upper bound.")
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

    if a.html:
        # Season-wide by-source totals, only when every breakdown was readable.
        season_sources = []
        if ledger_ok:
            agg = {}
            for j in joined:
                for e in j["entries"]:
                    if e["delta"] > 0:
                        agg[e["source"]] = agg.get(e["source"], 0) + e["delta"]
            season_sources = sorted(agg.items(), key=lambda kv: -kv[1])

        open_ep = "no epoch open"
        m = re.search(r"epoch_end_ns = ([\d_]+) : nat64", epoch)
        if "open_epoch = opt" in epoch and m:
            open_ep = f"open · ends {ts(int(m.group(1).replace('_', '')))}"
        snaps = re.findall(r"snapshot_(?:a|b)_ns = opt \(([\d_]+) : nat64\)", epoch)

        html = render_html({
            "generated": datetime.now(timezone.utc).strftime("%Y-%m-%d %H:%M UTC"),
            "canister": a.canister,
            "network": a.network,
            "season_start": season_start,
            "season_end": season_end,
            "epoch_index": one_num(cfg, "current_epoch_index"),
            "open_epoch_note": open_ep,
            "snap_a": ts(int(snaps[0].replace("_", ""))) if len(snaps) > 0 else "pending",
            "snap_b": ts(int(snaps[1].replace("_", ""))) if len(snaps) > 1 else "pending",
            "driver_on": "driver_enabled = true" in epoch,
            "driver_secs": one_num(epoch, "driver_interval_secs"),
            "seed": "snapshot_seed_committed = true" in epoch,
            "revealed": one_num(epoch, "revealed_seed_count"),
            "poll_on": "poll_enabled = true" in ingest,
            "poll_secs": one_num(ingest, "poll_interval_secs"),
            "sources": parse_sources(ingest),
            "board": joined,
            "epochs": epoch_hist,
            "excluded": excluded,
            "ledger_ok": ledger_ok,
            "ledger_note": "ledger readable" if ledger_ok else "ledger reader pending upgrade",
            "season_sources": season_sources,
        })
        with open(a.html, "w") as fh:
            fh.write(html)
        # Real stdout: --quiet must not hide where the file went.
        print(f"HTML dashboard written to {a.html}", file=sys.__stdout__)
        if a.open:
            webbrowser.open(f"file://{os.path.abspath(a.html)}")


if __name__ == "__main__":
    sys.exit(main())
