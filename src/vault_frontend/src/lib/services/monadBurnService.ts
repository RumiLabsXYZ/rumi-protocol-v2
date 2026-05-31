// monadBurnService.ts — Phase 1c "notify-then-verify" burn submission.
//
// After a user burns icUSD on Monad (chain 10143) via the IcUSD contract's
// `burn(...)` EVM call, the dApp submits the burn transaction hash to the
// backend so the canister can fetch the receipt, verify the Burn log was
// emitted by the configured icUSD contract, confirm finality, and decrement
// the chain-vault debt (via `submit_burn_proof`).
//
// The backend returns `ProtocolError::TemporarilyUnavailable(text)` while the
// receipt is not yet final ("receipt not yet final; retry") — that is a SOFT
// signal to poll again after finality lag. Any OTHER `Err` is TERMINAL
// (reverted tx, wrong contract, halt-class invariant, etc.) and must not be
// retried. `Ok(n)` is the count of burns newly applied (>= 0; 0 on a deduped
// re-submit).
//
// ──────────────────────────────────────────────────────────────────────────
// FUTURE ROBUSTNESS (flagged by Rob 2026-05-31):
//   This makes burn-state LIVENESS depend on the dApp staying open through tx
//   confirmation + finality. If the user closes the tab before the proof lands,
//   the burn is recorded on Monad but the canister never learns of it until an
//   operator submits the proof manually (`icp`/`dfx`) or enables the emergency
//   poll-scan (`set_burn_watch_poll_enabled`). Acceptable for v1; HARDEN LATER
//   with a relayer service or an incentivized permissionless submitter that
//   watches Monad and calls `submit_burn_proof` independently of any dApp.
// ──────────────────────────────────────────────────────────────────────────

import { Actor, HttpAgent } from '@dfinity/agent';
import { idlFactory as rumi_backendIDL } from '$declarations/rumi_protocol_backend/rumi_protocol_backend.did.js';
import type {
  _SERVICE,
  ProtocolError,
} from '$declarations/rumi_protocol_backend/rumi_protocol_backend.did.js';
import { CONFIG } from '../config';

/** Monad chain id (matches the backend's configured chain). */
export const MONAD_CHAIN_ID = 10143;

/**
 * Retry tuning. Monad blocks are ~0.5s and the configured finality depth is
 * small, so the receipt is usually final within a handful of seconds. We poll
 * a little slower than the finality interval and cap the total budget so a
 * stuck/terminal-but-misreported case can't loop forever.
 */
export interface SubmitBurnProofOptions {
  /** Delay between retries while the backend reports not-yet-final, ms. */
  pollIntervalMs?: number;
  /** Maximum number of attempts (including the first). */
  maxAttempts?: number;
  /** Optional progress callback, invoked before each retry wait. */
  onRetry?: (attempt: number, message: string) => void;
}

const DEFAULT_OPTIONS: Required<Omit<SubmitBurnProofOptions, 'onRetry'>> = {
  // ~finality interval: poll every 6s, up to ~3 minutes total.
  pollIntervalMs: 6_000,
  maxAttempts: 30,
};

export interface SubmitBurnProofResult {
  /** True if the backend applied (or had already applied) the burn. */
  ok: boolean;
  /** Number of burns newly applied by this call (0 on a deduped re-submit). */
  applied: number;
  /** Human-readable error on terminal failure or exhausted retries. */
  error?: string;
}

const sleep = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/** True when a `ProtocolError` means "not final yet — retry". */
function isRetryable(err: ProtocolError): boolean {
  return 'TemporarilyUnavailable' in err && typeof err.TemporarilyUnavailable === 'string';
}

