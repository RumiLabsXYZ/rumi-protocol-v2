import { describe, expect, it } from 'vitest';
import { Principal } from '@dfinity/principal';
import {
  XRP_NATIVE_PRINCIPAL_TEXT,
  buildManualXrpSettlementFailureCopy,
  buildManualXrpSettlementSuccessCopy,
  isIcrcClaimableCollateral,
  isNativeXrpPrincipal,
  mapLiquidationSuccessWithFee,
  mapOptionalXrpClaimId,
  unwrapNativePayoutAddresses,
  unwrapNativePayoutDestinationTags,
  validateXrpPayoutInput,
} from './xrpPayoutHelpers';

const XRP = Principal.fromText(XRP_NATIVE_PRINCIPAL_TEXT);
const ICP = Principal.fromText('ryjl3-tyaaa-aaaaa-aaaba-cai');

describe('XRP payout helpers', () => {
  it('detects native XRP by its synthetic principal only', () => {
    expect(isNativeXrpPrincipal(XRP)).toBe(true);
    expect(isNativeXrpPrincipal(XRP_NATIVE_PRINCIPAL_TEXT)).toBe(true);
    expect(isNativeXrpPrincipal(ICP)).toBe(false);
    expect(isNativeXrpPrincipal('not-a-principal')).toBe(false);
  });

  it('validates XRP payout address and exact destination tag bounds', () => {
    expect(validateXrpPayoutInput('', '')).toEqual({
      ok: false,
      error: 'Enter an XRP payout address.',
    });

    expect(validateXrpPayoutInput(' rLiquidator ', '')).toEqual({
      ok: true,
      address: 'rLiquidator',
      destinationTag: undefined,
    });
    expect(validateXrpPayoutInput('rLiquidator', '0')).toEqual({
      ok: true,
      address: 'rLiquidator',
      destinationTag: 0,
    });
    expect(validateXrpPayoutInput('rLiquidator', '4294967295')).toEqual({
      ok: true,
      address: 'rLiquidator',
      destinationTag: 4294967295,
    });

    for (const tag of ['-1', '1.5', '4294967296']) {
      expect(validateXrpPayoutInput('rLiquidator', tag)).toEqual({
        ok: false,
        error: 'Destination tag must be a whole number from 0 to 4294967295.',
      });
    }
  });

  it('maps optional u64 XRP claim ids without precision loss', () => {
    expect(mapOptionalXrpClaimId([])).toBeUndefined();
    expect(mapOptionalXrpClaimId([42n])).toBe('42');
    expect(mapOptionalXrpClaimId([9007199254740993n])).toBe('9007199254740993');
  });

  it('maps liquidation SuccessWithFee with optional XRP claim id', () => {
    expect(
      mapLiquidationSuccessWithFee(7, {
        block_index: 11n,
        fee_amount_paid: 250_000_000n,
        xrp_claim_id: [123n],
      })
    ).toEqual({
      success: true,
      vaultId: 7,
      blockIndex: 11,
      feePaid: 2.5,
      xrpClaimId: '123',
    });

    expect(
      mapLiquidationSuccessWithFee(8, {
        block_index: 12n,
        fee_amount_paid: 100_000_000n,
        xrp_claim_id: [],
      })
    ).toEqual({
      success: true,
      vaultId: 8,
      blockIndex: 12,
      feePaid: 1,
    });
  });

  it('unwraps Candid opt vec payout address and destination tag maps', () => {
    expect(unwrapNativePayoutAddresses({ native_payout_addresses: [] })).toEqual(new Map());
    expect(unwrapNativePayoutDestinationTags({ native_payout_destination_tags: [] })).toEqual(new Map());

    const addresses = unwrapNativePayoutAddresses({
      native_payout_addresses: [[[XRP, 'rReceiver']]],
    });
    const tags = unwrapNativePayoutDestinationTags({
      native_payout_destination_tags: [[[XRP, 4294967295]]],
    });

    expect(addresses.get(XRP_NATIVE_PRINCIPAL_TEXT)).toBe('rReceiver');
    expect(tags.get(XRP_NATIVE_PRINCIPAL_TEXT)).toBe(4294967295);
  });

  it('does not treat native XRP gains as ICRC-claimable collateral', () => {
    expect(isIcrcClaimableCollateral(XRP)).toBe(false);
    expect(isIcrcClaimableCollateral(ICP)).toBe(true);
  });

  it('uses two-phase copy and never claims XRP was received before settlement success', () => {
    const failureCopy = buildManualXrpSettlementFailureCopy('77');
    expect(failureCopy).toContain('claim #77 remains outstanding');
    expect(failureCopy.toLowerCase()).not.toContain('received xrp');

    const successCopy = buildManualXrpSettlementSuccessCopy('77', 'ABC123');
    expect(successCopy).toContain('Liquidation accepted and XRP claim #77 created');
    expect(successCopy).toContain('XRP settlement submitted');
    expect(successCopy).toContain('ABC123');
  });
});
