// Existing release-board publisher preparation entry point. No network or mutation.
// Call with the freshly read board revision and an owned evidence directory.
import {readFileSync,writeFileSync,statSync,mkdirSync} from 'node:fs';
import {resolve} from 'node:path';
import {parseMarkdownBoard,validatePriorities,validateCompletionMetadata} from '/Users/robertripley/coding/lookout-kanban/scripts/publisher.mjs';
import {idlFactory} from '/Users/robertripley/coding/lookout-kanban/src/idl.js';
const root='/Users/robertripley/coding/rumi-protocol-v2';
if(!process.argv[2]||!/^\d+$/.test(process.argv[2]))throw Error('Pass the freshly read current board revision; never replay a stale call.');
const out=resolve(root,process.argv[3]??'output/release-operations-current');mkdirSync(out,{recursive:true});
const source=`${root}/docs/coordination/release-operations-board.md`;
const config={projectId:'rumi-release-operations',title:'Rumi · Release closeout & operations',lead:'Dorian',repository:'RumiLabsXYZ/rumi-protocol-v2',source:{kind:'markdown-section',path:source,sectionHeading:'## Release and operations queue'},columns:['Backlog','Ready','In progress','Review','Done','Blocked','Paused']};
const snapshot=parseMarkdownBoard(readFileSync(source,'utf8'),config,statSync(source,{bigint:true}).mtimeNs);
const raw=JSON.parse(readFileSync(`${root}/docs/coordination/release-operations-lookout-metadata.json`));
for(const key of ['summary','plainSummary'])if(typeof raw[key]!=='string'||[...raw[key]].length>600)throw Error('Invalid '+key);
const presentation={sourceRevision:snapshot.sourceRevision,summary:raw.summary,glossary:raw.glossary};
const plain={sourceRevision:snapshot.sourceRevision,summary:raw.plainSummary};
const priorities=validatePriorities(raw.priorities,snapshot);
const completions=validateCompletionMetadata(raw.completions,snapshot);
const methods={publish:[snapshot,BigInt(process.argv[2])],publishPresentation:[snapshot.projectId,presentation],publishPlainSummary:[snapshot.projectId,plain],publishPriorities:[snapshot.projectId,priorities],publishCompletionMetadata:[snapshot.projectId,snapshot.sourceRevision,completions.entries],publishProjectInfo:[snapshot.projectId,{repository:config.repository}]};
const calls=Object.fromEntries(Object.entries(methods).map(([method,args])=>{const t=idlFactory()._fields.find(([k])=>k===method)[1];return[method,'('+t.argTypes.map((x,i)=>x.valueToString(args[i])).join(',')+')'];}));
const stringify=x=>JSON.stringify(x,(_k,v)=>typeof v==='bigint'?v.toString():v,2)+'\n';
for(const [name,data]of Object.entries({snapshot,metadata:{presentation,plain,priorities,completions,repository:config.repository},'source-config':config,calls}))writeFileSync(`${out}/${name}.json`,stringify(data));
console.log(stringify({preparedOnly:true,project:snapshot.projectId,expectedRevision:process.argv[2],sourceRevision:snapshot.sourceRevision,cards:snapshot.cards.length,priorityItems:priorities.items.length,explicitCompletionDates:completions.entries.length,output:out}));
