import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount, flushSync, tick } from 'svelte';

const fx = vi.hoisted(() => ({
  loadThreeUsdRate: vi.fn(),
  loadSpRate: vi.fn(),
}));

vi.mock('../../services/earnRates', async () => {
  const actual = await vi.importActual<typeof import('../../services/earnRates')>(
    '../../services/earnRates',
  );
  return {
    ...actual,
    loadThreeUsdRate: fx.loadThreeUsdRate,
    loadSpRate: fx.loadSpRate,
  };
});

import EarnSubNav from './EarnSubNav.svelte';

let host: HTMLDivElement;
let instance: unknown;

function render(active: '3usd' | 'stability-pool' | null) {
  instance = mount(EarnSubNav, { target: host, props: { active } });
  flushSync();
}

async function settle(rounds = 5) {
  for (let i = 0; i < rounds; i++) {
    await Promise.resolve();
    await tick();
    flushSync();
  }
}

function links(): HTMLAnchorElement[] {
  return Array.from(host.querySelectorAll('a'));
}

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
  fx.loadThreeUsdRate.mockReset().mockResolvedValue({ status: 'ready', pct: 4.5 });
  fx.loadSpRate.mockReset().mockResolvedValue({ status: 'ready', pct: 6.2 });
});

afterEach(() => {
  if (instance) {
    unmount(instance as any);
    instance = undefined;
  }
  host.remove();
});

describe('EarnSubNav', () => {
  it('is a two-option tab bar with real, accessible links to both canonical pool pages', () => {
    render('3usd');
    const hrefs = links().map((a) => a.getAttribute('href'));
    expect(hrefs).toEqual(['/3usd', '/stability-pool']);
    // Real route links, not fake `role="tab"` buttons without keyboard support.
    expect(host.querySelector('[role="tab"]')).toBeFalsy();
  });

  it('marks only the active pool with aria-current, and none when a selection has not been made yet', () => {
    render('3usd');
    const byHref = (href: string) => links().find((a) => a.getAttribute('href') === href)!;
    expect(byHref('/3usd').getAttribute('aria-current')).toBe('page');
    expect(byHref('/stability-pool').getAttribute('aria-current')).toBeNull();

    unmount(instance as any);
    render(null);
    expect(byHref('/3usd').getAttribute('aria-current')).toBeNull();
    expect(byHref('/stability-pool').getAttribute('aria-current')).toBeNull();
  });

  it('shows the 3USD label prominently and never labels the badge "Interest APY"', () => {
    render('3usd');
    const threeUsdLink = links().find((a) => a.getAttribute('href') === '/3usd')!;
    expect(threeUsdLink.querySelector('.tab-label')?.textContent).toContain('3USD');
    expect(host.textContent).not.toContain('Interest APY');
  });

  it('shows a loading badge for both tabs before either rate resolves', () => {
    fx.loadThreeUsdRate.mockReturnValue(new Promise(() => {}));
    fx.loadSpRate.mockReturnValue(new Promise(() => {}));
    render('3usd');
    const badges = host.querySelectorAll('.tab-badge-loading');
    expect(badges.length).toBe(2);
  });

  it('never leaves a badge stuck on "Loading…": a stalled request settles into unavailable after the bounded wait', async () => {
    vi.useFakeTimers();
    fx.loadThreeUsdRate.mockReturnValue(new Promise(() => {})); // never settles
    fx.loadSpRate.mockResolvedValue({ status: 'ready', pct: 4.5 });
    render('3usd');
    await vi.advanceTimersByTimeAsync(5000);
    await settle();
    const threeUsdLink = links().find((a) => a.getAttribute('href') === '/3usd')!;
    expect(threeUsdLink.querySelector('.tab-badge-unavailable')?.textContent).toContain('unavailable');
    vi.useRealTimers();
  });

  it('shows each tab badge as "X.XX% APY" once its rate resolves, independently of the other', async () => {
    render('3usd');
    await settle();
    const threeUsdLink = links().find((a) => a.getAttribute('href') === '/3usd')!;
    const spLink = links().find((a) => a.getAttribute('href') === '/stability-pool')!;
    expect(threeUsdLink.querySelector('.tab-badge')?.textContent).toBe('4.50% APY');
    expect(spLink.querySelector('.tab-badge')?.textContent).toBe('6.20% APY');
  });

  it('shows "Rate unavailable" for one tab while the other still shows its real rate', async () => {
    fx.loadSpRate.mockResolvedValue({ status: 'unavailable', pct: null });
    render('3usd');
    await settle();
    const threeUsdLink = links().find((a) => a.getAttribute('href') === '/3usd')!;
    const spLink = links().find((a) => a.getAttribute('href') === '/stability-pool')!;
    expect(threeUsdLink.querySelector('.tab-badge')?.textContent).toContain('%');
    expect(spLink.querySelector('.tab-badge-unavailable')?.textContent).toContain('unavailable');
  });

  it('lays each tab out with a label and a badge as separate elements, so narrow screens can stack them', () => {
    render('3usd');
    for (const a of links()) {
      expect(a.querySelector('.tab-label')).toBeTruthy();
      expect(a.querySelector('.tab-badge')).toBeTruthy();
    }
  });
});
