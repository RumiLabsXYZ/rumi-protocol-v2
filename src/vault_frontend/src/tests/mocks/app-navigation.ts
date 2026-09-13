/**
 * Test stub for SvelteKit's `$app/navigation`, which only exists inside a
 * `vite dev`/`svelte-kit build` graph. Aliased in vitest.config.ts so code
 * using `goto`/`beforeNavigate` can be unit-tested under jsdom; specs that
 * care about navigation calls replace this with their own `vi.mock`.
 */
export function goto(): Promise<void> {
  return Promise.resolve();
}

export function beforeNavigate(): void {}
