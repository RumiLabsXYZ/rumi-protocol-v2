from pathlib import Path
import subprocess,os
os.umask(0o077)
p=Path('/private/tmp/vault51-live-release-20261110');out=Path('/Users/robertripley/coding/rumi-protocol-v2/.recovery-artifacts/rumi-sec17-release-20261112')/os.environ.get('V51_PHASE','before')
escrow=bytes.fromhex('6bbd6ba3f7aa00a12f213cc93301395c882beb8bd5615f3177d6eb3336f1796a');blob=''.join('\\'+format(b,'02x') for b in escrow)
rows=[('escrow','icrc1_balance_of','(record {owner=principal "tfesu-vyaaa-aaaap-qrd7a-cai";subaccount=opt blob "'+blob+'"})'),('backend-default-icp','icrc1_balance_of','(record {owner=principal "tfesu-vyaaa-aaaap-qrd7a-cai";subaccount=null})'),('native-icp-fee','icrc1_fee','()')]
for n,m,a in rows:
 r=subprocess.run(['icp','canister','call','ryjl3-tyaaa-aaaaa-aaaba-cai',m,a,'--query','--network','ic','--identity','rumi_identity','--candid',str(p/'icrc-balance.did'),'--json'],capture_output=True,timeout=90)
 (out/(n+'.json')).write_bytes(r.stdout);(out/(n+'.stderr')).write_bytes(r.stderr)
 if r.returncode:raise RuntimeError(n+' failed')
print('Custody balance/fee queries captured')
