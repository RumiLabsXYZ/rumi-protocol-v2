import { readFile } from "node:fs/promises";
import { spawn } from "node:child_process";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  PUBLIC_CANISTER_NAME,
  PUBLIC_ENVIRONMENT,
  PUBLIC_RELEASE_FILE,
  PUBLIC_WASM_ARTIFACT,
  assertDedicatedBuildEnvironment,
  sha256Bytes,
} from "./production-public-recipe.mjs";
import { verifyReleaseSeal } from "./production-public-release.mjs";

const args = new Map();
for (let i = 2; i < process.argv.length; i += 2) args.set(process.argv[i], process.argv[i + 1]);
const approvedDigest = args.get("--approved-release-sha256");
const mode = args.get("--mode");
if (!/^[0-9a-f]{64}$/.test(approvedDigest ?? "")) throw new Error("exact --approved-release-sha256 is required");
if (mode !== "install" && mode !== "upgrade") throw new Error("--mode must be exactly install or upgrade");
assertDedicatedBuildEnvironment(process.env.ICP_ENVIRONMENT);

const frontendDir = fileURLToPath(new URL("../", import.meta.url));
const repoRoot = resolve(frontendDir, "../..");
const releasePath = join(repoRoot, PUBLIC_RELEASE_FILE);
const releaseText = await readFile(releasePath, "utf8");
if (sha256Bytes(releaseText) !== approvedDigest) throw new Error("release seal does not match the explicitly approved sha256");
const release = JSON.parse(releaseText);
let facts = await verifyReleaseSeal(repoRoot, release);

function verifyBundle(expected) {
  return new Promise((resolvePromise, reject) => {
    const child = spawn(process.execPath, [
      join(frontendDir, "scripts/assert-production-public-bundle.mjs"),
      expected.outputDir,
      "--context", "deployment",
      "--expected-canister", expected.provisioned.canisterId,
      "--expected-origin", expected.provisioned.canonicalOrigin,
    ], { cwd: frontendDir, env: process.env, stdio: "inherit" });
    child.on("error", reject);
    child.on("exit", (code, signal) => code === 0 ? resolvePromise() : reject(new Error(`bundle verification failed (${signal ?? code})`)));
  });
}
await verifyBundle(facts);

function run(commandArgs) {
  return new Promise((resolvePromise, reject) => {
    const child = spawn("/usr/local/bin/icp", commandArgs, { cwd: repoRoot, env: process.env, stdio: "inherit" });
    child.on("error", reject);
    child.on("exit", (code, signal) => code === 0
      ? resolvePromise()
      : reject(new Error(`icp command failed (${signal ?? code}); asset sync was not attempted`)));
  });
}

await run([
  "canister", "install", facts.provisioned.canisterId,
  "--mode", mode,
  "--wasm", join(repoRoot, PUBLIC_WASM_ARTIFACT),
  "-e", PUBLIC_ENVIRONMENT,
  "--identity", "rumi_identity",
]);

// Refuse sync if any mapped target, asset, manifest, Wasm, or release seal
// changed while the separately approved install was running.
const releaseTextAfterInstall = await readFile(releasePath, "utf8");
if (sha256Bytes(releaseTextAfterInstall) !== approvedDigest) throw new Error("release seal changed after install; asset sync refused");
facts = await verifyReleaseSeal(repoRoot, JSON.parse(releaseTextAfterInstall));
await verifyBundle(facts);
await run([
  "sync", PUBLIC_CANISTER_NAME,
  "-e", PUBLIC_ENVIRONMENT,
  "--identity", "rumi_identity",
]);
console.log(`installed and synced the approved production-public release ${approvedDigest}`);
