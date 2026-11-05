import concurrent.futures,subprocess,json,re,pathlib
import argparse
_parser=argparse.ArgumentParser();_parser.add_argument('--evidence-dir',required=True);p=pathlib.Path(_parser.parse_args().evidence_dir)
principals=[r['principal'] for r in json.loads((p/'comparison-not-allocation.json').read_text())['rows']]
def fetch(item):
 principal,kind,canister,method,args=item
 v=subprocess.run(['icp','canister','call',canister,method,args,'--network','ic','--query'],capture_output=True,text=True,check=True).stdout
 folder=p/'current-positions';folder.mkdir(exist_ok=True);(folder/(principal+'-'+kind+'.candid')).write_text(v)
 return kind
jobs=[]
for principal in principals:
 jobs.extend([(principal,'points','bfnu3-6aaaa-aaaab-qhanq-cai','get_principal_state',f'(principal "{principal}")'),(principal,'vaults','tfesu-vyaaa-aaaap-qrd7a-cai','get_vaults',f'(opt principal "{principal}")'),(principal,'sp','tmhzi-dqaaa-aaaap-qrd6q-cai','get_user_position',f'(opt principal "{principal}")'),(principal,'wallet','fohh4-yyaaa-aaaap-qtkpa-cai','icrc1_balance_of',f'(record {{ owner = principal "{principal}"; subaccount = null }})')])
with concurrent.futures.ThreadPoolExecutor(max_workers=5) as pool: results=list(pool.map(fetch,jobs))
print('saved',len(results),'position reads')
