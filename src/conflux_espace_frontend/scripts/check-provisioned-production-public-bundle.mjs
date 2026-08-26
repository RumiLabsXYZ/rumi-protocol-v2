import { readFile } from "node:fs/promises";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { join, resolve } from "node:path";
import { PUBLIC_MAPPING_FILE, provisioningFromMappingText, trackedCanisterIds } from "./production-public-recipe.mjs";

const frontendDir = fileURLToPath(new URL("../", import.meta.url));
const repoRoot = resolve(frontendDir, "../..");
const outputDir = join(frontendDir, "dist-production-public");
const scanner = join(frontendDir, "scripts/assert-production-public-bundle.mjs");
const mapping = provisioningFromMappingText(
  await readFile(join(repoRoot, PUBLIC_MAPPING_FILE), "utf8"),
  "deployment",
  await trackedCanisterIds(join(repoRoot, ".icp/data/mappings")),
);

const child = spawn(process.execPath, [
  scanner,
  outputDir,
  "--context", "deployment",
  "--expected-canister", mapping.canisterId,
  "--expected-origin", mapping.canonicalOrigin,
], { cwd: frontendDir, stdio: "inherit" });
child.on("error", (error) => { throw error; });
child.on("exit", (code, signal) => {
  if (code !== 0) process.exitCode = code ?? (signal ? 1 : 0);
});
