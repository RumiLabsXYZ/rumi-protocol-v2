import { defineConfig } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import path from 'path';

/**
 * Test-only config for src/lib/utils/dogeBorrowPage.fixture.spec.ts.
 *
 * Mounting the real +page.svelte with `mount()`/`unmount()` from 'svelte'
 * requires the client-side runtime, but Vite's default "node" resolve
 * condition pulls in svelte/internal/server (mount() throws
 * `lifecycle_function_unavailable` there). Adding the `browser` resolve
 * condition — scoped to this config only — makes Vite resolve svelte's
 * client build under jsdom, matching how a real browser bundle behaves.
 * The main vitest.config.ts is untouched.
 */
export default defineConfig({
  plugins: [svelte({ hot: false })],
  resolve: {
    conditions: ['browser'],
    alias: {
      '$declarations': path.resolve(__dirname, '../../declarations'),
      '$lib': path.resolve(__dirname, 'src/lib'),
      '$services': path.resolve(__dirname, 'src/lib/services'),
      '$components': path.resolve(__dirname, 'src/lib/components'),
      '$stores': path.resolve(__dirname, 'src/lib/stores'),
      '$utils': path.resolve(__dirname, 'src/lib/utils'),
      '$app/environment': path.resolve(__dirname, 'src/tests/mocks/app-environment.ts'),
    },
  },
  test: {
    environment: 'jsdom',
    globals: true,
    hookTimeout: 60000,
    testTimeout: 30000,
    include: ['src/lib/utils/dogeBorrowPage.fixture.spec.ts'],
    setupFiles: ['src/tests/vitest-setup.ts'],
  },
});
