import pathlib,subprocess,hashlib,json,datetime,os,shutil
src=pathlib.Path('/Users/robertripley/coding/rumi-protocol-v2/.worktrees/xrp-customer-withdrawal-address-20261106');out=pathlib.Path('/private/tmp/rumi-sec17-release-20261112');target=pathlib.Path('/private/tmp/rumi-cksol-icp-target')
expected='00cf550d3fcb97870cd4643883d5abd8900fa1a2'
assert subprocess.check_output(['git','rev-parse','HEAD'],cwd=src,text=True).strip()==expected
inputs=[]
for root in ['src/rumi_protocol_backend','src/rumi_cycle_manager']:
 for f in sorted((src/root).rglob('*')):
  if f.is_file() and f.suffix in ['.rs','.toml','.did']:
   inputs.append({'path':str(f.relative_to(src)),'sha256':hashlib.sha256(f.read_bytes()).hexdigest()});f.touch()
(out/'build-inputs.json').write_text(json.dumps(inputs,indent=2)+'\n')
env=os.environ.copy();env.update(CARGO_TARGET_DIR=str(target),CARGO_BUILD_JOBS='2',BINARYEN_CORES='2',NODE_OPTIONS='--max-old-space-size=6144',npm_config_offline='true',npm_config_update_notifier='false')
results=[]
def run(name,args):
 start=datetime.datetime.now(datetime.timezone.utc).isoformat()
 with (out/(name+'.log')).open('wb') as log:r=subprocess.run(args,cwd=src,env=env,stdout=log,stderr=subprocess.STDOUT)
 results.append({'name':name,'args':args,'start':start,'end':datetime.datetime.now(datetime.timezone.utc).isoformat(),'exit':r.returncode});(out/'build-steps.json').write_text(json.dumps(results,indent=2)+'\n')
 assert r.returncode==0,name+' failed; no automatic retry'
run('cargo-release',['cargo','build','--locked','--offline','--release','--target','wasm32-unknown-unknown','--package','rumi_protocol_backend'])
raw=out/'backend-raw.wasm';opt=out/'backend-optimized.wasm';shutil.copy2(target/'wasm32-unknown-unknown/release/rumi_protocol_backend.wasm',raw);shutil.copy2(raw,opt)
run('metadata',['ic-wasm',str(opt),'-o',str(opt),'metadata','candid:service','-f','src/rumi_protocol_backend/rumi_protocol_backend.did','-v','public'])
run('shrink',['ic-wasm',str(opt),'-o',str(opt),'shrink','--keep-name-section'])
run('optimize',['ic-wasm',str(opt),'-o',str(opt),'optimize','O1','--keep-name-section'])
with (out/'backend-optimized.wasm.gz').open('wb') as f:subprocess.run(['gzip','-9','-n','-c',str(opt)],stdout=f,check=True)
run('frontend-build',['npm','run','build','--workspace=src/vault_frontend'])
