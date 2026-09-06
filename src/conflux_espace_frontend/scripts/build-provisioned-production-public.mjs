import { copyFile, readFile, rm, writeFile } from "node:fs/promises";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { join, resolve } from "node:path";
import {
  PUBLIC_MANIFEST_FILE,
  PUBLIC_MAPPING_FILE,
  PUBLIC_POLICY_OUTPUT,
  assertDedicatedBuildEnvironment,
  computeAssetInventory,
  createPublicManifest,
  provisioningFromMappingText,
  trackedCanisterIds,
} from "./production-public-recipe.mjs";

const frontendDir = fileURLToPath(new URL("../", import.meta.url));
const repoRoot = resolve(frontendDir, "../..");
const mappingPath = join(repoRoot, PUBLIC_MAPPING_FILE);
const outputDir = join(frontendDir, "dist-production-public");
const policySource = join(frontendDir, ".ic-assets.production-public.json");
const viteBin = join(frontendDir, "node_modules/vite/bin/vite.js");
const scanner = join(frontendDir, "scripts/assert-production-public-bundle.mjs");

function run(args, env = process.env) {
  return new Promise((resolvePromise, reject) => {
    const child = spawn(process.execPath, args, { cwd: frontendDir, env, stdio: "inherit" });
    child.on("error", reject);
    child.on("exit", (code, signal) => {
      if (code === 0) resolvePromise();
      else reject(new Error(`production-public build subprocess failed (${signal ?? code})`));
    });
  });
}

let mappingText;
// Never leave a stale directory looking deployable when the current mapping is
// absent or invalid.
await rm(outputDir, { recursive: true, force: true });
assertDedicatedBuildEnvironment(process.env.ICP_ENVIRONMENT);
try {
  mappingText = await readFile(mappingPath, "utf8");
} catch (error) {
  throw new Error(
    `No deployable production-public artifact can be built before the dedicated canister is provisioned. ` +
    `After explicit approval, create it in the exact environment so icp-cli writes ${PUBLIC_MAPPING_FILE}.`,
    { cause: error },
  );
}

const knownCanisterIds = await trackedCanisterIds(join(repoRoot, ".icp/data/mappings"));
const provisioned = provisioningFromMappingText(mappingText, "deployment", knownCanisterIds);
try {
  await run([viteBin, "build", "--outDir", outputDir, "--emptyOutDir"], {
    ...process.env,
    VITE_DEPLOYMENT_MODE: "production-public",
    VITE_PUBLIC_ORIGIN_CONTEXT: "deployment",
    VITE_PUBLIC_CANONICAL_ORIGIN: provisioned.canonicalOrigin,
  });
  await copyFile(policySource, join(outputDir, PUBLIC_POLICY_OUTPUT));
  const assetInventory = await computeAssetInventory(outputDir);
  const manifest = createPublicManifest({ ...provisioned, assetInventory, context: "deployment" });
  await writeFile(join(outputDir, PUBLIC_MANIFEST_FILE), `${JSON.stringify(manifest, null, 2)}\n`, "utf8");
  await rm(join(outputDir, ".DS_Store"), { force: true });
  await run([
    scanner,
    outputDir,
    "--context", "deployment",
    "--expected-canister", provisioned.canisterId,
    "--expected-origin", provisioned.canonicalOrigin,
  ]);
} catch (error) {
  await rm(outputDir, { recursive: true, force: true });
  throw error;
}
console.log(`deployable production-public assets built for ${provisioned.canonicalOrigin}`);
