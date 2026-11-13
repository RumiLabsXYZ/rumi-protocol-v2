import pathlib,subprocess,json,hashlib,os,re,datetime
src=pathlib.Path('/private/tmp/rumi-swap-support-cleanup-20261111');out=pathlib.Path('/private/tmp/rumi-sec19-pic-20261113');prior=pathlib.Path('/private/tmp/rumi-sec19-continuation-20261113');expected='7dd8cb29b878cec9d01f256457a631ff2b27f08a'
def git(*args):return subprocess.check_output(['git',*args],cwd=src)
assert git('rev-parse','HEAD').decode().strip()==expected
roots=['Cargo.toml','Cargo.lock','src/rumi_protocol_backend','src/rumi_cycle_manager','src/flaky_ledger','src/xrc_demo/xrc/xrc.wasm']
assert not git('diff',expected,'--',*roots)
assert not git('diff','e2a0122a',expected,'--','Cargo.toml','Cargo.lock','src/rumi_protocol_backend/src','src/rumi_cycle_manager','src/flaky_ledger'), 'production source changed'
files=git('ls-tree','-r','--name-only',expected,'--',*roots).decode().splitlines()
for name in files:assert (src/name).read_bytes()==git('show',expected+':'+name)
server=pathlib.Path('/Users/robertripley/coding/rumi-protocol-v2/pocket-ic');assert hashlib.sha256(server.read_bytes()).hexdigest()=='87582439bf456221256c66e86b382a56f5df7a6a8da85738eaa233d2ada3ed47'
wasm=prior/'backend-test.wasm';ledger=pathlib.Path('/private/tmp/rumi-cksol-icp-target/wasm32-unknown-unknown/release/flaky_ledger.wasm')
assert hashlib.sha256(wasm.read_bytes()).hexdigest()=='2961d2d84008e43756c25c3361b59717676ea92d68eb82951648dc5a4703ea08'
assert hashlib.sha256(ledger.read_bytes()).hexdigest()=='e7f4ebb3b53f5239a6922ad6df836cc09a4d20fa6efc7ef7ba049b8c61a0eaf2'
assert hashlib.sha256((src/'src/rumi_protocol_backend/rumi_protocol_backend.did').read_bytes()).hexdigest()=='d8b0142e705e860640557b7ea6253a2a6027d7e96f3b15cfee9ba51fff0d2c97'
args=['cargo','test','--locked','--offline','-p','rumi_protocol_backend','--test','vault_collateral_pull_pic','sec19_','--','--test-threads=1']
env=os.environ.copy();env.update(CARGO_TARGET_DIR='/private/tmp/rumi-cksol-icp-target',CARGO_BUILD_JOBS='1',BINARYEN_CORES='1',CARGO_NET_OFFLINE='true',POCKET_IC_BIN=str(server),RUMI_VAULT_PULL_BACKEND_WASM=str(wasm),RUMI_VAULT_PULL_LEDGER_WASM=str(ledger))
start=datetime.datetime.now(datetime.timezone.utc).isoformat()
with (out/'pic-sec19.log').open('wb') as log:r=subprocess.run(args,cwd=src,env=env,stdout=log,stderr=subprocess.STDOUT)
(out/'steps.json').write_text(json.dumps([{'name':'pic-sec19','args':args,'start':start,'end':datetime.datetime.now(datetime.timezone.utc).isoformat(),'exit':r.returncode}],indent=2)+'\n')
assert r.returncode==0,'PIC failed; stop without retry'
assert re.search(r'test result: ok\. 3 passed; 0 failed;', (out/'pic-sec19.log').read_text()), 'expected exactly three positive tests'
assert not git('diff',expected,'--',*roots),'source changed mid-validation'
print('Three corrected SEC19 lifecycle tests passed; no production or financial action')
