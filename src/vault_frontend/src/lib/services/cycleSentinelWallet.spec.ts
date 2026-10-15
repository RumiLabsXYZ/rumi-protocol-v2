import { beforeEach, describe, expect, it, vi } from 'vitest';
import { writable } from 'svelte/store';

const wallet = vi.hoisted(() => ({ openChannel: vi.fn(), getActor: vi.fn() }));
vi.mock('./pnp', () => ({ pnp: { openChannel: wallet.openChannel } }));
vi.mock('./auth', () => ({
  auth: { getActor: wallet.getActor },
  currentWalletType: writable('oisy'),
  WALLET_TYPES: { OISY: 'oisy', PLUG: 'plug', INTERNET_IDENTITY: 'internet-identity' },
}));
vi.mock('../config', () => ({
  CONFIG: { isLocal: false },
  CANISTER_IDS: { CYCLE_SENTINEL: 'joh3a-5aaaa-aaaap-quy6a-cai' },
  isCycleSentinelConfigured: true,
}));

import { currentWalletType } from './auth';
import { createAuthenticatedSentinelActor } from './cycleSentinelService';

describe('Sentinel wallet channel', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    currentWalletType.set('oisy');
    wallet.openChannel.mockResolvedValue(undefined);
    wallet.getActor.mockResolvedValue({ get_operator_dashboard: vi.fn() });
  });

  it('opens Oisy synchronously before awaiting actor creation', async () => {
    let finishOpening!: () => void;
    wallet.openChannel.mockImplementation(() => new Promise<void>((resolve) => { finishOpening = resolve; }));
    const pending = createAuthenticatedSentinelActor();
    expect(wallet.openChannel).toHaveBeenCalledTimes(1);
    expect(wallet.getActor).not.toHaveBeenCalled();
    finishOpening();
    await pending;
    expect(wallet.getActor).toHaveBeenCalledTimes(1);
  });

  it('preserves popup failures and does not continue with an unusable actor', async () => {
    wallet.openChannel.mockRejectedValue(new Error('Signer window could not be opened'));
    await expect(createAuthenticatedSentinelActor()).rejects.toThrow('Signer window could not be opened');
    expect(wallet.getActor).not.toHaveBeenCalled();
  });

  it.each(['plug', 'internet-identity'] as const)('does not open an Oisy popup for %s', async (kind) => {
    currentWalletType.set(kind);
    await createAuthenticatedSentinelActor();
    expect(wallet.openChannel).not.toHaveBeenCalled();
    expect(wallet.getActor).toHaveBeenCalledTimes(1);
  });
});
