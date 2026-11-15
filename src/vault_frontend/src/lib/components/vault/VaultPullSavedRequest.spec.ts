import type { Writable } from 'svelte/store';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { mount, tick, unmount } from 'svelte';
import { Principal } from '@dfinity/principal';

const mocks = vi.hoisted(() => ({ inspect: vi.fn(), resume: vi.fn(), cancel: vi.fn(), refresh: vi.fn() }));
vi.mock('$lib/services/protocol/apiClient', () => ({ ApiClient: {
  getVaultPullRecovery: mocks.inspect,
  getSubmittedVaultCollateralPull: vi.fn().mockResolvedValue(null),
  resumeSavedVaultCollateralPull: mocks.resume,
  cancelSavedVaultCollateralPull: mocks.cancel,
  recoverSubmittedVaultCollateralPull: vi.fn(),
} }));
vi.mock('$lib/services/oisySigner', () => ({ openOisySignerChannel: vi.fn() }));
vi.mock('$lib/stores/vaultStore', async () => {
  const { writable } = await import('svelte/store');
  return { vaultStore: Object.assign(writable({ vaults: [] }), { refreshVaults: mocks.refresh }) };
});
vi.mock('$lib/stores/collateralStore', async () => {
  const { writable } = await import('svelte/store');
  return { collateralStore: Object.assign(writable({ collaterals: [] }), {
    getCollateralInfo: () => ({ symbol: 'ckSOL', decimals: 9 }),
  }) };
});
vi.mock('$lib/stores/toast', () => ({ toastStore: { success: vi.fn(), error: vi.fn(), info: vi.fn() } }));
vi.mock('$lib/services/auth', async () => {
  const { writable } = await import('svelte/store');
  return { currentWalletType: writable('internet-identity'), walletSessionGeneration: writable(0), WALLET_TYPES: { OISY: 'oisy' } };
});
vi.mock('$lib/stores/wallet', async () => {
  const { writable } = await import('svelte/store');
  return { walletStore: Object.assign(writable({ isConnected: true, loading: false, principal: null }), { refreshBalance: vi.fn() }) };
});
import Banner from './VaultPullRecoveryBanner.svelte';
import { openOisySignerChannel } from '$lib/services/oisySigner';
import { walletStore } from '$lib/stores/wallet';
const mockWalletStore = walletStore as unknown as Writable<{ isConnected: boolean; loading: boolean; principal: Principal | null }>;
import { currentWalletType, walletSessionGeneration } from '$lib/services/auth';

const owner = Principal.selfAuthenticating(new Uint8Array([7, 8, 9]));
const intent = { version: 1 as const, owner: owner.toText(), operationId: '1', savedAt: 10,
  request: { OpenVaultAndBorrow: { amount_e8s: 30_000_000n, borrow_amount_e8s: 192_000_000n, collateral_type: 'ls5lp-lqaaa-aaaar-qb5oa-cai' } } };
let component: ReturnType<typeof mount>;
let target: HTMLElement;
const button = (label: string) => Array.from(target.querySelectorAll('button')).find(b => b.textContent?.trim() === label)!;
beforeEach(() => {
  vi.resetAllMocks();
  mocks.inspect.mockResolvedValue({ intent, status: { active: [], acknowledged_through: 0n } });
  mocks.resume.mockResolvedValue({ success: true, vaultId: 42 });
  mocks.cancel.mockResolvedValue(1n);
  mocks.refresh.mockResolvedValue(undefined);
  currentWalletType.set('internet-identity'); walletSessionGeneration.set(0);
  mockWalletStore.set({ isConnected: true, loading: false, principal: owner } as never);
  target = document.createElement('div'); document.body.append(target);
});
afterEach(async () => { if (component) await unmount(component); target.remove(); });

