# Season 2 bounded release decision — November 10, 2026

Status: proposed; no approval, canister creation, installation, credit import or
activation is implied by this file. Owner: Orin; release operator: Dorian.

## Requested result and exact scope

Create one fresh Season 2 Points canister, preserve finalized Season 1, credit
the continuous historical interval beginning **November 1, 2026 00:00 UTC**, and
start the existing prospective scoring engine. Token allocation, claims,
promotion and the campaign end date remain separate decisions. No token moves
or changes to existing money-bearing canisters are included.

Proposed prospective cutoff **T0 = November 10, 2026 19:00 UTC (11:00 Pacific)**,
`1794337200000000000` nanoseconds. Historical credits cover `[November 1, T0)`;
ordinary prospective epochs begin at T0. Approval must arrive by 18:45 UTC to
leave setup time. A late approval or missed first capture keeps execution held;
the operator may not silently move the approved cutoff or invent a capture.

## Historical accounting choice

No Season 2 random A/B snapshots were captured during the historical interval.
The proposal uses a disclosed continuous USD-day model with inherited source
weights: debt, icUSD 3pool and icUSD Stability Pool 1x; fiat-stable 3pool 4x;
3USD Stability Pool and AMM LP 2x; verified qualifying stable repayments 5x.
This model is not a claim to reconstruct random captures that never occurred.
Each interval is floored in raw points at the existing `1e8` scale.

Use the existing 15-participant cohort and preserved registration evidence.
Refresh source observations through T0; do not default an unavailable input to
zero. If the cohort changes, another unresolved case appears, source coverage
cannot be reconciled, or proposed historical credits exceed **1,000,000 points**,
hold import and activation and report the changed facts.

- **Pool positions:** carry the accepted frozen recorded composition only while
  complete liquidity/LP-transfer evidence shows no change. Apply the source's
  shared verification cap to the combined wallet/SP/AMM 3USD holding. Positive
  yield donations can raise virtual price without a swap/liquidity record, so
  use the observed endpoint VP as the conservative historical model, under the
  verified fixed amplification and unchanged supply assumptions. At the sample
  cutoff this yields exactly the same category credits as the last-event VP
  model; the raw VP history is not asserted complete.
- **Debt, fourteen participants:** use observed endpoint debt reversed by exact
  subsequent principal changes, allowing nonnegative interest to be credited
  early. Validate against actual saved anchors and lifecycle evidence. Exclude
  the unreconciled canonical replay values: that diagnostic differs from live
  debt at the identical event cursor. Endpoint/capture timing and any events
  between T0 and readback must be included in the reconciliation.
- **One legacy debt case:** the reduction of fourteen old vault balances has
  no timestamp reconstructed from the retained public events, and a separate later vault has an observed borrow
  and successful Stability Pool liquidation. Holding the old balances and the
  later vault's final observed debt over its lifetime gives **19.01356466 points**
  through the sample cutoff. Propose **20.00000000 debt points** for this one case,
  including a disclosed **0.98643534-point** make-whole allowance. This is a
  policy choice, not proof of exact interest or a new multiplier. Refresh the
  same model to T0; if its value exceeds 20 points, hold rather than truncate it.
- **Stability Pool icUSD:** use per-participant upper balance envelopes bounded
  by actual anchor/current positions, complete positive-interest events and
  possible liquidation losses. Public interest records omit collateral-specific
  recipient allocation. Upper envelopes deliberately favor participants and may
  overcompensate; do not call them exact allocation replay. Include events up to
  the endpoint observation when bounding a readback taken after T0.
- **Verified zeros:** AMM LP positions are zero with complete event/readback
  evidence. One unique repayment was found and matched to its actual icUSD
  ledger burn; it is not a qualifying ckUSDC/ckUSDT repayment. The zero stable
  repayment category is based on that ledger proof, not just the decoder's
  currently missing asset tag. Repeat coverage to T0; any new qualifying asset
  requires its ledger proof and the inherited repayment-window calculation.

### Concrete sample, exclusive cutoff November 10 at 16:00 UTC

These are the proposed upper-model credits, not installed awards:

