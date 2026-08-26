import { Principal } from "@dfinity/principal";
import { createHash } from "node:crypto";
import { readFile, readdir } from "node:fs/promises";
import { basename, join, relative, sep } from "node:path";

export const PUBLIC_CANISTER_NAME = "conflux_public_frontend";
export const PUBLIC_ENVIRONMENT = "conflux-production-public";
export const PUBLIC_MAPPING_FILE = `.icp/data/mappings/${PUBLIC_ENVIRONMENT}.ids.json`;
export const PUBLIC_OUTPUT_DIR = "src/conflux_espace_frontend/dist-production-public";
export const PUBLIC_POLICY_SOURCE = "src/conflux_espace_frontend/.ic-assets.production-public.json";
export const PUBLIC_POLICY_OUTPUT = ".ic-assets.json";
export const PUBLIC_MANIFEST_FILE = "rumi-conflux-production-public.json";
export const PUBLIC_RELEASE_FILE = ".icp/data/reviews/conflux-production-public.release.json";
export const PUBLIC_WASM_ARTIFACT = ".icp/cache/artifacts/conflux_public_frontend";
export const PUBLIC_RECIPE_TYPE = "@dfinity/asset-canister@v2.3.0";
export const PUBLIC_BUILD_COMMAND =
  "bash -c 'cd src/conflux_espace_frontend && npm ci && npm run build:production-public:deploy'";

export const PRODUCTION_BACKEND = "tfesu-vyaaa-aaaap-qrd7a-cai";
export const PRODUCTION_CHAIN_ID = 1030;
export const PRODUCTION_ICUSD = "0x8DdB0a13B26ed28912e4B8cCa99Bc3E8c66Df7Ff";
export const PRODUCTION_RPC = "https://evm.confluxrpc.com";
export const PRODUCTION_RECEIPT_CONFIRMATIONS = 400;
export const VERIFICATION_CANISTER = "rrkah-fqaaa-aaaaa-aaaaq-cai";

function fail(message) {
  throw new Error(`production-public recipe: ${message}`);
}

function exactObjectKeys(value, expected, label) {
  if (!value || typeof value !== "object" || Array.isArray(value)) fail(`${label} must be an object`);
  const actual = Object.keys(value).sort();
  const wanted = [...expected].sort();
  if (actual.length !== wanted.length || actual.some((key, i) => key !== wanted[i])) {
    fail(`${label} must contain exactly: ${wanted.join(", ")}`);
  }
}

export function canonicalOriginForCanister(principalText, context = "deployment") {
  let principal;
  try {
    principal = Principal.fromText(principalText);
  } catch {
    fail("mapping must contain a valid canonical Principal");
  }
  if (principal.toText() !== principalText) fail("mapping Principal must use canonical lowercase text");
  if (principal.compareTo(Principal.managementCanister()) === "eq" || principal.isAnonymous()) {
    fail("mapping Principal is reserved");
  }
  const bytes = principal.toUint8Array();
  if (bytes.length < 2 || bytes[bytes.length - 1] !== 1) {
    fail("mapping Principal must identify a non-reserved opaque canister");
  }
  if (context === "deployment-verification") {
    if (principalText !== VERIFICATION_CANISTER) fail("verification is locked to its denylisted test Principal");
  } else if (context === "deployment") {
    if (principalText === VERIFICATION_CANISTER) fail("the deterministic verification Principal cannot be deployed");
    if (principalText === PRODUCTION_BACKEND) fail("the public asset canister must not be the production backend");
  } else {
    fail(`unsupported origin context ${context}`);
  }
  return `https://${principalText}.icp0.io`;
}

export function assertDedicatedBuildEnvironment(value) {
  if (value !== PUBLIC_ENVIRONMENT) {
    fail(`ICP_ENVIRONMENT must be exactly ${PUBLIC_ENVIRONMENT}; implicit ic/default builds are refused`);
  }
  return value;
}

