import pathlib,subprocess,os,json,hashlib
os.umask(0o077)
root=pathlib.Path('/Users/robertripley/coding/rumi-protocol-v2/.recovery-artifacts/rumi-sec17-release-20261112')/os.environ.get('V51_PHASE','before');src=pathlib.Path('/Users/robertripley/coding/rumi-protocol-v2/.worktrees/xrp-customer-withdrawal-address-20261106')
reads=[('vault-count','tfesu-vyaaa-aaaap-qrd7a-cai','get_vault_count','()',str(src/'src/rumi_protocol_backend/rumi_protocol_backend.did')),('backend-vaults','tfesu-vyaaa-aaaap-qrd7a-cai','get_vaults','(null)',str(src/'src/rumi_protocol_backend/rumi_protocol_backend.did')),('bot-vault158','nygob-3qaaa-aaaap-qttcq-cai','get_vault158_recovery','()',str(src/'src/liquidation_bot/liquidation_bot.did')),('bot-recovery-config','nygob-3qaaa-aaaap-qttcq-cai','get_recovery_config','()',str(src/'src/liquidation_bot/liquidation_bot.did'))]
for name,target,method,args,did in reads:
 r=subprocess.run(['icp','canister','call',target,method,args,'--query','--network','ic','--identity','rumi_identity','--candid',did,'--json'],capture_output=True,timeout=90)
 (root/(name+'.json')).write_bytes(r.stdout);(root/(name+'.stderr')).write_bytes(r.stderr)
 if r.returncode:raise RuntimeError(name+' read failed')
print('Extra vault and completed vault158/config query baselines captured privately')
