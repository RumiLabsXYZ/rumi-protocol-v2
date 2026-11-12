# XLM prepared settlement intent: source complete (UNBUILT)

Recorded 2026-11-12. Status: source accepted by independent review as an **unbuilt, dormant** candidate. Not compiled; the unit tests are **written, not run**. No production admission and no quorum proof is claimed.

## Code

Implementation commit `98b35c9a6f5bc167f468dd70f76cdb3bd913a1c9` on `codex/native-xlm-collateral`. Paths under `src/rumi_protocol_backend/src/`:

- `state.rs`:
  - `XlmClaim` gains `#[serde(default)] prepared_intent: Option<XlmPreparedIntent>`. Older stored claims without the field load as `None`.
  - New persisted type `XlmPreparedIntent` binding claim id, vault id, custody owner and nonce, network id, source public key and address, destination and its decoded key, gross, total fee (u32), net amount, observed and next sequence, min and max time, and the signing payload hash from the existing Stellar codec.
  - New `XlmPrepareError` reasons.
  - `State::prepare_xlm_settlement_intent` compiled only under `#[cfg(any(test, feature = "test_endpoints"))]`, wired to no endpoint. It requires a Pending claim with no prepared or signed intent; proves the source key against the custody address attributed to the claim's owner, nonce and network; requires the snapshot account to match; requires a fee that is nonzero, at most the reserved budget, below gross and within u32; derives the next sequence with overflow checks; validates time bounds through the codec; and only then stores the prepared intent in one step.
  - The signed `XlmSettlementIntent` type and `XlmClaimStatus` are unchanged; status stays Pending.
  - Focused unit tests in `mod xlm_prepare_intent_tests` (happy path binding every field, each rejection leaving stored claims byte-identical, double-prepare rejection, old-claim compatibility).
- `vault.rs`: `record_xlm_claim` initialises `prepared_intent: None` (production build).
- `chains/stellar/deposit_evidence.rs`: existing test literal initialises `prepared_intent: None`.

## Trust boundary

The account snapshot argument is plain parsed account data and is treated as an untrusted fixture. Its type does not prove that any provider quorum, freshness or finality check ran, and this function performs none. There is no signer, outcall, submission, endpoint or Candid change.

## Not yet built

- Production-grade account input: quorum and freshness binding for the snapshot.
- Balance coverage (account balance covers net payment plus fee) and destination-differs-from-source check.
- Clearing or re-preparing a stale prepared intent; reconciliation of unknown or expired submissions.
- Signing, submission, settlement status transitions and payout.

## Next essential validation (NOT RUN)

Needs its own allocation; nothing here has been executed.

1. `cargo test --locked --offline -p rumi_protocol_backend --lib xlm_prepare_intent_tests` (focused unit filter; compiles all library tests).
2. `cargo check --locked --offline -p rumi_protocol_backend --target wasm32-unknown-unknown` (production, features off).

## Unchanged

Full-withdrawal PASS (receipt `1caaefde39c3ed45b68c4d75027eb9cf065f22f36661ed4a6fca691ad7f3652a`) and earlier receipts stand. Production XLM remains Frozen. Settlement, payout and live behaviour are unproven.
