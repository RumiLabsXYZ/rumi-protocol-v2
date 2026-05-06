/**
 * Oisy false-negative resilience.
 *
 * Background
 * ----------
 * Oisy's signer popup (ICRC-49 `icrc49_call_canister` flow) sometimes returns a
 * JSON-RPC error response of the form:
 *
 *   { code: 4000, message: "Cannot read properties of undefined (reading '_arr')" }
 *
 * even though the canister call itself was signed and submitted successfully.
 * The on-chain state advances; only the response Oisy hands back to the calling
 * page is malformed. This shows up in the slide-computer-signer client as a
 * `Po` error with that message, surfaced to UI handlers via `err.message`.
 *
 * Reproduction (verified 2026-05-05): borrow_from_vault from vault_frontend
 * with Oisy as the active wallet — call lands, frontend toast shows the
 * `_arr` error, refresh shows new borrowed balance.
 *
 * Mitigation
 * ----------
 * Wrap any write call that might be subject to this false-negative in
 * `callWithOisyFalseNegativeGuard`. If the call rejects with the known Oisy
 * pattern, run a verifier that checks on-chain state. If the verifier
 * confirms the operation landed, return the OISY_LANDED sentinel; otherwise
 * re-throw the original error.
 *
 * The verifier is operation-specific (e.g. "borrowed amount on vault N
 * increased") because canister state has no generic "did this op land"
 * signal. Each call site supplies its own verifier.
 *
 * This guard is a defensive workaround. Once Oisy ships a fix, the helper
 * becomes a no-op for that error pattern but stays in the codebase as
 * cheap insurance against future signer-side glitches.
 */

/**
 * Sentinel value returned by `callWithOisyFalseNegativeGuard` when the
 * underlying call rejected with the Oisy `_arr` pattern AND on-chain
 * verification confirmed the operation actually succeeded.
 *
 * Call sites detect this with `isOisyLandedSentinel(result)` and translate
 * it into a success response (typically without a block index, since the
 * normal response carrying that info was lost).
 */
export const OISY_LANDED = Object.freeze({ __oisyLanded: true as const });
export type OisyLandedSentinel = typeof OISY_LANDED;

/**
 * Substring of the Oisy error message we treat as a false-negative
 * candidate. Kept narrow on purpose so we never accidentally swallow
 * unrelated errors that happen to contain the word "undefined".
 */
const OISY_ARR_PATTERN = "Cannot read properties of undefined (reading '_arr')";

/**
 * Returns true if `err` looks like the Oisy `_arr` false-negative.
 *
 * The slide-computer-signer wraps the JSON-RPC error in a `Po` error and
 * exposes the raw message via `err.message`, so a substring match on
 * `.message` is sufficient. We also check `.toString()` as a belt-and-
 * suspenders fallback for cases where the message has been re-thrown or
 * concatenated.
 */
export function isOisyArrFalseNegative(err: unknown): boolean {
  if (!err) return false;
  const msg =
    (err instanceof Error && err.message) ||
    (typeof err === 'object' && err !== null && 'message' in err && typeof (err as { message: unknown }).message === 'string'
      ? (err as { message: string }).message
      : '') ||
    String(err);
  return msg.includes(OISY_ARR_PATTERN);
}

/**
 * Returns true if `value` is the OISY_LANDED sentinel returned by
 * `callWithOisyFalseNegativeGuard`.
 */
export function isOisyLandedSentinel(value: unknown): value is OisyLandedSentinel {
  return (
    typeof value === 'object' &&
    value !== null &&
    (value as { __oisyLanded?: unknown }).__oisyLanded === true
  );
}

/**
 * Run `call`, catching the known Oisy `_arr` false-negative.
 *
 * If `call` rejects with the Oisy pattern, run `verify` to check whether
 * the operation landed on-chain anyway. If `verify` returns true, resolve
 * with `OISY_LANDED`. Otherwise (or if `verify` itself throws) re-throw
 * the original error.
 *
 * Other errors propagate untouched.
 *
 * @param call    The write operation that might be subject to the Oisy false-negative.
 * @param verify  Returns true if on-chain state shows the operation succeeded.
 * @param opLabel Short human-readable label for logging (e.g. "borrow 1 icUSD from vault #51").
 */
export async function callWithOisyFalseNegativeGuard<T>(
  call: () => Promise<T>,
  verify: () => Promise<boolean>,
  opLabel: string
): Promise<T | OisyLandedSentinel> {
  try {
    return await call();
  } catch (err) {
    if (!isOisyArrFalseNegative(err)) {
      throw err;
    }

    let landed = false;
    try {
      landed = await verify();
    } catch (verifyErr) {
      // Verifier failure must NEVER mask the original signer error. Log and
      // re-throw the original so the user sees real failure rather than a
      // silent recovery that might be wrong.
      console.error(
        `[Oisy resilience] ${opLabel}: verifier threw, re-throwing original signer error`,
        verifyErr
      );
      throw err;
    }

    if (!landed) {
      throw err;
    }

    console.warn(
      `[Oisy resilience] ${opLabel}: signer reported _arr error but on-chain verification confirms the operation landed. Treating as success.`
    );
    return OISY_LANDED;
  }
}
