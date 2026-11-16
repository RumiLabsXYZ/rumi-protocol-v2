import https from 'node:https';
import fs from 'node:fs';
import crypto from 'node:crypto';
import assert from 'node:assert/strict';
import {fileURLToPath} from 'node:url';
import path from 'node:path';
const dir=path.dirname(fileURLToPath(import.meta.url)),plan=JSON.parse(fs.readFileSync(path.join(dir,'publication-manifest.json'))),rows=[];
for(const item of plan.assets){
 const url='https://app.rumiprotocol.com'+item.key;
 const row=await new Promise((resolve,reject)=>{const req=https.get(url,{headers:{'Accept-Encoding':'identity','Cache-Control':'no-cache'}},res=>{const chunks=[];res.on('data',b=>chunks.push(b));res.on('error',reject);res.on('end',()=>{const bytes=Buffer.concat(chunks);resolve({url,status:res.statusCode,headers:res.headers,bytes:bytes.length,sha256:crypto.createHash('sha256').update(bytes).digest('hex'),body:bytes.toString('utf8')});});});req.setTimeout(30000,()=>req.destroy(Error('HTTP timeout')));req.on('error',reject);});
 assert.equal(row.status,200);assert.equal(row.sha256,item.sha256);assert.equal(row.headers['content-type'].split(';')[0],item.contentType.split(';')[0]);assert.ok(row.headers['ic-certificate']);rows.push(row);
}
const result={observedUtc:new Date().toISOString(),tlsVerificationEnabled:true,gatewayCertificateHeaderPresent:true,independentCertificateValidation:false,rows};fs.writeFileSync(path.join(dir,'http-readback.json'),JSON.stringify(result,null,2)+'\n');console.log(JSON.stringify(rows.map(({url,status,sha256,bytes,headers})=>({url,status,sha256,bytes,contentType:headers['content-type']}))));
