import { mkdir, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawn } from "node:child_process";
import {
  PUBLIC_RELEASE_FILE,
  assertDedicatedBuildEnvironment,
  sha256Bytes,
} from "./production-public-recipe.mjs";
import { currentReleaseFacts } from "./production-public-release.mjs";

const frontendDir = fileURLToPath(new URL("../", import.meta.url));
const repoRoot = resolve(frontendDir, "../..");
assertDedicatedBuildEnvironment(process.env.ICP_ENVIRONMENT);
await new Promise((resolvePromise, reject) => {
  const child = spawn(process.execPath, ["scripts/check-provisioned-production-public-bundle.mjs"], {
    cwd: frontendDir,
    env: process.env,
    stdio: "inherit",
  });
  child.on("error", reject);
  child.on("exit", (code, signal) => code === 0 ? resolvePromise() : reject(new Error(`bundle verification failed (${signal ?? code})`)));
});
const { release } = await currentReleaseFacts(repoRoot);
const releaseText = `${JSON.stringify(release, null, 2)}\n`;
const releasePath = resolve(repoRoot, PUBLIC_RELEASE_FILE);
await mkdir(dirname(releasePath), { recursive: true });
await writeFile(releasePath, releaseText, { encoding: "utf8", flag: "wx" });
console.log(`sealed production-public release ${sha256Bytes(releaseText)} at ${PUBLIC_RELEASE_FILE}`);
console.log("Review and commit the seal; its sha256 is the required explicit deployment approval token.");
