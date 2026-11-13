import os,pathlib,subprocess,json,hashlib,shutil,datetime,re
src=pathlib.Path('/private/tmp/rumi-swap-support-cleanup-20261111');out=pathlib.Path('/private/tmp/rumi-sec19-continuation-20261113');expected='e2a0122a9a35393e2c0509540551594a403695ab';target=pathlib.Path('/private/tmp/rumi-cksol-icp-target')
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
# Retain the first allocation's nine positive test results without rerunning them.
prior=pathlib.Path('/private/tmp/rumi-sec19-validation-20261113')
prior_checks={'source': 'e2a0122a9a35393e2c0509540551594a403695ab', 'allocationId': '174c2752-147f-4475-99b0-f86aafcc1816', 'completedTests': {'unit-sec19': 6, 'journal-compat': 1, 'dispatch-classification': 2}, 'stop': 'physical growth bound during unfinished Candid generation', 'files': {'validation-session.json': '27fe2bbf8407def44ccf26412bd6060b5c873ac48f74ec05bc18f2dac58fec8d', 'steps.json': '3c2a7233e65fb23e53c22650bbeb0370f9a23844cb8c6712b55a41136fb0d721', 'inputs.json': 'd7e11e79b653e3caaf5cf626e220559a516f50db36d058f8faa1164917deb273', 'unit-sec19.log': '31ee920fb91d55515b3fae655d6e76517cc23f805f0568070fedfc152d6a3053', 'journal-compat.log': 'edfc2c4bf465988c4d3a14d81c0226faba3bc7cc143f04a94577153d8dca65b4', 'dispatch-classification.log': 'd38b8857f44d36f6942f90d453a332912955ede95c466fe8951901a621af0dd6', 'candid-generate.log': '2f67cfa52fffbb0b12b07d2caedcb13d36cd029bcec92f170ca17a0a4e3bcbbd', 'independent-group-closure.json': '752aac94f207736624502beabdde4717b40dcdd013993ce4b99633ee72fc72e4'}, 'productionMutation': False}
assert prior_checks['source']==expected
for name,digest in prior_checks['files'].items():
 assert hashlib.sha256((prior/name).read_bytes()).hexdigest()==digest,'prior validation evidence changed: '+name
completed=json.loads((prior/'steps.json').read_text())
assert [(r['name'],r['exit']) for r in completed]==[('unit-sec19',0),('journal-compat',0),('dispatch-classification',0)]
for name,count in prior_checks['completedTests'].items():
 assert f'test result: ok. {count} passed;' in (prior/(name+'.log')).read_text()
base=['cargo','test','--locked','--offline','-p','rumi_protocol_backend']
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
