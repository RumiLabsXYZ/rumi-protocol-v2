import { mount, tick, unmount } from 'svelte';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import MyVaults from '../../../routes/vaults/+page.svelte';

const mocks = vi.hoisted(() => ({
  read: vi.fn(),
  settle: vi.fn(),
  context: { expectedPrincipalText: 'aaaaa-aa', assertCurrent: () => true },
}));

vi.mock('$lib/services/xrpVaultService', () => ({
  XrpVaultService: { readMyClaims: mocks.read, settleXrpClaim: mocks.settle },
}));
vi.mock('$lib/services/protocol/walletOperations', () => ({
  captureActionBoundContext: () => mocks.context,
}));
vi.mock('$lib/stores/wallet', async () => {
  const { writable } = await import('svelte/store');
  const { Principal } = await import('@dfinity/principal');
  const owner = Principal.fromText('aaaaa-aa');
  return {
    walletStore: writable({ isConnected: true, loading: false, principal: owner }),
    isConnected: writable(true),
    principal: writable(owner),
  };
});
vi.mock('$lib/services/auth', async () => {
  const { writable } = await import('svelte/store');
  return { walletSessionGeneration: writable(1), currentWalletType: writable('internet-identity'), WALLET_TYPES: { OISY: 'oisy' } };
});
vi.mock('$lib/stores/appDataStore', async () => {
  const { writable } = await import('svelte/store');
  return {
    userVaults: writable([]),
    isLoadingVaults: writable(false),
    appDataStore: {
      fetchProtocolStatus: async () => null,
      fetchCollateralTotals: async () => {},
      fetchUserVaults: async () => {},
    },
  };
});
vi.mock('$lib/stores/collateralStore', async () => {
  const { writable } = await import('svelte/store');
  return { collateralStore: Object.assign(writable({ collaterals: [] }), { fetchSupportedCollateral: async () => {} }) };
});
vi.mock('$lib/stores/permissionStore', async () => {
  const { writable } = await import('svelte/store');
  return { permissionStore: writable({ initialized: false }) };
});
vi.mock('$lib/stores/developer', async () => {
  const { writable } = await import('svelte/store');
  return { developerAccess: writable(false) };
});
vi.mock('$lib/config', () => ({ isDevelopment: false, CANISTER_IDS: {} }));
vi.mock('$lib/components/vault/VaultCard.svelte', () => ({ default: vi.fn() }));
vi.mock('$lib/components/vault/XrplIcusdPanel.svelte', () => ({ default: vi.fn() }));

const claim = {
  claimId: '20', vaultId: 208, custodyNonce: 208, drops: 275174474n, xrp: 275.174474,
  inFlight: false, inFlightTxHash: null, quarantineReason: null, createdAtMs: 0,
};
const address = 'rPEPPER7kfTD9w2To4CQk6UCfuHM9c6GDY';
let target: HTMLElement;
let page: ReturnType<typeof mount>;

function button(name: string) {
  return Array.from(target.querySelectorAll('button')).find(button => button.textContent?.trim() === name);
}

async function checkClaims() {
  await vi.waitFor(() => expect(button('Check XRP claims')?.disabled).toBe(false));
  button('Check XRP claims')!.click();
  await tick();
}

async function fill(input: HTMLInputElement, value: string) {
  input.value = value;
  input.dispatchEvent(new Event('input', { bubbles: true }));
  await tick();
}

beforeEach(async () => {
  mocks.read.mockReset().mockResolvedValue([claim]);
  mocks.settle.mockReset().mockResolvedValue({ success: true, data: { txHash: 'A'.repeat(64) } });
  target = document.createElement('main');
  document.body.append(target);
  page = mount(MyVaults, { target });
  await tick();
});
afterEach(async () => {
  await unmount(page);
  target.remove();
});

describe('XRP withdrawal claims on My Vaults', () => {
  it('offers claim recovery even when there are no active vaults and never submits on lookup', async () => {
    expect(target.textContent).toContain('No vaults yet');
    expect(mocks.read).not.toHaveBeenCalled();
    await checkClaims();
    await vi.waitFor(() => expect(target.textContent).toContain('275.174474'));
    expect(target.textContent).toContain('#20');
    expect(mocks.settle).not.toHaveBeenCalled();
  });

  it('rejects an invalid tag, submits the entered address and zero tag, and confirms the same claim', async () => {
    await checkClaims();
    await vi.waitFor(() => expect(button('Settle claim')).toBeDefined());
    const [destination, tag] = target.querySelectorAll<HTMLInputElement>('input');
    await fill(destination, address);
    await fill(tag, '4294967296');
    button('Settle claim')!.click();
    await tick();
    expect(mocks.settle).not.toHaveBeenCalled();
    await fill(tag, '0');
    button('Settle claim')!.click();
    await vi.waitFor(() => expect(mocks.settle).toHaveBeenCalledWith('20', address, 0, mocks.context));

    await vi.waitFor(() => expect(button('Settle claim')).toBeUndefined());
    expect(mocks.settle).toHaveBeenCalledOnce();
    mocks.read.mockResolvedValue([{ ...claim, inFlight: true, inFlightTxHash: 'A'.repeat(64) }]);
    await checkClaims();
    await vi.waitFor(() => expect(button('Confirm settlement')).toBeDefined());
    expect(button('Settle claim')).toBeUndefined();
    button('Confirm settlement')!.click();
    await vi.waitFor(() => expect(mocks.settle).toHaveBeenLastCalledWith('20', '', undefined, mocks.context));

    await vi.waitFor(() => expect(button('Confirm settlement')).toBeUndefined());
    mocks.read.mockResolvedValue([]);
    await checkClaims();
    await vi.waitFor(() => expect(target.textContent).not.toContain('275.174474'));
    expect(mocks.settle).toHaveBeenCalledTimes(2);
  });

  it('requires a fresh read after a lost response before another payout action', async () => {
    mocks.settle.mockRejectedValueOnce(new Error('Wallet response lost'));
    await checkClaims();
    await vi.waitFor(() => expect(button('Settle claim')).toBeDefined());
    await fill(target.querySelector('input')!, address);
    button('Settle claim')!.click();
    await vi.waitFor(() => expect(target.textContent).toContain('Wallet response lost'));
    expect(button('Settle claim')).toBeUndefined();
    expect(button('Confirm settlement')).toBeUndefined();
    expect(mocks.settle).toHaveBeenCalledWith('20', address, undefined, mocks.context);

    mocks.read.mockResolvedValue([{ ...claim, inFlight: true, inFlightTxHash: 'B'.repeat(64) }]);
    await checkClaims();
    await vi.waitFor(() => expect(target.textContent).toContain('B'.repeat(64)));
    expect(button('Confirm settlement')).toBeDefined();
    expect(target.querySelector('input')).toBeNull();
    expect(mocks.settle).toHaveBeenCalledOnce();
  });

  it('shows read errors and keeps quarantined claims out of settlement', async () => {
    mocks.read.mockRejectedValueOnce(new Error('Backend unavailable'));
    await checkClaims();
    await vi.waitFor(() => expect(target.textContent).toContain('Backend unavailable'));
    mocks.read.mockResolvedValue([{ ...claim, quarantineReason: 'Operator reconciliation' }]);
    await checkClaims();
    await vi.waitFor(() => expect(target.textContent).toContain('#20'));
    expect(button('Settle claim')?.disabled ?? true).toBe(true);
    expect(button('Confirm settlement')?.disabled ?? true).toBe(true);
    expect(mocks.settle).not.toHaveBeenCalled();
  });
});
