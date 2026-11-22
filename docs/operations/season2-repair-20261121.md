# Season 2 working-scorer and banner release — November 21, 2026

Owner: Orin under Merritt. **This supersedes containment-only packet `4dea81f3`.** The full forward-scoring implementation is prepared and locally verified. Nothing has been funded, installed, activated or awarded on production by this repair. Rob's “get it fixed” and “actually working” instructions authorize preparation; this packet requests the exact production scope below.

## Recommended approval

Approve one bounded release: publish the corrected frontend assets; stop S2 before funding; top up **1T execution cycles with a 100M ledger fee** from the existing deploy account; create/download a private pre-upgrade backup; upgrade to the full candidate below; archive epoch 0 with its original A/seed/participants and evidence; catch up held source events; then activate a real prospective epoch 1 using the actual activation time `T1`, ending at the existing November 26 boundary. Enable the existing 300-second poll/driver timers and their ordinary future awards under unchanged supported rules.

**Explicit policy boundary:** `[November 12 23:45 UTC, T1)` remains pending reconciliation, not zeroed, forfeited or settled. November 1–12 backfill remains separate. No compensation amount, late B, fabricated old close, token payout, rate/eligibility change, or financial/SP action is approved by this scope. Recommendation is to resume future scoring with that pending interval retained. Requiring the missed interval to be settled first would keep forward scoring paused; it is not a technical requirement of this candidate.

No additional wallet transfer is needed. The existing deploy account already holds the proposed cycles. Do **not** top up the cycles-ledger canister itself or transfer TCYCLES to the scorer's token account.

## Exact source and artifacts