describe('saved vault request recovery', () => {
  it('shows exact ckSOL and borrow amounts, then restores without approval or dispatch', async () => {
    component = mount(Banner, { target });
    await vi.waitFor(() => expect(target.textContent).toContain('0.03 ckSOL'));
    expect(target.textContent).toContain('1.92 icUSD');
    expect(mocks.resume).not.toHaveBeenCalled();
    button('Restore saved request').click(); await tick();
    expect(button('Continue saved request')).toBeTruthy();
    expect(mocks.resume).not.toHaveBeenCalled();
    button('Continue saved request').click();
    await vi.waitFor(() => expect(mocks.resume).toHaveBeenCalledWith(intent));
  });

  it('preserves the full nine-decimal ckSOL amount when restoring and continuing', async () => {
    const precise = { ...intent, request: { OpenVaultAndBorrow: {
      ...intent.request.OpenVaultAndBorrow, amount_e8s: 30_571_620n, borrow_amount_e8s: 192_000_001n,
    } } };
    mocks.inspect.mockResolvedValue({ intent: precise, status: { active: [], acknowledged_through: 0n } });
    component = mount(Banner, { target });
    await vi.waitFor(() => expect(target.textContent).toContain('0.03057162 ckSOL'));
    expect(target.textContent).toContain('1.92000001 icUSD');
    button('Restore saved request').click(); await tick();
    expect(mocks.resume).not.toHaveBeenCalled();
    button('Continue saved request').click();
    await vi.waitFor(() => expect(mocks.resume).toHaveBeenCalledWith(precise));
  });

  it('clears the previous retirement notice when a different saved request arrives', async () => {
    mocks.cancel.mockImplementationOnce(async () => {
      mocks.inspect.mockResolvedValue({ intent: null, status: { active: [], acknowledged_through: 1n } });
      return 1n;
    });
    component = mount(Banner, { target });
    await vi.waitFor(() => expect(button('Cancel saved request')).toBeTruthy());
    button('Cancel saved request').click(); await tick();
    button('Confirm cancellation').click();
    await vi.waitFor(() => expect(target.textContent).toContain('The request is retired.'));
    await vi.waitFor(() => expect(button('Refresh status').disabled).toBe(false));
    const next = { ...intent, operationId: '2', savedAt: 20 };
    mocks.inspect.mockResolvedValue({ intent: next, status: { active: [], acknowledged_through: 1n } });
    button('Refresh status').click();
    await vi.waitFor(() => expect(button('Restore saved request')).toBeTruthy());
    expect(target.textContent).not.toContain('The request is retired.');
    expect(target.textContent).not.toContain('Saved request completed.');
  });

  it('requires a separate confirmation for cancellation and does not dispatch a loan', async () => {
    component = mount(Banner, { target });
    await vi.waitFor(() => expect(button('Cancel saved request')).toBeTruthy());
    button('Cancel saved request').click(); await tick();
    expect(mocks.cancel).not.toHaveBeenCalled();
    button('Confirm cancellation').click();
    await vi.waitFor(() => expect(mocks.cancel).toHaveBeenCalledWith(intent));
    expect(mocks.resume).not.toHaveBeenCalled();
  });
});


it('opens read-only recovery from the form action without continuing or cancelling', async () => {
  component = mount(Banner, {target});
  await vi.waitFor(() => expect(button('Restore saved request')).toBeTruthy());
  mocks.inspect.mockClear();
  window.dispatchEvent(new CustomEvent('rumi:vault-pull-recovery-review', {detail:{owner: owner.toText()}}));
  await vi.waitFor(() => expect(mocks.inspect).toHaveBeenCalledOnce());
  expect(mocks.resume).not.toHaveBeenCalled(); expect(mocks.cancel).not.toHaveBeenCalled();
  await vi.waitFor(() => expect(document.activeElement?.id).toBe('vault-pull-recovery'));
});

it('does not inspect another owner when a form recovery action arrives', async () => {
  component = mount(Banner, {target});
  await vi.waitFor(() => expect(button('Restore saved request')).toBeTruthy());
  mocks.inspect.mockClear();
  window.dispatchEvent(new CustomEvent('rumi:vault-pull-recovery-review', {detail:{owner:'2vxsx-fae'}}));
  await tick(); expect(mocks.inspect).not.toHaveBeenCalled();
});


it('keeps the OISY read explicit and deduplicates recovery clicks without financial actions', async () => {
  currentWalletType.set('oisy');
  let resolve!: (value: unknown) => void;
  mocks.inspect.mockImplementationOnce(() => new Promise(done => {resolve=done;}));
  component=mount(Banner,{target}); await tick();
  expect(mocks.inspect).not.toHaveBeenCalled();
  const event=()=>new CustomEvent('rumi:vault-pull-recovery-review',{detail:{owner:owner.toText()}});
  window.dispatchEvent(event());
  expect(openOisySignerChannel).toHaveBeenCalledWith(owner);
  await vi.waitFor(()=>expect(mocks.inspect).toHaveBeenCalledOnce());
  window.dispatchEvent(event()); await tick();
  expect(mocks.inspect).toHaveBeenCalledOnce();
  resolve({intent,status:{active:[],acknowledged_through:0n}});
  await vi.waitFor(()=>expect(button('Restore saved request')).toBeTruthy());
  expect(mocks.resume).not.toHaveBeenCalled(); expect(mocks.cancel).not.toHaveBeenCalled();
});