| Category | Points |
| --- | ---: |
| icUSD debt, including the 20-point legacy case | 189,587.58316022 |
| icUSD 3pool | 87,826.63882752 |
| Fiat-stable 3pool | 287,355.12104224 |
| icUSD Stability Pool | 49,919.63777925 |
| 3USD Stability Pool | 282,886.55962675 |
| AMM LP | 0 |
| Qualifying stable repayment | 0 |
| **Total across fifteen participants** | **897,575.54043598** |

The SP icUSD envelope spans 49,782.92160515–49,919.63777925 points; the proposal
selects the upper values per participant. Private participant rows and raw source
archives are retained off the public branch. The complete sample transcript has
SHA-256 `49a509099127a26f969c289b0ee782c0d47d6c5c0c10df17745a8a8f32038cbe`.
A real fifteen-row sample batch encodes against the candidate Candid in 1,645
bytes; it has not been sent. The exact T0 transcript must be recalculated,
cross-checked and recorded using the approved methods and cap before import.

## Bounded creation, install and start

Use only `rumi_points_season2` in `mainnet-season2`, with its separate initially
empty ID mapping. The target is solely the newly returned canister ID.

- Verified deploy cycles-ledger balance: **4,300,650,044,313 cycles**, including
  the received 4T credit. Use **900,000,000,000 cycles** for creation on the
  pinned 13-node subnet in the release-preparation packet. The 500B creation
  charge leaves **400B** in the new canister; the separate ledger fee is **0.1B**.
  Expected deploy-account remainder: **3,400,550,044,313 cycles**. No other
  funding movement or automatic top-up is authorized by this proposal.
- Sole controller/admin: the existing `rumi_identity` principal in the frozen
  init/settings file. Zero reserved compute/fixed memory allocation, normal
  30-day freezing threshold, controller-only logs. Recheck actual subnet pricing,
  identity and funding before execution.
- Install only the reviewed optimized Wasm SHA-256
  `dc6b6fe48e79571d0bbe371b9fb8854bc77207abf3b5005c0eda7455388a9c17`,
  with the explicit T0, technical open end `u64::MAX`, existing protocol
  exclusions, mainnet source/asset bindings, and public commitment H0
  `d050af464ea96252b37b21da99eaa7e7dd8291766627aee3eabd05610d0448b8`.
  The readable init encodes to 139 bytes, SHA-256
  `be20ba1f4f068c00dd0a503942da86c0f0b948f853fae65e2c192cddae535c3a`.
  S0 remains private. Read back controller, module hash, config, resources and
  disabled timers before importing. The technical open end decides no token
  economics or campaign end date.
- Read the known cohort's public canister positions for reconciliation and
  ordinary scoring. This explicitly includes transmission of their principal
  identifiers to the configured protocol, pool, ledger and AMM query endpoints;
  raw participant data stays out of public repository/board updates.
- Begin the stopped fresh-instance import before T0. Freeze source cursors at
  the exclusive historical boundary, preserve participant state, then import
  the reviewed fifteen-row transcript at/after T0. Read back every receipt and
  total, verify the server's transcript digest against the intended source
  bindings/cursors, and finish only with no unresolved categories. No S1 reset,
  rerun credit, fabricated ordinary ledger entry or cross-boundary double credit.
- Enable polling and start the committed prospective epoch only on that new
  canister, after historical completion and a resource readback that supports
  the normal freezing reserve. The operator must compare the current clock with
  the private first-capture time immediately before start; the current start API
  does not enforce the missed-capture hold. Verify successful source polling, open epoch,
  scheduled driver and imported totals. Ordinary weekly epoch awards are only
  claimed after their actual close; running timers alone are not awarded points.

The source is in [PR 630](https://github.com/RumiLabsXYZ/rumi-protocol-v2/pull/630).
Canonical build/Candid checks, focused state/auth tests, sixteen calculator
tests, and the existing one/fifteen-participant import-upgrade-first-epoch
fixtures passed. No matching hosted CI jobs were configured for this diff;
there is no hosted-CI success claim. Sustained real-workload costs remain
unmeasured; use actual post-install readback rather than promising runway.

The [release-preparation packet](../operations/season2-release-preparation-20261110.md)
contains funding, subnet/settings and initialization evidence. Existing financial,
signing, release and activation holds remain until explicit approval of this
bounded decision. This proposal also requests normal source review/merge of
the scoped PR; it does not authorize changes to other canisters or token work.
