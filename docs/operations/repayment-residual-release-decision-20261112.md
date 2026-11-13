# SEC17 repayment residual release decision — HELD

Proposed single decision: authorize Dorian to coordinate, build and release only
exact source **00cf550d3fcb97870cd4643883d5abd8900fa1a2**, branch
`codex/repayment-residual-forgiveness-20261112`, with the bounded sequence below.
Source receipt descendant061af37b changes documentation only. No production
backend or frontend build exists for this candidate yet; hashes below describe
inputs, never an invented output. Previous137e8e71 approval covered source240 UI
only and is not reused. Existing production730f5471/backend and1794522798107/UI
remain the expected baselines until fresh preflight confirms them.

## Behavior being approved

Ordinary repayment minimum is fixed0.1icUSD. After a successful authorized
repayment, accrue callback-time interest, then forgive a positive residual
**strictly below0.1icUSD**; exactly0.1remains. Actual debit is capped by the
user's authorization/current debt, with no percentage upward snap. Only paid
interest earns distributions; forgiven debt is booked separately in the existing
forgiveness total. Forgiveness does not automatically withdraw collateral or
close the vault. The coupled UI/Max/API validation and preview reflect this rule.

**Economic cost:** each eligible settlement may write off up to0.09999999icUSD
without a matching token burn or stable deposit. Separate repayments/vaults can
repeat this cost; no aggregate cap is added. Depending on configuration, the
write-off may exceed fees. This is the explicit requested threshold, not new
backing, fee income or a promise of protocol revenue neutrality.

Preserve existing unpaid<=0.0005withdrawal/close exceptions and native-XRP
zero-debt guard unchanged, as accepted by Merritt. No unpaid<0.1exception is
introduced. The new optional RepayToVault settlement checkpoint pins pre-payment
debt, interest, accrual clock and forgiven amount; old omitted-field CBOR events
retain prior replay behavior. Only event.rs/vault.rs and canonical Candid change
in the backend relative to released312ebe03; Vault51 adoption source is identical.

## Completed source evidence

[Source receipt](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/061af37b/docs/operations/repayment-residual-correction-result-20261112.md),
[retained checks](evidence/repayment-residual-release-20261112/source-checks.json),
[exact changed-file pins](evidence/repayment-residual-release-20261112/source-pins.json).
Six backend, four rounding and20frontend tests/4suites passed. Canonical Candid
and bindings generation, source compatibility and prior-service compatibility
passed; four generated files add only the optional settlement field/type.
Ellis reports Iris and a second independent final reviewer returned PASS on
exact00cf550d; Iris independently read back the recorded log hashes. No additional review campaign is proposed.
Failed/unknown safety is source call-path inspection, not injected transport
uncertainty; generic cross-ingress duplicate/unknown-retry guarantees are not
claimed. Historical withdrawal/replay asymmetry is deferred, not a release gate.

## One bounded build and exact derived outputs

1. Explicitly permit Dorian↔Cody messages for this00cf550d lifecycle: one fresh
   allocation request/reply and own start/native-exit/resource/manifest/closure
   receipts. Existing chat IDs: Dorian01a1a800-7fde-71b2-a128-2b2325c4b232,
   Cody01a0eeee-d768-71b1-8090-0ca4e680e5b3. No unrelated private usage payloads,
   paid inference/API fallback or reuse of consumed grants. Cody serializes one
   exclusive build slot after fresh capacity checks; approval itself is not a grant.
2. Preserve all prior frozen artifacts. Verify existing isolated producer
   `.worktrees/xrp-customer-withdrawal-address-20261106` is clean in tracked source,
   verify its current740dist hashes against frozen240, then detach to exact00cf550d.
   Preserve both installed dependency links and source240frozen output. No changes
   in the primary dirty checkout and no deletion of unrelated worktrees/caches.
3. One sequential offline batch, total30minutes including60seconds owned closure:
   `CARGO_BUILD_JOBS=2`, `BINARYEN_CORES=2`, Node heap6GiB,8GiB aggregate owned RSS,
   maximum12GiB physical growth,20GiB free-space floor. Reuse existing
   `/private/tmp/rumi-cksol-icp-target` cache; hash current source and refresh only
   backend/cycle-manager input mtimes as in the accepted prior build. No cache
   cleanup, dependency install, competing heavy job, paid CI or broad tests.
   Native process groups and exclusive fsynced dispatch marker are required;
   fail/unknown/bound violation stops the batch without automatic retry.
4. Exact backend pipeline, same production profile and postprocessing as730f:
   `cargo build --locked --offline --release --target wasm32-unknown-unknown --package rumi_protocol_backend`
   with `CARGO_TARGET_DIR=/private/tmp/rumi-cksol-icp-target`; copy raw Wasm into a
   new dedicated SEC17 output directory; embed exact canonical candid:service via
   `ic-wasm metadata ... -v public`; `ic-wasm shrink --keep-name-section`;
   `ic-wasm optimize O1 --keep-name-section`; deterministic `gzip -9 -n -c`.
   Then once: `npm run build --workspace=src/vault_frontend`, with installed
   offline dependencies and its mandatory auth/domain postbuild check.
