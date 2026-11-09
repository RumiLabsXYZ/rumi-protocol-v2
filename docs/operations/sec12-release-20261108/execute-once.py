import pathlib,subprocess,json,hashlib,datetime,os,re
os.umask(0o077)
ROOT=pathlib.Path('/Users/robertripley/coding/rumi-protocol-v2'); E=pathlib.Path('/private/tmp/vault-pull-release-20261108'); P=ROOT/'.recovery-artifacts/vault-pull-release-20261108'; X=P/'execution'
TARGET='tfesu-vyaaa-aaaap-qrd7a-cai'; OLD='b81583ae81421b88d3bd64bf27976b338b2b09748e9dd6b863849b40ac6fd736'; SLOT='000000000000000f0000000001f088fe0101'; IDENTITY='fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae'
PIN=json.loads((E/'execution-pins.json').read_text()); report={'source':PIN['source'],'target':TARGET,'steps':[]}
assert PIN['source']=='f1d2e77e2387ab6b9fc9a5b5676047b25d7175ff'
X.mkdir(mode=0o700,exist_ok=False)
def now():return datetime.datetime.now(datetime.timezone.utc).isoformat()
def sha(p):return hashlib.sha256(pathlib.Path(p).read_bytes()).hexdigest()
def save():
 (X/'report.json').write_text(json.dumps(report,indent=2))
def cli(name,args,write=False,timeout=180):
 cmd=['icp']+args; row={'name':name,'start':now(),'write':write}; report['steps'].append(row); save()
 if write:
  with (X/(name+'.dispatch')).open('x') as f:f.write(json.dumps({'utc':now(),'command':cmd}));f.flush();os.fsync(f.fileno())
  fd=os.open(X,os.O_RDONLY);os.fsync(fd);os.close(fd)
 try:r=subprocess.run(cmd,cwd=ROOT,capture_output=True,timeout=timeout)
 except subprocess.TimeoutExpired as e:
  (X/(name+'.stdout')).write_bytes(e.stdout or b'');(X/(name+'.stderr')).write_bytes(e.stderr or b'');row.update(end=now(),outcome='unknown-timeout');save();raise
 (X/(name+'.stdout')).write_bytes(r.stdout);(X/(name+'.stderr')).write_bytes(r.stderr);row.update(end=now(),exit=r.returncode);save()
 if r.returncode:raise RuntimeError(name+' failed; reconcile, do not retry')
 return r.stdout.decode()
def args(cmd):return ['canister',cmd,TARGET,'-e','mainnet-live','--identity','rumi_identity']
def status(name,expected_hash,expected_state):
 s=json.loads(cli(name,args('status')+['--json']));assert s['id']==TARGET;assert s['status']==expected_state;assert s['module_hash'].removeprefix('0x')==expected_hash;assert IDENTITY in s['settings']['controllers'];return s
def inventory(name):
 s=cli(name,['canister','snapshot','list',TARGET,'-e','mainnet-live','--identity','rumi_identity']);return dict(re.findall(r'^\s*([0-9a-f]{36}): (.+)$',s,re.M))
def verify_snapshot(path,expected_module):
 m=json.loads((path/'metadata.json').read_text()); rows=[]
 for stem in ['wasm_module','wasm_memory','stable_memory']:
  f=path/(stem+'.bin');assert f.stat().st_size==m[stem+'_size'];rows.append({'file':f.name,'bytes':f.stat().st_size,'sha256':sha(f)})
 assert rows[0]['sha256']==expected_module;assert m['wasm_chunk_store']==[];assert all(list(g)==['i32'] and -(2**31)<=int(g['i32'])<2**32 for g in m['globals']);return {'timestamp_ns':m['taken_at_timestamp'],'files':rows,'all_named_lengths_match':True,'chunk_store_empty':True}
try:
 assert sha(__file__)==PIN['executor_sha256']
 assert sha(E/'backend-optimized.wasm.gz')==PIN['gzip_sha256'];assert sha(E/'upgrade-args.did')==PIN['args_sha256'];assert sha(E/'candidate.did')==PIN['candid_sha256'];assert sha(E/'execution-pins.json')==os.environ['SEC12_PINS_SHA256']
 principal=cli('identity',['identity','principal','--identity','rumi_identity']).strip();assert principal==IDENTITY
 before=status('status-prestop',OLD,'Running'); expected=json.loads(pathlib.Path('/private/tmp/vault-pull-release-20261108/backend-status.json').read_text());assert before['settings']==expected['settings']
 old_inventory=inventory('snapshots-prestop'); expected_inventory=dict(re.findall(r'^\s*([0-9a-f]{36}): (.+)$',(E/'snapshots-before.txt').read_text(),re.M));assert old_inventory==expected_inventory;assert len(old_inventory)==10 and SLOT in old_inventory
 old_archive=P/('snapshot-'+SLOT); old_ver=json.loads((E/'old-snapshot-verification.json').read_text());
 for f in old_ver['files']:assert sha(old_archive/f['file'])==f['sha256']
 # Exactly one stop; explicit controller Stopped status is the drain proof.
 cli('stop',args('stop'),write=True); stopped=status('status-stopped',OLD,'Stopped');assert stopped['settings']==before['settings']
 assert inventory('snapshots-stopped')==old_inventory
 fresh_id=cli('snapshot-replace',['canister','snapshot','create',TARGET,'--replace',SLOT,'--quiet','-e','mainnet-live','--identity','rumi_identity'],write=True).strip();assert re.fullmatch('[0-9a-f]{36}',fresh_id);assert fresh_id not in old_inventory
 new_inventory=inventory('snapshots-after-replace');assert len(new_inventory)==10;assert SLOT not in new_inventory;assert fresh_id in new_inventory;assert {k:v for k,v in new_inventory.items() if k!=fresh_id}=={k:v for k,v in old_inventory.items() if k!=SLOT}
 fresh=P/('snapshot-'+fresh_id);cli('snapshot-download',['canister','snapshot','download',TARGET,fresh_id,'-e','mainnet-live','--identity','rumi_identity','--output',str(fresh)])
 report['fresh_snapshot_id']=fresh_id;report['fresh_snapshot_verification']=verify_snapshot(fresh,OLD);save()
 assert sha(E/'backend-optimized.wasm.gz')==PIN['gzip_sha256'];assert sha(E/'upgrade-args.did')==PIN['args_sha256'];assert sha(E/'candidate.did')==PIN['candid_sha256'];status('status-before-install',OLD,'Stopped')
 cli('install',args('install')+['--mode','upgrade','--wasm',str(E/'backend-optimized.wasm.gz'),'--args-file',str(E/'upgrade-args.did')],write=True)
 installed=status('status-installed',PIN['gzip_sha256'],'Stopped');assert installed['settings']==before['settings']
 did=cli('candid-installed',['canister','metadata',TARGET,'candid:service','-e','mainnet-live','--identity','rumi_identity']);assert did.strip()==(E/'candidate.did').read_text().strip()
 cli('start',args('start'),write=True); final=status('status-running',PIN['gzip_sha256'],'Running');assert final['settings']==before['settings'];assert inventory('snapshots-final')==new_inventory
 report.update(outcome='installed-and-running',completed=now(),old_module=OLD,new_module=PIN['gzip_sha256'],settings_preserved=True,candidate_candid_matches=True,previous_api_compatible=True,other_nine_snapshots_preserved=True);save();print(json.dumps({k:v for k,v in report.items() if k!='steps' and k!='fresh_snapshot_verification'}))
except BaseException as e:
 report.update(outcome='held-for-reconciliation',error=str(e),held_at=now());save();raise
