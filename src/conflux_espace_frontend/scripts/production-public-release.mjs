import { readFile } from "node:fs/promises";
import { join } from "node:path";
import {
  PUBLIC_MANIFEST_FILE,
  PUBLIC_MAPPING_FILE,
  PUBLIC_OUTPUT_DIR,
  PUBLIC_WASM_ARTIFACT,
  assetInventoryDigest,
  computeAssetInventory,
  createPublicRelease,
  provisioningFromMappingText,
  sha256Bytes,
  sha256File,
  trackedCanisterIds,
  verifyIcpRecipeText,
  verifyPublicManifest,
  verifyPublicRelease,
} from "./production-public-recipe.mjs";

export async function currentReleaseFacts(repoRoot) {
  const icpYamlText = await readFile(join(repoRoot, "icp.yaml"), "utf8");
  verifyIcpRecipeText(icpYamlText);
  const mappingPath = join(repoRoot, PUBLIC_MAPPING_FILE);
  const mappingText = await readFile(mappingPath, "utf8");
  const knownIds = await trackedCanisterIds(join(repoRoot, ".icp/data/mappings"));
  const provisioned = provisioningFromMappingText(mappingText, "deployment", knownIds);
  const outputDir = join(repoRoot, PUBLIC_OUTPUT_DIR);
  const manifestPath = join(outputDir, PUBLIC_MANIFEST_FILE);
  const manifestText = await readFile(manifestPath, "utf8");
  const manifest = JSON.parse(manifestText);
  const assetInventory = await computeAssetInventory(outputDir);
  verifyPublicManifest(manifest, { ...provisioned, assetInventory, context: "deployment" });
  const expected = {
    provisioned,
    icpYamlSha256: sha256Bytes(icpYamlText),
    mappingSha256: sha256Bytes(mappingText),
    manifestSha256: sha256Bytes(manifestText),
    assetInventorySha256: assetInventoryDigest(assetInventory),
    wasmSha256: await sha256File(join(repoRoot, PUBLIC_WASM_ARTIFACT)),
  };
  return { provisioned, outputDir, expected, release: createPublicRelease(expected) };
}

export async function verifyReleaseSeal(repoRoot, release) {
  const facts = await currentReleaseFacts(repoRoot);
  verifyPublicRelease(release, facts.expected);
  return facts;
}
