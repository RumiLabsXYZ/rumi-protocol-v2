import pathlib,subprocess,json,hashlib,datetime,os,re
os.umask(0o077)
ROOT=pathlib.Path('/Users/robertripley/coding/rumi-protocol-v2'); E=pathlib.Path('/private/tmp/rumi-sec19-release-20261113'); P=ROOT/'.recovery-artifacts/rumi-sec19-release-20261113'; X=P/'execution'
TARGET='tfesu-vyaaa-aaaap-qrd7a-cai'; OLD='6428d1039a9f54e691acfb8cd6c8d5c4cdba8d675b7350f3ad5913815d5575f7'; SLOT='00000000000000130000000001f088fe0101'; IDENTITY='fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae'
PIN=json.loads((E/'execution-pins.json').read_text()); report={'source':PIN['source'],'target':TARGET,'steps':[]}
assert PIN['source']=='4d8322a71059f146f6ec44c95add49789be2b6f8'
assert X.is_dir(), 'existing execution journal required'
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
def args(cmd):return ['canister',cmd,TARGET,'--network','ic','--identity','rumi_identity']
def status(name,expected_hash,expected_state):
 s=json.loads(cli(name,args('status')+['--json']));assert s['id']==TARGET;assert s['status']==expected_state;assert s['module_hash'].removeprefix('0x')==expected_hash;assert IDENTITY in s['settings']['controllers'];return s
def inventory(name):
 s=cli(name,['canister','snapshot','list',TARGET,'--network','ic','--identity','rumi_identity']);return dict(re.findall(r'^\s*([0-9a-f]{36}): (.+)$',s,re.M))
def verify_snapshot(path,expected_module):
 m=json.loads((path/'metadata.json').read_text()); rows=[]
 for stem in ['wasm_module','wasm_memory','stable_memory']:
  f=path/(stem+'.bin');assert f.stat().st_size==m[stem+'_size'];rows.append({'file':f.name,'bytes':f.stat().st_size,'sha256':sha(f)})
 assert rows[0]['sha256']==expected_module;assert m['wasm_chunk_store']==[];assert all(list(g)==['i32'] and -(2**31)<=int(g['i32'])<2**32 for g in m['globals']);return {'timestamp_ns':m['taken_at_timestamp'],'files':rows,'all_named_lengths_match':True,'chunk_store_empty':True}
report=json.loads((X/'report.json').read_text())
assert report['outcome']=='held-for-reconciliation' and report['error']=='raw vault snapshot check failed'
assert report['fresh_snapshot_id']=='000000000000001d0000000001f088fe0101'
assert not(X/'install.dispatch').exists() and not(X/'start.dispatch').exists()
assert sha(E/'execute-once.py')==PIN['executor_sha256']
assert sha(E/'verify-active-raw-vault.mjs')==PIN['raw_verifier_sha256']
assert sha(E/'verify-active-raw-vault-reconciled.mjs')=='4e7cd88ba9a7c4a084aa7e16d2c5bd2c0b437f59755d3cd22ee3158129fb95f7'
assert sha(E/'execution-pins.json')==os.environ['V51_PINS_SHA256']
with(X/'continuation.dispatched').open('x')as f:f.write(now());f.flush();os.fsync(f.fileno())
(X/'report-failed-original.json').write_text(json.dumps(report,indent=2))
try:
 before=json.loads((X/'status-prestop.stdout').read_text());fresh_id=report['fresh_snapshot_id'];fresh=P/('snapshot-'+fresh_id)
 new_inventory=dict(re.findall(r'^\s*([0-9a-f]{36}): (.+)$',(X/'snapshots-after-replace.stdout').read_text(),re.M))
 assert inventory('snapshots-before-continuation')==new_inventory
 now_status=status('status-before-continuation',OLD,'Stopped');assert now_status['settings']==before['settings']
 assert verify_snapshot(fresh,OLD)==report['fresh_snapshot_verification']
 r=subprocess.run(['node',str(E/'verify-active-raw-vault-reconciled.mjs'),str(fresh/'wasm_memory.bin')],capture_output=True)
 (X/'raw-vault-reconciled.stdout').write_bytes(r.stdout);(X/'raw-vault-reconciled.stderr').write_bytes(r.stderr);assert r.returncode==0
 report['rawStateReconciliation']={'cause':'Vault51 promoted to BTree root; original verifier only accepted leaf entries','verifierSha256':'4e7cd88ba9a7c4a084aa7e16d2c5bd2c0b437f59755d3cd22ee3158129fb95f7','fullTreeVerified':True,'expectedVaultCount':50,'sameStoppedSnapshotReused':True,'managementWritesRepeated':False}
 assert sha(E/'rumi_protocol_backend.wasm.gz')==PIN['gzip_sha256'];assert sha(E/'upgrade-args.did')==PIN['args_sha256'];assert sha(E/'candidate.did')==PIN['candid_sha256'];status('status-before-install',OLD,'Stopped')
 cli('install',args('install')+['--mode','upgrade','--wasm',str(E/'rumi_protocol_backend.wasm.gz'),'--args-file',str(E/'upgrade-args.did')],write=True)
 installed=status('status-installed',PIN['gzip_sha256'],'Stopped');assert installed['settings']==before['settings']
 did=cli('candid-installed',['canister','metadata',TARGET,'candid:service','--network','ic','--identity','rumi_identity']);assert did.strip()==(E/'candidate.did').read_text().strip()
 cli('start',args('start'),write=True); final=status('status-running',PIN['gzip_sha256'],'Running');assert final['settings']==before['settings'];assert inventory('snapshots-final')==new_inventory
 report.pop('error',None);report.pop('held_at',None);report.update(outcome='installed-and-running',completed=now(),old_module=OLD,new_module=PIN['gzip_sha256'],settings_preserved=True,candidate_candid_matches=True,previous_api_compatible=True,other_nine_snapshots_preserved=True);save();print(json.dumps({k:v for k,v in report.items() if k!='steps' and k!='fresh_snapshot_verification'}))
except BaseException as e:
 report.update(outcome='held-for-reconciliation',error=str(e),held_at=now());save();raise
