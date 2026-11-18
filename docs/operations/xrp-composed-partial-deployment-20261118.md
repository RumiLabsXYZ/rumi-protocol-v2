# Exact composed XRP recovery-sizing backend deployment decision

**Tested and built; not installed. Separate deployment approval required.** This packet proposes one upgrade of the existing Rumi backend, retaining the live automatic XRP retry bound and Stability Pool new-liquidation pause while correcting partial-liquidation sizing. No financial action or liquidation resumption is included. The current installed backend is still Running with its Stability Pool pause true; the proposal below has not been executed.

## Exact candidate

| Item | Exact value |
| --- | --- |
| Network / operator | ic / rumi_identity |
| Only canister | `tfesu-vyaaa-aaaap-qrd7a-cai` |
| Resolved existing operator | `fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae` |
| Required installed baseline | `ce014cc2fdccdb59f8545ed4fc6af40a0ee484f290c0cdf6857f1c38bcb3ca5f`; Running; SP pause=true |
| Source-matched build commit | `67a5be7a650ad21ef9f013307d7aa0e897508724` |
| Built tree | `89a5f60f196a2f6a78d5c5e575560c9513c7214e` |
| Reviewed runtime source | `1165a8973cb2040c732753a8bf24e7825ff639c7`; 67a5be7a adds only draft/evidence |
| Frozen upload | [backend.wasm.gz](evidence/xrp-composed-partial-20261118/backend.wasm.gz); 2,796,643 bytes; SHA256 `4409bfb7d3bd04aab2fd48f22a850656a0fefda471f285b616f6562444effdde` |
| Optimized Wasm | 11,451,910 bytes; SHA256 `92271136ea6cd08907c70e024a6825b5116aee5076ade01bc188e31213b29c38` |
| Raw Cargo Wasm | 16,447,032 bytes; SHA256 `ed5966b2b7db28cc4f705d0c5f290b3870af1c9a52cd0f6ce753fba4bb822ad1` |
| Executable code section | 10,320,074 bytes; accepted 12 MiB limit with 256 KiB reserve; 2,000,694 bytes beyond reserve |
| Certified embedded Candid | SHA256 `0279b1f5395cf323e479ba18d842419232b371e8850eb246429fad4e8792ebb6`; byte-identical to current live public contract |

[Fresh authenticated baseline](evidence/xrp-composed-partial-20261118/preapproval-baseline.json) confirms the current module/settings/controllers/inventory and pause. [Independent anonymous readback](evidence/xrp-composed-partial-20261118/independent-live-baseline.json), observed November 18 at 19:18:33 UTC, confirms ce014 and pause=true.

The expected installed management module_hash is the frozen uploaded gzip SHA256, `4409bfb7d3bd04aab2fd48f22a850656a0fefda471f285b616f6562444effdde`. It is distinct from the decompressed hash. Use the named frozen artifact, not cached build output or a new build. The existing CLI supports the required chunked installation.

## Resulting behavior and preserved state

Only state.rs and vault.rs change relative to the deployed runtime `2ff2723a`, apart from documentation/evidence. Native-XRP dispatch and new burn admission use the same per-asset recovery-target cap with XRP drop rounding. Full debt remains allowed for existing dust handling, including the residual debt floor, or when no proper partial burn can restore the configured target. Invalid math fails closed. Non-XRP sizing is unchanged.

Fresh and repeated over-cap admission is refused without overwriting an immutable reservation. Accepted finding V208-COMPOSE-EXPIRY-1 is fixed: an expired reservation cannot admit a new burn at an old price. An already-burned proof-bound exact legacy tuple remains settleable, including after expiry. Partial settlement retains residual debt/collateral, proportional accrued interest and gross depositor/developer claim conservation. This upgrade does not undo Vault 208's historical liquidation or compensate anyone.

The deployed automatic-origin/signer/sibling retry guards are retained: unsigned initial settlement and validated reconciliation remain possible; retained failed/missing attempts cannot authorize automatic replacement signatures. Existing manual recovery and custody/auth/proof checks are preserved. Receipt polling still occurs; no zero-RPC/cycle-consumption claim is made.

No persisted State schema, stable MemoryId, Candid/API, caller/controller, storage, generated declaration, pause implementation or helper change is introduced. The alternative private hold mechanism from the older sizing branch is excluded. Source `67a5be7a` uses the deployed preserve_sp_liquidation_pause support; it applies before state publication/timers. Existing separately authorized automatic Vault 51 checks/eventual verified adoption remain untouched.

