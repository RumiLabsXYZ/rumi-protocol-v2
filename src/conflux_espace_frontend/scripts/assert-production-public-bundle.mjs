import { readdir, readFile } from "node:fs/promises";
import { basename, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  PRODUCTION_BACKEND,
  PRODUCTION_CHAIN_ID,
  PRODUCTION_ICUSD,
  PRODUCTION_RECEIPT_CONFIRMATIONS,
  PRODUCTION_RPC,
  PUBLIC_MANIFEST_FILE,
  PUBLIC_POLICY_OUTPUT,
  computeAssetInventory,
  verifyPublicAssetPolicy,
  verifyPublicManifest,
  verifyPublicRuntimeText,
} from "./production-public-recipe.mjs";

const defaultOutputDir = fileURLToPath(new URL("../dist-production-public/", import.meta.url));
const outputDir = resolve(process.argv[2] ?? defaultOutputDir);
const options = new Map();
for (let i = 3; i < process.argv.length; i += 2) options.set(process.argv[i], process.argv[i + 1]);
const context = options.get("--context");
const expectedCanister = options.get("--expected-canister");
const expectedOrigin = options.get("--expected-origin");
if (!context || !expectedOrigin) throw new Error("bundle scanner requires an exact context and expected origin");
const bundleOnly = context === "local-verification";
if (!bundleOnly && !expectedCanister) throw new Error("deployment bundle scanner requires an exact expected canister");

// Product labels and test-signer symbols are exact, case-sensitive sentinels.
// Generic lowercase cryptographic validation messages from dependencies are
// not the local test signer implementation and are intentionally not matched.
const forbidden = [
  /Private key/,
  /Use a dev key/,
  /scalar=1/,
  /Bad key/,
  /demo key/,
  /Dev key/,
  /devkey/,
  /connectDevKey/,
  /privateKeyToAccount/,
  /Development signer/,
  /0x0{63}1/i,
  /production-canary/,
  /productionCanary/,
  /canaryPhase/,
  /isCanaryVault/,
  /guidedPhase/,
  /isGuidedVault/,
  /guidedLifecycle/,
  /rumi:conflux-canary/i,
  /Canary state/i,
  /persisted canary/i,
  /canary lifecycle/i,
  /eSpace testnet/i,
  /chain 71/i,
  /badge\.testnet/i,
  /canary-steps/i,
  /evmtestnet\.confluxrpc\.com/i,
  /evmtestnet\.confluxscan\.org/i,
  /kvg63-wiaaa-aaaao-bbabq-cai/i,
  /0xBD02222D388BC43095A4758C3e977d5dF8f68f7a/i,
];

async function filesBelow(dir) {
  const entries = await readdir(dir, { withFileTypes: true });
  const nested = await Promise.all(entries.map((entry) => {
    const path = join(dir, entry.name);
    return entry.isDirectory() ? filesBelow(path) : [path];
  }));
  return nested.flat();
}

const files = await filesBelow(outputDir);
if (!files.length) throw new Error("production-public bundle directory is empty");
const fileNames = new Set(files.map((path) => basename(path)));
if (fileNames.has(".ic-assets.production-public.json")) {
  throw new Error("source asset policy must be installed only under the canonical .ic-assets.json name");
}

let runtimeText = "";
for (const path of files) {
  const contents = await readFile(path, "utf8").catch(() => "");
  if (/\.(?:html|js)$/.test(path)) runtimeText += `\n${contents}`;
  for (const pattern of forbidden) {
    if (pattern.test(contents)) {
      throw new Error(`production-public bundle contains forbidden dev/canary/testnet material (${pattern}) in ${path}`);
    }
  }
}

verifyPublicRuntimeText(runtimeText, expectedOrigin);
if (context === "deployment" && /rrkah-fqaaa-aaaaa-aaaaq-cai/.test(runtimeText)) {
  throw new Error("deployable bundle contains the deterministic verification Principal");
}

if (bundleOnly) {
  if (fileNames.has(PUBLIC_MANIFEST_FILE) || fileNames.has(PUBLIC_POLICY_OUTPUT)) {
    throw new Error("local verification must not emit deployment manifest or asset policy files");
  }
} else {
  const manifest = JSON.parse(await readFile(join(outputDir, PUBLIC_MANIFEST_FILE), "utf8"));
  const assetInventory = await computeAssetInventory(outputDir);
  verifyPublicManifest(manifest, { canisterId: expectedCanister, canonicalOrigin: expectedOrigin, assetInventory, context });
  const policy = JSON.parse(await readFile(join(outputDir, PUBLIC_POLICY_OUTPUT), "utf8"));
  verifyPublicAssetPolicy(policy);
}

console.log(`production-public ${context} bundle policy passed (${files.length} files scanned)`);
