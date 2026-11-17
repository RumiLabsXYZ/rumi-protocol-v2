# Exact recovery frontend dispatch — preparation only

Ellis prepared this bounded dispatch for Dorian, the sole execution owner. **No asset write has been made. Execution remains held with Dorian.** The existing human authority for the unchanged SEC20/SEC21 restoration is recorded in [the artifact packet](recovery-frontend-publication-20261116.md); the source is already merged through PR #671. This document is an operational handoff, not a new user approval request or financial authorization.

## Frozen inputs and actual scope

[Execution pins](evidence/recovery-frontend-publication-20261116/dispatch/execution-pins.json), SHA256 `75b69c92da01358b75a19a74e9ba0a76d88073033532273d929ea33542c9e50e`, hash the exact publisher, read-only preflight/verifier, decoder, materializer, typed asset Candid, full artifact manifest and fresh baseline. [Dispatch manifest](evidence/recovery-frontend-publication-20261116/dispatch/dispatch-manifest.json), SHA256 `b6486dfa559d10fe8d0741fb30ec49d2b3289781759a03a75b763d8ef5e5d86f`, freezes every new asset property and every payload key, encoding, source file, size and SHA256. The archive remains SHA256 `a765d0410b30fec71f63883635550dcd306cc791a3e8e5348a4cb95c31102780`; the complete 764-file manifest remains `82768e536040d1a45b77215f3bd48ae1290945e80a44f3d5968735c97a319e82`. No rebuild is needed.

Target only existing asset canister `tcfua-yaaaa-aaaap-qrd7q-cai`, network `ic`, project environment `mainnet-live`, existing identity `rumi_identity`, pinned public principal `fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae`. Use a checkout with the frozen dispatch files and existing project environment mappings. The CLI signs ordinary asset ingress with the existing identity; no customer wallet signing, backend/bot state-changing call, permission/settings/module change, financial transaction or Stability Pool resumption is included.

The fresh November 16 Pacific baseline has 761 assets, 761 identity and 207 gzip encodings. Its complete double-read asset rows equal the earlier independent baseline. Certified frontend/backend modules remain `04e565b3…` and `59a64f78…`, controller sets and backend Candid hash remain unchanged, both upload roles contain the existing operator principal, and the queried Stability Pool hold is true. Full settings are retained privately; only their canonical hashes, module states and upload-permission response bytes are published. The runtime refreshes and checks all of these immediately before upload and again before each commit.

| Phase | Creates | Content sets | Chunk calls | Commit operations |
| --- | ---: | ---: | ---: | ---: |
| Stage immutable assets | 327 | 436 (327 identity + 109 gzip) | 7 | 763 |
| Activate existing entries | 0 | 7 | 2 | 7 |

There are exactly two `create_batch`, nine `create_chunks` and two `commit_batch` calls if both phases complete normally. Total payload bytes are 6,482,334. Encoded chunk groups measured at dummy batch 0 range from 475 to 1,400,230 bytes; the stage commit is 584,587 bytes and activation commit 652 bytes at dummy IDs. Actual encoded arguments are checked again against the 1,800,000-byte ceiling. Dynamic batch/chunk IDs come only from recorded, typed CLI replies; exact argument files and SHA256 are durably journaled before every actual dispatch.

Stage adds only immutable keys. Activation atomically changes `/index.html`, `/_app/version.json`, and each explicit `.br`/`.gz` sidecar, including the existing gzip encoding of `/index.html`. All old properties and untouched encoding hashes, sizes and modification timestamps remain exact. All 327 historical keys absent from this build remain. Final inventory is 1,088 keys, 1,088 identity and 316 gzip encodings. Three upload-configuration files (`.ic-assets.json` and its compressed sidecars) are excluded. New properties use the single unambiguous existing immutable template for each suffix; no security/header policy is invented or weakened. No delete, unset, clear or property-update operation is available.

## Commands and phase boundaries

Run from a valid project checkout; the following variables are ordinary local paths, not new services or identities. Use a fresh artifact directory. Materialization and inspection are local-only, without a build or canister call.

```sh
DISPATCH=docs/operations/evidence/recovery-frontend-publication-20261116/dispatch
EVIDENCE=docs/operations/evidence/recovery-frontend-publication-20261116
ARTIFACT_DIR=/private/tmp/rumi-recovery-dispatch-20261116/operator-artifact
JOURNAL_DIR=/private/tmp/rumi-recovery-dispatch-20261116/operator-journal
export PUBLISH_PINS_SHA256=75b69c92da01358b75a19a74e9ba0a76d88073033532273d929ea33542c9e50e
python3 "$DISPATCH/materialize-artifact.py" "$EVIDENCE/frontend-artifact.tar.gz" "$EVIDENCE/artifact-manifest.json" "$ARTIFACT_DIR"
node "$DISPATCH/publish-frontend.mjs" inspect "$ARTIFACT_DIR"
```

