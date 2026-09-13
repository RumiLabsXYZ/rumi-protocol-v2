import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { mount, unmount, flushSync, tick } from 'svelte';

const fx = vi.hoisted(() => ({
  getProtocolStatus: vi.fn(),
  getPoolStatus: vi.fn(),
  getThreePoolApy: vi.fn(),
}));

vi.mock('../../lib/services/protocol', () => ({
  ProtocolService: { getProtocolStatus: fx.getProtocolStatus },
}));

vi.mock('../../lib/services/stabilityPoolService', () => ({
  stabilityPoolService: { getPoolStatus: fx.getPoolStatus },
}));

vi.mock('../../lib/services/threePoolApyService', () => ({
  getThreePoolApy: fx.getThreePoolApy,
}));

import Page from './+page.svelte';

// `totalDebtE8s` here is already normalized to icUSD (see liveApy.ts's
// comment on spInterestApr); `eligible_icusd_per_collateral` values are
// e8s. Chosen to produce a small, realistic, finite APR (~1.25%).
const SP_PROTOCOL_STATUS = {
  interestSplit: [{ destination: 'stability_pool', bps: 5000 }],
  perCollateralInterest: [
    { collateralType: 'icp', totalDebtE8s: 500, weightedInterestRate: 0.05 },
  ],
};
const SP_POOL_STATUS = {
  eligible_icusd_per_collateral: [['icp', 100_000_000_000n]],
};

async function settle(rounds = 8) {
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

function q(sel: string): HTMLElement | null {
  return host.querySelector(sel);
}

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
  fx.getProtocolStatus.mockReset().mockResolvedValue(SP_PROTOCOL_STATUS);
  fx.getPoolStatus.mockReset().mockResolvedValue(SP_POOL_STATUS);
  fx.getThreePoolApy.mockReset().mockResolvedValue({
    total_apy_pct: 4.5,
    interest_apr_pct: 2,
    swap_fee_apr_pct: 2.5,
    pool_tvl_icusd: 1_000_000,
    three_pool_share_bps: 5000,
    complete: true,
  });
});

afterEach(() => {
  if (instance) {
    unmount(instance as any);
    instance = undefined;
  }
  host.remove();
});

describe('Earn overview page', () => {
  it('shows both action links immediately, before any rate resolves', () => {
    render();
    expect(q('a[href="/3usd"].earn-cta')?.textContent).toContain('Provide liquidity');
    expect(q('a[href="/stability-pool"].earn-cta')?.textContent).toContain('Deposit');
  });

  it('renders both real rates once their independent loads resolve', async () => {
    render();
    await settle();

    expect(host.textContent).toContain('4.50%');
    // A real, positive SP rate given the mocked protocol/pool status above.
    const cards = host.querySelectorAll('.earn-card');
    const spPill = cards[1].querySelector('.rate-pill');
    expect(spPill?.textContent).not.toContain('unavailable');
    expect(spPill?.textContent).toMatch(/%/);
  });

  it('shows the 3USD card as unavailable (not an invented 0.00%) when its rate is incomplete, while the SP card still loads and both CTAs stay visible', async () => {
    fx.getThreePoolApy.mockResolvedValue({
      total_apy_pct: 0,
      interest_apr_pct: 0,
      swap_fee_apr_pct: 0,
      pool_tvl_icusd: 0,
      three_pool_share_bps: 5000,
      complete: false,
    });
    render();
    await settle();

    const cards = host.querySelectorAll('.earn-card');
    expect(cards[0].querySelector('.rate-unavailable')).toBeTruthy();
    expect(cards[0].querySelector('a.earn-cta')?.getAttribute('href')).toBe('/3usd');
    expect(cards[1].querySelector('.rate-unavailable')).toBeFalsy();
    expect(cards[1].querySelector('a.earn-cta')?.getAttribute('href')).toBe('/stability-pool');
  });

  it('shows the SP card as unavailable when its data fetch rejects, independent of a healthy 3USD rate, with both CTAs still visible', async () => {
    fx.getPoolStatus.mockRejectedValue(new Error('stability pool status unavailable'));
    render();
    await settle();

    const cards = host.querySelectorAll('.earn-card');
    expect(cards[0].querySelector('.rate-unavailable')).toBeFalsy();
    expect(cards[0].textContent).toContain('4.50%');
    expect(cards[1].querySelector('.rate-unavailable')).toBeTruthy();
    expect(q('a[href="/3usd"].earn-cta')).toBeTruthy();
    expect(q('a[href="/stability-pool"].earn-cta')).toBeTruthy();
  });

  it('never renders a non-finite APY as a rate', async () => {
    fx.getThreePoolApy.mockResolvedValue({
      total_apy_pct: Infinity,
      interest_apr_pct: 0,
      swap_fee_apr_pct: 0,
      pool_tvl_icusd: 1,
      three_pool_share_bps: 5000,
      complete: true,
    });
    render();
    await settle();

    const cards = host.querySelectorAll('.earn-card');
    expect(cards[0].querySelector('.rate-unavailable')).toBeTruthy();
    expect(host.textContent).not.toContain('Infinity');
  });
});
