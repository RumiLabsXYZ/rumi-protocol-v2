// Read-only controller for the currently-connected EVM wallet's CFX balance,
// plus the gas-reserve policy behind the Borrow page's Max button. Every read
// re-verifies the wallet's chain and address on each call (mirroring the
// existing walletChainId/walletStillControlsAddress checks used elsewhere)
// and is guarded against stale responses from a superseded wallet identity.

import type { Address } from "viem";
import { CHAIN_ID } from "./config";
import {
  cfxBalance,
  estimateCfxFeePerGasWei,
  fmtCfx,
  walletChainId,
  walletStillControlsAddress,
  weiToInputString,
  type Wallet,
} from "./evm";

export type WalletBalanceSnapshot = {
  address: Address;
  balanceWei: bigint;
  /** null when a live fee estimate could not be read; never masquerades as 0n. */
  feePerGasWei: bigint | null;
};

export type WalletBalanceReaders = {
  getChainId: (wallet: Wallet) => Promise<number>;
  controlsAddress: (wallet: Wallet) => Promise<boolean>;
  getBalance: (address: Address) => Promise<bigint>;
  getFeePerGas: () => Promise<bigint | null>;
};

export const liveWalletBalanceReaders: WalletBalanceReaders = {
  getChainId: walletChainId,
  controlsAddress: walletStillControlsAddress,
  getBalance: cfxBalance,
  getFeePerGas: estimateCfxFeePerGasWei,
};

export type WalletBalanceResult =
  | { stale: true }
  | { stale: false; invalid: true }
  | { stale: false; invalid: false; snapshot: WalletBalanceSnapshot | null };

export type WalletBalanceController = {
  fetch: (wallet: Wallet) => Promise<WalletBalanceResult>;
  /** Discards the result of any in-flight fetch (e.g. on disconnect) even if
   * no new fetch is started to supersede it. */
  invalidate: () => void;
};

type IdentityCheck = "ok" | "mismatch" | "error";

/** Serializes CFX-balance reads behind a monotonically increasing request
 * token: a response can only update state if it is still the most recent
 * request issued for whatever wallet identity is now connected. Identity
 * (chain + address) is verified both before AND after the awaited balance/fee
 * reads, so a provider account or network switch that happens mid-read can
 * never publish a snapshot for an identity that is no longer current. */
export function createWalletBalanceController(
  readers: WalletBalanceReaders = liveWalletBalanceReaders,
): WalletBalanceController {
  let generation = 0;

  async function fetch(wallet: Wallet): Promise<WalletBalanceResult> {
    generation += 1;
    const token = generation;
    const address = wallet.address;

    async function verifyIdentity(): Promise<IdentityCheck> {
      try {
        const [chainId, controlsAddress] = await Promise.all([
          readers.getChainId(wallet),
          readers.controlsAddress(wallet),
        ]);
        return chainId === CHAIN_ID && controlsAddress ? "ok" : "mismatch";
      } catch {
        return "error";
      }
    }

    const initialIdentity = await verifyIdentity();
    if (token !== generation) return { stale: true };
    if (initialIdentity === "error") return { stale: false, invalid: false, snapshot: null };
    if (initialIdentity === "mismatch") return { stale: false, invalid: true };

    let balanceWei: bigint;
    try {
      balanceWei = await readers.getBalance(address);
    } catch {
      if (token !== generation) return { stale: true };
      return { stale: false, invalid: false, snapshot: null };
    }
    if (token !== generation) return { stale: true };

    let feePerGasWei: bigint | null;
    try {
      feePerGasWei = await readers.getFeePerGas();
    } catch {
      feePerGasWei = null;
    }
    if (token !== generation) return { stale: true };

    const finalIdentity = await verifyIdentity();
    if (token !== generation) return { stale: true };
    if (finalIdentity === "mismatch") return { stale: false, invalid: true };
    if (finalIdentity === "error") return { stale: false, invalid: false, snapshot: null };

    return { stale: false, invalid: false, snapshot: { address, balanceWei, feePerGasWei } };
  }

  function invalidate(): void {
    generation += 1;
  }

  return { fetch, invalidate };
}

// -- Max-collateral gas reserve policy --------------------------------------

/** Gas used by a plain CFX transfer (the vault deposit is a value transfer
 * to the canister-issued custody address, not a contract call). */
