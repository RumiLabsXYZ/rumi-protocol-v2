import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount, flushSync, tick } from 'svelte';

const fx = vi.hoisted(() => ({
  loadThreeUsdRate: vi.fn(),
  loadSpRate: vi.fn(),
  replaceRoute: vi.fn(),
  beforeNavigate: vi.fn(),
}));

vi.mock('../../lib/services/earnRates', async () => {
  const actual = await vi.importActual<typeof import('../../lib/services/earnRates')>(
    '../../lib/services/earnRates',
  );
  return {
    ...actual,
    loadThreeUsdRate: fx.loadThreeUsdRate,
    loadSpRate: fx.loadSpRate,
    // `freshEarnSnapshot` is what the page actually calls; route it to the
    // same mocked loaders so each test's rate setup still applies.
    freshEarnSnapshot: () => ({
      loadThreeUsdRate: fx.loadThreeUsdRate,
      loadSpRate: fx.loadSpRate,
    }),
  };
});

vi.mock('../../lib/utils/earnNavigation', () => ({
  replaceRoute: fx.replaceRoute,
}));

vi.mock('$app/navigation', () => ({
  beforeNavigate: fx.beforeNavigate,
  goto: vi.fn(),
}));

import Page from './+page.svelte';

// The rendered `EarnSubNav` reads the same mocked `earnRates` loaders as the
// page, so it just displays whatever rates each case sets up above.

async function settle(rounds = 10) {
  for (let i = 0; i < rounds; i++) {
    await Promise.resolve();
    await tick();
    flushSync();
  }
}

let host: HTMLDivElement;
let instance: unknown;

function render() {
  instance = mount(Page, { target: host });
  flushSync();
}

function cleanup() {
  if (instance) {
    unmount(instance as any);
    instance = undefined;
  }
}

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
  fx.loadThreeUsdRate.mockReset();
  fx.loadSpRate.mockReset();
  fx.replaceRoute.mockReset();
  fx.beforeNavigate.mockReset();
});

afterEach(() => {
  cleanup();
  host.remove();
  vi.useRealTimers();
});

describe('Earn auto-selection page', () => {
  it('shows the shared tab bar for both pools before either rate resolves, with neither marked selected', () => {
    fx.loadThreeUsdRate.mockReturnValue(new Promise(() => {}));
    fx.loadSpRate.mockReturnValue(new Promise(() => {}));
    render();
    const hrefs = Array.from(host.querySelectorAll('a')).map((a) => a.getAttribute('href'));
    expect(hrefs).toEqual(['/3usd', '/stability-pool']);
    expect(host.querySelector('[aria-current="page"]')).toBeFalsy();
  });

  it('redirects to 3USD when its rate is higher', async () => {
    fx.loadThreeUsdRate.mockResolvedValue({ status: 'ready', pct: 6 });
    fx.loadSpRate.mockResolvedValue({ status: 'ready', pct: 4 });
    render();
    await settle();
    expect(fx.replaceRoute).toHaveBeenCalledWith('/3usd');
  });

  it('redirects to the stability pool when its rate is higher', async () => {
    fx.loadThreeUsdRate.mockResolvedValue({ status: 'ready', pct: 4 });
    fx.loadSpRate.mockResolvedValue({ status: 'ready', pct: 6 });
    render();
    await settle();
    expect(fx.replaceRoute).toHaveBeenCalledWith('/stability-pool');
  });

  it('redirects to 3USD on an exact tie', async () => {
    fx.loadThreeUsdRate.mockResolvedValue({ status: 'ready', pct: 5 });
    fx.loadSpRate.mockResolvedValue({ status: 'ready', pct: 5 });
    render();
    await settle();
    expect(fx.replaceRoute).toHaveBeenCalledWith('/3usd');
  });

  it('redirects to whichever single rate is ready when the other is unavailable', async () => {
    fx.loadThreeUsdRate.mockResolvedValue({ status: 'unavailable', pct: null });
    fx.loadSpRate.mockResolvedValue({ status: 'ready', pct: 3 });
    render();
    await settle();
    expect(fx.replaceRoute).toHaveBeenCalledWith('/stability-pool');
  });

  it('redirects to 3USD when both rates are unavailable', async () => {
    fx.loadThreeUsdRate.mockResolvedValue({ status: 'unavailable', pct: null });
    fx.loadSpRate.mockResolvedValue({ status: 'unavailable', pct: null });
    render();
    await settle();
    expect(fx.replaceRoute).toHaveBeenCalledWith('/3usd');
  });

  it('never hangs on a stalled request: still redirects after the bounded wait using the rate that did resolve', async () => {
    vi.useFakeTimers();
    fx.loadThreeUsdRate.mockReturnValue(new Promise(() => {})); // never settles
    fx.loadSpRate.mockResolvedValue({ status: 'ready', pct: 3 });
    render();
    await vi.advanceTimersByTimeAsync(5000);
    await settle();
    expect(fx.replaceRoute).toHaveBeenCalledWith('/stability-pool');
  });

  it('does not navigate if the page unmounts (manual navigation) before both rates resolve', async () => {
    let resolveThreeUsd: (v: any) => void;
    fx.loadThreeUsdRate.mockReturnValue(new Promise((resolve) => { resolveThreeUsd = resolve; }));
    fx.loadSpRate.mockResolvedValue({ status: 'ready', pct: 3 });
    render();
    await settle(2);
    cleanup(); // simulates the user clicking a tab link and navigating away
    resolveThreeUsd!({ status: 'ready', pct: 9 });
    await settle();
    expect(fx.replaceRoute).not.toHaveBeenCalled();
  });

  it('does not navigate once a manual navigation is already underway (beforeNavigate), even while this page is still mounted', async () => {
    let resolveThreeUsd: (v: any) => void;
    fx.loadThreeUsdRate.mockReturnValue(new Promise((resolve) => { resolveThreeUsd = resolve; }));
    fx.loadSpRate.mockResolvedValue({ status: 'ready', pct: 3 });
    render();
    await settle(2);

    // Simulates the router firing `beforeNavigate` the instant the user
    // clicks a tab link, before the old page has actually unmounted (e.g.
    // while the destination route's bundle/data is still loading).
    expect(fx.beforeNavigate).toHaveBeenCalledTimes(1);
    fx.beforeNavigate.mock.calls[0][0]();

    resolveThreeUsd!({ status: 'ready', pct: 9 });
    await settle();
    expect(fx.replaceRoute).not.toHaveBeenCalled();
  });
});