## Verification actually completed

- One native allocation passed **149 XRP tests, 4 existing pause tests and 1 normal canonical Candid test**, all with 0 failures. Actual passing tests include the six-decimal Vault 208 vector, residual claim conservation, expired new-admission refusal with exact post-burn settlement, and all three automatic retry-origin regressions. All three owned process groups closed; elapsed 138.328 seconds. No regeneration or runtime source change occurred.
- Directional compatibility against certified live ce014 Candid, exact DID/declaration equality, typed args encode/decode, retained source/contract/helper/build-input hashes and diff checks passed. Unchanged State snapshot/storage proofs are reused; no redundant run is claimed.
- One new source-matched backend-only offline/locked/jobs=1 release build completed all seven canonical stages: Cargo with default features and release profile `z`, raw copy, public Candid metadata, shrink, O1, deterministic gzip and mainnet marker guard. Raw Cargo output/provenance, unchanged inputs/source, embedded Candid, composed cap/expiry/retry/pause markers, accepted 12 MiB minus 256 KiB code criterion, gzip roundtrip/zero mtime and bounded resource/process closure pass. Actual elapsed 157.958 seconds.
- Adrian's focused source/claim review and draft/typed-args/preservation review pass, with the one concrete expired-admission finding resolved. [Cody independently verified](evidence/xrp-composed-partial-20261118/cody-artifact-resource-verification.json) actual native/build receipts, artifacts/provenance/resources and process closure. Adrian independently rechecked [actual native logs](evidence/xrp-composed-partial-20261118/adrian-native-readback.json) and [all three final artifacts](evidence/xrp-composed-partial-20261118/adrian-artifact-readback.json); no new source-review campaign was run.

[Native proof](evidence/xrp-composed-partial-20261118/native-results.json) and [build proof](evidence/xrp-composed-partial-20261118/release-results.json) retain original receipt/log hashes. Original executed runners/receipts/logs and raw/optimized artifacts remain preserved privately. Prior failed sizing/runner receipts and the old source/artifacts remain preserved; this is not a replay of either old producer. No new broad review campaign, funded burn/signature/payment, hosted CI or main merge is claimed. Current read-only GitHub checks find no main branch protection/rulesets; optional broad eight-canister/SP workflows remain deferred under the demo policy. Enforced checks are not disabled or bypassed. [Actual omitted coverage](../coordination/demo-deferred/xrp-composed-partial-20261118.md) is recorded without adding a new optional gate.

## Exact upgrade arguments

[Readable args](evidence/xrp-composed-partial-20261118/upgrade-args.did), [typed hex](evidence/xrp-composed-partial-20261118/upgrade-args.hex), and [decoded value](evidence/xrp-composed-partial-20261118/upgrade-args.decoded.did) are frozen. Hex-file SHA256 `26cbae560b9d7020ff305b4775f0429b5164fb490a11e7aee711ba246a406cb3`.

```candid
(variant { Upgrade = record {
  mode = null;
  description = opt "Cap native XRP liquidation at recovery CR; retain automatic retry bound and Stability Pool pause";
  preserve_sp_liquidation_pause = opt true
} })
```

Immediately before an approved installation, regenerate with unchanged scripts/rumi-backend-upgrade-args.sh (SHA256 `b33206cd3f7177badc4baaeeb8a2404346e1ec1aa0d498b768b2c7f0ebf8ba3e`), --preserve-sp-liquidation-pause and exactly this description. Encode against the frozen candidate Candid and require byte equality. mode=null requests no mode change; there is no resume argument.

Only after approval and verified preservation below:

```sh
icp canister install tfesu-vyaaa-aaaap-qrd7a-cai \
  --network ic --identity rumi_identity --mode upgrade \
  --wasm docs/operations/evidence/xrp-composed-partial-20261118/backend.wasm.gz \
  --args-file docs/operations/evidence/xrp-composed-partial-20261118/upgrade-args.hex \
  --args-format hex
```

Do not use --yes, automatic/install/reinstall mode, a different target/identity, default args, a permission change or an unnamed artifact.

## Exact preservation and deployment scope requested