5. Freeze/push actual raw/optimized/gzip hashes, full frontend manifest/version,
   embedded API/export comparison, gzip equality, executable-code section size,
   actual resource/native closure and full additive asset comparison/publisher
   pins before any production mutation. Preserve the existing10223616-byte
   project code guard/reserve; do not confuse it with the12MiB network limit.
   Exact canonical source/embedded Candid must match, public methods/export count
   must match the canonical API and prior-service compatibility must hold.
   Ellis independently verifies the frozen outputs/plan, reusing accepted source
   checks. Source changes or essential check failure stop; no automatic rebuild.

These hashes/version/new immutable-key count and returned management/batch IDs
are mechanical outputs of this exact source and existing environment. This
single approval covers the verified derived outputs and publication below;
producing output pins alone does not request a second approval. No source swap,
property expansion or materially different runner/pipeline is covered. Freeze
and push the source-bound execution runner/pins before executing their steps.

## Mainnet sequence and preservation

Use only `rumi_identity`, networkic, backend **tfesu-vyaaa-aaaap-qrd7a-cai** and
frontend **tcfua-yaaaa-aaaap-qrd7q-cai**. No whole-stack deploy/newcanister/reinstall,
controller/permission/settings/mode change, S1/S2/incentives/bot/reserve mutation.

1. Freshly verify identity, live modules/API/settings/controllers, stable full
   frontend inventory, Prepare/Commit lists and frozen source/output pins. Capture
   private vault/debt/interest/config/claim/custody/SEC11receipts/SEC12cancellation
   and replay state, pending payouts and all exact holds. Reconcile drift; the
   dated baselines are not permission to overwrite a newer release. Coordinate
   with current owners without touching their separate canisters.
2. Reuse the accepted additive publisher: stage only new content-addressed
   `/_app/immutable/` keys, verify full stable inventory and served bytes before
   backend mutation. Existing-key writes are only the seven existing index/version
   content sets and encodings. Preserve every prior key/property/untouched
   encoding and auth/domain bytes. No deletes/clear/unset/property edits.
3. Reuse the accepted [SEC11 stopped-state preservation procedure](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/b78e8db916835ec78a7c812ef3f09e173b2c5155/docs/operations/sec11-live-release-20261107.md#L15):
   stop/drain and controller-confirm Stopped; fully archive/verify the exact oldest
   managed snapshot before replacing only its slot with a fresh stopped backup;
   retain all other9snapshot entries and download/verify the fresh backup. No
   standalone snapshot delete/restore. Compare the actual active state tree,
   reconciling interest accrued between query and stop rather than copied heap
   records, as in [corrective release](vault51-corrective-live-release-20261111.md).
4. Install only the frozen new gzip once in **upgrade** mode using
   [exact args](evidence/repayment-residual-release-20261112/upgrade-args.did),
   SHA256 **bfcf7fa35ea8c3dafbce6b5a8bd6dfc3b775cbcf1760c83d73ce12130461a7d5**.
   `Upgrade.mode=null` preserves mode. Verify module/API/settings while stopped;
   start once and reconcile preserved state against the stopped baseline, allowing
   only independently evidenced natural interest/timer progression.
5. Activate only the seven prepared frontend entry content sets. Full stable
   inventory, changed served bytes, primary domain/auth/fallback and relevant
   repayment/recovery/help/OISY/ckSOL desktop/mobile readback follow. Ellis supplies
   independent final inventory/public-byte/module/controller/layout verification.
   Separate software delivery from a real customer repayment; do not sign or
   execute a customer transaction to manufacture acceptance.

Every management/asset write uses exclusive durable dispatch evidence and actual
returned IDs. Unknown outcomes stop for readback, never a write retry/alternative
route. New repayment events must not be discarded by an old-writer rollback;
recover forward. Preserve all existing client balances, journals and holds.

## Existing automatic behavior and exclusions

Upgrading re-registers existing Vault51 ten-second/hourly checks. Preserve the
already-approved097bf775scope: only the bound request1696/burn3435761/owner and
same vault; after actual canonical settlement, verified escrow sweep with its
frozen bounded fee and receipt, then same-vault ICP collateral adoption. No new
WaterNeuron request or changed payout/adoption preference. Read current journal
and receipts rather than assuming pending custody stayed unchanged. Report
Running, automatic check, settlement, sweep and adoption separately. This decision
preserves that earlier automatic authority; it adds no manual sweep or transfer.

No customer approval/burn/repayment/withdrawal/close/cancellation/swap, account
switch, saved-data clear, reserve activation, financial retry or new asset/collateral
movement is authorized. Actual customer repayment and recovery remain unverified.
The requested release deliberately changes forgiveness policy for future successful
user-authorized repayments; it does not forgive existing debt merely on upgrade.

This preparation does not authorize the production build/install/publication
sequence until Rob approves this pushed decision. Source preparation was separately
authorized and completed. The requested source policy, including repeatable write-off cost,
is one combined decision; there is no separate optional cap/harness approval gate.
