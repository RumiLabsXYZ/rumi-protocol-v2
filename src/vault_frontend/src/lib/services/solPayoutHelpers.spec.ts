import { describe, expect, it } from 'vitest';
import { Principal } from '@dfinity/principal';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import {
  SOL_NATIVE_PRINCIPAL_TEXT,
  buildManualSolSettlementFailureCopy,
  buildManualSolSettlementSuccessCopy,
  isNativeSolPrincipal,
  isPlausibleSolAddress,
  mapOptionalSolClaimId,
  validateSolPayoutInput,
} from './solPayoutHelpers';

const SOL = Principal.fromText(SOL_NATIVE_PRINCIPAL_TEXT);
const ICP = Principal.fromText('ryjl3-tyaaa-aaaaa-aaaba-cai');

// 32 zero bytes base58-encodes to 32 leading '1' characters (each zero byte maps
// to a leading '1' in this repo's base58 decode, same as Solana's real System
// Program address "111111...1"), so this is a guaranteed-valid 32-byte address
// without hardcoding a real pubkey string.
const VALID_32_BYTE_ADDRESS = '1'.repeat(32);

describe('SOL payout helpers', () => {
  it('detects native SOL by its synthetic principal only', () => {
    expect(isNativeSolPrincipal(SOL)).toBe(true);
    expect(isNativeSolPrincipal(SOL_NATIVE_PRINCIPAL_TEXT)).toBe(true);
    expect(isNativeSolPrincipal(ICP)).toBe(false);
    expect(isNativeSolPrincipal('not-a-principal')).toBe(false);
  });

  it('matches the backend Principal::from_slice(b"rumi-sol-native") textual encoding', () => {
    // Cross-check against the known-good XRP constant's derivation methodology:
    // both are 15-byte ASCII tags run through the standard Principal
    // base32+CRC32 textual encoding. This constant was independently verified
    // during implementation via `Principal.fromUint8Array(Buffer.from('rumi-sol-native'))`.
    expect(SOL_NATIVE_PRINCIPAL_TEXT).toBe('mau3v-slsov-wwsll-tn5wc-23tbo-ruxmz-i');
    // A textual principal must round-trip through Principal.fromText without throwing.
    expect(() => Principal.fromText(SOL_NATIVE_PRINCIPAL_TEXT)).not.toThrow();
  });

  it('accepts a structurally valid base58, exactly-32-byte address', () => {
    expect(isPlausibleSolAddress(VALID_32_BYTE_ADDRESS)).toBe(true);
    expect(validateSolPayoutInput(VALID_32_BYTE_ADDRESS)).toEqual({
      ok: true,
      address: VALID_32_BYTE_ADDRESS,
    });
  });

  it('rejects an address that decodes to the wrong byte length', () => {
    const tooShort = '1'.repeat(20);
    expect(isPlausibleSolAddress(tooShort)).toBe(false);
    expect(validateSolPayoutInput(tooShort)).toEqual({
      ok: false,
      error: 'Enter a valid Solana address (base58, 32 bytes).',
    });
  });

  it('rejects characters outside the base58 alphabet (0, O, I, l)', () => {
    // Same 32-char length as the valid case but with a disallowed character.
    const withZero = `0${'1'.repeat(31)}`;
    const withCapitalO = `O${'1'.repeat(31)}`;
    const withCapitalI = `I${'1'.repeat(31)}`;
    const withLowercaseL = `l${'1'.repeat(31)}`;
    for (const bad of [withZero, withCapitalO, withCapitalI, withLowercaseL]) {
      expect(isPlausibleSolAddress(bad)).toBe(false);
    }
  });

  it('rejects an empty payout address with a distinct error', () => {
    expect(validateSolPayoutInput('')).toEqual({
      ok: false,
      error: 'Enter a SOL payout address.',
    });
    expect(validateSolPayoutInput('   ')).toEqual({
      ok: false,
      error: 'Enter a SOL payout address.',
    });
  });

  it('trims surrounding whitespace before validating', () => {
    expect(validateSolPayoutInput(` ${VALID_32_BYTE_ADDRESS} `)).toEqual({
      ok: true,
      address: VALID_32_BYTE_ADDRESS,
    });
  });

  it('has no destination-tag concept anywhere in its validation surface', () => {
    // validateSolPayoutInput takes only one argument (the address); unlike
    // validateXrpPayoutInput, there is no second (tag) parameter to pass.
    expect(validateSolPayoutInput.length).toBe(1);
  });

  it('maps optional u64 SOL claim ids without precision loss', () => {
    expect(mapOptionalSolClaimId([])).toBeUndefined();
    expect(mapOptionalSolClaimId([42n])).toBe('42');
    expect(mapOptionalSolClaimId([9007199254740993n])).toBe('9007199254740993');
  });

  it('uses two-phase copy and never claims SOL was received before settlement success', () => {
    const failureCopy = buildManualSolSettlementFailureCopy('77');
    expect(failureCopy).toContain('claim #77 remains outstanding');
    expect(failureCopy.toLowerCase()).not.toContain('received sol');

    const successCopy = buildManualSolSettlementSuccessCopy('77', 'ABC123');
    expect(successCopy).toContain('Liquidation accepted and SOL claim #77 created');
    expect(successCopy).toContain('SOL settlement submitted');
    expect(successCopy).toContain('ABC123');
  });
});

describe('SOL address validation is threaded through all three entry points', () => {
  // Unlike XRP, which only wired its structural validator into claim
  // settlement, the SOL rail must reject an implausible address at claim
  // settlement, SP opt-in, AND manual-liquidation payout. A regression here
  // (one entry point silently skipping validation) would let a typo'd address
  // reach the backend as a settlement destination.
  it('is called from claim settlement (SolVaultPanel.svelte)', () => {
    const source = readFileSync(
      resolve(__dirname, '../components/vault/SolVaultPanel.svelte'),
      'utf8'
    );
    expect(source).toContain('isPlausibleSolAddress');
    expect(source).toContain("from '$lib/services/solPayoutHelpers'");
  });

  it('is called from Stability Pool opt-in (SolPayoutRouting.svelte)', () => {
    const source = readFileSync(
      resolve(__dirname, '../components/stability-pool/SolPayoutRouting.svelte'),
      'utf8'
    );
    expect(source).toContain('validateSolPayoutInput(payoutAddress)');
  });

  it('is called from manual-liquidation payout (ManualLiquidations.svelte)', () => {
    const source = readFileSync(
      resolve(__dirname, '../components/liquidations/ManualLiquidations.svelte'),
      'utf8'
    );
    expect(source).toContain("validateSolPayoutInput(solPayoutAddresses[vault.vault_id] ?? '')");
  });
});
