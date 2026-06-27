import { describe, it, expect } from 'vitest';
import { IDL } from '@dfinity/candid';
import { Principal } from '@dfinity/principal';
import { idlFactory } from '$declarations/rumi_stability_pool/rumi_stability_pool.did.js';

// Regression guard for the stability-pool position decode skew.
//
// `get_user_position` returns `opt UserStabilityPosition`. The frontend
// declarations can be deployed ahead of the SP canister, so any field the
// frontend marks REQUIRED but the live (older) canister omits makes the whole
// position fail to decode — silently hiding every depositor's deposit, gains,
// Claim button and Opt-out menu. This happened with `native_payout_addresses`
// when it was generated as a required `vec` instead of `opt vec`.
//
// This test encodes a response in the OLD canister shape (no native-collateral
// fields) and decodes it with the CURRENT generated idlFactory. It must not
// throw, and the position must survive. Reintroducing a required response
// field will fail here.

// What the pre-native-collateral SP canister actually returns on the wire.
const OLD_UserStabilityPosition = IDL.Record({
  deposit_timestamp: IDL.Nat64,
  total_interest_earned_e8s: IDL.Nat64,
  collateral_gains: IDL.Vec(IDL.Tuple(IDL.Principal, IDL.Nat64)),
  stablecoin_balances: IDL.Vec(IDL.Tuple(IDL.Principal, IDL.Nat64)),
  total_claimed_gains: IDL.Vec(IDL.Tuple(IDL.Principal, IDL.Nat64)),
  total_usd_value_e8s: IDL.Nat64,
  opted_out_collateral: IDL.Vec(IDL.Principal),
});

const ICUSD_LEDGER = Principal.fromText('t6bor-paaaa-aaaap-qrd5q-cai');

function getUserPositionReturnType(): any {
  const service: any = idlFactory({ IDL });
  const entry = service._fields.find(([name]: [string, unknown]) => name === 'get_user_position');
  if (!entry) throw new Error('get_user_position not found in idlFactory');
  // FuncClass.retTypes[0] === `opt UserStabilityPosition`
  return entry[1].retTypes[0];
}

describe('stability pool get_user_position decode compatibility', () => {
  it('decodes an old-canister (pre native-collateral) response without throwing', () => {
    const sample = {
      deposit_timestamp: 1_700_000_000_000_000_000n,
      total_interest_earned_e8s: 0n,
      collateral_gains: [] as Array<[Principal, bigint]>,
      stablecoin_balances: [[ICUSD_LEDGER, 550_0000_0000n]] as Array<[Principal, bigint]>,
      total_claimed_gains: [] as Array<[Principal, bigint]>,
      total_usd_value_e8s: 550_0000_0000n,
      opted_out_collateral: [] as Principal[],
    };

    // Encode exactly as the deployed (old) canister would: `opt OLD`.
    const wireBytes = IDL.encode([IDL.Opt(OLD_UserStabilityPosition)], [[sample]]);

    const retType = getUserPositionReturnType();

    let decoded: unknown;
    expect(() => {
      decoded = IDL.decode([retType], wireBytes)[0];
    }).not.toThrow();

    // `opt` => array of 0 or 1; must be the populated position, not None.
    expect(Array.isArray(decoded)).toBe(true);
    const opt = decoded as unknown[];
    expect(opt.length).toBe(1);

    const position = opt[0] as Record<string, unknown>;
    expect(position.total_usd_value_e8s).toBe(550_0000_0000n);
    // Missing native field must decode to None (`[]`), proving it is `opt`.
    expect(position.native_payout_addresses).toEqual([]);
  });
});
