import fs from 'node:fs';
import {execFileSync} from 'node:child_process';
import crypto from 'node:crypto';
import {IDL} from '/Users/robertripley/coding/lookout-kanban/node_modules/@icp-sdk/core/lib/esm/candid/index.js';
import {idlFactory} from '/private/tmp/rumi-sec10-release-20261107/src/declarations/rumi_protocol_backend/rumi_protocol_backend.did.js';
const root='/Users/robertripley/coding/rumi-protocol-v2/.recovery-artifacts/vault-pull-release-20261108';
const service=idlFactory({IDL}); const types=service._fields.find(([n])=>n==='get_protocol_config')[1].retTypes;
function value(phase){
 const hex=execFileSync('didc',['encode','--defs','/private/tmp/vault-pull-release-20261108/live.did','--types','(ProtocolConfig)'],{input:fs.readFileSync(`${root}/${phase}/get_protocol_config.txt`),encoding:'utf8'}).trim();
 return IDL.decode(types,Buffer.from(hex,'hex'))[0];
}
function normalize(v){
 if(typeof v==='bigint')return v.toString(); if(v?.toText)return v.toText(); if(Array.isArray(v))return v.map(normalize); if(ArrayBuffer.isView(v))return Buffer.from(v).toString('hex');
 if(v&&typeof v==='object')return Object.fromEntries(Object.keys(v).sort().map(k=>[k,normalize(v[k])])); return v;
}
function fixed(v){delete v.recovery_mode_threshold; delete v.borrowing_fee_curve; for(const [,c] of v.collateral_configs){delete c.last_price;delete c.last_price_timestamp;} return normalize(v);}
const before=fixed(value(process.argv[2]||'before')); const after=fixed(value(process.argv[3]||'after'));
const sha=x=>crypto.createHash('sha256').update(JSON.stringify(x)).digest('hex'); const a=sha(before),b=sha(after);
const out={fixedConfigurationPreserved:a===b,excludedDynamicFields:['collateral last_price','collateral last_price_timestamp','derived recovery_mode_threshold','derived borrowing_fee_curve'],beforeSha256:a,afterSha256:b};
fs.writeFileSync('/private/tmp/vault-pull-release-20261108/configuration-comparison.json',JSON.stringify(out,null,2)); console.log(JSON.stringify(out));if(a!==b)process.exitCode=1;