export function provisioningFromMappingText(text, context = "deployment", knownCanisterIds = []) {
  let mapping;
  try {
    mapping = JSON.parse(text);
  } catch {
    fail("canister-ID mapping is not valid JSON");
  }
  exactObjectKeys(mapping, [PUBLIC_CANISTER_NAME], "dedicated environment mapping");
  const canisterId = mapping[PUBLIC_CANISTER_NAME];
  if (typeof canisterId !== "string") fail("mapped canister ID must be text");
  if (knownCanisterIds.includes(canisterId)) fail("mapped canister ID is already assigned in another tracked environment");
  return {
    canisterId,
    canonicalOrigin: canonicalOriginForCanister(canisterId, context),
  };
}

export function createPublicManifest({ canisterId, canonicalOrigin, assetInventory, context = "deployment" }) {
  if (!Array.isArray(assetInventory) || assetInventory.length === 0) fail("artifact manifest requires a non-empty asset inventory");
  return {
    schema: "rumi-conflux-production-public/v2",
    context,
    assetCanister: canisterId,
    canonicalOrigin,
    backendCanister: PRODUCTION_BACKEND,
    chainId: PRODUCTION_CHAIN_ID,
    icusdContract: PRODUCTION_ICUSD,
    rpcOrigin: PRODUCTION_RPC,
    receiptConfirmations: PRODUCTION_RECEIPT_CONFIRMATIONS,
    assetPolicy: PUBLIC_POLICY_OUTPUT,
    certifiedGatewayOnly: true,
    allowRawAccess: false,
    spaAliasing: true,
    assets: assetInventory,
  };
}

export function verifyPublicManifest(manifest, expected) {
  exactObjectKeys(manifest, [
    "schema",
    "context",
    "assetCanister",
    "canonicalOrigin",
    "backendCanister",
    "chainId",
    "icusdContract",
    "rpcOrigin",
    "receiptConfirmations",
    "assetPolicy",
    "certifiedGatewayOnly",
    "allowRawAccess",
    "spaAliasing",
    "assets",
  ], "artifact manifest");
  verifyAssetInventory(manifest.assets);
  const wanted = createPublicManifest(expected);
  for (const [key, value] of Object.entries(wanted)) {
    if (JSON.stringify(manifest[key]) !== JSON.stringify(value)) fail(`artifact manifest field ${key} is wrong`);
  }
  return manifest;
}

export function sha256Bytes(value) {
  return createHash("sha256").update(value).digest("hex");
}

export async function sha256File(path) {
  return sha256Bytes(await readFile(path));
}

async function filesBelow(dir) {
  const entries = await readdir(dir, { withFileTypes: true });
  const nested = await Promise.all(entries.map((entry) => {
    const path = join(dir, entry.name);
    return entry.isDirectory() ? filesBelow(path) : [path];
  }));
  return nested.flat();
}

export async function computeAssetInventory(outputDir) {
  const paths = (await filesBelow(outputDir))
    .filter((path) => basename(path) !== PUBLIC_MANIFEST_FILE)
    .sort();
  return Promise.all(paths.map(async (path) => {
    const contents = await readFile(path);
    return {
      path: relative(outputDir, path).split(sep).join("/"),
      bytes: contents.byteLength,
      sha256: sha256Bytes(contents),
    };
  }));
}

export function verifyAssetInventory(inventory) {
  if (!Array.isArray(inventory) || inventory.length === 0) fail("asset inventory must be non-empty");
  let previous = "";
  for (const item of inventory) {
    exactObjectKeys(item, ["path", "bytes", "sha256"], "asset inventory entry");
    if (typeof item.path !== "string" || item.path.startsWith("/") || item.path.includes("..") || item.path === PUBLIC_MANIFEST_FILE) {
      fail("asset inventory path is invalid");
    }
    if (item.path <= previous) fail("asset inventory must be strictly path-sorted and unique");
    if (!Number.isSafeInteger(item.bytes) || item.bytes < 0) fail("asset inventory byte size is invalid");
    if (!/^[0-9a-f]{64}$/.test(item.sha256)) fail("asset inventory sha256 is invalid");
    previous = item.path;
  }
  return inventory;
}

