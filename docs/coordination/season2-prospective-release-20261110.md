# Season 2 prospective scoring release — historical backfill separate

Status: concrete prospective-only release packet; exact execution approval pending. This supersedes the requirement in the earlier decision/operations documents to finish historical reconciliation before activation. The old fixed-time approval request and estimated credits remain withdrawn. Funding is resolved; no creation, installation, credits or activation has occurred.

## Requested outcome and implementation

Rob directs ongoing Season 2 scoring to start without waiting for November 1 reconciliation. Source commit **719a1c0b**, on `codex/season2-accounting-20261110`, supports that split:

- Complete the existing separate-instance bootstrap with **zero historical points**. It supplies registration metadata, recorded 3pool composition and any proven live repayment windows, not estimated historical awards. Debt, SP and AMM balances are fetched by the existing scorer at actual captures.
- Before T0, initialize bootstrap with zero provisional rows. At/after T0, finalize its actual all-eligible row count before writing any row. This count can change only before the first imported row; it never changes T0, seed or source bindings. Fifteen retained registrations are a baseline, not an eligibility cap. Include qualifying entrants through T0 from complete source tails and keep post-T0 events for normal polling.
- The new admin-only deferred path uses separate stable memory 22. It is available only after a completed **zero-credit** bootstrap; a combined import that credited history cannot use it. It freezes history to **[November 1 00:00 UTC, T0)** and copies the bootstrap source bindings/cursors. It adds seven category totals to current totals, preserving existing registration, positions, repayment windows, epoch index, cursors, ledger and seed. A historical actor absent from the current registry may be registered from reviewed historical metadata; this does not replace any live participant state.
- Ordered batches are bounded, unique per participant, checked for overflow before writes, receipted and retry-safe across upgrades. Finish seals a domain-separated transcript digest without resetting prospective cursors. These checks bind the submitted data; they do not prove historical amounts. No historical-credit batch is authorized by this prospective release.

Prospective weekly epochs cover **[T0, ...)**. Later reviewed history covers the disjoint prefix **[Nov1, T0)**. Finalized S1 is untouched. Inherited exclusions, holding caps, rates, registration and actual future A/B captures remain in force. Token allocation, campaign end, claims and promotion remain deferred.

## Exact financial, target and artifact scope

One fresh canister only, selector `rumi_points_season2`, environment `mainnet-season2`, network `ic`. Existing S1 `bfnu3-6aaaa-aaaab-qhanq-cai` and all money-bearing canisters are outside the change scope. Root executes; Dorian coordinates the next slot after his approved Vault 51 release.

Sole controller/admin and payer: `rumi_identity`, principal `fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae`; default/null subaccount on cycles ledger `um5iw-rqaaa-aaaaq-qaaba-cai`. Observed funding balance is 4,300,650,044,313 cycles. The separate earlier canister top-up is not this account balance.

Create with **900,000,000,000 cycles**, including the selected subnet's **500B creation charge**, leaving **400B before installation**; ledger fee **100,000,000 cycles**. Total source debit is **900,100,000,000 cycles**, expected source remainder **3,400,550,044,313** absent intervening changes. No further funding, automatic top-up or spending of the whole account is authorized.

Pinned 13-node subnet: `jtdsg-3h6gi-hs7o5-z2soi-43w3z-soyl3-ajnp3-ekni5-sw553-5kw67-nqe`. Verified pricing revision in preparation: `83f29e0803496ba6aaea3d6b43f6c6bebea12dd9`. Recheck fee, eligibility, subnet size and deployed pricing before creation; drift requires reconciliation. Settings: sole controller above, zero reserved compute/fixed memory, 2,592,000-second freezing threshold, controller-only logs.

| Artifact | Pin |
| --- | --- |
| Optimized module | SHA-256 `2ffde290fb25fff915dcc2cfd68b97a0d6e9fb5827b180831a0339872642c603`; 3,044,642 bytes |
| Executable code section | 2,479,311 bytes |
| Deterministic gzip | SHA-256 `d301b54c5c2db14fa26baff7435f8152bbb8623866d3bf2fd703ae2b2c039d9c`; 753,351 bytes |
| Raw target exercised in PocketIC | SHA-256 `2d22cae99adfaf5ff39649f7df2740685acf25703f89e03dc86855e23978848e` |
| Public initial seed commitment H0 | `d050af464ea96252b37b21da99eaa7e7dd8291766627aee3eabd05610d0448b8` |

S0 stays private; reuse the already committed seed, with no reselection. Technical end is `u64::MAX`, not a decided campaign end. The old dc6b6f artifact and old executable init are superseded.