Rob's November 18 approval 01a1f00b-5229-7c70-894b-9672ba95956d authorizes Cody–Dorian coordination/tests/build only and expressly **defers snapshot export to deployment approval**. No production snapshot was exported, replaced or restored by this preparation. The prior oldest 17 export rejection is retained; it is not retried. The deployment decision requested by this packet includes these named private exports and verification as prerequisites to the later writes:

1. Refresh exact module/operator authority/settings/controllers/public Candid/SP pause and complete ten-snapshot inventory. Capture retained vault/config/claim/custody/outbox/reserve/bot readbacks. Require live ce014 and SP pause=true, and unchanged settings/controllers. Unexpected release or inventory drift stops this packet rather than overwriting a later writer. Normal authorized timers/market changes are captured explicitly.
2. **First private export after deployment approval:** fully download oldest snapshot `00000000000000170000000001f088fe0101`, timestamp `1794099176324026000`, server size 113,996,773 bytes, to `/Users/robertripley/coding/rumi-protocol-v2/.recovery-artifacts/xrp-composed-partial-20261117/snapshot-00000000000000170000000001f088fe0101/`. Verify complete metadata and every named component/chunk, declared lengths, full hashes, completion and fsync; private directories `0700`/files `0600`. Raw state must stay local/private and never be pushed. Preserve normal empty CLI lock/chunk controls. If export/coverage fails, halt before stop/replacement/install.
3. Stop only backend and prove Stopped on ce014 with settings intact. While stopped create one fresh snapshot replacing ONLY the now-independently-archived oldest 17, using `icp canister snapshot create tfesu-vyaaa-aaaap-qrd7a-cai --replace 00000000000000170000000001f088fe0101 --quiet --network ic --identity rumi_identity`. Retain all other nine exact IDs with suffixes `18`, `19`, `1a`, `1b`, `1c`, `1d`, `1e`, `1f`, `20` from the [inventory](evidence/xrp-composed-partial-20261118/snapshot-inventory.json), SHA256 `a6e71fd7724309905788d64b52305c27cecd0955f6d7cfbd0c9091e3cb127ab3`. No standalone delete or restore. Persist actual returned new ID; never assume it.
4. **Second private export:** fully download that fresh stopped snapshot to the same private recovery root, directory `snapshot-<actual-returned-id>/`. Verify complete metadata/bytes/full hashes/fsync and the fresh snapshot's module against ce014; prove other nine snapshots unchanged. If any backup check fails, do not install. Keep all raw state private. Snapshot creation does not call Rust pre_upgrade: stale serialized stable-State or a pre-stop accrued query is not the active stopped financial state. Preserve the whole stopped image and use the established active-tree/readback comparison, reconciling actual interest/timer deltas explicitly.
5. Dispatch one upgrade of the exact frozen gzip/typed pause=true args. Record durable intent and exact inputs before dispatch, preserve chunk/install replies and unknown-outcome receipts. Verify the new module hash, embedded Candid and unchanged settings/controllers while Stopped, then start only backend. Verify Running/newmodule/SP pause=true, retained vault IDs/owners/collateral types/collateral, actual stopped debt/accrual, custody/claims/escrows/reservations/liabilities/outboxes/recovery/configuration and bot/reserve holds. Independent anonymous pause check and method/contract readback are included; no live payout/retry is used as a test.

No automatic old-writer rollback or financial snapshot restore is authorized. Reconcile an unknown result against actual module/status and durable receipts before another write; never blindly resend. If no new code installed, restarting the unchanged ce014 backend is the bounded pre-install abort path, with its pause rechecked. On any post-install invariant failure retain/return Stopped containment and preserve evidence. Any snapshot restore, reinstall or source-changed recovery requires its own concrete decision.

## Exclusions and holds

This is **backend code plus its stated preservation procedure only**. It does not authorize SP liquidation resumption, compensation/retroactive Vault 208 remedy, Vault 202 recovery, USDC/reserve/cap activation, operator claim payment/retry, treasury/funding/conversion/topup, bot upgrade/worker resumption, controller/permission change or frontend publication. All financial and liquidation holds remain. UI publication remains a separate exact packet. No archive restore, client-money transaction or deployed outcome is claimed here. The existing live ce014 backend remains untouched, with **new Stability Pool liquidations paused**.

Later backend releases must preserve both the retry bound and this recovery-sizing/admission correction and bind to the actual deployed baseline. The final candidate artifact is installable only through the exact separately approved scope above.
