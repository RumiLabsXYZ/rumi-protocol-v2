import { beforeEach, describe, expect, it, vi } from 'vitest';

const mocks = vi.hoisted(() => ({ getActor: vi.fn(), read: vi.fn(), setWallet: vi.fn() }));
vi.mock('../stores/wallet', async () => {
  const { writable } = await import('svelte/store');
  const state = writable({ principal: null });
  mocks.setWallet.mockImplementation(state.set);
  return { walletStore: Object.assign(state, { getActor: mocks.getActor }) };
});
vi.mock('./protocol/apiClient', () => ({
  ApiClient: { executeSequentialOperation: (run: () => unknown) => run(), formatProtocolError: () => 'Backend refused the proof' },
  publicActor: { get_events_forward_filtered: mocks.read }
}));
import { confirmXrpVaultTopup, copyXrpVaultAddress, findXrpVaultOrigin, loadXrpVaultAddress, normalizeXrpDepositHash } from './xrpExistingDeposit';

const owner = 'owner-a';
const principal = (text: string) => ({ toText: () => text });
const connect = (text = owner) => mocks.setWallet({ principal: principal(text) });

describe('existing XRP deposit session and proof flow', () => {
  beforeEach(() => { vi.clearAllMocks(); connect(); });

  it('rejects malformed hashes without asking the wallet or calling the backend', async () => {
    await expect(confirmXrpVaultTopup(7, owner, 'not-a-hash')).rejects.toThrow('64-character');
    expect(mocks.getActor).not.toHaveBeenCalled();
    expect(normalizeXrpDepositHash(` ${'ab'.repeat(32)} `)).toBe('AB'.repeat(32));
  });

  it('does not dispatch an address call when the wallet changes while the actor loads', async () => {
    const show = vi.fn();
    mocks.getActor.mockImplementation(async () => {
      connect('owner-b');
      return { get_xrp_vault_deposit_address: show };
    });
    await expect(loadXrpVaultAddress(7, owner)).rejects.toThrow('Wallet changed');
    expect(show).not.toHaveBeenCalled();
  });

  it('bounds public history discovery and ignores another owners opening record', async () => {
    const read = vi.fn().mockResolvedValue({ next_start: 2000n, reached_end: false, events: [
      [4n, { open_vault: { vault: { vault_id: 7n, owner: principal('owner-b') } } }],
      [8n, { open_vault: { vault: { vault_id: 7n, owner: principal(owner) } } }]
    ] });
    await expect(findXrpVaultOrigin(7, owner, 0n, read)).resolves.toEqual({ index: 8n, next: 2000n, end: false });
    expect(read).toHaveBeenCalledOnce();
    expect(read).toHaveBeenCalledWith(0n, 2000n, [[{ OpenVault: null }]]);
  });

  it('discards history after an account change and refuses a non-advancing cursor', async () => {
    const changed = vi.fn().mockImplementation(async () => {
      connect('owner-b');
      return { next_start: 1n, reached_end: true, events: [] };
    });
    await expect(findXrpVaultOrigin(7, owner, 0n, changed)).rejects.toThrow('Wallet changed');
    connect();
    await expect(findXrpVaultOrigin(7, owner, 0n, vi.fn().mockResolvedValue({ next_start: 0n, reached_end: false, events: [] })))
      .rejects.toThrow('did not advance');
  });

  it('does not retry a lost reply or claim success from a balance change', async () => {
    const verify = vi.fn().mockRejectedValue(new Error('Reply lost'));
    mocks.getActor.mockResolvedValue({ confirm_xrp_vault_topup: verify });
    await expect(confirmXrpVaultTopup(7, owner, 'ab'.repeat(32), 8n)).rejects.toThrow('Reply lost');
    expect(verify).toHaveBeenCalledOnce();
    expect(verify).toHaveBeenCalledWith(7n, 'AB'.repeat(32), [8n]);
  });

  it('discards a confirmation reply after the connected account changes', async () => {
    const verify = vi.fn().mockImplementation(async () => { connect('owner-b'); return { Ok: 1_000_000n }; });
    mocks.getActor.mockResolvedValue({ confirm_xrp_vault_topup: verify });
    await expect(confirmXrpVaultTopup(7, owner, 'ab'.repeat(32))).rejects.toThrow('Wallet changed');
  });

  it('refuses to copy the previous wallets address before the component has flushed an account change', async () => {
    const write = vi.fn().mockResolvedValue(undefined);
    connect('owner-b');
    await expect(copyXrpVaultAddress(owner, 'rPreviousOwnerCustody', write)).rejects.toThrow('Wallet changed');
    expect(write).not.toHaveBeenCalled();
  });
});
