import { defineConfig, loadEnv } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { resolvePublicCanonicalOrigin } from "./src/origin";

// Standalone SPA. `global`/`process` shims keep @dfinity/agent happy in the
// browser. Vitest runs in node (jsdom not needed — the unit tests are pure logic).
export default defineConfig(({ mode, command }) => {
  const env = loadEnv(mode, ".", "");
  const enableDevKey = !env.VITE_DEPLOYMENT_MODE || env.VITE_DEPLOYMENT_MODE === "testnet";
  const productionCanaryBuild = env.VITE_DEPLOYMENT_MODE === "production-canary";
  const productionPublicBuild = env.VITE_DEPLOYMENT_MODE === "production-public";
  const productionPublicArtifactBuild = command === "build" && productionPublicBuild;
  const deploymentVerification = env.VITE_PUBLIC_ORIGIN_CONTEXT === "deployment-verification";
  const prunePublicRegion = (source: string, name: string, replacement = "") => {
    const start = `/* production-public-prune:start ${name} */`;
    const end = `/* production-public-prune:end ${name} */`;
    const first = source.indexOf(start);
    const last = source.indexOf(end);
    if (first < 0 || last < first || source.indexOf(start, first + start.length) >= 0 || source.indexOf(end, last + end.length) >= 0) {
      throw new Error(`production-public source pruning markers are missing or duplicated: ${name}`);
    }
    return `${source.slice(0, first)}${replacement}${source.slice(last + end.length)}`;
  };
  const replacePublicExact = (source: string, expected: string, replacement: string, label: string) => {
    const first = source.indexOf(expected);
    if (first < 0 || source.indexOf(expected, first + expected.length) >= 0) {
      throw new Error(`production-public exact source pruning seam is missing or duplicated: ${label}`);
    }
    return `${source.slice(0, first)}${replacement}${source.slice(first + expected.length)}`;
  };
  // Validate while Vite is starting so a public build with a missing, raw,
  // malformed, or local deployment origin cannot emit an artifact at all.
  resolvePublicCanonicalOrigin(
    env.VITE_DEPLOYMENT_MODE,
    env.VITE_PUBLIC_CANONICAL_ORIGIN,
    env.VITE_PUBLIC_ORIGIN_CONTEXT,
  );
  return {
    plugins: [{
      name: "production-public-ephemeral-verification-output",
      configResolved(config) {
        if (!deploymentVerification) return;
        const expected = env.VITE_PUBLIC_VERIFICATION_OUTPUT_DIR;
        const outsideProject = !!expected && expected.startsWith("/") &&
          expected.includes("/rumi-conflux-public-verify-") &&
          !expected.startsWith(`${config.root}/`);
        if (config.command !== "build" || !outsideProject || config.build.outDir !== expected) {
          throw new Error(
            "Deployment verification must build only into its guarded ephemeral directory; it cannot emit the deploy artifact.",
          );
        }
      },
    }, {
      name: "deployment-mode-svelte-pruning",
      enforce: "pre",
      transform(source, id) {
        if (!id.endsWith(".svelte") && !id.endsWith("/config.ts") && !id.endsWith("/styles.css")) return;
        let transformed = source
          .replaceAll("__RUMI_PRODUCTION_CANARY_BUILD__", JSON.stringify(productionCanaryBuild))
          .replaceAll("__RUMI_PRODUCTION_PUBLIC_BUILD__", JSON.stringify(productionPublicBuild));
        if (productionPublicArtifactBuild && id.endsWith("/App.svelte")) {
          transformed = prunePublicRegion(transformed, "derived-canary", [
            "const productionLifecycleUsed = false;",
            "const canaryPolling = false;",
            "const unresolvedAuthorization = false;",
            "const transactions: Array<{ label: string; hash: string; url: string }> = [];",
          ].join("\n  "));
          transformed = prunePublicRegion(transformed, "canary-storage");
          transformed = prunePublicRegion(transformed, "canary-reconciliation");
          transformed = replacePublicExact(
            transformed,
            "        isGuidedVault={false && canary?.vaultId === v.vault_id.toString()}\n" +
              "        guidedPhase={false && canary?.vaultId === v.vault_id.toString() ? canary.phase : null}\n",
            "",
            "App guided lifecycle props",
          );
        }
        if (productionPublicArtifactBuild && id.endsWith("/VaultCard.svelte")) {
          transformed = replacePublicExact(transformed, ", isGuidedVault, guidedPhase", "", "VaultCard guided destructuring");
          transformed = replacePublicExact(
            transformed,
            "    isGuidedVault: boolean;\n    guidedPhase: CanaryPhase | null;\n",
            "",
            "VaultCard guided prop types",
          );
        }
        if (productionPublicArtifactBuild && id.endsWith("/config.ts")) {
          transformed = prunePublicRegion(transformed, "testnet-chain-flag");
          transformed = prunePublicRegion(transformed, "public-guided-flag");
          transformed = replacePublicExact(
            transformed,
            "export const IS_PRODUCTION_CANARY = DEPLOYMENT.guidedLifecycle;",
            "export const IS_PRODUCTION_CANARY = false;",
            "public guided lifecycle selector",
          );
          transformed = prunePublicRegion(
            transformed,
            "guided-open-terms",
            "export const CANARY_COLLATERAL_WEI = 0n;\n" +
              "export const CANARY_DEBT_E8S = 0n;\n" +
              "export function openTermsFor(_config: DeploymentConfig, requestedCollateral: bigint, requestedDebt: bigint) {\n" +
              "  return { collateralWei: requestedCollateral, debtE8s: requestedDebt };\n" +
              "}",
          );
        }
        if (productionPublicArtifactBuild && id.endsWith("/styles.css")) {
          transformed = prunePublicRegion(transformed, "testnet-badge-style");
          transformed = prunePublicRegion(transformed, "canary-step-style");
        }
        return transformed;
      },
    }, svelte()],
    resolve: {
      alias: [
        ...(!enableDevKey ? [{
            find: /^\.\/devWallet$/,
            replacement: new URL("./src/devWallet.disabled.ts", import.meta.url).pathname,
          }] : []),
        ...(productionPublicArtifactBuild ? [{
            find: /^\.\/canaryState$/,
            replacement: new URL("./src/canaryState.disabled.ts", import.meta.url).pathname,
          }] : []),
      ],
    },
    define: {
      __ENABLE_DEV_KEY__: JSON.stringify(enableDevKey),
      __RUMI_PRODUCTION_CANARY_BUILD__: JSON.stringify(productionCanaryBuild),
      __RUMI_PRODUCTION_PUBLIC_BUILD__: JSON.stringify(productionPublicBuild),
      global: "globalThis",
      "process.env": {},
    },
    server: { port: 5180 },
    test: {
      environment: "node",
      include: ["src/**/*.test.ts"],
    },
  };
});
