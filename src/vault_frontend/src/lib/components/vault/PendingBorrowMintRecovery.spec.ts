import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';

const fx = vi.hoisted(() => ({
  getPending: vi.fn(),
  retryPending: vi.fn(),
  reconcilePending: vi.fn(),
  principal: { toText: () => 'owner-principal' },
  wallet: { isConnected: true, principal: { toText: () => 'owner-principal' } },
  generation: 4,
  walletType: 'plug',
  setGeneration: (_value: number): void => {},
}));

vi.mock('$lib/stores/wallet', async () => {
  const { writable } = await import('svelte/store');
  return { walletStore: writable(fx.wallet) };
});

vi.mock('$lib/services/auth', async () => {
  const { writable } = await import('svelte/store');
  const generation = writable(fx.generation);
  fx.setGeneration = generation.set;
  return {
    walletSessionGeneration: generation,
    currentWalletType: writable(fx.walletType),
  };
});

vi.mock('$lib/services/protocol', () => ({
  protocolService: {
    getMyPendingBorrowMintsBound: fx.getPending,
    retryPendingBorrowMintBound: fx.retryPending,
    reconcilePendingBorrowMintFromBlockBound: fx.reconcilePending,
  },
}));

import PendingBorrowMintRecovery from './PendingBorrowMintRecovery.svelte';

describe('PendingBorrowMintRecovery', () => {
  let target: HTMLDivElement;
  let component: ReturnType<typeof mount> | null = null;

  beforeEach(() => {
    target = document.createElement('div');
    document.body.appendChild(target);
    fx.getPending.mockReset().mockResolvedValue([{
      vault_id: 77n,
      borrowed_amount_e8s: 50_000_001n,
      phase: { SubmittedOrUnknown: null },
    }]);
    fx.retryPending.mockReset().mockResolvedValue({
      kind: 'dispatched_ok',
      vaultId: 77,
      blockIndex: 90,
      feePaidRaw: 0n,
      errorMessage: null,
      submittedIcusdRaw: 50_000_001n,
    });
  });

  afterEach(() => {
    if (component) unmount(component);
    component = null;
    target.remove();
  });

  it('discovers a lost-response journal and retries the original raw amount despite changed live CR', async () => {
    component = mount(PendingBorrowMintRecovery, { target, props: { principalText: 'owner-principal' } });
    await vi.waitFor(() => expect(target.textContent).toContain('0.50000001 icUSD'));
    expect(target.textContent).toContain('outcome is unknown');
    expect(fx.getPending.mock.calls[0][0].expectedPrincipalText).toBe('owner-principal');
    expect(fx.getPending.mock.calls[0][0].assertCurrent()).toBe(true);

    // A changed calculator/CR is intentionally irrelevant to this recovery UI:
    // it calls the backend with the raw amount and vault returned by the journal.
    target.querySelector('button')!.click();
    flushSync();
    await vi.waitFor(() => expect(fx.retryPending).toHaveBeenCalledTimes(1));
    expect(fx.retryPending.mock.calls[0][1]).toBe(77n);
    expect(fx.retryPending.mock.calls[0][2]).toBe(50_000_001n);
    await vi.waitFor(() => expect(fx.getPending).toHaveBeenCalledTimes(2));
  });

  it('does not expose a pending result after an A to B to A wallet-session transition', async () => {
    let resolvePending!: (rows: unknown[]) => void;
    fx.getPending.mockReturnValueOnce(new Promise((resolve) => { resolvePending = resolve; }));
    component = mount(PendingBorrowMintRecovery, { target, props: { principalText: 'owner-principal' } });
    await vi.waitFor(() => expect(fx.getPending).toHaveBeenCalledTimes(1));

    // The principal returns to A, but the generation proves this is a new session.
    fx.setGeneration(6);
    expect(fx.getPending.mock.calls[0][0].assertCurrent()).toBe(false);
    resolvePending([{
      vault_id: 77n,
      borrowed_amount_e8s: 50_000_001n,
      phase: { SubmittedOrUnknown: null },
    }]);
    await Promise.resolve();
    flushSync();
    expect(target.textContent).not.toContain('0.50000001 icUSD');
  });

  it('uses a caller-supplied candidate block only for TooOld receipt recovery', async () => {
    fx.getPending.mockReset().mockResolvedValue([{
      vault_id: 77n,
      borrowed_amount_e8s: 50_000_001n,
      phase: { ReceiptRecoveryRequired: null },
    }]);
    fx.reconcilePending.mockReset().mockResolvedValue({
      kind: 'dispatched_ok', vaultId: 77, blockIndex: 90, feePaidRaw: 0n,
      errorMessage: null, submittedIcusdRaw: 50_000_001n,
    });
    component = mount(PendingBorrowMintRecovery, { target, props: { principalText: 'owner-principal' } });
    await vi.waitFor(() => expect(target.textContent).toContain('No further mint will be sent'));
    const input = target.querySelector('input') as HTMLInputElement;
    input.value = '9007199254740993';
    input.dispatchEvent(new Event('input', { bubbles: true }));
    target.querySelector('button')!.click();
    flushSync();
    await vi.waitFor(() => expect(fx.reconcilePending).toHaveBeenCalledTimes(1));
    expect(fx.reconcilePending.mock.calls[0][1]).toBe(77n);
    expect(fx.reconcilePending.mock.calls[0][2]).toBe(9007199254740993n);
    expect(fx.reconcilePending.mock.calls[0][3]).toBe(50_000_001n);
    expect(fx.retryPending).not.toHaveBeenCalled();
  });

});