const CFX_TRANSFER_GAS_LIMIT = 21_000n;
/** Applied over the raw fee-estimate reserve to absorb fee volatility
 * between this read and the signed deposit transaction. */
const GAS_RESERVE_SAFETY_MULTIPLIER = 2n;
/** Bounded, explicitly-labeled fallback reserve (0.01 CFX) used only when a
 * live fee estimate is unavailable. This is a deliberately conservative
 * placeholder, not a fee measurement. */
export const FALLBACK_GAS_RESERVE_WEI = 10_000_000_000_000_000n;

export function gasReserveWeiFromFeeEstimate(feePerGasWei: bigint): bigint {
  return feePerGasWei * CFX_TRANSFER_GAS_LIMIT * GAS_RESERVE_SAFETY_MULTIPLIER;
}

export type MaxCollateralResult = {
  maxWei: bigint;
  reserveWei: bigint;
  reserveIsFallback: boolean;
};

/** Reserves gas from the wallet's CFX balance before authorizing Max. Returns
 * null (Max disabled) whenever the balance itself is unknown - an unknown
 * balance must never be treated as zero, and must never authorize Max. A
 * nonpositive live fee estimate is treated the same as a missing one (falls
 * back to the bounded reserve) so a bad/zero RPC fee reading can never
 * collapse the reserve to zero or below. */
export function computeMaxCollateralWei(
  balanceWei: bigint | null,
  feePerGasWei: bigint | null,
): MaxCollateralResult | null {
  if (balanceWei === null) return null;
  const reserveIsFallback = feePerGasWei === null || feePerGasWei <= 0n;
  const reserveWei = reserveIsFallback ? FALLBACK_GAS_RESERVE_WEI : gasReserveWeiFromFeeEstimate(feePerGasWei);
  const maxWei = balanceWei > reserveWei ? balanceWei - reserveWei : 0n;
  return { maxWei, reserveWei, reserveIsFallback };
}

/** Exact decimal string for the Max collateral input, or null when there is
 * nothing safe to fill in (unknown balance, or balance below the reserve). */
export function maxCollateralInputValue(result: MaxCollateralResult | null): string | null {
  return result && result.maxWei > 0n ? weiToInputString(result.maxWei) : null;
}

export function formatWalletBalanceLabel(
  snapshot: WalletBalanceSnapshot | null,
  loading: boolean,
  invalid: boolean,
  hasWallet: boolean,
): string {
  if (!hasWallet) return "Not connected";
  if (invalid) return "Unavailable - wrong network or address";
  if (snapshot) return fmtCfx(snapshot.balanceWei) + " CFX";
  return loading ? "Loading..." : "Unavailable";
}

/** Formats a reserve amount for the Max-collateral note. `fmtCfx` rounds to
 * 4 decimal places for general display, which would mislabel any positive
 * sub-0.0001-CFX reserve as "0 CFX" - use the exact decimal string instead
 * whenever rounding would erase a genuinely positive reserve. */
function formatReserveCfx(wei: bigint): string {
  const rounded = fmtCfx(wei);
  if (wei > 0n && Number(rounded.replace(/,/g, "")) === 0) return weiToInputString(wei);
  return rounded;
}

export function formatWalletBalanceNote(result: MaxCollateralResult | null, invalid: boolean, hasWallet: boolean): string {
  if (!hasWallet) return "Connect a wallet to see your CFX balance.";
  if (invalid) return "Switch the connected wallet to the correct network and address to see your CFX balance.";
  if (!result) return "Balance unavailable - Max is disabled until a fresh read succeeds.";
  if (result.maxWei <= 0n) {
    return result.reserveIsFallback
      ? "Balance is too low to reserve the fallback " + formatReserveCfx(result.reserveWei) + " CFX for gas - Max is disabled."
      : "Balance is too low to reserve the estimated " + formatReserveCfx(result.reserveWei) + " CFX for gas - Max is disabled.";
  }
  return result.reserveIsFallback
    ? "Max reserves a fallback " + formatReserveCfx(result.reserveWei) + " CFX for gas because a live fee estimate is unavailable."
    : "Max reserves an estimated " + formatReserveCfx(result.reserveWei) + " CFX for gas.";
}
