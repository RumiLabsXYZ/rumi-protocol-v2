# Liquidation bot: ordinary swap input-fee accounting (2026-11-07)

Scope: `src/liquidation_bot` only. No Candid, stable-state, or config change.

## Problem

ICPSwap debits the ICP ledger input fee (0.0001 ICP) in addition to the swap
input amount. The ordinary liquidation path recorded `swap_input_fee_e8s: 0`
in its `BotPaymentJournal`, so the treasury residual
(`collateral_received - swapped`) was one fee too large per swap. The fee was
silently taken from the bot's shared ICP float. The bounded top-up path had the
same gap and also ignored the initial swap's fee when sizing its input.

## Invariant

For every ordinary claim: `swap input + its input fee` never exceeds the
claim's remaining net collateral, checked before external dispatch. The journal
records the sum of fees for swaps that actually confirmed, so the treasury
receives only the genuine residual:

    residual = net_collateral - sum(swapped) - sum(input_fees)

## Behaviour

- Initial swap is capped to `net - fee`; if the prepared swap still does not fit, it is refused as
  no-effect (the existing safe collateral-return path) without dispatch.
- Top-up is sized from `net - swapped - initial_fee`, bounded by its own fee,
  and re-checked against the prepared fee before dispatch. A non-fitting
  top-up is a no-effect failure (existing short-payment hold).
- Fees are added to the journal only after a swap is confirmed. An ambiguous
  outcome stays held in `SwapMayHaveStarted`; nothing is replayed.
- `icp_swapped_e8s` still records the actual swapped amount; no other field is altered.

## Unchanged

- Payment journals persisted before this release decode with
  `swap_input_fee_e8s = 0` and keep their old residual semantics; no fee is guessed.
- Vault 158 settled-recovery evidence, its tombstone, and historical journals.

## Expected effect

Treasury ICP bonus per ordinary liquidation is smaller by 0.0001 ICP per
confirmed swap (0.0002 with a top-up); shared float is no longer drawn.

## Validation and release artifact

- `cargo test --locked -p liquidation_bot --lib`: 67 passed, 0 failed.
- `cargo build --release --locked --target wasm32-unknown-unknown -p liquidation_bot`: passed.
- Wasm/Candid endpoint comparison passed, allowing only the six standard
  lifecycle/timer exports; no public interface changed.
- Two independent Claude Sonnet source reviews found no concrete blocking issue.
- Release artifact: 2,357,741 bytes; SHA256
  `256dc8101b0e0d616bb7c660b17168a8ad11d146869802bb483849c08875359e`.

The four added tests cover arithmetic and bounds. They do not simulate a live
ICPSwap execution. Generic delayed-receipt reconciliation remains deferred;
ambiguous swaps retain the existing operator-reconciliation hold. Source,
merge, installation and live processing evidence are separate.
