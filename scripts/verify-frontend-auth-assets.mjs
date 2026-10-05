import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { resolve } from 'node:path';

const frontend = fileURLToPath(new URL('../src/vault_frontend/', import.meta.url));
const output = process.argv[2] ? resolve(process.argv[2]) : resolve(frontend, 'dist');
const read = (base, name) => readFileSync(resolve(base, '.well-known', name), 'utf8');
const source = resolve(frontend, 'static');

// These are required production assets, not optional application metadata.
// A sync removes remote assets absent from the build directory.
for (const name of ['ii-alternative-origins', 'ic-domains']) {
  assert.equal(read(output, name), read(source, name), `Missing or changed ${name} in ${output}`);
}
const { alternativeOrigins } = JSON.parse(read(output, 'ii-alternative-origins'));
assert.ok(Array.isArray(alternativeOrigins) && alternativeOrigins.length <= 100);
const domains = read(output, 'ic-domains').trim().split(/\r?\n/);
assert.ok(domains.includes('app.rumiprotocol.com'), 'Missing production custom domain');
for (const domain of domains) {
  assert.ok(alternativeOrigins.includes(`https://${domain}`), `II does not authorize ${domain}`);
}
const rules = JSON.parse(readFileSync(resolve(output, '.ic-assets.json'), 'utf8'));
assert.ok(rules.some(rule => rule.match === '.well-known' && rule.ignore === false),
  '.well-known must be included in asset uploads');
assert.ok(rules.some(rule => rule.match === '.well-known/ii-alternative-origins'
  && rule.ignore === false && rule.headers?.['Content-Type'] === 'application/json'
  && rule.headers?.['Access-Control-Allow-Origin'] === '*'), 'Missing II JSON/CORS upload headers');
console.log('Verified Internet Identity and custom-domain assets in frontend build.');
