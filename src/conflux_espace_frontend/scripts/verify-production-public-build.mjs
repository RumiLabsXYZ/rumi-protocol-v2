import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawn } from "node:child_process";

const verificationMode = process.argv[2];
if (verificationMode !== "local" && verificationMode !== "deployment") {
  throw new Error("verification mode must be local or deployment");
}

const frontendDir = fileURLToPath(new URL("../", import.meta.url));
const viteBin = fileURLToPath(new URL("../node_modules/vite/bin/vite.js", import.meta.url));
const scanner = fileURLToPath(new URL("./assert-production-public-bundle.mjs", import.meta.url));
const outputDir = await mkdtemp(join(tmpdir(), "rumi-conflux-public-verify-"));

function run(args, env = process.env) {
  return new Promise((resolvePromise, reject) => {
    const child = spawn(process.execPath, args, { cwd: frontendDir, env, stdio: "inherit" });
    child.on("error", reject);
    child.on("exit", (code, signal) => {
      if (code === 0) resolvePromise();
      else reject(new Error(`verification subprocess failed (${signal ?? code})`));
    });
  });
}

try {
  const deploymentVerification = verificationMode === "deployment";
  await run([viteBin, "build", "--outDir", outputDir, "--emptyOutDir"], {
    ...process.env,
    VITE_DEPLOYMENT_MODE: "production-public",
    VITE_PUBLIC_ORIGIN_CONTEXT: deploymentVerification ? "deployment-verification" : "local-verification",
    VITE_PUBLIC_VERIFICATION_OUTPUT_DIR: deploymentVerification ? outputDir : "",
    VITE_PUBLIC_CANONICAL_ORIGIN: deploymentVerification
      ? "https://rrkah-fqaaa-aaaaa-aaaaq-cai.icp0.io"
      : "http://127.0.0.1:5174",
  });
  await run([scanner, outputDir]);
  console.log(`production-public ${verificationMode} build verified in an ephemeral directory`);
} finally {
  await rm(outputDir, { recursive: true, force: true });
}
