import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

// The auto-selection redirect must live only on `/earn`. A direct link to
// `/3usd` or `/stability-pool` must keep showing that pool regardless of
// which rate is higher, so neither canonical page may wire in the redirect
// helpers at all. Source-level guard rather than a full mount: this is an
// architectural invariant (no import ⇒ no redirect call is possible), and it
// stays correct even as the pages' own data loading evolves.
function read(relativePath: string): string {
  return readFileSync(fileURLToPath(new URL(relativePath, import.meta.url)), 'utf-8');
}

describe('direct pool routes never auto-select', () => {
  it('/3usd does not import the earn redirect decision or navigation helpers', () => {
    const src = read('../3usd/+page.svelte');
    expect(src).not.toContain('earnRates');
    expect(src).not.toContain('earnNavigation');
  });

  it('/stability-pool does not import the earn redirect decision or navigation helpers', () => {
    const src = read('../stability-pool/+page.svelte');
    expect(src).not.toContain('earnRates');
    expect(src).not.toContain('earnNavigation');
  });

  it('EarnSubNav (shared by all three routes) never calls the navigation helper itself', () => {
    const src = read('../../lib/components/layout/EarnSubNav.svelte');
    expect(src).not.toContain('earnNavigation');
    expect(src).not.toContain('replaceRoute');
  });
});
