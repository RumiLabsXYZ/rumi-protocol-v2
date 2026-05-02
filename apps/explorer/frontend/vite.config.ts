import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import path from "node:path";

// @icp-sdk/bindgen Vite plugin — subpath is /plugins/vite, not /vite
import { icpBindgen } from "@icp-sdk/bindgen/plugins/vite";

// @icp-sdk/core does not export getDevServerConfig in v5.x.
// The ic_env cookie shim is implemented inline below via a custom Vite plugin.

export default defineConfig(({ command }) => ({
  plugins: [
    react(),
    icpBindgen({
      didFile: path.resolve(
        __dirname,
        "../canisters/explorer_bff/explorer_bff.did",
      ),
      outDir: path.resolve(__dirname, "src/bindings/explorer_bff"),
      output: {
        declarations: {
          typescript: true,
          flat: true,
        },
      },
    }),
    // Dev-server cookie shim: inject ic_env cookie so safeGetCanisterEnv()
    // works when the page is served by Vite directly instead of the asset canister.
    // Only runs during `vite dev`, not `vite build`.
    ...(command === "serve"
      ? [
          {
            name: "ic-env-cookie-shim",
            configureServer(server: import("vite").ViteDevServer) {
              server.middlewares.use((_req, res, next) => {
                // Read the local canister IDs from .icp/local/state.json if available,
                // falling back to empty. The cookie value matches what the asset canister
                // would set: a JSON blob with IC_ROOT_KEY and canister ID env vars.
                // For local dev the root key is the well-known local test key (64 zero bytes).
                const localRootKey = new Uint8Array(64).fill(0);
                const env: Record<string, unknown> = {
                  IC_ROOT_KEY: Array.from(localRootKey),
                };
                const cookieValue = encodeURIComponent(JSON.stringify(env));
                // Set the cookie if not already present (don't override real asset-canister cookies)
                const existingCookies = (_req as import("http").IncomingMessage).headers.cookie ?? "";
                if (!existingCookies.includes("ic_env=")) {
                  (res as import("http").ServerResponse).setHeader(
                    "Set-Cookie",
                    `ic_env=${cookieValue}; Path=/; SameSite=Strict`,
                  );
                }
                next();
              });
            },
          },
        ]
      : []),
  ],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  server: {
    port: 5173,
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
  },
}));
