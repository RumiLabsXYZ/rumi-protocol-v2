import fs from 'node:fs';
import {spawnSync} from 'node:child_process';
import {fileURLToPath} from 'node:url';
import {dirname,resolve} from 'node:path';
const dir=dirname(fileURLToPath(import.meta.url));
const phase=process.argv[2];if(!/^(before|precommit|after)$/.test(phase))throw Error('bad phase');
function read(name,args){const r=spawnSync('icp',args,{encoding:'utf8',timeout:90000,maxBuffer:4000000});fs.writeFileSync(resolve(dir,`${name}-${phase}.stdout`),r.stdout||'');fs.writeFileSync(resolve(dir,`${name}-${phase}.stderr`),r.stderr||'');if(r.status!==0)throw Error(name+' failed: '+r.stderr);JSON.parse(r.stdout);}
for(const [name,id] of [['frontend-status','tcfua-yaaaa-aaaap-qrd7q-cai'],['backend-status','tfesu-vyaaa-aaaap-qrd7a-cai']])read(name,['canister','status',id,'-e','mainnet-live','--identity','rumi_identity','--json']);
for(const role of ['Commit','Prepare'])read('permissions-'+role,['canister','call','tcfua-yaaaa-aaaap-qrd7q-cai','list_permitted',`(record { permission = variant { ${role} } })`,'--network','ic','--identity','rumi_identity','--candid',resolve(dir,'frontend-live.did'),'--json']);
read('sp-hold',['canister','call','tfesu-vyaaa-aaaap-qrd7a-cai','get_sp_writedown_disabled','()','--network','ic','--query','--json']);
