import pathlib,subprocess,json,datetime,hashlib,os
root=pathlib.Path('/Users/robertripley/coding/rumi-protocol-v2/.recovery-artifacts/vault-pull-release-20261108')
phase=os.environ.get('SEC12_PHASE','before'); out=root/phase; out.mkdir(exist_ok=False,mode=0o700)
base=['icp','canister','call','tfesu-vyaaa-aaaap-qrd7a-cai']
reads=json.loads(pathlib.Path('/Users/robertripley/.codex/worktrees/solana-icp-processing/rumi-protocol-v2/build-evidence/cksol-integrated-6847982b/reads.json').read_text())
reads += [{'method':m,'args':'()'} for m in ['get_cksol_icp_terms','get_nicp_redemption_claim','get_xrp_claims']]
reads += [{'method':m,'args':pathlib.Path('/private/tmp/sec09-cksol-diagnostic-20261106/customer-owner.did').read_text().strip(),'label':'customer-'+m} for m in ['get_cksol_deposit','get_cksol_payment']]
results=[]
for row in reads:
 name=row.get('label',row['method']); cmd=base+[row['method'],row['args'],'--query','-e','mainnet-live','--identity','rumi_identity','--candid','/private/tmp/vault-pull-release-20261108/live.did']
 p=subprocess.run(cmd,capture_output=True,timeout=90); (out/(name+'.txt')).write_bytes(p.stdout); (out/(name+'.stderr')).write_bytes(p.stderr)
 results.append({'method':row['method'],'label':name,'exit':p.returncode,'sha256':hashlib.sha256(p.stdout).hexdigest(),'utc':datetime.datetime.now(datetime.timezone.utc).isoformat()})
 if p.returncode: raise RuntimeError('read failed: '+name)
(out/'read-manifest.json').write_text(json.dumps(results,indent=2)); print('Saved',len(results),'private baseline reads; no update calls')
