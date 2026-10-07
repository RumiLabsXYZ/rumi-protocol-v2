import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, tick, unmount } from 'svelte';

const mocks = vi.hoisted(() => {
  const subscribers = new Set<(value: any) => void>();
  const walletState = {
    isConnected: true,
    principal: { toText: () => 'aaaaa-aa' },
    tokenBalances: {
      ICUSD: { raw: 100_000_000_000n },
      CKUSDT: { raw: 100_000_000_000n },
      CKUSDC: { raw: 100_000_000_000n },
    },
  };
  return {
    walletStore: {
      subscribe(run: (value: any) => void) {
        subscribers.add(run);
        run(walletState);
        return () => subscribers.delete(run);
      },
    },
    threePoolService: {
      getPoolStatus: vi.fn(async () => ({
        balances: [70_000_000_000n, 150_000_000n, 150_000_000n],
        lp_total_supply: 1_000_000_000n,
        current_a: 100n,
        virtual_price: 100_000_000_000_000_000n,
        swap_fee_bps: 4n,
        admin_fee_bps: 5000n,
        tokens: [
          { decimals: 8 },
          { decimals: 6 },
          { decimals: 6 },
        ],
      })),
      calcAddLiquidity: vi.fn(async () => 100_000_000n),
      addLiquidity: vi.fn(async () => {
        throw new Error('Deposit quote is stale. Refresh the quote before opening wallet approvals.');
      }),
      captureLpSnapshot: vi.fn(async () => null),
      getLpBalance: vi.fn(async () => 0n),
      calcRemoveLiquidity: vi.fn(async () => [0n, 0n, 0n]),
      calcRemoveOneCoin: vi.fn(async () => 0n),
    },
    seasonStore: { ensureLoaded: vi.fn(), subscribe: (run: (value: boolean) => void) => { run(false); return () => {}; } },
    earningActive: { subscribe: (run: (value: boolean) => void) => { run(false); return () => {}; } },
    isOisyWallet: vi.fn(() => false),
  };
});

vi.mock('../../stores/wallet', () => ({ walletStore: mocks.walletStore }));
vi.mock('../../services/threePoolService', () => ({
  threePoolService: mocks.threePoolService,
  POOL_TOKENS: [
    { index: 0, symbol: 'icUSD', ledgerId: 'icUsd', decimals: 8, color: '#818cf8' },
    { index: 1, symbol: 'ckUSDT', ledgerId: 'ckUsdt', decimals: 6, color: '#26A17B' },
    { index: 2, symbol: 'ckUSDC', ledgerId: 'ckUsdc', decimals: 6, color: '#2775CA' },
  ],
  parseTokenAmount: (amount: string, decimals: number) => BigInt(Math.floor(Number(amount) * 10 ** decimals)),
  formatTokenAmount: (amount: bigint, decimals: number) => (Number(amount) / 10 ** decimals).toFixed(6),
}));
vi.mock('../../services/ledgerFeeService', () => ({
  fetchLedgerFee: vi.fn().mockResolvedValue(0n),
  getCachedLedgerFee: vi.fn(() => 0n),
}));
vi.mock('../../utils/format', () => ({ formatStableTokenDisplay: (amount: bigint, decimals: number) => (Number(amount) / 10 ** decimals).toFixed(2) }));
vi.mock('../../services/protocol/oisyResilience', () => ({ isOisyLandedSentinel: () => false }));
vi.mock('../../services/protocol/walletOperations', () => ({ isOisyWallet: mocks.isOisyWallet }));
vi.mock('../points/PointsCallout.svelte', () => ({ default: () => null }));
vi.mock('$lib/stores/seasonStore', () => ({ seasonStore: mocks.seasonStore, earningActive: mocks.earningActive }));
vi.mock('$lib/utils/pointsRules', () => ({
  compute3poolMultiplier: () => ({ nudge: '', headline: 0 }),
  threePoolHeadline: () => '',
}));

import LiquidityInterface from './LiquidityInterface.svelte';

