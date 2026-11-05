# Rumi top-priority release decisions — 2026-11-04

This document separates source delivery, canister upgrades, feature activation,
and financial recovery. It records the smallest current decision for the two
joint top priorities. It does **not** authorize an upgrade, recovery call,
reserve funding, signing, transfer, or unpause.

## Vault 158: upgrade-only recipient readback

### Pinned source and artifact

- Recovery source PR [#614](https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/614)
  merged as `99027b67a52324836dbba872a847843bf56821cc` from reviewed head
  `cff9f2a7d26c917431adcc8ce6db56dac42cbc09`.
- The liquidation-bot source, lockfile entry, and generated bot declarations
  are identical between that reviewed head and the merge commit.
- Retained raw release Wasm at the canonical build output
  `target/wasm32-unknown-unknown/release/liquidation_bot.wasm`: 2,356,524 bytes,
  SHA-256
  `db8931273a83bb3dfbd8c516b85808b24ad5bd9daed0df9f7265b440658fe098`.
  The file was rehashed during this pass. It is a reviewed candidate, not an
  installation receipt.
- Reported deterministic evidence: 63 liquidation-bot tests, generated Candid
  parity, `didc check`, `ic-wasm check-endpoints`, and the listed PR check passed.
  Two independent source-only merge reviews found no blocker. Neither review
  reproduced the test suite.

### Current live observation

At the 2026-11-04 controller-authenticated read-only preflight, the liquidation
bot was Running with module hash
`184fe54ee091935f4e5dd5331c58a2743ea4cfa80f6c55a18db13556cc8e19a7`.
An admin-authenticated query for `get_recovery_config` returned `IC0536` because
the method is not installed. Public Candid metadata likewise reflects the older
interface. The live bot therefore cannot yet prove its configured treasury
recipient through the new readback.

### Existing hold and smallest release action

PR #614 and `VAULT158_RECOVERY_RUNBOOK.md` explicitly stop at source merge and
separate installation from preparation and execution. No current approval
authorizes installing the candidate. The smallest useful live action is an
**upgrade-only** release by the existing bot release owner:

1. Recheck the old live module hash and the claim/payment/treasury journals.
2. Pause the worker, stop the bot, and wait for confirmed stopped/drained state.
3. Upgrade only the liquidation bot with the exact candidate hash above.
4. Restart it held, verify Running state, the exact new module hash, and
   `get_processing_paused = true`.
5. Call the admin-only `get_recovery_config` query and record the configured
   treasury, backend, ledgers, and fees.
6. Stop. Do not call `admin_prepare_vault158_recovery`,
   `admin_execute_vault158_recovery`, any reconciliation method, or unpause.

Upgrade approval must not be interpreted as financial-recovery approval. If
the configured treasury or any pinned ledger/backend field differs from the
reviewed expectation, preserve the hold and do not change configuration to make
it match.

An exact approval, if Rob chooses to grant it, can be limited to: install the
2,356,524-byte candidate with SHA-256
`db8931273a83bb3dfbd8c516b85808b24ad5bd9daed0df9f7265b440658fe098` on the existing
liquidation bot using the six steps above, with no recovery preparation,
execution, reconciliation, unpause, or funds movement.

## ckSOL processing paid in ICP: lifecycle decision

Draft PR [#610](https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/610) is at
`ac3b2d6872fabe731369b16d40d2d960f2a07817`; its two listed checks passed, but
GitHub currently reports it conflicting with main. Its reserve defaults to zero
and remains disabled.

The owner's evidence establishes one concrete lifecycle defect: a direct
running backend upgrade with an outstanding `ic-cdk` callback can trap and
leave the paid operation held. Three PocketIC controls support the distinction:
the direct-upgrade failure reproduced, stop-and-drain completed safely, and a
pending stop could be cancelled. The opt-in drained-upgrade helper passed seven
mocked tests, two scoped reviews, and a real CLI stop/upgrade/start on a
disposable idle canister. It cannot force unrelated deployment tools or hosts
to use it, and the exact original Rumi `Arc` panic stack remains unproven.

The narrow demo-first decision is whether to accept **stop and drain as the
mandatory release procedure** for this feature while deferring a durable raw
callback ABI or separate payment relay. The recommended disposition is yes:

- while callbacks may be outstanding, prohibit direct running upgrades of this
  backend and use the interim stop-and-drain release procedure;
- require the tested stop/drain wrapper and journal readback for its releases;
- keep the ICP processing reserve at zero through source merge and installation;
- treat reserve funding and a real-wallet canary as later, separately authorized
  activation steps; and
- keep the durable-callback ABI/relay investigation as deferred hardening, not a
  newly invented source-merge gate.

This recommendation does not make draft PR #610 merge-ready by itself. Its
owner must first refresh the branch against current main, re-establish the
changed-head checks and review evidence, and remove draft status. No current
decision authorizes installation or reserve funding.