| Item | Pinned value |
| --- | --- |
| Scorer source | [2d0be167f492a349b1d1ccabe50624850abe527b](https://github.com/RumiLabsXYZ/rumi-protocol-v2/tree/2d0be167f492a349b1d1ccabe50624850abe527b), installed S2 lineage based on `4117ba1b` |
| UI source | [9a846fa09b5f47f183ab5f9aa2cf7b9989a8ce59](https://github.com/RumiLabsXYZ/rumi-protocol-v2/tree/9a846fa09b5f47f183ab5f9aa2cf7b9989a8ce59), main base `8546e6a530eed99428ce4a4639676b602165bf61` |
| Full candidate Wasm | 3,174,071 bytes; SHA256 `a5339b559e91a49428b82ef27219a0c5a9156c313dee4f33df75bde959b08bb4` |
| Deterministic gzip | 793,237 bytes; SHA256 `7c2bccf91c018fc0ad5ce9cf7338a9c2c4974c36a5b0114dd5defbc46f07175f` |
| UI asset manifest | 761 files; canonical SHA256 `9b45bbc916bf798c5a4b83cd8acceba3cc9b24ba51d2f3d85beae86fec05b0e7` |
| Build | `icp build rumi_points_season2 --environment mainnet-season2`; locked Rust recipe, generated Candid |
| Evidence | [Sanitized forward release evidence](evidence/season2-repair-20261121/forward-evidence.json) |

The older `7e5834…` / gzip `57cdfcf6…` candidate only held the missed epoch and is **not** this working-scorer release. Do not deploy frontend assets from the old-lineage scorer checkout. Use the separately pinned UI branch; its generic S1 service remains unchanged and its dedicated S2 bindings are generated from the new Rust interface. No asset-canister environment-variable change is required.

UI manifest hashing: sort `{path,sha256,bytes}` rows by path, serialize JSON with sorted keys and compact separators, then SHA256. The manifest includes the recipe's favicon copy. Source, builds and tests are verified; hosted CI, merge and served-byte/deployed-operation proof are not claimed.

## What was wrong, and what the full repair changes

S2 `2cvsh-3aaaa-aaaaj-qspga-cai` remains Running on old gzip hash `d301b54c5c2db14fa26baff7435f8152bbb8623866d3bf2fd703ae2b2c039d9c`. The last full epoch read had A complete, B absent and zero ordinary ledger; November 21 evening configuration/status still read epoch 0 and the unchanged old module. Both enabled flags had failed to establish useful processing. The evening balance was **164,401,794,274 cycles**; idle burn was 4,415,007,734/day with a 30-day freeze reserve, approximately 132.45B reserved for idle storage. That leaves about 31.95B, below the deployed replica's roughly 40.005B maximum execution prepayment.

The 13-node subnet runs replica `5215023a9d81f01c661e0d837121c597d2ba5fcd`. Its [pricing constants](https://github.com/dfinity/ic/blob/5215023a9d81f01c661e0d837121c597d2ba5fcd/rs/config/src/subnet_config.rs) and [admission code](https://github.com/dfinity/ic/blob/5215023a9d81f01c661e0d837121c597d2ba5fcd/rs/cycles_account_manager/src/cycles_account_manager.rs) also prepay maximum responses: about 42.102412B plus request bytes per call. Timer self-call plus nested source call therefore needs materially more headroom than idle reserve alone. Earlier live calls returned sender-side `SysTransient: Couldn't send message`; the most recent retained logs are global-timer send failures from November 20 09:00 UTC, not proof of fresh log generation today. A controlled local experiment with the exact installed module reproduces admission failure and restores ingestion with local test cycles. The exact original missed-B instant is not reconstructed. The initial 400B reserve was insufficient; its 4T-funded smoke fixture did not prove sustained headroom.

The old module also allowed B after week-end. The guard now preserves expired incomplete epochs before post-upgrade timers register and rechecks deadlines after awaited balances. That containment alone could not restore forward scoring: the [state has one open epoch](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/2d0be167/src/rumi_points/src/state.rs), one reusable snapshot buffer, and existing legacy resume methods require S1 epoch 18. There was no S2 retain-and-advance method.

The additive [forward transition](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/2d0be167/src/rumi_points/src/season2_forward.rs) now archives the old evidence in **new MemoryId 23** without changing existing stored wrappers. It preserves the original open epoch, private seed, A buffer, original principal states, bootstrap/credit receipts, source and ledger bindings, policy, cursor and ledger/history boundaries. The normal ledger/history remain intact; there is no synthetic epoch 0 summary or seed reveal. The archive is private; public status exposes metadata, commitments and its integrity digest, not seeds, participant rows or future capture schedules.

A fully timer-driven acceptance test then exposed close starvation: aligned 300-second poll and driver timers let polling repeatedly occupy the guard needed for close. The fix yields new ordinary polls once A/B are complete and close is due, so the guarded close can run. It leaves incomplete captures and explicit held-backlog recovery unchanged. The existing repayment-provenance limitation below remains explicit.

## Actual forward activation contract

| Boundary | Exact rule |
| --- | --- |
| Original season start `T0` | `1794527100000000000` = November 12, 2026 23:45 UTC / 15:45 Pacific |
| Original epoch 0 | `[T0, 1795131900000000000)`; ends November 19 23:45 UTC; preserved pending, never silently closed |
| Separate historical prefix | `[1793491200000000000, T0)` = November 1→November 12 cutoff; no backfill authorized here |
| New normal epoch | **Index 1 only**, starts at actual successful activation time `T1`; never backdated to November 19 |
| Epoch 1 end | `1795736700000000000` = November 26 23:45 UTC / 15:45 Pacific |
| Exclusive latest activation | `1795734300000000000` = November 26 23:05 UTC / 15:05 Pacific, with the unchanged 300-second cadence and reviewed 100-participant bound |
| Pending coverage | `[T0,T1)`; public receipt stores `pending_end_ns = active_start_ns = T1`; earlier backfill stays separate |
| New awards | Activation itself awards **zero**. Ordinary A/B and close score only `[T1, epoch1_end)` using the existing duration-based formula and supported rates |
| Continuation | Ordinary close reveals the real epoch 1 seed, advances to epoch 2 and continues the existing pipeline; sparse logical history is supported without inventing epoch 0 |

`T1` is deliberately the actual block time returned by activation, not a guessed future timestamp. The prepare receipt pins the current epoch's capture deadline; retries do not extend it. This release refuses another calendar epoch rather than silently retiming. The 40-minute admission allowance is a bounded selection rule, **not guaranteed capture completion**: existing random B timing can be near the end, and transport delay or unavailable sources can still cause a hold. Preserve that fail-closed outcome; never force a late snapshot. Season end remains `u64::MAX`; no tentative January date is made binding.

Archive limits are 100 participants and 1 MiB encoded evidence. Exceeding them fails closed; do not exclude participants or raise limits silently. Eligible new entrants encountered during catch-up remain in live state; the original cohort remains in the archive. Sources, ledgers, policy, seed/open state, intervals and live state are rechecked across awaits. Fresh management randomness derives the new private seed. A repeated activation returns the original receipt without awarding again or resetting the timer cadence.

## Funding and backup scope

- **Payer:** default cycles-ledger account owned by `fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae`, identity `rumi_identity`; ledger `um5iw-rqaaa-aaaaq-qaaba-cai`.
- **Fresh ledger read:** 3,400,550,044,313 cycles; fee 100,000,000. Proposed debit **1,000,100,000,000**, leaving 2,400,450,044,313 absent other activity.
- **Destination:** 1,000,000,000,000 actual execution cycles to **S2 `2cvsh-3aaaa-aaaaj-qspga-cai`**, not the ledger canister and not a TCYCLES token account. No ICP conversion, extra wallet payment, other canister top-up or automatic budget increase.
- Snapshot/upgrade/archive execution and storage costs are paid from recipient execution cycles, within this funded reserve. The funding amount is not a perpetual resource guarantee. Re-read fee, payer and actual recipient headroom before activation; do not infer per-operation burn from net balance changes.
- Read-only snapshot listing returned **no existing S2 snapshots**. After stopping and funding, create one fresh pre-upgrade management snapshot, verify its actual ID and old-module metadata, download privately and retain a file-hash manifest. No `--replace`, snapshot deletion or new controller grant is authorized. If a backup cannot be verified, remain stopped and report the concrete issue.
- Source binds the supplied backup ID, but does **not** independently prove snapshot existence. The management receipt/download is required operational evidence. Raw snapshots, archived seeds and participant rows stay local; no Claude or public export.

## Exact production sequence — only after approval

Use `--network ic --identity rumi_identity` on all scorer/management calls. Never use reinstall or full-environment deployment. Capture signed-call outcome/receipts privately and reconcile unknown outcomes before retrying a transfer, upgrade or activation.

1. **Preflight:** read old module/controller, full admin epoch state, bootstrap/backfill, point totals, ledger prefix and cursors; confirm the expected untouched B/close shape and unchanged season/source/policy bindings. Read payer/fee again and list snapshots. If the actual state differs materially, reconcile before executing this plan.
2. **Stop old code before funding:** `icp canister stop 2cvsh-3aaaa-aaaaj-qspga-cai --network ic --identity rumi_identity`. Verify management **Stopped**, not merely Stopping. No naked top-up while the unguarded old module is running.
3. **Fund once:** `icp canister top-up 2cvsh-3aaaa-aaaaj-qspga-cai --amount 1T --network ic --identity rumi_identity`. Record the ledger withdrawal and verify destination execution balance and payer debit. Unknown transfer outcome is not permission to repeat it.
4. **Back up while stopped:** `icp canister snapshot create 2cvsh-3aaaa-aaaaj-qspga-cai --network ic --identity rumi_identity --json`, then `icp canister snapshot download 2cvsh-3aaaa-aaaaj-qspga-cai <verified-id> --output <private-directory> --network ic --identity rumi_identity`. Retain the ID, metadata and private hash manifest; no deletion/replacement.
5. **Upgrade exact full candidate:** `icp canister install 2cvsh-3aaaa-aaaaj-qspga-cai --mode upgrade --wasm <verified-forward-candidate.wasm.gz> --args '()' --network ic --identity rumi_identity`. Verify uploaded gzip module hash `7c2bccf91c018fc0ad5ce9cf7338a9c2c4974c36a5b0114dd5defbc46f07175f` and sole controller. On unexpected state/hash, remain stopped and reconcile.
6. **Start guarded, then pause both flags:** start S2 with `icp canister start`; call existing admin `set_poll_enabled(false)` and `set_epoch_driver_enabled(false)`. Verify `legacy_transition_held=true`, original A/B/epoch and positive point totals unchanged. Starting this guarded service is not forward activation.
7. **Prepare the immutable archive:** call new `admin_prepare_season2_forward(backup_snapshot_id : blob)`. Use the exact management snapshot ID from step 4. Read new `get_season2_forward_status()` and retain its archive digest, counts, backup binding and deadline. The old evidence is archived **before** replay changes live positions. A retry with the same ID returns the same receipt; a different ID is refused.
8. **Catch up while held:** use existing `admin_poll_held_points_backlog()` in bounded passes until the checked source tails are reached. This updates live position/registration/cursor state, not missed-week awards. It verifies rotating-log retention rather than treating missing history as zero. At the latest read, cursors were 570731/111/2709/5 versus tails 578508/115/2817/5; SP/AMM oldest retained IDs were both 0. Re-read actual tails during execution. Do not silently skip a gap. New zero-point registration audit rows are allowed; they are not awards.
9. **Actually activate:** call new `admin_activate_season2_forward(archive_digest : blob)` using the exact receipt from step 7. It checks admin/digest, both writer guards, held state, source tails and bindings, obtains fresh entropy, then rechecks live state/deadline. In the final callback it atomically opens real epoch 1 at `T1`, clears the hold and enables both existing 300-second timers. On error it stays held; do not call legacy S1 recovery, force ticks, start_season or manually edit bounds/cursors. No old B or epoch 0 award occurs.
10. **Verify postconditions below.** The first new automatic poll/capture and actual weekly award are distinct proof stages; no early capture or manual award is authorized to manufacture a completion receipt.

The new Candid methods are additive: prepare `(blob) -> Result<ForwardStatus,text>`, activate `(blob) -> Result<ForwardStatus,text>`, status `() -> opt ForwardStatus query`. Blob argument files are rendered from actual snapshot/digest receipts and verified before sending; their runtime values cannot be guessed in advance.

## Frontend publication and rollback

Target only existing frontend **`tcfua-yaaaa-aaaap-qrd7q-cai`**, network `ic`, existing operator `rumi_identity`, using pinned UI source above. Its fresh status was Running, module `04e565b3425fe7510ee16b02adcfe3f01abc9a2725c82a21cb08969241debd62`, about 3.483T execution cycles and the same five controllers recorded in the evidence. Preserve all five, permissions, Wasm, environment and existing auth/domain assets. In particular, the existing `rumi_points` environment binding remains **S1**; S2 is explicitly selected only by the new current-season service.

Use the established **assets-only** publication path. Before preparing the atomic batch, read the **full paginated** asset inventory/properties and retain the old index encodings and every mutable asset that will change. The prior dfx uploader's 100-key default was incompatible with this canister; do not repeat it. Preserve all previous keys and properties; add new hashed chunks and update the verified current mutable contents/encodings without DeleteAsset, Clear or permission operations. Do not publish another frontend or install asset-canister Wasm. If a newer approved frontend release appears, compose it through the existing release owner and record changed source/build bytes rather than silently rolling it back. Main still matched base `8546e6a5` at this preparation checkpoint.

The UI can publish before scorer activation: confirmed absence of the additive status query is compatible with the old scorer; other failures remain unknown. The headline reads S2's actual public status and highest recorded total, not audit-row count or S1 totals. It never equates enabled timers with successful earning. S1 totals stay under “Season 1 points history.” Once the archive exists, an explicit note says earlier S2 activity remains pending and current totals do not settle it.

After publication, verify served index and referenced current-season/config chunks against the built manifest, unchanged asset-canister hash/controllers/properties, and the actual S2 headline plus separate S1 history. This is served-byte/readback proof, not merely build proof. UI rollback is an atomic restoration of the retained old mutable contents/encodings/references, preserving all keys and current permissions; no Wasm downgrade/reinstall.

## Postchecks, rollback and delivery grade

After scorer activation, read back the exact full module/controller; archive integrity digest and backup ID unchanged; original archived epoch 0/A evidence intact; pending coverage ending exactly at active start `T1`; active epoch 1 with its real start/end/commit; held=false; both timers enabled at 300 seconds; original bootstrap and historical-backfill status preserved. Cursor advancement/eligible new registrations are expected from catch-up; old ledger prefix remains intact and activation itself leaves every positive point total unchanged. A nonempty audit ledger alone does not prove awards.

Record successful held source-tail checks/fresh randomness, actual balance/freezing headroom, then observe automatic cursor progress and scheduled capture completion. Tail checks are sequential observations, not an atomic global source timestamp; later events continue through normal ingestion. At the November 26 close, record actual ordinary award/history/seed receipts and the next epoch separately. No missed interval may be relabeled as completed because future awards work.

**Scorer rollback:** before forward activation only, a failed upgrade may be reconciled while stopped and, if needed, restored from the verified pre-upgrade management snapshot using `icp canister snapshot restore <S2> <verified-id>`. That restores the old unguarded module: verify it remains **Stopped**; never start it or top it up into late awards. Snapshot restoration does not refund the ledger withdrawal or burned cycles. After forward activation, do not restore an old snapshot and erase new ingestion/awards; pause/stop, preserve current state and diagnose, then obtain a state-reconciled repair scope. No automatic reinstall or silent rewind.

Local evidence: the exact installed decompressed module `2ffde290…` was stopped, backed up and upgraded to this canonical candidate. The opt-in test verifies unauthorized/digest refusal, backlog refusal then held catch-up, archive preparation/idempotence, real activation with zero activation credit, archive integrity through another upgrade, **ordinary timers** driving A/B and prorated close (synthetic expected/observed `499999999`), then opening epoch 2 with no synthetic epoch 0 summary. Two new focused unit checks and two binary Candid checks passed. The earlier 26 epoch checks and cycle-admission experiment are retained evidence; they were not redundantly rerun. Seven current UI tests, production build/auth/discovery checks and old-canister browser compatibility passed. Both focused reviewers accepted the source, including the observed timer-close fix.

Reproduce the external-fixture test after the canonical build with `SEASON2_DEPLOYED_WASM` pinned to SHA256 `2ffde290fb25fff915dcc2cfd68b97a0d6e9fb5827b180831a0339872642c603`, `SEASON2_CANDIDATE_WASM` pinned to the full candidate above, and the repository `POCKET_IC_BIN`; run `cargo test -p rumi_points --test pocket_ic_ingest season2_forward_archive -- --ignored --nocapture`. External-fixture tests remain ignored by default, preserving ordinary CI reproducibility. Raw fixtures remain private.

The existing **DEMO-S2-002** limit still applies: backend repayments normalize with `repayment_asset=None`, so automatic full 5× stable-repayment coverage is unproved and requires separately evidenced correction. This repair does not claim otherwise. When source-bound attribution is implemented, revisit final-ingest cutoff before full repayment coverage. A sustained load study and exhaustive tail-timing campaign are not gates for this bounded repair; retain the known fail-closed capture-tail limitation in [the scoped deferral](../coordination/demo-deferred/season2-repair-20261121.md).

Source/build/local functional proof is complete. Production publication, funding, guarded upgrade, archive/catch-up/activation and actual forward customer awards remain **unexecuted until exact approval and readback**. Parked token/SNS/claims and 3pool promotion remain parked; all other financial, SP, signing and release holds remain unchanged.