export function assetInventoryDigest(inventory) {
  verifyAssetInventory(inventory);
  return sha256Bytes(JSON.stringify(inventory));
}

export function createPublicRelease({ provisioned, icpYamlSha256, mappingSha256, manifestSha256, assetInventorySha256, wasmSha256 }) {
  return {
    schema: "rumi-conflux-production-public-release/v1",
    environment: PUBLIC_ENVIRONMENT,
    canisterName: PUBLIC_CANISTER_NAME,
    assetCanister: provisioned.canisterId,
    canonicalOrigin: provisioned.canonicalOrigin,
    icpYamlSha256,
    mappingFile: PUBLIC_MAPPING_FILE,
    mappingSha256,
    manifestFile: `${PUBLIC_OUTPUT_DIR}/${PUBLIC_MANIFEST_FILE}`,
    manifestSha256,
    assetInventorySha256,
    wasmArtifact: PUBLIC_WASM_ARTIFACT,
    wasmSha256,
  };
}

export function verifyPublicRelease(release, expected) {
  exactObjectKeys(release, [
    "schema", "environment", "canisterName", "assetCanister", "canonicalOrigin", "icpYamlSha256",
    "mappingFile", "mappingSha256", "manifestFile", "manifestSha256",
    "assetInventorySha256", "wasmArtifact", "wasmSha256",
  ], "release seal");
  const wanted = createPublicRelease(expected);
  for (const [key, value] of Object.entries(wanted)) {
    if (release[key] !== value) fail(`release seal field ${key} is wrong`);
  }
  for (const key of ["icpYamlSha256", "mappingSha256", "manifestSha256", "assetInventorySha256", "wasmSha256"]) {
    if (!/^[0-9a-f]{64}$/.test(release[key])) fail(`release seal ${key} is not sha256`);
  }
  return release;
}

export async function trackedCanisterIds(mappingDirectory) {
  const result = [];
  const entries = await readdir(mappingDirectory, { withFileTypes: true }).catch(() => []);
  for (const entry of entries.sort((a, b) => a.name.localeCompare(b.name))) {
    if (!entry.isFile() || !entry.name.endsWith(".ids.json") || entry.name === `${PUBLIC_ENVIRONMENT}.ids.json`) continue;
    const mapping = JSON.parse(await readFile(join(mappingDirectory, entry.name), "utf8"));
    for (const value of Object.values(mapping)) if (typeof value === "string") result.push(value);
  }
  return [...new Set(result)].sort();
}

export function verifyPublicAssetPolicy(policy) {
  if (!Array.isArray(policy) || policy.length === 0) fail("asset policy must contain at least one rule");
  if (!policy.every((rule) => rule && rule.allow_raw_access === false)) {
    fail("every production-public asset rule must disable raw access");
  }
  if (!policy.some((rule) => rule.match === "**/*" && rule.enable_aliasing === true)) {
    fail("asset policy must enable SPA aliasing for the catch-all rule");
  }
  const serialized = JSON.stringify(policy);
  if (!serialized.includes(PRODUCTION_RPC)) fail("asset policy must permit the production Conflux RPC");
  if (serialized.includes("evmtestnet.confluxrpc.com")) fail("asset policy must not permit the testnet RPC");
  return policy;
}

function escapedRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

export function verifyPublicRuntimeText(runtimeText, expectedOrigin) {
  if (typeof runtimeText !== "string" || runtimeText.length === 0) fail("executable runtime text is empty");
  for (const [label, required] of [
    ["canonical origin", expectedOrigin],
    ["production backend", PRODUCTION_BACKEND],
    ["production IcUSD", PRODUCTION_ICUSD],
    ["production RPC", PRODUCTION_RPC],
    ["chain 1030", String(PRODUCTION_CHAIN_ID)],
  ]) {
    if (!runtimeText.includes(required)) fail(`executable assets are missing ${label}`);
  }
  if (!new RegExp(`receiptConfirmations:\\s*${PRODUCTION_RECEIPT_CONFIRMATIONS}\\b`).test(runtimeText)) {
    fail("executable assets are missing 400 receipt confirmations binding");
  }
  return true;
}

