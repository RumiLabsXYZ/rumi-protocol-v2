import { describe, expect, it } from "vitest";
import gitignore from "../.gitignore?raw";
import packageJson from "../package.json";
import verifier from "../scripts/verify-production-public-build.mjs?raw";
import deployBuilder from "../scripts/build-provisioned-production-public.mjs?raw";
import evmSource from "./evm.ts?raw";
import icpManifest from "../../../icp.yaml?raw";

describe("frontend artifact policy", () => {
  it("ignores both ordinary and canonical-origin production output", () => {
    const ignore = gitignore.split(/\r?\n/).map((line) => line.trim()).filter(Boolean);
    expect(ignore).toContain("dist/");
    expect(ignore).toContain("dist-production-public/");
  });

  it("keeps deterministic verification ephemeral and derives deploy origin only from the mapping", () => {
    const deploy = packageJson.scripts["build:production-public:deploy"];
    expect(deploy).toBe("node scripts/build-provisioned-production-public.mjs");
    expect(deploy).not.toContain("VITE_PUBLIC_CANONICAL_ORIGIN=");
    expect(deploy).not.toContain("local-verification");
    expect(deployBuilder).toContain("PUBLIC_MAPPING_FILE");
    expect(deployBuilder).toContain("provisioningFromMappingText");
    expect(deployBuilder).toContain("VITE_PUBLIC_CANONICAL_ORIGIN: provisioned.canonicalOrigin");
    expect(verifier).toContain("mkdtemp");
    expect(verifier).toContain("deployment-verification");
    expect(verifier).toContain("VITE_PUBLIC_VERIFICATION_OUTPUT_DIR");
    expect(verifier).toContain("await rm(outputDir, { recursive: true, force: true })");
  });

  it("binds receipt polling to deployment-configured finality rather than a one-block production default", () => {
    expect(evmSource).toContain("confirmations: RECEIPT_CONFIRMATIONS");
    expect(evmSource).not.toMatch(/confirmations:\s*1\b/);
  });

  it("pins a separate production-public recipe without replacing staging", () => {
    expect(icpManifest).toContain("- name: conflux_public_frontend");
    expect(icpManifest).toContain("- name: conflux-production-public");
    expect(icpManifest).toContain("dir: src/conflux_espace_frontend/dist-production-public");
    expect(icpManifest).toContain("npm run build:production-public:deploy");
    expect(icpManifest).toContain("- name: conflux_espace_frontend");
    expect(icpManifest).toContain("dir: src/conflux_espace_frontend/dist");
    expect(icpManifest).toContain("npm ci && npm run build");
  });
});
