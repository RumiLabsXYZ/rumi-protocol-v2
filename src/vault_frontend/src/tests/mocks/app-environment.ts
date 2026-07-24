/**
 * Test stub for SvelteKit's `$app/environment`, which only exists inside a
 * `vite dev`/`svelte-kit build` graph. Aliased in vitest.config.ts so services
 * that guard browser-only work (localStorage) can be unit-tested under jsdom.
 */
export const browser = true;
export const dev = true;
export const building = false;
export const version = 'test';
