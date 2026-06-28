import { describe, expect, it, vi } from 'vitest';
import { Principal } from '@dfinity/principal';
import {
  XRP_NATIVE_PRINCIPAL_TEXT,
  ackNativeXrpPayoutSettledWithActor,
  getMyNativeXrpPayoutsWithActor,
  optInNativeCollateralWithTagUsingActor,
} from './stabilityPoolNativeXrp';

const XRP = Principal.fromText(XRP_NATIVE_PRINCIPAL_TEXT);

describe('stability pool native XRP service helpers', () => {
  it('calls tag-aware native opt-in with Candid opt nat32', async () => {
    const actor = {
      opt_in_native_collateral_with_tag: vi.fn().mockResolvedValue({ Ok: null }),
    };

    await optInNativeCollateralWithTagUsingActor(actor, XRP, ' rPayout ', 123, (err) => JSON.stringify(err));

    expect(actor.opt_in_native_collateral_with_tag).toHaveBeenCalledWith(XRP, 'rPayout', [123]);
  });

  it('falls back to address-only native opt-in only when no destination tag was supplied', async () => {
    const actor = {
      opt_in_native_collateral: vi.fn().mockResolvedValue({ Ok: null }),
    };

    await optInNativeCollateralWithTagUsingActor(actor, XRP, 'rPayout', undefined, () => 'formatted');

    expect(actor.opt_in_native_collateral).toHaveBeenCalledWith(XRP, 'rPayout');
    await expect(
      optInNativeCollateralWithTagUsingActor(actor, XRP, 'rPayout', 7, () => 'formatted')
    ).rejects.toThrow('Destination tags are not available on this Stability Pool canister yet.');
  });

  it('wraps pending payout read and ack calls', async () => {
    const payout = {
      claim_id: 55n,
      collateral_type: XRP,
      vault_id: 9n,
      drops: 1_000_000n,
      payout_address: 'rPayout',
      destination_tag: [44],
      created_at_ns: 123n,
    };
    const actor = {
      get_my_native_xrp_payouts: vi.fn().mockResolvedValue([payout]),
      ack_native_xrp_payout_settled: vi.fn().mockResolvedValue({ Ok: null }),
    };

    await expect(getMyNativeXrpPayoutsWithActor(actor)).resolves.toEqual([payout]);
    await ackNativeXrpPayoutSettledWithActor(actor, '55', () => 'formatted');

    expect(actor.ack_native_xrp_payout_settled).toHaveBeenCalledWith(55n);
  });
});
