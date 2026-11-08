# Liquidation bot fee-fix release and resumption — November 7, 2026

Rob authorized continuing this project using best judgment after the recommendation
to fix ordinary swap-input-fee accounting, verify/release, then resume the bot.
This receipt records that scoped continuation; it does not authorize unrelated
recovery actions, configuration changes or forced trades. Times below are UTC
(November 7 locally in America/Los_Angeles).

## Source and artifact

- [PR #629](https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/629) merged
  at `5ecd519648b64a91ec982ea731e11f61a709ddee` on November 8 at 00:33:28 UTC.
- Reviewed build source: `2edd5e01b9501d2349265674ec11f0aaa22add49`. Merged
  bot/backend source, Cargo files and toolchain had no differences from this build.
- Release Wasm: 2,357,741 bytes, SHA256
  `256dc8101b0e0d616bb7c660b17168a8ad11d146869802bb483849c08875359e`.
- 67 bot lib tests passed, including four new accounting/boundary tests.
  Locked release build and Wasm/Candid endpoint comparison passed. Two independent
  Claude Sonnet source reviewers found no concrete blocker. No CI jobs applied
  to the changed files; no required checks were disabled.
- No Candid or stable-state shape changed. Old journals keep their existing
  zero-fee semantics. New ordinary initial/top-up swaps must fund their input
  fees from their own claim, and confirmed fees reduce the treasury residual.

## Installation and preserved state

Target: `nygob-3qaaa-aaaap-qttcq-cai`, network `ic`, identity `rumi_identity`.
The worker was paused with an empty queue and empty bot/backend claim and payment
lists. The canister was stopped and its Stopped state verified before upgrade.
One upgrade call installed the pinned artifact; status readback showed the exact
new module hash while still Stopped. Starting it retained `processing_paused=true`.

At 00:43:32 UTC, recovery evidence, pending journals, recipient config and metrics
matched the pre-upgrade readbacks exactly. Vault 158 remained `TreasurySettled`,
history record 229 remained `Completed`, bot claim/payment lists were empty,
and treasury intent 229 was absent. The configured backend remained
`tfesu-vyaaa-aaaap-qrd7a-cai`; treasury remained `tlg74-oiaaa-aaaap-qrd6a-cai`.
Both cached ledger fees remained 10,000 raw units.

## Resumption

One `set_processing_paused(false)` call returned `Ok`. Readback at
00:44:09 UTC confirmed `false`. The upgraded module was Running.
The backend claim list remained empty. Bot ledger balances remained:

| Asset | Raw balance | Display balance |
| --- | ---: | ---: |
| ICP | 40,669,812 e8 | 0.40669812 ICP |
| ckUSDC | 26,587,885 e6 | 26.587885 ckUSDC |

These match the completed recovery balances; no additional recovery transfer
or forced canary trade was submitted.

Readbacks at 00:44:44 and 00:45:23 UTC spanned more than two 30-second
timer intervals after the first unpaused readback. Processing remained unpaused,
the queue stayed at zero, all recovery/config/journal readbacks were unchanged,
and the cumulative operation count remained 163. No new liquidation was observed.

## Limits and follow-up

The queue was empty: this proves installation, preserved state and unpause, not
a newly completed liquidation or live demonstration of the changed fee arithmetic.
The new tests cover accounting/bounds rather than an async ICPSwap integration.
An ambiguous swap still stays held for receipt reconciliation; it must not be
blindly repeated. Generic delayed-receipt reconciliation and clearer held-state
reporting remain follow-ups in `VAULT158_RECOVERY_LESSONS.md`, not additional
resumption gates. Vault 158's completed partial liquidation does not mean its
remaining debt is paid off or prove the remaining vault is healthy.
