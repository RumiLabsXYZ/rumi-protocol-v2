import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { resolve } from "node:path";
import { Principal } from "@dfinity/principal";
import {
  PRODUCTION_BACKEND,
  PRODUCTION_ICUSD,
  PUBLIC_CANISTER_NAME,
  PUBLIC_ENVIRONMENT,
  VERIFICATION_CANISTER,
  assertDedicatedBuildEnvironment,
  canonicalOriginForCanister,
  createPublicManifest,
  createPublicRelease,
  provisioningFromMappingText,
  trackedCanisterIds,
  verifyIcpRecipeText,
  verifyPublicAssetPolicy,
  verifyPublicManifest,
  verifyPublicRelease,
  verifyPublicRuntimeText,
} from "./production-public-recipe.mjs";

const frontendDir = fileURLToPath(new URL("../", import.meta.url));
const repoRoot = resolve(frontendDir, "../..");
const icpText = await readFile(resolve(repoRoot, "icp.yaml"), "utf8");
const policy = JSON.parse(await readFile(resolve(frontendDir, ".ic-assets.production-public.json"), "utf8"));

function rejects(fn, label) {
  assert.throws(fn, undefined, label);
}

assert.equal(assertDedicatedBuildEnvironment(PUBLIC_ENVIRONMENT), PUBLIC_ENVIRONMENT);
rejects(() => assertDedicatedBuildEnvironment(undefined), "missing explicit environment must fail");
rejects(() => assertDedicatedBuildEnvironment("ic"), "implicit IC environment must fail");

// The checked-in recipe is itself the positive fixture; exact mutations must
// fail so missing/wrong recipe, environment, directory, and staging isolation
// cannot quietly pass.
assert.equal(verifyIcpRecipeText(icpText), true);
rejects(
  () => verifyIcpRecipeText(icpText.replace(
    "dir: src/conflux_espace_frontend/dist-production-public",
    "dir: src/conflux_espace_frontend/dist",
  )),
  "wrong asset directory must fail",
);
rejects(
  () => verifyIcpRecipeText(icpText.replace(
    `  - name: ${PUBLIC_CANISTER_NAME}\n    recipe:\n      type: "@dfinity/asset-canister@v2.3.0"`,
    `  - name: ${PUBLIC_CANISTER_NAME}\n    recipe:\n      type: "@dfinity/asset-canister@v0.0.0"`,
  )),
  "wrong recipe pin must fail",
);
rejects(
  () => verifyIcpRecipeText(icpText.replace(`  - name: ${PUBLIC_ENVIRONMENT}`, `  - name: wrong-${PUBLIC_ENVIRONMENT}`)),
  "missing dedicated environment must fail",
);
rejects(
  () => verifyIcpRecipeText(icpText.replace(
    "      - conflux_espace_frontend\n    init_args:",
    `      - conflux_espace_frontend\n      - ${PUBLIC_CANISTER_NAME}\n    init_args:`,
  )),
  "public canister attached to staging must fail",
);
rejects(
  () => verifyIcpRecipeText(icpText.replace(
    "npm run build:production-public:deploy'",
    "npm run build:production-public:deploy'\n          - echo UNREVIEWED_EXTRA_COMMAND",
  )),
  "extra public build command must fail",
);

assert.equal(verifyPublicAssetPolicy(policy), policy);
rejects(
  () => verifyPublicAssetPolicy(policy.map((rule) => ({ ...rule, allow_raw_access: true }))),
  "raw access must fail",
);
rejects(
  () => verifyPublicAssetPolicy(policy.map((rule) => ({ ...rule, enable_aliasing: false }))),
  "missing SPA aliasing must fail",
);