## Bounded release sequence

1. Record direct exact-scope approval. One initial release attempt is valid for 24 hours from that timestamp. Wait for Dorian's Vault 51 release slot to clear. Freshly verify account/principal, funding/fee, target mapping empty, S1 mapping unchanged, artifact and subnet/settings pins.
2. Use `scripts/prepare_season2_release_window.py` to derive T0 as the first UTC quarter-hour at least 60 minutes after fresh preflight, no later than approval +24 hours. Freeze and push the exact derived binding/init before installation. Its credit authorization stays false. Example renders grant no authority.
3. Create only the fresh S2 selector with 900B cycles on the pinned subnet. Save the creation/debit receipt and independently verify the returned ID, empty module, sole controller, settings and expected 400B before install. Unknown result means reconcile; never retry creation blindly.
4. Install only the pinned gzip on that empty ID in `install` mode using the derived init. Independently read back the decompressed module hash, admin, T0/H0/open end, source/asset bindings, exclusions, cycles, memory and both timers off. No reinstall, reset or substitution is included.
5. Before T0, call `begin_season2_import(0)`. At/after T0, use retained registration/3pool inputs plus **global source queries/tails only** to establish the full cutoff cohort and four exact next-event cursors. Preserve participant data locally. Do not repeat the previously denied operator queries addressed to participant principals. Seal the actual row count with `begin_season2_import(n)`, encode/verify every category amount is zero, and import bounded position/registration batches. Finish with the matching bootstrap transcript/source cursors. This marks the position bootstrap complete, not the historical reconciliation complete.
6. Verify private S0 against H0 and confirm the first A capture is still more than 30 minutes ahead. Check actual post-install memory/cycles against the existing 256MiB reserve envelope. Poll every configured source through the post-T0 tail, enable polling, call `start_season` with private S0 and verify epoch 0 bounds plus driver/poll enabled. The scorer's ordinary participant-addressed inter-canister reads for real A/B captures are part of this requested live scoring flow; no separate denied operator-query replay is included.
7. Read back the new ID/module, zero historical receipts, fixed T0, open epoch, source progress and enabled timers. Observe the first real capture when due; ordinary awards are proved at close, normally weekly. Keep all historical estimates and the old 20-point proposal out of this release.

A missed T0 before begin, insufficient first-A margin, expired window, uncertain mutation result or unreconciled current bootstrap input means HOLD on that attempt without retiming/resetting it. It does not make historical debt replay a release gate. Uncertain calls are reconciled from readback/receipts before any retry.

## Command binding

After approval, use the exact create command in the earlier command plan with the unchanged 900B amount, pinned subnet/controller/settings and `mainnet-season2` selector. Installation uses the same `install` mode on the newly verified empty ID, but the staged file is `.recovery-artifacts/season2-prospective-release-20261110/candidate.wasm.gz`, pinned to the gzip hash in this packet. Its args file is the newly rendered and pushed `season2-init.did` from the approved window directory. The old held init and old artifact must not be used. No whole-stack deployment, reinstall or S1 mutation is included.

## Verification and practical limits

The canonical local build passed. Generated Candid/declarations were regenerated from the Rust Wasm; native Candid equality and optimized-artifact strict Candid check passed. Eight focused Season 2 state tests pass, including S1 preservation, cutoff-cohort sealing, late additive totals, duplicate/overflow/authorization rejection and stable reopen. The existing PocketIC smoke passed on the final raw target: zero-credit bootstrap, upgrade, real prospective A/B/epoch close, later historical credit, second upgrade and identical retry with no double credit or epoch/ordinary-ledger rewrite.

The fixture has one synthetic debt position and partial event polling; it is not a sustained real-cohort cost measurement. Existing 15-person smoke/resource observations and the 256MiB reserve envelope remain contextual evidence, not a new full-workload claim. No matching hosted CI jobs are configured for this Points diff. Extra review campaigns and broader runs are deferred under the demo-first instruction; this packet preserves the enforced checks and actual action holds.

The inherited repayment decoder still lacks stable-token attribution in its source event. No qualifying stable repayment was proved in the examined history. Do not claim automatic 5x repayment coverage from that absent field: any such case requires ledger provenance and explicit accounting correction. This limitation and sustained workload measurement remain recorded for follow-through; they do not justify holding unrelated ongoing categories.

Historical reconstruction continues independently. The corrected price-aware diagnostic reduced global debt mismatches from 39/49 to 8/49, with two cohort residuals. It is not an approved award model. See the [retained-history evidence note](season2-analytics-coverage-20261110.md).
