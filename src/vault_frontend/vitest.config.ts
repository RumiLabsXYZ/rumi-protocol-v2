import { defineConfig } from 'vitest/config';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import path from 'path';

export default defineConfig({
  plugins: [svelte({ hot: false })],
  resolve: {
    alias: {
      '$declarations': path.resolve(__dirname, '../../declarations'),
      '$lib': path.resolve(__dirname, 'src/lib'),
      '$app/environment': path.resolve(__dirname, 'src/tests/mocks/app-environment.ts'),
    },
  },
  test: {
    environment: 'jsdom',
    globals: true,
    hookTimeout: 60000, // Increase timeout to 60 seconds for PocketIC setup
    testTimeout: 30000, // Increase test timeout to 30 seconds
    include: ['src/**/*.{test,spec}.{js,ts,jsx,tsx}'],
    setupFiles: ['src/tests/vitest-setup.ts'],
    coverage: {
      reporter: ['text', 'json', 'html'],
    },
  },
});