const deploymentCanister = Principal.fromUint8Array(Uint8Array.from([1, 2, 3, 4, 1])).toText();
const deploymentOrigin = canonicalOriginForCanister(deploymentCanister, "deployment");
const mappingText = JSON.stringify({ [PUBLIC_CANISTER_NAME]: deploymentCanister });
assert.deepEqual(provisioningFromMappingText(mappingText), {
  canisterId: deploymentCanister,
  canonicalOrigin: deploymentOrigin,
});
const trackedIds = await trackedCanisterIds(resolve(repoRoot, ".icp/data/mappings"));
for (const knownId of trackedIds) {
  rejects(
    () => provisioningFromMappingText(JSON.stringify({ [PUBLIC_CANISTER_NAME]: knownId }), "deployment", trackedIds),
    "an already tracked production/staging canister must fail",
  );
}
for (const badMapping of [
  "{}",
  JSON.stringify({ wrong_name: deploymentCanister }),
  JSON.stringify({ [PUBLIC_CANISTER_NAME]: deploymentCanister, extra: deploymentCanister }),
  JSON.stringify({ [PUBLIC_CANISTER_NAME]: "aaaaa-aa" }),
  JSON.stringify({ [PUBLIC_CANISTER_NAME]: VERIFICATION_CANISTER }),
]) {
  rejects(() => provisioningFromMappingText(badMapping), "missing/wrong/reserved mapping must fail");
}

const assetInventory = [{ path: "index.html", bytes: 17, sha256: "a".repeat(64) }];
const expected = { canisterId: deploymentCanister, canonicalOrigin: deploymentOrigin, assetInventory, context: "deployment" };
const manifest = createPublicManifest(expected);
assert.equal(verifyPublicManifest(manifest, expected), manifest);
for (const mutation of [
  { canonicalOrigin: "https://wrong-principal.icp0.io" },
  { backendCanister: "kvg63-wiaaa-aaaao-bbabq-cai" },
  { icusdContract: "0xBD02222D388BC43095A4758C3e977d5dF8f68f7a" },
  { allowRawAccess: true },
  { certifiedGatewayOnly: false },
  { spaAliasing: false },
  { assets: [{ ...assetInventory[0], sha256: "b".repeat(64) }] },
]) {
  rejects(() => verifyPublicManifest({ ...manifest, ...mutation }, expected), "wrong manifest field must fail");
}
const { canonicalOrigin: _missing, ...missingOrigin } = manifest;
rejects(() => verifyPublicManifest(missingOrigin, expected), "missing canonical origin must fail");
assert.equal(manifest.backendCanister, PRODUCTION_BACKEND);
assert.equal(manifest.icusdContract, PRODUCTION_ICUSD);

const runtimeText = [
  `origin="${deploymentOrigin}"`,
  `backendCanisterId:"${PRODUCTION_BACKEND}"`,
  `icusdContract:"${PRODUCTION_ICUSD}"`,
  "rpcUrl:\"https://evm.confluxrpc.com\"",
  "chainId:1030",
  "receiptConfirmations:400",
].join(";");
assert.equal(verifyPublicRuntimeText(runtimeText, deploymentOrigin), true);
for (const [from, to] of [
  [deploymentOrigin, "https://wrong-principal.icp0.io"],
  [PRODUCTION_BACKEND, "aaaaa-aa"],
  [PRODUCTION_ICUSD, "0x1111111111111111111111111111111111111111"],
  ["https://evm.confluxrpc.com", "https://wrong-rpc.invalid"],
  ["chainId:1030", "chainId:999"],
  ["receiptConfirmations:400", "receiptConfirmations:1"],
]) {
  rejects(() => verifyPublicRuntimeText(runtimeText.replace(from, to), deploymentOrigin), `wrong runtime pin ${from} must fail`);
}

const releaseExpected = {
  provisioned: { canisterId: deploymentCanister, canonicalOrigin: deploymentOrigin },
  icpYamlSha256: "0".repeat(64),
  mappingSha256: "1".repeat(64),
  manifestSha256: "2".repeat(64),
  assetInventorySha256: "3".repeat(64),
  wasmSha256: "4".repeat(64),
};
const release = createPublicRelease(releaseExpected);
assert.equal(verifyPublicRelease(release, releaseExpected), release);
for (const key of ["assetCanister", "icpYamlSha256", "mappingSha256", "manifestSha256", "assetInventorySha256", "wasmSha256"]) {
  rejects(() => verifyPublicRelease({ ...release, [key]: key.endsWith("Sha256") ? "f".repeat(64) : VERIFICATION_CANISTER }, releaseExpected),
    `wrong release seal ${key} must fail`);
}

console.log("production-public recipe negative tests passed");
