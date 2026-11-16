import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { mount, tick, unmount } from 'svelte';
import { Principal } from '@dfinity/principal';

const fx = vi.hoisted(() => ({ inspect: vi.fn(), channel: vi.fn(), resume: vi.fn(), cancel: vi.fn(), transfer: vi.fn() }));
vi.mock('$lib/services/pnp', () => ({ initializePNP: vi.fn(), pnp: { getEnabledWallets: () => [] } }));
vi.mock('$lib/services/PermissionManager', () => ({ permissionManager: {} }));
vi.mock('$lib/services/tokenService', () => ({ TokenService: { fetchTokenLogo: vi.fn().mockResolvedValue(null) } }));
vi.mock('$lib/services/transferService', () => ({ transferICRC1: fx.transfer, queryICRC1Fee: vi.fn(), isValidPrincipal: vi.fn() }));
vi.mock('$lib/services/threePoolService', () => ({ threePoolService: { getLpBalance: vi.fn().mockResolvedValue(0n) }, formatTokenAmount: () => '0' }));
vi.mock('$lib/services/oisySigner', () => ({ openOisySignerChannel: fx.channel }));
vi.mock('$lib/services/protocol/apiClient', () => ({ ApiClient: {
  getVaultPullRecovery: fx.inspect, resumeSavedVaultCollateralPull: fx.resume, cancelSavedVaultCollateralPull: fx.cancel,
} }));
vi.mock('$lib/stores/wallet', async () => {
  const { writable } = await import('svelte/store');
  return { walletStore: Object.assign(writable({ isConnected: true, loading: false, principal: null, tokenBalances: {} }), { refreshBalance: vi.fn().mockResolvedValue(undefined) }) };
});
vi.mock('$lib/services/auth', async () => {
  const { writable } = await import('svelte/store');
  return { currentWalletType: writable('oisy'), walletSessionGeneration: writable(0), WALLET_TYPES: { OISY: 'oisy', INTERNET_IDENTITY: 'internet-identity' } };
});
vi.mock('$lib/stores/vaultStore', async () => {
  const { writable } = await import('svelte/store');
  return { vaultStore: Object.assign(writable({ vaults: [] }), { refreshVaults: vi.fn() }) };
});
vi.mock('$lib/stores/collateralStore', async () => {
  const { writable } = await import('svelte/store');
  return { collateralStore: Object.assign(writable({ collaterals: [] }), { getCollateralInfo: () => ({ symbol: 'ckSOL', decimals: 9 }) }) };
});
vi.mock('$lib/stores/toast', () => ({ toastStore: { success: vi.fn(), error: vi.fn(), info: vi.fn() } }));

import Connector from './WalletConnector.svelte';
import Banner from '../vault/VaultPullRecoveryBanner.svelte';
import { walletStore } from '$lib/stores/wallet';
import { currentWalletType } from '$lib/services/auth';
const owner = Principal.selfAuthenticating(new Uint8Array([1, 2, 3]));
let target: HTMLElement;
let instances: ReturnType<typeof mount>[];
const button = (label: string) => [...target.querySelectorAll('button')].find(b => b.textContent?.trim() === label)!;
beforeEach(() => {
  vi.clearAllMocks();
  fx.channel.mockResolvedValue(undefined);
  fx.inspect.mockResolvedValue({ intent: null, status: { active: [], acknowledged_through: 0n } });
  currentWalletType.set('oisy');
  walletStore.set({ isConnected: true, loading: false, principal: owner, tokenBalances: {} } as never);
  target = document.createElement('div'); document.body.append(target); instances = [];
});
afterEach(async () => { for (const instance of instances) await unmount(instance); target.remove(); });

it('keeps unchecked recovery in the wallet menu and reads only on its explicit action', async () => {
  instances.push(mount(Connector, { target }), mount(Banner, { target })); await tick();
  expect(target.querySelector('#vault-pull-recovery section')).toBeNull();
  expect(button('Recover a vault action')).toBeUndefined();
  expect(fx.channel).not.toHaveBeenCalled(); expect(fx.inspect).not.toHaveBeenCalled();
  (target.querySelector('#wallet-button') as HTMLButtonElement).click(); await tick();
  expect(button('Recover a vault action')).toBeTruthy();
  expect(fx.channel).not.toHaveBeenCalled(); expect(fx.inspect).not.toHaveBeenCalled();
  button('Recover a vault action').click();
  // Dispatch stays synchronous with the wallet-menu click for popup ownership.
  expect(fx.channel).toHaveBeenCalledWith(owner);
  await vi.waitFor(() => expect(fx.inspect).toHaveBeenCalledOnce()); await tick();
  expect(target.querySelector('#wallet-dialog')).toBeNull();
  expect(target.textContent).toContain('No saved request is pending');
  expect(fx.resume).not.toHaveBeenCalled(); expect(fx.cancel).not.toHaveBeenCalled(); expect(fx.transfer).not.toHaveBeenCalled();
});

it('discovers a submitted action from another browser without allowing a replacement or cancellation', async () => {
  fx.inspect.mockResolvedValue({ intent: null, status: { active: [{ operation_id: 7n, request: { AddMargin: { vault_id: 213n, amount_e8s: 100n } }, phase: { Submitted: null }, confirmed_wide_block_index: [] }], acknowledged_through: 0n } });
  instances.push(mount(Connector, { target }), mount(Banner, { target })); await tick();
  (target.querySelector('#wallet-button') as HTMLButtonElement).click(); await tick();
  button('Recover a vault action').click();
  await vi.waitFor(() => expect(target.textContent).toContain('unknown collateral transfer result'));
  expect(button('Verify block and resume')).toBeTruthy();
  expect(button('Cancel saved request')).toBeUndefined();
  expect(fx.resume).not.toHaveBeenCalled(); expect(fx.cancel).not.toHaveBeenCalled(); expect(fx.transfer).not.toHaveBeenCalled();
});