function namedYamlBlock(text, name, occurrence = 0) {
  const lines = text.split(/\r?\n/);
  const marker = `  - name: ${name}`;
  const starts = [];
  for (let i = 0; i < lines.length; i += 1) if (lines[i] === marker) starts.push(i);
  const start = starts[occurrence];
  if (start === undefined) fail(`icp.yaml is missing ${name}`);
  let end = lines.length;
  for (let i = start + 1; i < lines.length; i += 1) {
    if (lines[i].startsWith("  - name: ") || (lines[i].trim() !== "" && !lines[i].startsWith(" "))) {
      end = i;
      break;
    }
  }
  return lines.slice(start, end).join("\n");
}

function exactCanisterList(block) {
  const lines = block.split(/\r?\n/);
  const index = lines.findIndex((line) => line === "    canisters:");
  if (index < 0) fail("dedicated environment is missing its canister list");
  const result = [];
  for (let i = index + 1; i < lines.length; i += 1) {
    const match = /^      - (\S+)$/.exec(lines[i]);
    if (match) result.push(match[1]);
    else if (lines[i].trim() !== "" && !lines[i].startsWith("      #")) break;
  }
  return result;
}

export function verifyIcpRecipeText(text) {
  const recipe = namedYamlBlock(text, PUBLIC_CANISTER_NAME);
  const expectedRecipe = [
    `  - name: ${PUBLIC_CANISTER_NAME}`,
    "    recipe:",
    `      type: "${PUBLIC_RECIPE_TYPE}"`,
    "      configuration:",
    `        dir: ${PUBLIC_OUTPUT_DIR}`,
    "        build:",
    `          - ${PUBLIC_BUILD_COMMAND}`,
  ].join("\n");
  if (recipe.trimEnd() !== expectedRecipe) {
    fail("public canister recipe must exactly match the reviewed type, directory, and sole build command");
  }

  const environment = namedYamlBlock(text, PUBLIC_ENVIRONMENT);
  if (!environment.includes("    network: ic")) fail("dedicated public environment must target the IC network");
  const members = exactCanisterList(environment);
  if (members.length !== 1 || members[0] !== PUBLIC_CANISTER_NAME) {
    fail("dedicated public environment must contain only the public asset canister");
  }

  for (const environmentName of ["mainnet-live", "mainnet-staging", "local"]) {
    const block = namedYamlBlock(text, environmentName, environmentName === "local" ? 1 : 0);
    if (exactCanisterList(block).includes(PUBLIC_CANISTER_NAME)) {
      fail(`public asset canister must not be attached to ${environmentName}`);
    }
  }

  const staging = namedYamlBlock(text, "conflux_espace_frontend");
  for (const exact of [
    "dir: src/conflux_espace_frontend/dist",
    "npm ci && npm run build",
    "cp src/conflux_espace_frontend/.ic-assets.json src/conflux_espace_frontend/dist/.ic-assets.json",
  ]) {
    if (!staging.includes(exact)) fail(`staging/testnet recipe drifted: ${exact}`);
  }
  return true;
}

export function verifyPackageScripts(packageJson) {
  const scripts = packageJson?.scripts;
  if (!scripts || scripts["build:production-public:deploy"] !== "node scripts/build-provisioned-production-public.mjs") {
    fail("deploy build script must use the provisioned-ID wrapper");
  }
  if (scripts["check:production-public-bundle"] !== "node scripts/check-provisioned-production-public-bundle.mjs") {
    fail("bundle checker must derive expectations from the provisioned mapping");
  }
  if (scripts["seal:production-public-release"] !== "node scripts/seal-production-public-release.mjs" ||
      scripts["install:reviewed-production-public"] !== "node scripts/install-reviewed-production-public.mjs") {
    fail("review-seal and guarded install/sync scripts are missing");
  }
  if (scripts["verify:production-public:recipe"] !==
    "node scripts/test-production-public-recipe.mjs && node scripts/verify-production-public-recipe.mjs") {
    fail("deterministic recipe verifier script is missing");
  }
  return true;
}