**Dorian must explicitly release stage execution before running this command.** The release variable is an intent guard; it does not confer new authority or alter canister permissions. The script is one-use per phase and journals before dispatch. Each phase is bounded by a deadline (15 minutes stage, 10 minutes activation), each mutation call by 180 seconds or the remaining deadline, and every ingress argument by 1.8 MB. An expired deadline prevents another mutation. Read-only captures may finish after a deadline; they never authorize a later write.

```sh
PUBLISH_EXECUTION_RELEASE=DORIAN_STAGE node "$DISPATCH/publish-frontend.mjs" stage "$ARTIFACT_DIR" "$JOURNAL_DIR"
```

After a successful stage reply, Ellis independently runs the read-only verifier. It compares the full inventory, preserves all old assets/properties/untouched encodings, reads every planned encoding back through the asset API, and checks every new identity resource and protected auth/discovery file over the primary domain with correct MIME type. It also confirms that the active index/version are still old. A second fresh capture must remain identical before the PASS proof is emitted. HTTP bodies and certificate-header presence are observed; independent cryptographic HTTP-certificate validation is not claimed.

```sh
node "$DISPATCH/verify-dispatch.mjs" stage /private/tmp/rumi-recovery-dispatch-20261116/verified-stage.json
```

**Activation remains held until Dorian receives and checks that actual PASS proof and releases activation.** Set its real SHA256; no placeholder value is an authorization. The publisher binds that hash and dispatch-plan hash, refreshes the complete live inventory/state, and requires an exact match to the verified stage snapshot before uploading or committing activation.

```sh
export PUBLISH_STAGE_PROOF=/private/tmp/rumi-recovery-dispatch-20261116/verified-stage.json
export PUBLISH_STAGE_PROOF_SHA256=ACTUAL_VERIFIED_STAGE_PROOF_SHA256
PUBLISH_EXECUTION_RELEASE=DORIAN_ACTIVATE node "$DISPATCH/publish-frontend.mjs" activate "$ARTIFACT_DIR" "$JOURNAL_DIR"
node "$DISPATCH/verify-dispatch.mjs" activate /private/tmp/rumi-recovery-dispatch-20261116/verified-final.json
```

Ellis then checks actual primary-domain desktop/mobile layouts and retains the distinction between connected mocked recovery checks and customer wallet use. Only after final inventory/bytes/state/layout checks pass may Lookout mark the focused UI card done. No customer collateral deposit or financial canary is requested.

## Unknown-outcome reconciliation

The only available mutation methods are the three fixed asset methods above, all targeting the pinned asset canister. A phase-start marker and each call's method/arguments/hash are saved with exclusive creation and filesystem sync before dispatch. A prior phase marker prevents replay. A nonzero CLI exit, missing response bytes, unexpected decoding, unknown chunk IDs or deadline stops the sequence. Do not rerun a phase, rebuild, create another batch, clear the journal, delete chunks or roll back the entry by assumption.

Preserve all argument files, method markers, stdout/stderr, dynamic IDs and preflight snapshots. If a saved stdout contains a complete typed response, inspect it locally without making another call:

```sh
node "$DISPATCH/decode-saved-reply.mjs" "$JOURNAL_DIR" stage-create
# Or the actual saved label, such as stage-chunks-0 or activate-commit.
```

A decoded saved reply still requires effect readback. For an uncertain stage or activation commit, the corresponding read-only verifier can positively establish the complete expected public state; record the CLI uncertainty separately from observed effects. Absence of public keys does not prove a chunk/batch call had no effect, and one absent query is not proof that a commit cannot still land. If create/chunk delivery or a commit remains unresolved, retain the hold and reconcile the original ingress/typed receipt with Dorian before any separately reviewed exact continuation. There is no automatic resend or replacement call in this implementation. A CLI reply alone is never deployment proof.

## Preparation evidence

The full frozen archive materialized locally with all 764 hashes and sizes matching. JavaScript syntax checks and local-only `inspect` encoding passed. No stage/activation command was run, no asset journal was consumed and no live asset was written by preparation. The normal source tests, CI and production build are not repeated. The exact dispatch code and encoding plan receive two focused independent reviews before Dorian's execution checkpoint; all existing financial and permission holds remain unchanged.
