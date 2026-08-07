import type { Principal } from '@dfinity/principal';

/**
 * Native-SOL synthetic collateral principal, textual encoding of
 * `Principal::from_slice(b"rumi-sol-native")` (backend `state::sol_collateral_principal`).
 * Verified to match the backend's computed value: both are 15 raw bytes run through
 * the standard Principal base32+CRC32 textual encoding, and this constant was checked
 * against a local `Principal.fromUint8Array(Buffer.from('rumi-sol-native'))` encode
 * during implementation (same procedure that produced XRP_NATIVE_PRINCIPAL_TEXT below,
 * which independently reproduces `xrp_collateral_principal()`'s known-good value).
 */
export const SOL_NATIVE_PRINCIPAL_TEXT = 'mau3v-slsov-wwsll-tn5wc-23tbo-ruxmz-i';

export type SolClaimId = string;
export type CandidOpt<T> = [] | [T];

export interface SolPayoutValidation {
  ok: boolean;
  address?: string;
  error?: string;
}

type PrincipalLike = string | Principal | { toText?: () => string } | null | undefined;

function principalText(value: PrincipalLike): string | null {
  if (!value) return null;
  if (typeof value === 'string') return value;
  try {
    return value.toText?.() ?? null;
  } catch {
    return null;
  }
}

export function isNativeSolPrincipal(value: PrincipalLike): boolean {
  return principalText(value) === SOL_NATIVE_PRINCIPAL_TEXT;
}

/**
 * Base58 alphabet used by Solana (Bitcoin alphabet, no 0/O/I/l). Solana addresses
 * have NO version byte and NO checksum, unlike XRPL classic addresses, so a
 * structurally valid decode is weaker evidence of correctness here than for XRP.
 * This is why the backend's on-curve check (chains/sol/address.rs,
 * `Pubkey::is_on_curve()`) is the real trust boundary; see the comment on
 * {@link isPlausibleSolAddress} below.
 */
const SOL_BASE58_ALPHABET = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';

/**
 * Structural-only Solana address check: base58 alphabet + exactly 32 decoded
 * bytes. Deliberately SYNCHRONOUS and deliberately NOT an on-curve check.
 *
 * An on-curve check needs elliptic-curve crypto the browser would have to load,
 * and running it here would burn the browser's user-gesture window between the
 * click and opening the Oisy signer popup, the same reasoning XrpVaultPanel.svelte
 * documents for skipping the XRPL checksum client-side. A 32-byte off-curve value
 * is a valid program-derived address (PDA) with no private key; sending collateral
 * to one destroys it irrecoverably, so this length/alphabet check only catches
 * typos and wrong-format input. The backend (`chains/sol/address.rs`,
 * `is_valid_sol_address`) performs the full base58 + on-curve validation before
 * any funds move, and is the actual trust boundary.
 */
export function isPlausibleSolAddress(address: string): boolean {
  const trimmed = address.trim();
  if (!trimmed || trimmed.length < 32 || trimmed.length > 44) return false;
  for (const ch of trimmed) {
    if (!SOL_BASE58_ALPHABET.includes(ch)) return false;
  }
  let num = 0n;
  for (const ch of trimmed) {
    num = num * 58n + BigInt(SOL_BASE58_ALPHABET.indexOf(ch));
  }
  const bytes: number[] = [];
  while (num > 0n) {
    bytes.unshift(Number(num & 0xffn));
    num >>= 8n;
  }
  for (const ch of trimmed) {
    if (ch === '1') bytes.unshift(0);
    else break;
  }
  return bytes.length === 32;
}

/**
 * Validate a Solana payout address. No destination-tag parameter: unlike XRPL,
 * Solana has no analogue (exchanges use unique deposit addresses per user), so
 * there is no `_with_tag` counterpart anywhere in the SOL rail.
 */
export function validateSolPayoutInput(addressInput: string): SolPayoutValidation {
  const address = addressInput.trim();
  if (!address) {
    return { ok: false, error: 'Enter a SOL payout address.' };
  }
  if (!isPlausibleSolAddress(address)) {
    return { ok: false, error: 'Enter a valid Solana address (base58, 32 bytes).' };
  }
  return { ok: true, address };
}

export function mapOptionalSolClaimId(opt: CandidOpt<bigint | number | string> | undefined): SolClaimId | undefined {
  if (!opt || opt.length === 0) return undefined;
  const raw = opt[0];
  if (typeof raw === 'bigint') {
    if (raw < 0n) throw new Error('SOL claim id cannot be negative');
    return raw.toString();
  }
  if (typeof raw === 'number') {
    if (!Number.isSafeInteger(raw) || raw < 0) {
      throw new Error('SOL claim id is outside the safe integer range');
    }
    return String(raw);
  }
  if (!/^\d+$/.test(raw)) {
    throw new Error('SOL claim id must be an unsigned integer string');
  }
  return raw;
}

export function solClaimIdToBigInt(claimId: SolClaimId | number | bigint): bigint {
  if (typeof claimId === 'bigint') {
    if (claimId < 0n) throw new Error('SOL claim id cannot be negative');
    return claimId;
  }
  if (typeof claimId === 'number') {
    if (!Number.isSafeInteger(claimId) || claimId < 0) {
      throw new Error('SOL claim id is outside the safe integer range');
    }
    return BigInt(claimId);
  }
  if (!/^\d+$/.test(claimId)) {
    throw new Error('SOL claim id must be an unsigned integer string');
  }
  return BigInt(claimId);
}

// NOTE: `SuccessWithFee.xrp_claim_id` is the shared native-custody claim-id
// field, not an XRP-only one. The backend keeps that name because Candid
// identifies record fields by a hash of the name, so renaming it would break
// the wire format for existing clients; for a native-SOL vault it carries the
// SOL claim id. A manual SOL liquidation reads it the same way XRP does, and
// only falls back to `get_my_sol_claims` filtered by vault id when it comes
// back empty (the ambiguous case, e.g. the call errored after the claim was
// created). See manualSolLiquidation.ts.

// Payout-address unwrapping is collateral-agnostic (SP `native_payout_addresses`
// is keyed by collateral principal for every native rail); reuse XRP's helper
// from xrpPayoutHelpers.ts instead of duplicating it here.

export function buildManualSolSettlementFailureCopy(claimId: SolClaimId): string {
  return `Liquidation accepted and SOL claim #${claimId} created, but settlement did not complete. The claim #${claimId} remains outstanding and can be retried from this screen.`;
}

export function buildManualSolSettlementSuccessCopy(claimId: SolClaimId, signature?: string): string {
  const suffix = signature ? ` Signature: ${signature}.` : '';
  return `Liquidation accepted and SOL claim #${claimId} created. SOL settlement submitted.${suffix}`;
}
