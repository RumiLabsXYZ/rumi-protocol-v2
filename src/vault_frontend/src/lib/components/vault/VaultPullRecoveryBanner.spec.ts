import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { mount, tick, unmount } from 'svelte';
import { Principal } from '@dfinity/principal';

const mocks = vi.hoisted(() => ({ getSubmitted: vi.fn(), openChannel: vi.fn() }));
vi.mock('$lib/services/protocol/apiClient', () => ({ ApiClient: {
  getSubmittedVaultCollateralPull: mocks.getSubmitted,
  recoverSubmittedVaultCollateralPull: vi.fn(),
} }));
vi.mock('$lib/services/oisySigner', () => ({ openOisySignerChannel: mocks.openChannel }));
vi.mock('$lib/stores/vaultStore', () => ({ vaultStore: { refreshVaults: vi.fn() } }));
vi.mock('$lib/stores/toast', () => ({ toastStore: { success: vi.fn(), error: vi.fn() } }));
vi.mock('$lib/services/auth', async () => {
  const { writable } = await import('svelte/store');
  return { currentWalletType: writable('oisy'), walletSessionGeneration: writable(0), WALLET_TYPES: { OISY: 'oisy' } };
});
vi.mock('$lib/stores/wallet', async () => {
  const { writable } = await import('svelte/store');
  return { walletStore: Object.assign(writable({ isConnected: true, loading: false, principal: null }), { refreshBalance: vi.fn() }) };
});
import Banner from './VaultPullRecoveryBanner.svelte';
import { walletStore } from '$lib/stores/wallet';
import { currentWalletType, walletSessionGeneration } from '$lib/services/auth';
import { VAULT_PULL_STATUS_EVENT } from '$lib/services/protocol/vaultOperationJournal';
let component: ReturnType<typeof mount>;
let target: HTMLElement;
const owner = Principal.selfAuthenticating(new Uint8Array([1, 2, 3]));

beforeEach(() => {
  mocks.getSubmitted.mockReset().mockResolvedValue(null);
  mocks.openChannel.mockReset().mockResolvedValue(undefined);
  currentWalletType.set('oisy');
  walletSessionGeneration.set(0);
  walletStore.set({ isConnected: true, loading: false, principal: owner } as any);
  target = document.createElement('div'); document.body.append(target);
});
afterEach(async () => { if (component) await unmount(component); target.remove(); });

describe('Oisy vault recovery status consent', () => {
  it('does not ask the signer on mount or background notifications; an explicit click performs the read', async () => {
    component = mount(Banner, { target }); await tick();
    expect(mocks.getSubmitted).not.toHaveBeenCalled();
    window.dispatchEvent(new CustomEvent(VAULT_PULL_STATUS_EVENT, { detail: { owner: owner.toText() } }));
    await tick(); expect(mocks.getSubmitted).not.toHaveBeenCalled();
    const button = target.querySelector('button')!;
    expect(button.textContent).toContain('Check pending vault action');
    button.click(); await vi.waitFor(() => expect(mocks.getSubmitted).toHaveBeenCalledTimes(1)); await tick();
    expect(mocks.openChannel).toHaveBeenCalledWith(owner);
    expect(target.textContent).not.toContain('Check pending vault action');
  });

  it('does not read a replacement wallet after waiting for the signer channel', async () => {
    let opened!: () => void;
    mocks.openChannel.mockImplementation(() => new Promise<void>(resolve => { opened = resolve; }));
    component = mount(Banner, { target }); await tick();
    target.querySelector('button')!.click();
    walletStore.set({ isConnected: true, loading: false, principal: Principal.selfAuthenticating(new Uint8Array([4, 5, 6])) } as any);
    await tick(); opened(); await tick();
    expect(mocks.getSubmitted).not.toHaveBeenCalled();
  });

  it('does not read after a same-owner signer session transition', async () => {
    let opened!: () => void;
    mocks.openChannel.mockImplementation(() => new Promise<void>(resolve => { opened = resolve; }));
    component = mount(Banner, { target }); await tick();
    target.querySelector('button')!.click();
    walletSessionGeneration.set(1);
    await tick(); opened(); await tick();
    expect(mocks.getSubmitted).not.toHaveBeenCalled();
  });

  it('keeps silent recovery discovery for delegation wallets', async () => {
    currentWalletType.set('internet-identity');
    component = mount(Banner, { target });
    await vi.waitFor(() => expect(mocks.getSubmitted).toHaveBeenCalledTimes(1));
    expect(mocks.openChannel).not.toHaveBeenCalled();
  });
});
