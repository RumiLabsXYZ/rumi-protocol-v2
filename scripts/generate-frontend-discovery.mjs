import assert from 'node:assert/strict';
import { mkdirSync, readFileSync, writeFileSync, rmSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { Principal } from '@dfinity/principal';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const output = resolve(process.argv[2] || resolve(root, 'src/vault_frontend/dist'));
const network = process.env.DFX_NETWORK || 'ic';
const known = resolve(output, '.well-known');
const names = ['ic-architecture', 'ii-derivation-origin'];
// This initial opt-in is for the approved mainnet app only. Never leak mainnet IDs into local/staging.
if (network !== 'ic') {
  for (const name of names) rmSync(resolve(known, name), { force: true });
  console.log(`Discovery disabled for unapproved network ${network}`);
  process.exit(0);
}
const ids = JSON.parse(readFileSync(resolve(root, 'canister_ids.json'), 'utf8'));
const roles = [
  ['vault_frontend', 'Rumi application frontend'],
  ['rumi_protocol_backend', 'Collateral vaults and protocol accounting'],
  ['rumi_stability_pool', 'Stability Pool deposits, gains and liquidation accounting'],
  ['rumi_analytics', 'Protocol analytics and historical metrics'],
  ['rumi_3pool', 'StableSwap pool and internal 3USD LP token ledger'],
  ['icusd_ledger', 'Rumi icUSD token ledger'],
  ['icusd_index', 'Rumi icUSD transaction index'],
];
const canisters = roles.map(([name, role]) => {
  const id = ids[name]?.[network];
  assert.equal(typeof id, 'string', `Missing ${network} canister ID for ${name}`);
  assert.equal(Principal.fromText(id).toText(), id, `Noncanonical principal for ${name}`);
  return { id, name, role };
});
assert.equal(new Set(canisters.map(c => c.id)).size, canisters.length);
const frontendId = canisters[0].id;
// Fail if this declaration drifts from the existing login mapping; never change identities here.
const config = readFileSync(resolve(root, 'src/vault_frontend/src/lib/config.ts'), 'utf8');
assert.ok(config.includes(`export const vault_frontend = "${frontendId}"`), 'Frontend auth ID drift');
const auth = readFileSync(resolve(root, 'src/vault_frontend/src/lib/services/auth.ts'), 'utf8');
assert.ok(auth.includes('`https://${vault_frontend}.icp0.io`'), 'Existing derivation configuration changed');
mkdirSync(known, { recursive: true });
writeFileSync(resolve(known, names[0]), JSON.stringify({ version: '1.0.0', canisters }, null, 2) + '\n');
writeFileSync(resolve(known, names[1]), `https://${frontendId}.icp0.io\n`);
console.log(`Generated two discovery assets for ${canisters.length} approved Rumi services (${network})`);
