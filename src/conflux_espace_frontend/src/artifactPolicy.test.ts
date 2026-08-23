import { describe, expect, it } from "vitest";
import gitignore from "../.gitignore?raw";
import packageJson from "../package.json";
import verifier from "../scripts/verify-production-public-build.mjs?raw";
import evmSource from "./evm.ts?raw";
import icpManifest from "../../../icp.yaml?raw";

describe("frontend artifact policy", () => {
  it("ignores both ordinary and canonical-origin production output", () => {
    const ignore = gitignore.split(/\r?\n/).map((line) => line.trim()).filter(Boolean);
    expect(ignore).toContain("dist/");
    expect(ignore).toContain("dist-production-public/");
  });

  it("keeps deterministic verification ephemeral and the deploy origin external", () => {
    const deploy = packageJson.scripts["build:production-public:deploy"];
    expect(deploy).toContain("VITE_PUBLIC_ORIGIN_CONTEXT=deployment");
    expect(deploy).not.toContain("VITE_PUBLIC_CANONICAL_ORIGIN=");
    expect(deploy).not.toContain("local-verification");
    expect(verifier).toContain("mkdtemp");
    expect(verifier).toContain("deployment-verification");
    expect(verifier).toContain("VITE_PUBLIC_VERIFICATION_OUTPUT_DIR");
    expect(verifier).toContain("await rm(outputDir, { recursive: true, force: true })");
  });

  it("binds receipt polling to deployment-configured finality rather than a one-block production default", () => {
    expect(evmSource).toContain("confirmations: RECEIPT_CONFIRMATIONS");
    expect(evmSource).not.toMatch(/confirmations:\s*1\b/);
  });

  it("keeps the unprovisioned public frontend completely outside the ICP project manifest", () => {
    expect(icpManifest).not.toContain("conflux_espace_public_frontend");
    expect(icpManifest).not.toContain("conflux-public-build-only");
  });
});
