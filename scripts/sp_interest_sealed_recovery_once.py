"""Root-only sealed interest credits AFTER a separately authorized/verified upgrade.
No install, ledger call, notification replay, resume, deletion or receipt promotion.
Never rerun after an uncertain update: retain its request and reconcile read-only.
"""
from pathlib import Path
from datetime import datetime, timezone
import argparse, hashlib, json, subprocess
TARGET = 'tmhzi-dqaaa-aaaap-qrd6q-cai'
MODULE = 'b26fb0358ad3c5d34256e13f069ae3ddfe50072afffc477f7fbe2d402cb384a8'
SEAL = '13fa5654846d816d8debf8de544c358f13d25874a36e31ae35225fac3fba6183'
DEVELOPER = 'fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae'
CONTROLLERS = {DEVELOPER, 'cpbhu-5iaaa-aaaad-aalta-cai', 'mi66c-zqlu4-4kxd6-2gtp7-szg5v-6a62a-geoty-fahu5-4trje-xyfby-wqe'}
FLAGS = ['-n', 'ic', '--identity', 'rumi_identity']

def require(condition, message):
    if not condition:
        raise RuntimeError(message)

def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--evidence', type=Path, required=True)
    p.add_argument('--manifest', type=Path, required=True)
    p.add_argument('--candid', type=Path, required=True)
    p.add_argument('--decoder', type=Path, required=True)
    a = p.parse_args()
    e = a.evidence
    grant = json.loads((e / 'authorization.json').read_text())
    require(grant['one_SP_attempt_and_exact237_credits_authorized'] is True, 'Recovery precondition failed; stop without retry')
    require(grant['unresolved_earlier_SP_request_risk_accepted'] is True, 'Recovery precondition failed; stop without retry')
    require(grant['actual_Wasm_migration_alternative_explicitly_accepted'] is True, 'Recovery precondition failed; stop without retry')
    require(grant['seal'] == SEAL and grant['target'] == TARGET, 'Recovery precondition failed; stop without retry')
    require(grant['total_credit_raw'] == 1256672472, 'Recovery precondition failed; stop without retry')
    ready = json.loads((e / 'ready-for-credit.json').read_text())
    require(ready['passed'] is True and ready['root_sole_executor'] is True, 'Recovery precondition failed; stop without retry')
    require(ready['fresh_backup_verified'] is True, 'Recovery precondition failed; stop without retry')
    require(ready['installed_candidate_and_retained_state_verified'] is True, 'Recovery precondition failed; stop without retry')
    require(len(ready['independent_actual_preflight_reviews']) == 2, 'Recovery precondition failed; stop without retry')
    require(ready['all13_positions_and_accounting_baseline_exact'] is True, 'Recovery precondition failed; stop without retry')
    require(ready['no_prior_recovery_progress'] is True, 'Recovery precondition failed; stop without retry')
    require(hashlib.sha256(a.manifest.read_bytes()).hexdigest() == SEAL, 'Recovery precondition failed; stop without retry')
    require(hashlib.sha256(a.candid.read_bytes()).hexdigest() == ready['candid_sha256'], 'Recovery precondition failed; stop without retry')
    require(hashlib.sha256(a.decoder.read_bytes()).hexdigest() == ready['decoder_sha256'], 'Recovery precondition failed; stop without retry')
    out = e / 'sealed-credit-once'
    out.mkdir(mode=448, exist_ok=False)

    def now():
        return datetime.now(timezone.utc).isoformat()

    def fresh():
        age = (datetime.now(timezone.utc) - datetime.fromisoformat(ready['verified_utc'])).total_seconds()
        require(0 <= age <= 300, 'Preflight expired before first credit; no dispatch')

    def run(tag, cmd, method=None):
        (out / (tag + '-request.json')).write_text(json.dumps({'began_utc': now(), 'command': cmd}, indent=2))
        try:
            r = subprocess.run(cmd, capture_output=True, text=True, timeout=120)
        except (OSError, subprocess.TimeoutExpired) as err:
            (out / (tag + '-unknown.json')).write_text(json.dumps({'error': str(err), 'no_resubmission': True}))
            raise
        (out / (tag + '.txt')).write_text(r.stdout)
        (out / (tag + '.stderr.txt')).write_text(r.stderr)
        receipt = {'ended_utc': now(), 'exit_code': r.returncode, 'no_automatic_retry': True}
        (out / (tag + '-receipt.json')).write_text(json.dumps(receipt))
        require(r.returncode == 0, (tag, 'STOP; reconcile original request, no retry'))
        if method is None:
            return r.stdout
        decoded = out / (tag + '.typed.json')
        d = subprocess.run([str(a.decoder), str(a.candid), method, str(out / (tag + '.txt')), str(decoded)], capture_output=True, text=True, timeout=30)
        (out / (tag + '.decode.stderr.txt')).write_text(d.stderr)
        receipt['decoder_exit_code'] = d.returncode
        (out / (tag + '-receipt.json')).write_text(json.dumps(receipt))
        require(d.returncode == 0, (tag, 'decode uncertain; reconcile read-only'))
        return json.loads(decoded.read_text())[0]

    def query(tag, method):
        return run(tag, ['icp', 'canister', 'call', TARGET, method, '()', '--query', '--candid', str(a.candid), *FLAGS], method)

    def update(tag, method, args=None, binary=None):
        cmd = ['icp', 'canister', 'call', TARGET, method]
        if binary is not None:
            cmd += ['--args-file', str(binary), '--args-format', 'bin']
        else:
            cmd += [args]
        return run(tag, cmd + ['--candid', str(a.candid), *FLAGS], method)

    def ok(v):
        require(isinstance(v, dict) and set(v) == {'Ok'}, v)
        return v['Ok']

    def integer(v):
        return int(str(v).replace('_', ''))

    def status(v, cursor):
        v = ok(v)
        require(integer(v['packet_count']) == 237 and integer(v['next_packet']) == cursor, 'Recovery precondition failed; stop without retry')
        require(v['manifest_hash'] == {'blob_hex': SEAL}, 'Recovery precondition failed; stop without retry')
        return v
    fresh()
    require(run('identity', ['icp', 'identity', 'principal', '--identity', 'rumi_identity']).strip() == DEVELOPER, 'Recovery precondition failed; stop without retry')
    management = run('management-status', ['icp', 'canister', 'status', TARGET, *FLAGS])
    require('Status: Running' in management and MODULE in management, 'Recovery precondition failed; stop without retry')
    actual = set(next((l for l in management.splitlines() if l.strip().startswith('Controllers:'))).split(':', 1)[1].replace(',', ' ').split())
    require(actual == CONTROLLERS, 'Recovery precondition failed; stop without retry')
    require(ok(query('initial-recovery-status', 'get_interest_recovery_status')) is None, 'Recovery precondition failed; stop without retry')
    fresh()
    (out / 'exclusive-stage-marker.json').write_text(json.dumps({'dispatched_utc': now(), 'seal': SEAL, 'do_not_repeat': True}))
    status(update('stage', 'stage_interest_recovery', binary=a.manifest), 0)
    seal_arg = 'blob "' + ''.join(('\\' + SEAL[i:i + 2] for i in range(0, 64, 2))) + '"'
    for cursor in range(0, 237, 10):
        expected = min(cursor + 10, 237)
        v = status(update('continue-' + str(cursor), 'continue_interest_recovery', '(' + seal_arg + ', ' + str(cursor) + ' : nat16, 10 : nat8)'), expected)
        require(v['live_interest_only'] is False, 'Recovery precondition failed; stop without retry')
        observed = ok(query('after-' + str(expected), 'get_interest_recovery_status'))
        require(observed == {'Some': v}, 'Readback differs; stop without another update')
    require(integer(v['credited_raw']) == 1256672472, 'Final sealed sum differs; do not finish')
    (out / 'all-credit-batches-returned.json').write_text(json.dumps({'completed_utc': now(), 'next_packet': 237, 'seal': SEAL, 'receiver_verification_and_finish_still_required': True, 'overall_backlog_complete': False}, indent=2))
if __name__ == '__main__':
    main()
