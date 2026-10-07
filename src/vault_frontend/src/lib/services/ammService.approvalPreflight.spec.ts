import { beforeEach, describe, expect, it, vi } from 'vitest';
import { Principal } from '@dfinity/principal';

const mocks = vi.hoisted(() => {
  const store = <T>(initial: T) => {
    let value = initial;
    const subscribers = new Set<(next: T) => void>();
    return {
      subscribe(run: (next: T) => void) {
        subscribers.add(run);
        run(value);
        return () => subscribers.delete(run);
      },
      get: () => value,
      set(next: T) {
        value = next;
        subscribers.forEach(run => run(value));
      },
      update(updater: (current: T) => T) {
        this.set(updater(value));
      },
    };
  };
  return {
    wallet: { isConnected: true, principal: null as any },
    walletType: store('plug'),
    generation: store(0),
    isOisy: false,
    getActor: vi.fn(),
    approve: vi.fn(),
    getOperation: vi.fn(),
    swap: vi.fn(),
    addLiquidity: vi.fn(),
  };
});

vi.mock('../stores/wallet', () => ({
  walletStore: {
    subscribe(run: (value: typeof mocks.wallet) => void) {
      run(mocks.wallet);
      return () => {};
    },
    getActor: mocks.getActor,
  },
}));

vi.mock('./auth', () => ({
  currentWalletType: mocks.walletType,
  walletSessionGeneration: mocks.generation,
  WALLET_TYPES: { PLUG: 'plug', OISY: 'oisy' },
  assertPlugPrincipal: vi.fn(),
}));

vi.mock('./protocol/walletOperations', () => ({ isOisyWallet: () => mocks.isOisy }));
vi.mock('./oisySigner', () => ({ getOisySignerAgent: vi.fn(), createOisyActor: vi.fn() }));
vi.mock('./ledgerFeeService', () => ({
  fetchLedgerFee: vi.fn().mockResolvedValue(10n),
  getCachedLedgerFee: vi.fn().mockReturnValue(10n),
}));
vi.mock('./pnp', () => ({ canisterIDLs: { rumi_amm: {} } }));
vi.mock('../config', () => ({
  CANISTER_IDS: { RUMI_AMM: 'aaaaa-aa' },
  CONFIG: { icusd_ledgerIDL: {} },
}));

import { walletSessionGeneration } from './auth';
import { createOisyActor, getOisySignerAgent } from './oisySigner';
import { ammService } from './ammService';

const principal = Principal.fromUint8Array(new Uint8Array([1, 2, 3]));
const tokenPrincipal = Principal.fromUint8Array(new Uint8Array([4, 5, 6]));
const token = {
  symbol: 'icUSD', ledgerId: 'ledger-canister', decimals: 8, color: '',
  balanceKey: 'ICUSD', is3USD: false, threePoolIndex: 0,
};

function makePendingOperation() {
  return {
    request_id: new Uint8Array(32),
    phase: { Processing: null },
    pool_id: 'other-pool',
    kind: { Swap: {
      token_in: tokenPrincipal,
      amount_in: 7n,
      min_amount_out: 1n,
    } },
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  localStorage.clear();
  vi.useRealTimers();
  mocks.wallet.isConnected = true;
  mocks.wallet.principal = principal;
  mocks.walletType.set('plug');
  mocks.generation.set(0);
  mocks.isOisy = false;
  mocks.getOperation.mockResolvedValue([]);
  mocks.approve.mockResolvedValue({ Ok: 1n });
  mocks.swap.mockResolvedValue({ Ok: { amount_out: 100n, fee: 1n } });
  mocks.addLiquidity.mockResolvedValue({ Ok: 10n });
  const ammActor = {
    get_my_amm_operation: mocks.getOperation,
    swap_v2: mocks.swap,
    add_liquidity_v2: mocks.addLiquidity,
  };
  const ledgerActor = { icrc2_approve: mocks.approve };
  mocks.getActor.mockImplementation(async (canisterId: string) =>
    canisterId === 'aaaaa-aa' ? ammActor : ledgerActor,
  );
  vi.mocked(getOisySignerAgent).mockResolvedValue({} as any);
  vi.mocked(createOisyActor).mockImplementation(((canisterId: string) =>
    canisterId === 'aaaaa-aa'
      ? { get_my_amm_operation: mocks.getOperation, swap_v2: mocks.swap, add_liquidity_v2: mocks.addLiquidity }
      : { icrc2_approve: mocks.approve }) as any);
});

