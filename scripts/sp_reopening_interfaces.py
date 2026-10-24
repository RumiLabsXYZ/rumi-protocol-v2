#!/usr/bin/env python3
"""Derive additive withdrawal wire types from a full Rust Candid export.

The input baseline is the deployed contract. Existing liquidation metadata
drift is recorded separately; this generator does not alter those handlers.
"""
from pathlib import Path
import argparse
import hashlib
import json
import re
import subprocess

parser = argparse.ArgumentParser()
parser.add_argument('--source', type=Path, required=True)
parser.add_argument('--baseline', type=Path, required=True)
parser.add_argument('--root', type=Path, required=True)
parser.add_argument('--evidence', type=Path, required=True)
args = parser.parse_args()
args.evidence.mkdir(parents=True, exist_ok=True)
added = ['get_my_pending_withdrawals', 'reconcile_withdrawal']
full = args.source.read_text()
legacy = args.baseline.read_text()
starts = list(re.finditer(r'(?m)^type (\w+)\s*=', full))
service = re.search(r'(?m)^service\s*:', full)
assert service
blocks = {m.group(1): full[m.start():(starts[i+1].start() if i+1 < len(starts) else service.start())].strip() for i, m in enumerate(starts)}
methods = []
for name in added:
    m = re.search(r'(?m)^  '+name+r' :[\s\S]*?;', full[service.start():])
    assert m, name
    methods.append(m.group(0))
needed = set()
todo = set(re.findall(r'\b[A-Za-z_]\w*\b', '\n'.join(methods))) & set(blocks)
while todo:
    name = todo.pop()
    if name in needed:
        continue
    needed.add(name)
    todo |= (set(re.findall(r'\b[A-Za-z_]\w*\b', blocks[name])) & set(blocks)) - needed
aliases = {name: 'WithdrawalWire_'+name for name in needed}
def rename(text):
    return re.sub(r'\b[A-Za-z_]\w*\b', lambda m: aliases.get(m.group(0), m.group(0)), text)
newtypes = '\n'.join(rename(blocks[name]) for name in blocks if name in needed)+'\n'
actor = re.search(r'(?m)^service\s*:', legacy)
assert actor
body = legacy[actor.start():]
end = body.rfind('}')
contract = legacy[:actor.start()]+newtypes+body[:end]+'\n'+'\n'.join(rename(m) for m in methods)+'\n'+body[end:]
contract = contract.rstrip()+'\n'
generated = args.evidence/'generated.did'
generated.write_text(contract)
projected = contract
for name in added:
    projected, count = re.subn(r'(?m)^  '+name+r' :[\s\S]*?;\n', '', projected)
    assert count == 1
projection = args.evidence/'legacy-projection.did'
projection.write_text(projected)
source_only = args.evidence/'new-source-only.did'
source_only.write_text('\n'.join(blocks[n] for n in blocks if n in needed)+'\nservice : {\n'+'\n'.join(methods)+'\n}\n')
wire_only = args.evidence/'new-wire-only.did'
wire_only.write_text(newtypes+'\nservice : {\n'+'\n'.join(rename(m) for m in methods)+'\n}\n')
checks = []
for name, left, right in [('upgrade-compatible', generated, args.baseline), ('legacy-forward', projection, args.baseline), ('legacy-reverse', args.baseline, projection), ('new-source-forward', source_only, wire_only), ('new-source-reverse', wire_only, source_only)]:
    cmd = ['didc', 'check', str(left), str(right)]
    p = subprocess.run(cmd, capture_output=True, text=True)
    checks.append({'name': name, 'command': cmd, 'exit_code': p.returncode, 'stderr': p.stderr})
    assert p.returncode == 0, checks[-1]
targets = [args.root/'src/stability_pool/stability_pool.did', args.root/'src/declarations/rumi_stability_pool/rumi_stability_pool.did']
for target in targets:
    target.write_text(contract)
for kind, suffix in [('js', '.did.js'), ('ts', '.did.d.ts')]:
    p = subprocess.run(['didc', 'bind', str(generated), '-t', kind], capture_output=True, text=True)
    assert p.returncode == 0, p.stderr
    target = args.root/'src/declarations/rumi_stability_pool'/('rumi_stability_pool'+suffix)
    target.write_text(p.stdout)
report = {'checks': checks, 'new_methods': added, 'generated_from_full_Rust_export': True, 'existing_published_methods_unchanged': True, 'source_export_sha256': hashlib.sha256(args.source.read_bytes()).hexdigest(), 'baseline_sha256': hashlib.sha256(args.baseline.read_bytes()).hexdigest(), 'contract_sha256': hashlib.sha256(generated.read_bytes()).hexdigest(), 'existing_liquidation_metadata_drift_repaired': False}
(args.evidence/'verification.json').write_text(json.dumps(report, indent=2)+'\n')
print(json.dumps({'checks_passed': len(checks), 'new_methods': added}))
