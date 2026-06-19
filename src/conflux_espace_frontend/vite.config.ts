import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// Standalone SPA. `global`/`process` shims keep @dfinity/agent happy in the
// browser. Vitest runs in node (jsdom not needed — the unit tests are pure logic).
export default defineConfig({
  plugins: [svelte()],
  define: {
    global: "globalThis",
    "process.env": {},
  },
  server: { port: 5180 },
  test: {
    environment: "node",
    include: ["src/**/*.test.ts"],
  },
});
