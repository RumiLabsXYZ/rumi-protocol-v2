import os,pathlib,subprocess,json,hashlib,shutil,datetime,re
src=pathlib.Path('/private/tmp/rumi-swap-support-cleanup-20261111');out=pathlib.Path('/private/tmp/rumi-sec19-validation-20261113');expected='e2a0122a9a35393e2c0509540551594a403695ab';target=pathlib.Path('/private/tmp/rumi-cksol-icp-target')
def now():return datetime.datetime.now(datetime.timezone.utc).isoformat()
def git(*args):return subprocess.check_output(['git',*args],cwd=src)
assert git('rev-parse','HEAD').decode().strip()==expected
roots=['Cargo.toml','Cargo.lock','src/rumi_protocol_backend','src/rumi_cycle_manager','src/flaky_ledger','src/xrc_demo/xrc/xrc.wasm']
assert not git('diff',expected,'--',*roots),'backend validation inputs changed'
paths=git('ls-tree','-r','--name-only',expected,'--',*roots).decode().splitlines();inputs=[]
for name in paths:
 p=src/name;assert p.read_bytes()==git('show',expected+':'+name);inputs.append({'path':name,'sha256':hashlib.sha256(p.read_bytes()).hexdigest()})
(out/'inputs.json').write_text(json.dumps(inputs,indent=2)+'\n');(out/'prior.did').write_bytes(git('show','00cf550d:src/rumi_protocol_backend/rumi_protocol_backend.did'))
server=pathlib.Path('/Users/robertripley/coding/rumi-protocol-v2/pocket-ic');assert subprocess.check_output([str(server),'--version'],text=True).strip()=='pocket-ic-server 7.0.0'
assert hashlib.sha256(server.read_bytes()).hexdigest()==json.loads((out/'tool-pins.json').read_text())['pocketIcSha256']
env=os.environ.copy();env.update(CARGO_TARGET_DIR=str(target),CARGO_BUILD_JOBS='1',BINARYEN_CORES='1',CARGO_NET_OFFLINE='true',POCKET_IC_BIN=str(server))
rows=[]
def run(name,args,extra=None):
 e=env.copy();e.update(extra or {});start=now()
 with (out/(name+'.log')).open('wb') as log:r=subprocess.run(args,cwd=src,env=e,stdout=log,stderr=subprocess.STDOUT)
 rows.append({'name':name,'args':args,'start':start,'end':now(),'exit':r.returncode});(out/'steps.json').write_text(json.dumps(rows,indent=2)+'\n');assert r.returncode==0,name+' failed; stop without retry'
 if name in ['unit-sec19','journal-compat','dispatch-classification','candid-generate','candid-declared','pic-sec19']:
  text=(out/(name+'.log')).read_text();assert re.search(r'test result: ok\. [1-9][0-9]* passed',text),name+' ran no tests'
for name in paths:
 if name.endswith(('.rs','.toml')):(src/name).touch()
base=['cargo','test','--locked','--offline','-p','rumi_protocol_backend']
run('unit-sec19',base+['--lib','sec19_','--','--test-threads=1'])
run('journal-compat',base+['--lib','wide_confirmed_collateral_pull_keeps_receipt_and_cannot_dispatch_again','--','--test-threads=1'])
run('dispatch-classification',base+['--lib','management::vault_collateral_pull_error_tests','--','--test-threads=1'])
run('candid-generate',base+['--bin','rumi_protocol_backend','check_candid_interface_compatibility','--','--exact','--test-threads=1'],{'RUMI_REGEN_DID':'1'})
run('bindings',['bash','scripts/regenerate-declarations.sh','rumi_protocol_backend'])
run('candid-live-compat',['didc','check','src/rumi_protocol_backend/rumi_protocol_backend.did',str(out/'prior.did')])
run('candid-declared',base+['--bin','rumi_protocol_backend','check_candid_interface_compatibility','--','--exact','--test-threads=1'])
run('wasm',['cargo','build','--locked','--offline','--release','--target','wasm32-unknown-unknown','-p','rumi_protocol_backend','-p','flaky_ledger'])
raw=target/'wasm32-unknown-unknown/release/rumi_protocol_backend.wasm';opt=out/'backend-test.wasm';shutil.copy2(raw,opt)
run('wasm-metadata',['ic-wasm',str(opt),'-o',str(opt),'metadata','candid:service','-f','src/rumi_protocol_backend/rumi_protocol_backend.did','-v','public'])
run('wasm-shrink',['ic-wasm',str(opt),'-o',str(opt),'shrink','--keep-name-section'])
run('wasm-optimize',['ic-wasm',str(opt),'-o',str(opt),'optimize','O1','--keep-name-section'])
b=opt.read_bytes();assert b[:4]==b'\0asm';pos=8;code=None
def leb(p):
 n=0;s=0
 while True:
  v=b[p];p+=1;n|=(v&127)<<s
  if v<128:return n,p
  s+=7
while pos<len(b):
 kind=b[pos];length,start=leb(pos+1)
 if kind==10:code=length
 pos=start+length
assert code is not None and code<=10223616,'test code exceeds preserved project guard'
ledger=target/'wasm32-unknown-unknown/release/flaky_ledger.wasm'
(out/'test-artifacts.json').write_text(json.dumps({'source':expected,'codeSectionBytes':code,'backendSha256':hashlib.sha256(b).hexdigest(),'ledgerSha256':hashlib.sha256(ledger.read_bytes()).hexdigest()},indent=2)+'\n')
run('pic-sec19',base+['--test','vault_collateral_pull_pic','sec19_','--','--test-threads=1'],{'RUMI_VAULT_PULL_BACKEND_WASM':str(opt),'RUMI_VAULT_PULL_LEDGER_WASM':str(ledger)})
# Generated Candid/bindings are expected writes. Frontend work is a separate source lane.
for f in inputs:
 if not f['path'].endswith('.did'):assert hashlib.sha256((src/f['path']).read_bytes()).hexdigest()==f['sha256'],'backend source changed mid-validation'
print('Bounded SEC19 validation completed; no production or financial action')