describe('AMM approval preflight', () => {
  it('rejects a conflicting pending operation before requesting token approval', async () => {
    mocks.getOperation.mockResolvedValue([makePendingOperation()]);

    await expect(ammService.swap('new-pool', tokenPrincipal, 9n, 2n, token))
      .rejects.toThrow('A prior AMM operation is unresolved');

    expect(mocks.approve).not.toHaveBeenCalled();
    expect(mocks.swap).not.toHaveBeenCalled();
  });

  it('rejects a conflicting pending operation before either liquidity approval', async () => {
    mocks.getOperation.mockResolvedValue([makePendingOperation()]);

    await expect(ammService.addLiquidity('new-pool', 5n, 8n, 1n, token, token))
      .rejects.toThrow('A prior AMM operation is unresolved');

    expect(mocks.approve).not.toHaveBeenCalled();
    expect(mocks.addLiquidity).not.toHaveBeenCalled();
  });

  it('rejects a conflicting pending operation before the direct Oisy swap approval', async () => {
    mocks.isOisy = true;
    mocks.walletType.set('oisy');
    mocks.getOperation.mockResolvedValue([makePendingOperation()]);

    await expect(ammService.swap('new-pool', tokenPrincipal, 9n, 2n, token))
      .rejects.toThrow('A prior AMM operation is unresolved');

    expect(mocks.approve).not.toHaveBeenCalled();
    expect(mocks.swap).not.toHaveBeenCalled();
  });

  it('fails closed when cross-tab Oisy routing disagrees with this tab session type', async () => {
    mocks.isOisy = true;
    mocks.walletType.set('plug');

    await expect(ammService.swap('pool', tokenPrincipal, 9n, 2n, token))
      .rejects.toThrow('Wallet changed during the AMM operation');

    expect(mocks.approve).not.toHaveBeenCalled();
    expect(mocks.swap).not.toHaveBeenCalled();
  });

  it('rejects a conflicting pending operation before either direct Oisy liquidity approval', async () => {
    mocks.isOisy = true;
    mocks.walletType.set('oisy');
    mocks.getOperation.mockResolvedValue([makePendingOperation()]);

    await expect(ammService.addLiquidity('new-pool', 5n, 8n, 1n, token, token))
      .rejects.toThrow('A prior AMM operation is unresolved');

    expect(mocks.approve).not.toHaveBeenCalled();
    expect(mocks.addLiquidity).not.toHaveBeenCalled();
  });

  it('reuses the persisted intent after approval rejection when no on-chain row exists', async () => {
    vi.useFakeTimers();
    mocks.approve.mockRejectedValueOnce(new Error('user rejected approval'));

    await expect(ammService.swap('pool', tokenPrincipal, 9n, 2n, token))
      .rejects.toThrow('user rejected approval');
    const saved = JSON.parse(localStorage.getItem(`rumi-amm-v2:${principal.toText()}`)!);

    const retry = ammService.swap('pool', tokenPrincipal, 9n, 2n, token);
    await vi.runAllTimersAsync();
    await retry;

    expect(Array.from(mocks.swap.mock.calls[0][0])).toEqual(
      Array.from(Uint8Array.from(saved.id.match(/.{2}/g)!, (byte: string) => parseInt(byte, 16))),
    );
    expect(mocks.getOperation).toHaveBeenCalledTimes(2);
  });

  it('stops before approval if actor acquisition observes a newly selected wallet', async () => {
    const ammActor = { get_my_amm_operation: mocks.getOperation, swap_v2: mocks.swap };
    const ledgerActor = { icrc2_approve: mocks.approve };
    const otherPrincipal = Principal.fromUint8Array(new Uint8Array([7, 8, 9]));
    mocks.getActor
      .mockImplementationOnce(async () => ammActor)
      .mockImplementationOnce(async () => {
        mocks.wallet.principal = otherPrincipal;
        walletSessionGeneration.update((generation: number) => generation + 1);
        return ledgerActor;
      });

    await expect(ammService.swap('pool', tokenPrincipal, 9n, 2n, token))
      .rejects.toThrow('Wallet changed during the AMM operation');

    expect(mocks.approve).not.toHaveBeenCalled();
    expect(mocks.swap).not.toHaveBeenCalled();
  });

  it('stops removeLiquidity if the wallet session changes during actor acquisition', async () => {
    const otherPrincipal = Principal.fromUint8Array(new Uint8Array([7, 8, 9]));
    const remove = vi.fn();
    mocks.getActor.mockImplementationOnce(async () => {
      mocks.wallet.principal = otherPrincipal;
      walletSessionGeneration.update((generation: number) => generation + 1);
      return { get_my_amm_operation: mocks.getOperation, remove_liquidity_v2: remove };
    });

    await expect(ammService.removeLiquidity('pool', 3n, 1n, 1n))
      .rejects.toThrow('Wallet changed during the AMM operation');

    expect(remove).not.toHaveBeenCalled();
  });

  it('stops a preapproved AMM swap if the session changes during pending-operation lookup', async () => {
    const otherPrincipal = Principal.fromUint8Array(new Uint8Array([7, 8, 9]));
    const swap = vi.fn();
    const actor = {
      get_my_amm_operation: vi.fn(async () => {
        mocks.wallet.principal = otherPrincipal;
        walletSessionGeneration.update((generation: number) => generation + 1);
        return [];
      }),
      swap_v2: swap,
    };

    await expect(ammService.swapWithPreapprovedActor(
      actor, principal, 'pool', tokenPrincipal, 9n, 2n,
    )).rejects.toThrow('Wallet changed during the AMM operation');

    expect(swap).not.toHaveBeenCalled();
  });
});
