import { defineConfig, loadEnv } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { resolvePublicCanonicalOrigin } from "./src/origin";

// Standalone SPA. `global`/`process` shims keep @dfinity/agent happy in the
// browser. Vitest runs in node (jsdom not needed — the unit tests are pure logic).
export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, ".", "");
  const enableDevKey = !env.VITE_DEPLOYMENT_MODE || env.VITE_DEPLOYMENT_MODE === "testnet";
  const deploymentVerification = env.VITE_PUBLIC_ORIGIN_CONTEXT === "deployment-verification";
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
    }, svelte()],
    resolve: {
      alias: enableDevKey
        ? []
        : [{
            find: /^\.\/devWallet$/,
            replacement: new URL("./src/devWallet.disabled.ts", import.meta.url).pathname,
          }],
    },
    define: {
      __ENABLE_DEV_KEY__: JSON.stringify(enableDevKey),
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
