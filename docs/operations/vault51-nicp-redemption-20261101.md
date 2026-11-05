# Vault 51 nICP redemption migration

Rob authorized migration of vault 51's existing 1,500 nICP into a protocol-controlled WaterNeuron withdrawal, keeping its outstanding icUSD debt and **the existing nICP collateral configuration and dynamic interest rules**, including after ICP arrives. He accepted suspension of liquidation and personally committed to covering any shortfall. This commitment is not additional on-chain collateral. No emergency mint, debt forgiveness, new token ledger or personal collateral payout is used.

## Implemented behavior

The existing vault retains its debt and collateral configuration. A dedicated Rumi subaccount receives only its existing nICP, less funding/approval fees. WaterNeuron burns from that subaccount and directs the ICP payout back to it. Ordinary borrowing, withdrawal, close, liquidation and redemption against this vault are fenced; debt repayment remains available. The fixed ICP receivable is priced using ICP/USD and the nICP haircut. It does not earn further nICP staking rewards. Settlement records actual received ICP, verified against the canonical ledger block and dedicated account balance.

Preparation checks the owner, exact collateral and debt ceiling and rejects existing payout/liquidation reservations. Each external action has a durable phase written before dispatch. Funding and allowance use immutable deduplication tuples. WaterNeuron's conversion has no idempotency parameter: after dispatch, every subsequent call only reconciles; it never burns again. Unknown outcomes stay fenced. A snapshot upgrade and ordered private replay preserve the claim and nonce counters; the public event variant remains compatible.

## Timing

Read-only observations on November 1: 1,500 nICP and about 2,871.08 icUSD debt in the connected vault view; WaterNeuron exchange rate 77,205,959 e8 gives approximately 1,942.86 ICP before deductions. The backing neuron's observed dissolve delay was 13,062,171 seconds, about 151 days. The interface's two-week label is not a confirmed settlement date. The actual withdrawal record and split neuron determine timing and amount.

## Software release and operator boundary

Backend: `tfesu-vyaaa-aaaap-qrd7a-cai`; frontend: `tcfua-yaaaa-aaaap-qrd7q-cai`; network `ic`. The fresh backend baseline is gzip SHA256 `6cfe35511bd36515977d2ef34f2b34af6a1d7a1003a04ea137b13e9f6467816b`, matched to the retained sunset/manual-liquidation release artifact. That already-deployed backend delta is preserved. Already-served sunset UI and 3USD wallet-balance fixes are also preserved; their served index was SHA256 `f47a4b1095a5fbc49853af59d0a537005bdf5d4e6501a20036391d5e93b04502`.

Installing software does not execute the migration. The developer must explicitly initiate it after release verification. Read `get_nicp_redemption_claim` before and after each step. `prepare_vault51_nicp_redemption` takes the verified owner, exact current collateral amount and a bounded maximum current debt (e8). Preparation creates a fence but moves no tokens. Each `advance_vault51_nicp_redemption` performs at most one next action:

1. `Prepared` → funding transfer → `Funded`.
2. `Funded` → exact allowance → `Approved`.
3. `Approved` → one WaterNeuron withdrawal dispatch → `WithdrawalSubmitted`.
4. `WithdrawalSubmitted` → independently verified request → `ClaimConfirmed`.
5. Later reconciliation verifies the payout block and balance → `Settled`.

An error is not permission to invent a new tuple, reapprove, reset the claim, or restore pre-transaction state. Do not retry a rejected/unknown operator action without checking its saved phase and original external receipt. In `WithdrawalSubmitted`, advancing only performs reads. Pending settlement requires later operator reconciliation; no timer automatically executes financial actions.

`Settled` remains in protocol custody under nICP terms. This release deliberately has no personal payout endpoint: releasing debt-free ICP or restoring ordinary vault operations requires a later explicit change. Do not advertise withdrawal availability. No production financial transaction is part of source/build validation.

## Validation

Focused backend claim/accounting/replay tests: 12 passed. Candid regeneration test passed; generated source and declarations agree. `didc check` against the freshly fetched deployed interface passed. Focused frontend and preserved-release regressions: 33 passed. Frontend production build and auth/domain asset check passed. Svelte check retains 30 existing errors in unrelated files; modified files have no errors. Independent custody and accounting reviews found no remaining concrete blocker after the preparation recheck, displayed-APR and public-event compatibility fixes. Real WaterNeuron redemption/settlement is not yet proven.

See [actual deferred work](../coordination/demo-deferred/vault51-redemption.md). Public notice: `/transparency#vault-51-redemption`; it distinguishes planned, uncertain, confirmed and settled states and discloses retained interest, illiquidity and the personal commitment.

## Published software readback

Source `240927a00f6d814a4ca8e09ad94d22884ba14894`, PR [#612](https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/612). Backend upgrade returned success; independent status shows Running with gzip SHA256 `c543cb1aed4a91f12b57994fe14c2fc7cba6c86e025e6dbecec3df7ec25ea2c1`. Raw artifact SHA256 `cb64e929b0ada1ac58321c1d08c57c40dfa10d12ec9f4511092014b53e40fe23`; executable code 8,600,472 bytes, 3,720,296 bytes below the network limit after the established 262,144-byte reserve. Embedded/source Candid match. Controllers and fixed allocation/freeze/log settings match preflight. Existing snapshots were preserved; none was removed or restored.

Frontend immutable batches 657–662 and final atomic batch 663 published index SHA256 `cf54006eda03a2b6ff960c0e21dd23b58067ee879dcb3f38215357418f4a41c5`. Readback preserved all 1,710 prior keys/properties, verified all 310 changed/new encodings, and checked served index/route bytes; 2,013 total keys. The browser displayed the notice with “Planned — no migration recorded.” Publication was then handed off to the separately authorized XRP frontend release; no further frontend writes are planned here.

Post-upgrade `get_nicp_redemption_claim` returned `null`. The narrowly scoped vault 51 query still showed 1,500 nICP and approximately 2,871.0917 icUSD debt (continuing interest). **No preparation, funding, approval or WaterNeuron withdrawal was dispatched.** The financial initiation is a manual user action, separate from this software installation. The prepared local runner stops on a call failure, typed rejection or unexpected phase and refuses to resume an existing claim.

Hosted frontend CI passed; the broader Stability Pool receipt job was still running at software publication. Main has no enforced branch status checks. Its pending result is not reported as a pass; local scoped tests and the reviewed exact artifact are the release evidence above.

The remaining Stability Pool receipt workflow subsequently completed successfully: [run 41339002142](https://github.com/RumiLabsXYZ/rumi-protocol-v2/actions/runs/41339002142), on the unchanged implementation commit. Later commits only record publication evidence.