describe('LiquidityInterface deposit concentration UX', () => {
  let host: HTMLDivElement;
  let instance: any;

  beforeEach(() => {
    host = document.createElement('div');
    document.body.appendChild(host);
    vi.clearAllMocks();
    mocks.threePoolService.calcAddLiquidity.mockImplementation(async () => 100_000_000n);
    mocks.threePoolService.addLiquidity.mockImplementation(async () => {
      throw new Error('Deposit quote is stale. Refresh the quote before opening wallet approvals.');
    });
  });

  afterEach(() => {
    if (instance) unmount(instance);
    host.remove();
  });

  async function settleQuote() {
    await new Promise((resolve) => setTimeout(resolve, 450));
    await tick();
    flushSync();
  }

  it('explains the missing stable amount and disables an invalid icUSD-only deposit', async () => {
    instance = mount(LiquidityInterface, { target: host });
    const inputs = host.querySelectorAll<HTMLInputElement>('.amount-input');
    inputs[0].value = '100';
    inputs[0].dispatchEvent(new Event('input', { bubbles: true }));
    flushSync();
    await settleQuote();

    expect(host.textContent).toContain('more stablecoin value');
    expect(host.textContent).toContain('Either ckUSDT or ckUSDC qualifies');
    expect(host.querySelector<HTMLButtonElement>('.submit-btn')?.disabled).toBe(true);
    expect(mocks.threePoolService.calcAddLiquidity).not.toHaveBeenCalled();
  });

  it('fills the exact rounded minimum pair and enables deposit after a valid quote', async () => {
    instance = mount(LiquidityInterface, { target: host });
    const inputs = host.querySelectorAll<HTMLInputElement>('.amount-input');
    inputs[0].value = '200';
    inputs[0].dispatchEvent(new Event('input', { bubbles: true }));
    flushSync();
    await settleQuote();

    const preset = Array.from(host.querySelectorAll('button')).find((button) => button.textContent?.includes('Add minimum stable')) as HTMLButtonElement;
    expect(preset.disabled).toBe(false);
    // The mounted fixture pool is already above cap, so the policy response
    // exposes the required stable side before the action is used.
    expect(host.textContent).toContain('more stablecoin value');
    preset.click();
    flushSync();
    await settleQuote();

    expect(inputs[0].value).toBe('200');
    expect(inputs[1].value).toBe('50.150151');
    expect(inputs[2].value).toBe('50.150150');
    expect(mocks.threePoolService.calcAddLiquidity).toHaveBeenCalledOnce();
    expect(host.querySelector<HTMLButtonElement>('.submit-btn')?.disabled).toBe(false);
  });

  it('keeps only the latest quote when replies resolve out of order', async () => {
    const quoteResolvers: Array<(value: bigint) => void> = [];
    mocks.threePoolService.calcAddLiquidity.mockImplementation(
      () => new Promise<bigint>((resolve) => quoteResolvers.push(resolve)),
    );

    instance = mount(LiquidityInterface, { target: host });
    const inputs = host.querySelectorAll<HTMLInputElement>('.amount-input');
    inputs[1].value = '1';
    inputs[1].dispatchEvent(new Event('input', { bubbles: true }));
    flushSync();
    await settleQuote();
    inputs[1].value = '2';
    inputs[1].dispatchEvent(new Event('input', { bubbles: true }));
    flushSync();
    await settleQuote();

    expect(quoteResolvers).toHaveLength(2);
    quoteResolvers[1](200_000_000n);
    await tick();
    flushSync();
    quoteResolvers[0](100_000_000n);
    await tick();
    flushSync();

    expect(host.textContent).toContain('2.00 3USD');
    expect(host.textContent).not.toContain('1.00 3USD');
    expect(host.querySelector<HTMLButtonElement>('.submit-btn')?.disabled).toBe(false);
  });

  it('offers a quote refresh after a stale preflight without changing amounts', async () => {
    instance = mount(LiquidityInterface, { target: host });
    const inputs = host.querySelectorAll<HTMLInputElement>('.amount-input');
    inputs[0].value = '200';
    inputs[0].dispatchEvent(new Event('input', { bubbles: true }));
    flushSync();
    await settleQuote();

    const fill = Array.from(host.querySelectorAll('button')).find((button) => button.textContent?.includes('Add minimum stable')) as HTMLButtonElement;
    fill.click();
    flushSync();
    await settleQuote();

    host.querySelector<HTMLButtonElement>('.submit-btn')?.click();
    await tick();
    flushSync();
    expect(host.textContent).toContain('quote is stale');
    expect(mocks.threePoolService.addLiquidity).toHaveBeenCalledOnce();

    const quoteCallsBeforeRefresh = mocks.threePoolService.calcAddLiquidity.mock.calls.length;
    const refresh = Array.from(host.querySelectorAll('button')).find((button) => button.textContent?.includes('Refresh quote')) as HTMLButtonElement;
    expect(refresh).toBeTruthy();
    refresh.click();
    await settleQuote();

    expect(mocks.threePoolService.calcAddLiquidity.mock.calls.length).toBe(quoteCallsBeforeRefresh + 1);
    expect(inputs[0].value).toBe('200');
    expect(inputs[1].value).toBe('50.150151');
    expect(inputs[2].value).toBe('50.150150');
  });
});
