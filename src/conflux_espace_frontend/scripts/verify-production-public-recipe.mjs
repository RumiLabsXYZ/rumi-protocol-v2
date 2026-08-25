import { access, readFile } from "node:fs/promises";
import { constants } from "node:fs";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { join, resolve } from "node:path";
import {
  PUBLIC_MAPPING_FILE,
  provisioningFromMappingText,
  trackedCanisterIds,
  verifyIcpRecipeText,
  verifyPackageScripts,
  verifyPublicAssetPolicy,
} from "./production-public-recipe.mjs";

const frontendDir = fileURLToPath(new URL("../", import.meta.url));
const repoRoot = resolve(frontendDir, "../..");
const mappingPath = join(repoRoot, PUBLIC_MAPPING_FILE);

verifyIcpRecipeText(await readFile(join(repoRoot, "icp.yaml"), "utf8"));
verifyPackageScripts(JSON.parse(await readFile(join(frontendDir, "package.json"), "utf8")));
verifyPublicAssetPolicy(JSON.parse(await readFile(join(frontendDir, ".ic-assets.production-public.json"), "utf8")));

try {
  await access(mappingPath, constants.F_OK);
  const mapped = provisioningFromMappingText(
    await readFile(mappingPath, "utf8"),
    "deployment",
    await trackedCanisterIds(join(repoRoot, ".icp/data/mappings")),
  );
  console.log(`validated provisioned canonical origin ${mapped.canonicalOrigin}`);
} catch (error) {
  if (error?.code !== "ENOENT") throw error;
  console.log(`no ${PUBLIC_MAPPING_FILE}: source-only pre-provisioning state correctly emits no deploy artifact`);
}

await new Promise((resolvePromise, reject) => {
  const child = spawn(process.execPath, [
    join(frontendDir, "scripts/verify-production-public-build.mjs"),
    "deployment",
  ], { cwd: frontendDir, stdio: "inherit" });
  child.on("error", reject);
  child.on("exit", (code, signal) => {
    if (code === 0) resolvePromise();
    else reject(new Error(`ephemeral deployment-shape verification failed (${signal ?? code})`));
  });
});

console.log("production-public ICP recipe and ephemeral deployment shape verified");
