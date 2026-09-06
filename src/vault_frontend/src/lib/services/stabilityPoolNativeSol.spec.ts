import { describe, expect, it, vi } from 'vitest';
import { Principal } from '@dfinity/principal';
import {
  SOL_NATIVE_PRINCIPAL_TEXT,
  ackNativeSolPayoutSettledWithActor,
  getMyNativeSolPayoutsWithActor,
  optInNativeSolCollateralUsingActor,
} from './stabilityPoolNativeSol';

const SOL = Principal.fromText(SOL_NATIVE_PRINCIPAL_TEXT);

describe('stability pool native SOL service helpers', () => {
  it('calls tagless native opt-in (there is no _with_tag variant for SOL)', async () => {
    const actor = {
      opt_in_native_collateral: vi.fn().mockResolvedValue({ Ok: null }),
    };

    await optInNativeSolCollateralUsingActor(actor, SOL, ' SolPayout ', (err) => JSON.stringify(err));

    expect(actor.opt_in_native_collateral).toHaveBeenCalledWith(SOL, 'SolPayout');
  });

  it('throws a clear error when the actor lacks native SOL opt-in', async () => {
    await expect(
      optInNativeSolCollateralUsingActor({}, SOL, 'SolPayout', () => 'formatted')
    ).rejects.toThrow('Native SOL opt-in is not available on this Stability Pool canister yet.');
  });

  it('rejects an empty payout address before calling the actor', async () => {
    const actor = {
      opt_in_native_collateral: vi.fn().mockResolvedValue({ Ok: null }),
    };

    await expect(
      optInNativeSolCollateralUsingActor(actor, SOL, '   ', () => 'formatted')
    ).rejects.toThrow('Enter a SOL address');
    expect(actor.opt_in_native_collateral).not.toHaveBeenCalled();
  });

  it('wraps pending payout read and ack calls', async () => {
    const payout = {
      claim_id: 55n,
      collateral_type: SOL,
      vault_id: 9n,
      lamports: 1_000_000_000n,
      payout_address: 'SolPayout',
      created_at_ns: 123n,
    };
    const actor = {
      get_my_native_sol_payouts: vi.fn().mockResolvedValue([payout]),
      ack_native_sol_payout_settled: vi.fn().mockResolvedValue({ Ok: null }),
    };

    await expect(getMyNativeSolPayoutsWithActor(actor)).resolves.toEqual([payout]);
    await ackNativeSolPayoutSettledWithActor(actor, '55', () => 'formatted');

    expect(actor.ack_native_sol_payout_settled).toHaveBeenCalledWith(55n);
  });

  it('returns an empty list rather than throwing when the actor lacks the pending-payout query (older canister)', async () => {
    await expect(getMyNativeSolPayoutsWithActor({})).resolves.toEqual([]);
  });
});
