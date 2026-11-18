import fs from 'node:fs';import path from 'node:path';import crypto from 'node:crypto';import zlib from 'node:zlib';import assert from 'node:assert/strict';import {fileURLToPath}from'node:url';import{IDL}from'@dfinity/candid';import{idlFactory}from'./frontend-live-idl.mjs';
const dir=path.dirname(fileURLToPath(import.meta.url)),artifact=process.argv[2];assert.ok(artifact,'Pass the frozen artifact directory');
const sha=b=>crypto.createHash('sha256').update(b).digest('hex'),read=n=>JSON.parse(fs.readFileSync(path.join(dir,n))),canonical=x=>JSON.stringify(x,(_k,v)=>v&&typeof v==='object'&&!Array.isArray(v)?Object.fromEntries(Object.entries(v).sort(([a],[b])=>a.localeCompare(b))):v);
const before=read('inventory-before.json'),manifest=read('artifact-manifest.json'),types=new Map(idlFactory({IDL})._fields);assert.equal(before.assets.length,761);assert.equal(manifest.files.length,764);
const files=new Map(manifest.files.map(f=>[f.path,f])),assets=new Map(before.assets.map(a=>[a.key,a])),ignored=new Set(['.ic-assets.json','.ic-assets.json.br','.ic-assets.json.gz']);
for(const f of files.values()){const bytes=fs.readFileSync(path.join(artifact,f.path));assert.equal(sha(bytes),f.sha256);assert.equal(bytes.length,f.bytes)}
const props=a=>Object.fromEntries(Object.entries(a).filter(([k])=>!['key','encodings'].includes(k)));
const templates=new Map;for(const a of before.assets.filter(a=>a.key.startsWith('/_app/immutable/'))){let ext=path.extname(a.key),table=templates.get(ext)||new Map;table.set(canonical(props(a)),props(a));templates.set(ext,table)}
for(const [ext,table]of templates)assert.equal(table.size,1,'Ambiguous existing property template '+ext);
const allowed=new Set(['/index.html','/index.html.br','/index.html.gz','/_app/version.json','/_app/version.json.br','/_app/version.json.gz']);
let creates=[],sets=[],unchanged=[],retained=[];
function add(key,encoding,sourcePath,phase){const f=files.get(sourcePath);assert.ok(f,sourcePath);sets.push({key,encoding,sourcePath,phase,bytes:f.bytes,sha256:f.sha256});}
for(const f of files.values()){
 if(ignored.has(f.path))continue;const key='/'+f.path,old=assets.get(key);
 if(!old){assert.ok(key.startsWith('/_app/immutable/'),'Unexpected nonimmutable creation '+key);const table=templates.get(path.extname(key));assert.equal(table?.size,1);creates.push({key,properties:[...table.values()][0]});add(key,'identity',f.path,'stage');
  const gz=files.get(f.path+'.gz');if(gz&&gz.bytes<f.bytes){assert.deepEqual(zlib.gunzipSync(fs.readFileSync(path.join(artifact,gz.path))),fs.readFileSync(path.join(artifact,f.path)));add(key,'gzip',gz.path,'stage')}
 }else{
  const identity=old.encodings.find(e=>e.content_encoding==='identity');assert.ok(identity,'Missing baseline identity '+key);
  if(identity.sha256===f.sha256){unchanged.push(key);continue}
  assert.ok(allowed.has(key),'Unexpected existing-key change '+key);
  for(const e of old.encodings){assert.ok(['identity','gzip'].includes(e.content_encoding),'Unsupported existing encoding');const source=e.content_encoding==='identity'?f.path:f.path+'.gz';if(e.content_encoding==='gzip')assert.deepEqual(zlib.gunzipSync(fs.readFileSync(path.join(artifact,source))),fs.readFileSync(path.join(artifact,f.path)));add(key,e.content_encoding,source,'activate')}
 }
}
for(const key of assets.keys())if(!files.has(key.slice(1)))retained.push(key);
creates.sort((a,b)=>a.key.localeCompare(b.key));sets.sort((a,b)=>(a.phase+'|'+a.key+'|'+a.encoding).localeCompare(b.phase+'|'+b.key+'|'+b.encoding));
assert.equal(creates.length,327);assert.equal(unchanged.length,428);assert.equal(retained.length,327);assert.equal(new Set(sets.filter(s=>s.phase==='activate').map(s=>s.key)).size,6);assert.equal(sets.filter(s=>s.phase==='activate').length,7);
for(const key of ['/.well-known/ic-architecture','/.well-known/ii-derivation-origin','/.well-known/ii-alternative-origins','/.well-known/ic-domains'])assert.ok(unchanged.includes(key));
let phases={};for(const phase of ['stage','activate']){
 const rows=sets.filter(s=>s.phase===phase),groups=[];
 for(const s of rows){let group=groups.find(g=>g.bytes+s.bytes<=1400000&&!g.hashes.has(s.sha256));if(!group){group={bytes:0,hashes:new Set,rows:[]};groups.push(group)}group.bytes+=s.bytes;group.hashes.add(s.sha256);group.rows.push(s)}
 phases[phase]={creates:phase==='stage'?creates:[],sets:rows,chunkGroups:groups.map((g,i)=>({group:i,rawBytes:g.bytes,sets:g.rows.map(s=>({key:s.key,encoding:s.encoding,sourcePath:s.sourcePath,bytes:s.bytes,sha256:s.sha256})),encodedArgumentBytesAtBatch0:IDL.encode(types.get('create_chunks').argTypes,[{batch_id:0n,content:g.rows.map(s=>fs.readFileSync(path.join(artifact,s.sourcePath)))}]).byteLength}))};
 for(const g of phases[phase].chunkGroups)assert.ok(g.encodedArgumentBytesAtBatch0<1800000);
}
const plan={schema:'rumi-bounded-recovery-asset-dispatch-v1',executionOwner:'Dorian',executionHeld:true,target:'tcfua-yaaaa-aaaap-qrd7q-cai',backend:'tfesu-vyaaa-aaaap-qrd7a-cai',network:'ic',environment:'mainnet-live',identity:'rumi_identity',identityPrincipal:'fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae',canonicalMerge:manifest.canonicalMerge,buildSource:manifest.source,manifestSha256:sha(fs.readFileSync(path.join(dir,'artifact-manifest.json'))),baselineInventorySha256:sha(fs.readFileSync(path.join(dir,'inventory-before.json'))),baselineStateSha256:sha(fs.readFileSync(path.join(dir,'state-before.json'))),frontendModule:'04e565b3425fe7510ee16b02adcfe3f01abc9a2725c82a21cb08969241debd62',backendModule:'59a64f78e0e652d0e48413aba587d3f826c1dc25258f56ab09575c59de444111',allowedMutations:['create_batch','create_chunks','commit_batch'],allowedOperations:['CreateAsset','SetAssetContent'],activationKeys:[...allowed].sort(),ignoredConfigurationFiles:[...ignored],protectedKeys:['/.well-known/ic-architecture','/.well-known/ii-derivation-origin','/.well-known/ii-alternative-origins','/.well-known/ic-domains'],retainedOldKeysAbsentFromBuild:retained.sort(),unchangedBuildKeys:unchanged.sort(),expectedFinalAssetKeys:1088,phases,summary:{createCount:creates.length,setEncodingCount:sets.length,newEncodingSets:phases.stage.sets.length,changedExistingEncodingSets:phases.activate.sets.length,uploadBytes:sets.reduce((a,s)=>a+s.bytes,0),chunkCalls:phases.stage.chunkGroups.length+phases.activate.chunkGroups.length,deletes:0,unsets:0,propertyChanges:0}};
fs.writeFileSync(path.join(dir,'dispatch-manifest.json'),JSON.stringify(plan,null,2)+'\n');console.log(JSON.stringify({summary:plan.summary,groups:Object.fromEntries(Object.entries(phases).map(([n,p])=>[n,p.chunkGroups.map(g=>({rawBytes:g.rawBytes,encodedBytes:g.encodedArgumentBytesAtBatch0,sets:g.sets.length}))])),manifestSha256:sha(fs.readFileSync(path.join(dir,'dispatch-manifest.json')))},null,2));
