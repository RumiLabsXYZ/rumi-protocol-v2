import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { mount, unmount, flushSync } from 'svelte';
import EarnSubNav from './EarnSubNav.svelte';

let host: HTMLDivElement;
let instance: unknown;

function render(active: 'overview' | '3usd' | 'stability-pool') {
  instance = mount(EarnSubNav, { target: host, props: { active } });
  flushSync();
}

function links(): HTMLAnchorElement[] {
  return Array.from(host.querySelectorAll('a'));
}

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  if (instance) {
    unmount(instance as any);
    instance = undefined;
  }
  host.remove();
});

describe('EarnSubNav', () => {
  it('links to the three canonical Earn pages', () => {
    render('overview');
    const hrefs = links().map((a) => a.getAttribute('href'));
    expect(hrefs).toEqual(['/earn', '/3usd', '/stability-pool']);
  });

  it('marks only the active page with aria-current, and moves it as the prop changes', () => {
    render('3usd');
    const byHref = (href: string) => links().find((a) => a.getAttribute('href') === href)!;

    expect(byHref('/3usd').getAttribute('aria-current')).toBe('page');
    expect(byHref('/earn').getAttribute('aria-current')).toBeNull();
    expect(byHref('/stability-pool').getAttribute('aria-current')).toBeNull();

    unmount(instance as any);
    render('stability-pool');
    expect(byHref('/stability-pool').getAttribute('aria-current')).toBe('page');
    expect(byHref('/3usd').getAttribute('aria-current')).toBeNull();
  });

  it('gives the 3USD link a short label alongside its full label, so narrow screens can swap to it via CSS', () => {
    render('overview');
    const threeUsdLink = links().find((a) => a.getAttribute('href') === '/3usd')!;
    expect(threeUsdLink.querySelector('.label-full')?.textContent).toContain('3USD');
    expect(threeUsdLink.querySelector('.label-short')?.textContent).toBe('3USD Liquidity');
  });
});
