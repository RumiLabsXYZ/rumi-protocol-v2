import assert from 'node:assert/strict';import{effectiveContentType}from'./effectiveContentType.mjs';
assert.equal(effectiveContentType({content_type:'application/octet-stream',headers:[['Content-Type','application/json']]}),'application/json');
assert.equal(effectiveContentType({content_type:'application/octet-stream',headers:[['cOnTeNt-TyPe','text/plain; charset=utf-8']]}),'text/plain; charset=utf-8');
assert.equal(effectiveContentType({content_type:'application/javascript',headers:null}),'application/javascript');
assert.throws(()=>effectiveContentType({content_type:'text/plain',headers:[['Content-Type','text/plain'],['content-type','application/json']]}),/Ambiguous/);
assert.notEqual(effectiveContentType({content_type:'text/plain',headers:null}),'application/json');
console.log('PASS: explicit override, case-insensitive name, base fallback, duplicate refusal, wrong MIME still differs');
