import fs from 'node:fs';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {spawnSync} from 'node:child_process';
import crypto from 'node:crypto';
import assert from 'node:assert/strict';
import {IDL} from '@dfinity/candid';
import {idlFactory} from './frontend-live-idl.mjs';
const dir=path.dirname(fileURLToPath(import.meta.url));
const journal=path.join(dir,'dispatch');
const sha=b=>crypto.createHash('sha256').update(b).digest('hex');
const read=n=>JSON.parse(fs.readFileSync(path.join(dir,n),'utf8'));
const canonical=x=>JSON.stringify(x,(_k,v)=>v&&typeof v==='object'&&!Array.isArray(v)?Object.fromEntries(Object.entries(v).sort(([a],[b])=>a.localeCompare(b))):v);
const save=(name,value)=>fs.writeFileSync(path.join(journal,name),JSON.stringify(value,(_k,v)=>typeof v==='bigint'?v.toString():v,2)+'\n',{mode:0o600});
const plan=read('publication-manifest.json'),target=plan.target;
assert.equal(sha(fs.readFileSync(path.join(dir,'publication-manifest.json'))),process.env.DISCOVERY_MANIFEST_SHA256,'Frozen manifest mismatch');
assert.equal(target,'tcfua-yaaaa-aaaap-qrd7q-cai');assert.equal(plan.assets.length,2);
assert.deepEqual(plan.assets.map(x=>x.key),['/.well-known/ic-architecture','/.well-known/ii-derivation-origin']);
const before=read('inventory-before.json'),pre=read('inventory-precommit.json');
assert.deepEqual(pre.assets,before.assets,'Live asset inventory drift');
assert.ok(Date.now()-Date.parse(pre.finished)<60000,'Stale inventory');
for(const item of plan.assets){assert.ok(!before.assets.some(x=>x.key===item.key),'Key already exists; reconcile');const b=fs.readFileSync(path.join(dir,item.path));assert.equal(sha(b),item.sha256);assert.equal(b.length,item.bytes);}
for(const name of ['frontend-status','backend-status']){
 const old=read(name+'-before.stdout'),now=read(name+'-precommit.stdout');
 assert.equal(now.status,'Running');assert.equal(now.module_hash,old.module_hash);assert.equal(canonical(now.settings),canonical(old.settings),'Settings drift');
}
for(const role of ['Commit','Prepare'])assert.equal(read('permissions-'+role+'-before.stdout').response_bytes,read('permissions-'+role+'-precommit.stdout').response_bytes,'Permission drift');
for(const phase of ['before','precommit'])assert.equal(read('sp-hold-'+phase+'.stdout').response_candid,'(true)','Stability Pool hold changed');
const who=spawnSync('icp',['identity','principal','--identity','rumi_identity'],{encoding:'utf8',timeout:30000});assert.equal(who.status,0);assert.equal(who.stdout.trim(),process.env.DISCOVERY_IDENTITY_PRINCIPAL,'Identity mismatch');
assert.ok(!fs.existsSync(journal),'Prior dispatch exists; reconcile, never replay');fs.mkdirSync(journal,{mode:0o700});
const types=new Map(idlFactory({IDL})._fields);
function call(label,method,args){
 assert.ok(plan.allowedMutations.includes(method));const typ=types.get(method),bytes=Buffer.from(IDL.encode(typ.argTypes,args));assert.ok(bytes.length<1800000);
 const file=path.join(journal,label+'.args.hex');fs.writeFileSync(file,bytes.toString('hex'),{flag:'wx',mode:0o600});
 const record={method,argsSha256:sha(bytes),dispatchedUtc:new Date().toISOString(),state:'dispatched-outcome-unknown'};
 const marker=path.join(journal,label+'.json'),fd=fs.openSync(marker,'wx',0o600);fs.writeSync(fd,JSON.stringify(record));fs.fsyncSync(fd);fs.closeSync(fd);
 const dfd=fs.openSync(journal,'r');fs.fsyncSync(dfd);fs.closeSync(dfd);
 const r=spawnSync('icp',['canister','call',target,method,'--args-file',file,'--args-format','hex','--candid',path.join(dir,'frontend-live.did'),'--network','ic','--identity','rumi_identity','--json'],{encoding:'utf8',timeout:180000,maxBuffer:4000000});
 fs.writeFileSync(path.join(journal,label+'.stdout'),r.stdout||'');fs.writeFileSync(path.join(journal,label+'.stderr'),r.stderr||'');record.exit=r.status;record.finishedUtc=new Date().toISOString();
 if(r.status!==0){record.state='uncertain-reconcile-before-retry';save(label+'.json',record);throw Error(label+' failed, STOP/readback: '+r.stderr);}
 const j=JSON.parse(r.stdout);const result=IDL.decode(typ.retTypes,Uint8Array.from(Buffer.from(j.response_bytes,'hex')).buffer)[0];record.state='cli-success-readback-pending';record.result=result;save(label+'.json',record);return result;
}
const batch=call('create','create_batch',[{}]).batch_id;
const chunks=call('chunks','create_chunks',[{batch_id:batch,content:plan.assets.map(x=>fs.readFileSync(path.join(dir,x.path)))}]).chunk_ids;
assert.equal(chunks.length,2);assert.notEqual(String(chunks[0]),String(chunks[1]));
const template=before.assets.find(x=>x.key==='/.well-known/ii-alternative-origins');assert.ok(template);
const operations=plan.assets.flatMap((item,i)=>{
 const headers=(template.headers||[]).filter(([k])=>!['content-type','set-cookie'].includes(k.toLowerCase()));headers.push(['Content-Type',item.contentType]);
 return [{CreateAsset:{key:item.key,content_type:item.contentType,max_age:[],headers:[headers],enable_aliasing:[false],allow_raw_access:[false]}},{SetAssetContent:{key:item.key,content_encoding:'identity',chunk_ids:[chunks[i]],last_chunk:[],sha256:[Uint8Array.from(Buffer.from(item.sha256,'hex'))]}}];
});
save('operations.json',{batch_id:batch,operations});
// No delete, unset, clear, existing-key set, property update, permission or module operation is available.
call('commit','commit_batch',[{batch_id:batch,operations}]);
save('completed.json',{batchId:batch,finished:new Date().toISOString(),authoritativeReadbackPending:true});
console.log('Two-key commit returned; verify full inventory and connector before claiming success.');