/** Best-effort human-readable rendering of a terminal `ProtocolError`. */
function describeError(err: ProtocolError): string {
  if ('TemporarilyUnavailable' in err && typeof err.TemporarilyUnavailable === 'string') {
    return `Service temporarily unavailable: ${err.TemporarilyUnavailable}`;
  }
  if ('GenericError' in err && typeof err.GenericError === 'string') {
    return err.GenericError;
  }
  if ('ChainAdmin' in err && typeof (err as { ChainAdmin?: unknown }).ChainAdmin === 'string') {
    return `Burn verification failed: ${(err as { ChainAdmin: string }).ChainAdmin}`;
  }
  return `Burn verification failed: ${JSON.stringify(err)}`;
}

/**
 * Anonymous backend actor. `submit_burn_proof` is permissionless (the backend
 * verifies the on-chain receipt and dedups; the caller cannot forge a burn), so
 * an anonymous actor is sufficient and avoids a wallet round-trip per poll.
 */
function getBackendActor(): _SERVICE {
  const agent = new HttpAgent({ host: CONFIG.host });
  if (CONFIG.isLocal) {
    agent.fetchRootKey().catch((err) => {
      console.warn('monadBurnService: failed to fetch root key (local only):', err);
    });
  }
  return Actor.createActor<_SERVICE>(rumi_backendIDL as any, {
    agent,
    canisterId: CONFIG.currentCanisterId,
  });
}

/**
 * Submit a Monad burn tx hash to the backend and poll until the receipt is
 * final. Resolves once the backend returns `Ok` (burn applied or already
 * applied) or a terminal error, or once the retry budget is exhausted.
 *
 * @param chainId   EVM chain id of the burn (use {@link MONAD_CHAIN_ID}).
 * @param txHash    The confirmed `IcUSD.burn(...)` transaction hash (0x…).
 * @param options   Retry tuning + optional progress callback.
 *
 * TODO (wire-up): This is ready to use but not yet called from a UI flow —
 * the Monad burn UI does not exist in vault_frontend yet (no chain-vault burn
 * screen, no IcUSD.burn EVM call). Once that flow lands, call this right after
 * the burn tx confirms in the user's wallet, show "confirming burn on-chain…"
 * while it polls, and refresh the vault view on `ok` so the decremented debt
 * is reflected. See plan Task 7.
 */
export async function submitBurnProof(
  chainId: number,
  txHash: string,
  options: SubmitBurnProofOptions = {},
): Promise<SubmitBurnProofResult> {
  const { pollIntervalMs, maxAttempts } = { ...DEFAULT_OPTIONS, ...options };
  const actor = getBackendActor();

  let lastMessage = 'Burn proof submission did not complete';

  for (let attempt = 1; attempt <= maxAttempts; attempt++) {
    try {
      const result = await actor.submit_burn_proof(chainId, txHash);

      if ('Ok' in result) {
        return { ok: true, applied: Number(result.Ok) };
      }

      // 'Err' in result
      const err = result.Err;
      if (isRetryable(err)) {
        lastMessage = describeError(err);
        if (attempt < maxAttempts) {
          options.onRetry?.(attempt, lastMessage);
          await sleep(pollIntervalMs);
          continue;
        }
        // Budget exhausted while still not final.
        return {
          ok: false,
          applied: 0,
          error: `Burn not confirmed on-chain after ${maxAttempts} attempts; it may still finalize. Last status: ${lastMessage}`,
        };
      }

      // Terminal error — do not retry.
      return { ok: false, applied: 0, error: describeError(err) };
    } catch (e) {
      // Transient transport/agent error — treat like a retryable poll, but
      // still bounded by the attempt budget.
      lastMessage = e instanceof Error ? e.message : String(e);
      console.warn(`monadBurnService: submit_burn_proof attempt ${attempt} threw:`, e);
      if (attempt < maxAttempts) {
        options.onRetry?.(attempt, lastMessage);
        await sleep(pollIntervalMs);
        continue;
      }
      return {
        ok: false,
        applied: 0,
        error: `Burn proof submission failed after ${maxAttempts} attempts: ${lastMessage}`,
      };
    }
  }

  return { ok: false, applied: 0, error: lastMessage };
}