it('discards a delayed read and focus after the owner or signer session changes', async () => {
  currentWalletType.set('oisy');
  let resolve!: (value: unknown) => void;
  mocks.inspect.mockImplementationOnce(()=>new Promise(done=>{resolve=done;}));
  component=mount(Banner,{target}); await tick();
  window.dispatchEvent(new CustomEvent('rumi:vault-pull-recovery-review',{detail:{owner:owner.toText()}}));
  await vi.waitFor(()=>expect(mocks.inspect).toHaveBeenCalledOnce());
  walletSessionGeneration.set(1); await tick();
  resolve({intent,status:{active:[],acknowledged_through:0n}}); await tick(); await tick();
  expect(button('Restore saved request')).toBeUndefined();
  expect(target.textContent).not.toContain('0.03 ckSOL');
  expect(mocks.resume).not.toHaveBeenCalled(); expect(mocks.cancel).not.toHaveBeenCalled();
});

it('shows a truthful empty-read result without inviting an automatic replacement', async () => {
  component=mount(Banner,{target}); await vi.waitFor(()=>expect(button('Restore saved request')).toBeTruthy());
  mocks.inspect.mockResolvedValue({intent:null,status:{active:[],acknowledged_through:1n}});
  window.dispatchEvent(new CustomEvent('rumi:vault-pull-recovery-review',{detail:{owner:owner.toText()}}));
  await vi.waitFor(()=>expect(target.textContent).toContain('No saved request is pending'));
  expect(mocks.resume).not.toHaveBeenCalled(); expect(mocks.cancel).not.toHaveBeenCalled();
});

it('keeps a submitted transfer in receipt recovery instead of offering cancellation', async () => {
  component=mount(Banner,{target}); await vi.waitFor(()=>expect(button('Restore saved request')).toBeTruthy());
  mocks.inspect.mockResolvedValue({intent,status:{active:[{operation_id:1n,request:intent.request,vault_id:0n,phase:{Submitted:null},confirmed_wide_block_index:[]}],acknowledged_through:0n}});
  window.dispatchEvent(new CustomEvent('rumi:vault-pull-recovery-review',{detail:{owner:owner.toText()}}));
  await vi.waitFor(()=>expect(button('Verify block and resume')).toBeTruthy());
  expect(button('Cancel saved request')).toBeUndefined(); expect(button('Continue saved request')).toBeUndefined();
  expect(mocks.resume).not.toHaveBeenCalled(); expect(mocks.cancel).not.toHaveBeenCalled();
});


it('retains the empty-read notice after repeated review clicks during the same read', async () => {
  component=mount(Banner,{target}); await vi.waitFor(()=>expect(button('Restore saved request')).toBeTruthy());
  let resolve!: (value: unknown) => void;
  mocks.inspect.mockImplementationOnce(()=>new Promise(done=>{resolve=done;}));
  const review=()=>window.dispatchEvent(new CustomEvent('rumi:vault-pull-recovery-review',{detail:{owner:owner.toText()}}));
  review(); await tick(); review(); await tick();
  resolve({intent:null,status:{active:[],acknowledged_through:1n}});
  await vi.waitFor(()=>expect(target.textContent).toContain('No saved request is pending'));
  expect(mocks.resume).not.toHaveBeenCalled(); expect(mocks.cancel).not.toHaveBeenCalled();
});


it('joins a background status read when the form opens recovery instead of contending with itself', async () => {
  let resolve!: (value: unknown) => void;
  mocks.inspect.mockImplementationOnce(()=>new Promise(done=>{resolve=done;})).mockRejectedValueOnce(Error('Another tab is already handling this wallet vault operation.'));
  component=mount(Banner,{target}); await vi.waitFor(()=>expect(mocks.inspect).toHaveBeenCalledOnce());
  window.dispatchEvent(new CustomEvent('rumi:vault-pull-recovery-review',{detail:{owner:owner.toText()}}));
  await tick(); await tick(); resolve({intent,status:{active:[],acknowledged_through:0n}});
  await vi.waitFor(()=>expect(button('Restore saved request')).toBeTruthy());
  expect(mocks.inspect).toHaveBeenCalledOnce(); expect(target.textContent).not.toContain('Another tab');
  expect(mocks.resume).not.toHaveBeenCalled(); expect(mocks.cancel).not.toHaveBeenCalled();
});

it('does not share or publish a pending read across owner changes', async () => {
  let resolve!: (value: unknown) => void;
  mocks.inspect.mockImplementationOnce(()=>new Promise(done=>{resolve=done;})).mockResolvedValueOnce({intent:null,status:{active:[],acknowledged_through:0n}});
  component=mount(Banner,{target}); await vi.waitFor(()=>expect(mocks.inspect).toHaveBeenCalledOnce());
  mockWalletStore.set({isConnected:true,loading:false,principal:Principal.fromText('rrkah-fqaaa-aaaaa-aaaaq-cai')}); await tick();
  await vi.waitFor(()=>expect(mocks.inspect).toHaveBeenCalledTimes(2));
  resolve({intent,status:{active:[],acknowledged_through:0n}}); await tick(); await tick();
  expect(button('Restore saved request')).toBeUndefined(); expect(target.textContent).not.toContain('0.03 ckSOL');
});
