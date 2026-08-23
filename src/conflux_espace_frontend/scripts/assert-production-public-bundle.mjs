import { readdir, readFile } from "node:fs/promises";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const defaultOutputDir = fileURLToPath(new URL("../dist-production-public/", import.meta.url));
const outputDir = resolve(process.argv[2] ?? defaultOutputDir);
// Product labels and test-signer symbols are exact, case-sensitive sentinels.
// The anonymous IC agent legitimately bundles generic lowercase cryptographic
// validation messages; those are not the local test signer implementation.
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
let productionFinalityDepthPresent = false;
for (const path of files) {
  const contents = await readFile(path, "utf8").catch(() => "");
  if (/receiptConfirmations:\s*400\b/.test(contents)) productionFinalityDepthPresent = true;
  for (const pattern of forbidden) {
    if (pattern.test(contents)) throw new Error(`production-public bundle contains forbidden dev-wallet material (${pattern}) in ${path}`);
  }
}
if (!productionFinalityDepthPresent) {
  throw new Error("production-public bundle does not require 400 transaction-receipt confirmations");
}

console.log(`production-public bundle policy passed (${files.length} files scanned)`);
