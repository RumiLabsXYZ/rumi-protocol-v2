"""Bounded controller backfill operator; no upgrade, mint, transfer or retry.
Requires separately recorded explicit authority and successful installed-module
verification. Unknown outcomes stop; preserve the marker and all receipts.
"""
from pathlib import Path
from datetime import datetime, timezone
import argparse, json, re, subprocess

CID = "tlg74-oiaaa-aaaap-qrd6a-cai"
METHOD = "continue_icusd_deposit_block_backfill"
HASH = "73f5a6b7e06cd91ab808887a6366f1e780054b40b0621928b9b739e5964d32e3"
parser = argparse.ArgumentParser()
parser.add_argument("evidence", type=Path)
parser.add_argument("--candid", required=True, type=Path)
parser.add_argument("--decoder", required=True, type=Path)
a = parser.parse_args()
auth = json.loads((a.evidence / "authorization.json").read_text())
assert auth["treasury_one_attempt_authorized"]
assert auth["unresolved_earlier_request_risk_accepted"]
assert json.loads((a.evidence / "installation-verification.json").read_text())["passed"]
status = subprocess.run(["icp", "canister", "status", CID, "-n", "ic", "--identity", "rumi_identity"], capture_output=True, text=True, timeout=90)
(a.evidence / "before-backfill-module.txt").write_text(status.stdout + status.stderr)
assert status.returncode == 0 and "Status: Running" in status.stdout and HASH in status.stdout
with (a.evidence / "backfill-exclusive-marker.json").open("x") as f:
    json.dump({"began_utc": datetime.now(timezone.utc).isoformat(), "max_total_update_calls": 32, "max_migration_calls": 31, "completed_noop_calls": 1}, f)

def step(tag):
    cmd = ["icp", "canister", "call", CID, METHOD, "()", "-n", "ic", "--identity", "rumi_identity", "--candid", str(a.candid)]
    began = datetime.now(timezone.utc).isoformat()
    with (a.evidence / (tag + "-request.json")).open("x") as f:
        json.dump({"began_utc": began, "command": cmd}, f)
    try:
        p = subprocess.run(cmd, capture_output=True, text=True, timeout=90)
        code, out, err = p.returncode, p.stdout, p.stderr
    except subprocess.TimeoutExpired:
        code, out, err = 124, "", "Unknown outcome; no automatic retry"
    raw = a.evidence / (tag + ".txt")
    raw.write_text(out)
    (a.evidence / (tag + ".stderr.txt")).write_text(err)
    receipt = {"began_utc": began, "ended_utc": datetime.now(timezone.utc).isoformat(), "exit_code": code, "method": METHOD}
    if code == 0:
        try:
            d = subprocess.run([str(a.decoder), str(a.candid), METHOD, str(raw), str(a.evidence / (tag + ".typed.json"))], capture_output=True, text=True, timeout=30)
            receipt["decoder_exit_code"], decoder_error = d.returncode, d.stderr
        except subprocess.TimeoutExpired:
            receipt["decoder_exit_code"], decoder_error = 124, "Decoder timeout after submitted update; inspect retained raw result, no retry"
        except OSError as error:
            receipt["decoder_exit_code"], decoder_error = 127, str(error)
        (a.evidence / (tag + ".decoder.stderr.txt")).write_text(decoder_error)
    (a.evidence / (tag + "-receipt.json")).write_text(json.dumps(receipt, indent=2) + "\n")
    assert code == receipt.get("decoder_exit_code") == 0, receipt
    value, = json.loads((a.evidence / (tag + ".typed.json")).read_text())
    assert set(value) == {"Ok"}, value
    return value["Ok"]

last_next_id = 0
for i in range(31):
    message = step("backfill-" + str(i))
    match = re.fullmatch(r"ICUSD deposit block backfill complete; processed (\d+) rows", message)
    if match:
        assert int(match.group(1)) <= 100
        again = step("backfill-completed-noop")
        assert again == "ICUSD deposit block backfill complete; processed 0 rows"
        print("Actual backfill completion and zero-row completed no-op observed. Retained-data verification still required.")
        break
    match = re.fullmatch(r"ICUSD deposit block backfill processed (\d+) rows; next deposit ID (\d+)", message)
    assert match and 0 < int(match.group(1)) <= 100, message
    next_id = int(match.group(2))
    assert next_id > last_next_id, "Backfill cursor failed to advance"
    last_next_id = next_id
    print(message, flush=True)
else:
    raise RuntimeError("Bound exhausted; inspect durable state and receipts, never rerun the helper blindly")
