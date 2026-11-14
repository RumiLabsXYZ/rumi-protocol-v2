# Native XLM: testnet readiness packet

Living document, first written 2026-11-14 at source commit `d38b6e46`; last updated after the Horizon submission builder and parser source (`843e4e1c`, unbuilt). Updated as code progresses. Three columns matter: what exists in source, what is proven locally (mocked tests or unit tests with recorded receipts), and what has never run on a live network. Nothing here is live-proven.

## Where things stand

| Area | In source | Local proof | Live (testnet) |
|---|---|---|---|
| Custody address derivation and pending deposit (test build) | yes, test endpoints | PocketIC (address demo, readback) | not run |
| Deposit confirmation to vault | yes, test endpoint `confirm_xlm_deposit` with 3-provider Horizon quorum | mocked PocketIC PASS | not run |
| Horizon providers | compiled-in testnet SDF, Ankr, Tatum (Tatum keyless is throttled) | read-only public probes 2026-11-04; mocked PocketIC | quorum never exercised from a canister |
| Borrow, repay | existing protocol paths with test-only XLM exceptions | mocked PocketIC PASS | not run |
| Partial and full withdrawal to durable claim | test endpoints | mocked PocketIC PASS (receipts `4db74cac`, `1caaefde`) | not run |
| Unsigned settlement preparation | dormant test-only method | unit PASS (receipt `5f7f2a1d`) | not applicable |
| Settlement funding and sequencing preflight | dormant test-only method (`d38b6e46`) | unit PASS (receipt `0aa05cb5`, 15 tests incl. prepare regression) | not applicable |
| Signing the payment (threshold Ed25519) | `sign_payload_hash` exists, never called | none | not run |
| Recording the exact signed intent before submission | dormant test-only method (`a1760abb`) taking an untrusted fixture signature (no verifier available), marks claim InFlight | unit PASS with fixture signature (receipt `4e2ae10e`, 20 tests incl. regression) | not run |
| Submission to Horizon | dormant test-only request builder, deterministic response reduction and advisory parser (`843e4e1c`); no `main.rs` transform export, no driver, no outcall | written, not run (runner `67e9d11c`, 22 `horizon_rpc` tests); validation waiting for shared-machine capacity (free disk below the 30 GiB startup floor on 2026-11-14) and an existing coordination question | not run |
| Transaction status, replacement, reconciliation | pure helpers plus dormant test-only `apply_xlm_settlement_decision` (`f1f0ed82`) applying a fixture decision to an in-flight claim (settled, hold, quarantine, replace, failed fee charged once); no driver | unit PASS with fixture decisions (receipt `a65c3d4b`, 27 tests incl. regression) | not run |
| Claim payout | absent (no submission, so nothing is paid) | none | not run |
| Production quorum and freshness binding for account input | absent (inputs are untrusted test data) | none | not run |
| XLM price feed | absent (test price only) | none | not run |
| Liquidation and stability pool for XLM | absent | none | not run |
| Top-up of an open vault | absent | none | not run |
| Production endpoints and generated Candid | only key-name get/set and transform queries | n/a | n/a |
| Frontend | display-only references | n/a | n/a |

Production XLM stays Frozen; route guards are unchanged.

## Smallest code still needed before a testnet trial of deposit through payout

1. Signing: call the existing threshold Ed25519 signer on the prepared payload hash and feed the real signature into the signed-intent recording step (the recording step itself is written; real signing and signature verification are not).
2. Submission driver: send the signed envelope to Horizon and record the outcome without inventing finality.
3. Status and reconciliation: drive the existing transaction-status helpers to move a claim to Settled, or to a retry or quarantine path, including unknown and expired transactions.
4. Account input binding: feed preflight and preparation from the existing 2-of-3 account and reserve quorum instead of test inputs.
5. Test-build endpoints to trigger the above for the demo flow.

Each needs its own focused local validation before any testnet step.

## What a testnet trial would need (not authorized yet)

- A test-build backend installed on a testnet-keyed canister (not the production backend), using the test key `test_key_1` and the compiled-in testnet Horizon providers.
- Free testnet XLM from Stellar's faucet to fund a custody address (no monetary value).
- Test transactions, in order: open address, fund it from the faucet, confirm deposit, borrow and repay icUSD on the test canister's ledger, partial and full withdrawal to claim, then preparation, signing, submission and settlement of one claim payment to a test destination.
- Resources: one canister install with HTTPS outcall cycles for Horizon reads and one submission per payment; a fresh exclusive local build slot for the test Wasm.
- Authorizations required, each explicit: the testnet install, the faucet funding and the testnet transactions (signing and broadcast). None is granted by any approval recorded so far.

## Readiness signal

Testnet is ready to propose when items 1–5 above have passed focused local validation. That point has not been reached.
