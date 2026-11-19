import fs from 'node:fs';import crypto from 'node:crypto';import path from 'node:path';import assert from 'node:assert/strict';import{fileURLToPath}from'node:url';
const dir=path.dirname(fileURLToPath(import.meta.url)),sha=b=>crypto.createHash('sha256').update(b).digest('hex');
const bytes=fs.readFileSync(path.join(dir,'verification-pins.json'));assert.equal(sha(bytes),process.env.VERIFICATION_PINS_SHA256,'Verification amendment pins mismatch');
const pins=JSON.parse(bytes);for(const[name,digest]of Object.entries(pins.filesSha256))assert.equal(sha(fs.readFileSync(path.join(dir,name))),digest,name);
assert.equal(process.env.PUBLISH_PINS_SHA256,pins.executionPinsSha256);assert.equal(sha(fs.readFileSync(path.join(dir,'../dispatch/execution-pins.json'))),pins.executionPinsSha256);
await import('./verify-dispatch.mjs');
