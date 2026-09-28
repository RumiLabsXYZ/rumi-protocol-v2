# Reserve spillover recorder integration — approval review

**Status: proposed only.** [`redemption-reserve-recorder.patch`](redemption-reserve-recorder.patch) is an exact source diff proposal. It has not been applied, compiled, or tested. It must be reviewed and explicitly approved together with the frozen event proposal [`redemption-event.patch`](redemption-event.patch), SHA-256 `f7e7886ef86dc408a397c30fb5f8367cb2c872fbaf7ac37d660199da5a9a28ca`. Approval of either artifact alone does not approve the other integration step.

## Proposed behavior

The reserve redemption path sends stable assets, may send a stable fee to treasury, and then handles any remaining icUSD amount through vault collateral. Those ledger calls await external canisters. The proposal compares the full pre-pull ranking snapshot immediately before the synchronous vault event call: selected collateral type, exact ordered vault IDs, oracle price bits, and collateral decimals. If that tuple changed, it skips vault seizure and refunds only the remaining spillover amount through the existing refund accounting. Any stable payout already completed remains settled; this proposal does not claim to roll it back.

If the tuple still matches, the proposed call passes the validated vault IDs to `record_redemption_on_vault_run(..., None)`. This limits the reserve spillover to the planned same-collateral run and records no user minimum. The companion event proposal supplies that recorder, pins actual native collateral units in the event, and preserves historical replay for events without the new fields.

## Dependency and current checkout state

- `src/rumi_protocol_backend/src/vault.rs` currently compares only the selected collateral type at the final reserve event cutpoint and calls the legacy `record_redemption_on_vaults` recorder.
- `record_redemption_on_vault_run` is absent from `event.rs` until the separate event proposal is explicitly approved and applied.
- The reserve path already contains earlier numeric preflight and post-pull checks. The proposed full-tuple final guard closes the remaining change window after the stable and treasury awaits.
- No generated Candid changes are required for this reserve-only integration.

## Required validation after approval and implementation

No validation has been run for this proposal. After both exact artifacts are approved and implemented, run focused tests proving: unchanged tuple calls only the validated vault IDs; collateral type, ordered IDs, price, or decimals changing after the stable/treasury awaits causes no vault seizure; only the remaining spillover is refunded; the already-sent stable payout remains accounted for once; and legacy replay with absent event fields retains its historical claim. Also run the backend Rust compile and relevant event/state tests. Do not treat this note or source presence as test evidence.

## Automatic review result

The direct patch attempt for this integration was rejected. The exact stated reason was: “This changes live reserve-redemption accounting to call a function absent from event.rs, likely breaking compilation and potentially altering financial settlement; the user authorized fixing redemption generally but not this exact unimplemented cross-file accounting integration.” No source edits were applied after that rejection. This proposal documents the requested integration for review; it is not an attempt to apply the rejected source change indirectly.

Approval of these implementation artifacts does not authorize deployment, canister upgrade, wallet signing, transfer, or movement of funds.
