# Exact XRP retry-bound backend deployment decision

**Ready for Rob's separate deployment decision. Not installed.** Approve only one upgrade of the existing Rumi backend to stop automatic replacement signatures for retained XRP payout attempts and retain the Stability Pool new-liquidation pause before timers start. This packet does not authorize any other release or financial action.

## Immutable candidate

| Item | Exact value |
| --- | --- |
| Network / existing operator | `ic` / `rumi_identity` |
| Only target canister | `tfesu-vyaaa-aaaap-qrd7a-cai` |
| Required installed baseline | `59a64f78e0e652d0e48413aba587d3f826c1dc25258f56ab09575c59de444111` |
| Built source | `2ff2723a8f85e960eff2e32b141926334cc5b4b5` |
| Built source tree | `aa457279cfa8e7943635d19be59510cf3b45ae7a` |
| [Frozen gzip upload](evidence/xrp-cycle-bound-20261116/backend.wasm.gz) | 2,796,198 bytes; SHA256 `ce014cc2fdccdb59f8545ed4fc6af40a0ee484f290c0cdf6857f1c38bcb3ca5f` |
| Optimized, decompressed Wasm | 11,449,431 bytes; SHA256 `6f87ef8db5db5dec0166f1294ebd3695aba23b3325fc0e95ae8d1b2217bb3efa` |
| Original Cargo Wasm | SHA256 `3f8dfc9f7d2956e77b423ba88fe2af0e889801ac244dc83a5ab4d1e7ce245346` |
| Executable code section | 10,317,840 bytes; within 12 MiB with the established 256 KiB reserve |
| Embedded public Candid | SHA256 `0279b1f5395cf323e479ba18d842419232b371e8850eb246429fad4e8792ebb6` |
| Raw Rust-exported Candid | SHA256 `5294a3721ea1b1df23ee2dd319e1f1cecc83d16507c6133d28fb1f7fbdf8a593`; structurally identical to the embedded contract after whitespace normalization |

The expected management `module_hash` is the uploaded gzip hash `ce014cc2…b3ca5f`, following the established observed backend upload convention; the decompressed hash is recorded separately. Do not substitute one for the other. The gzip is larger than the ingress limit; use the existing CLI's chunked installation support, without rebuilding or changing the frozen bytes.

The preserved installed-release receipt identifies source tree `18b286ab639f714cb10bbf37cdae0f795d1cad06`, which is exactly PR #666 merge `f72ede8195b23299c297fb24f37ac76d7d957269`. Its backend, Cargo manifests/lockfile and `icp.yaml` are unchanged through candidate base `9b7669e7`. The candidate therefore preserves that deployed backend base and adds only this correction and hold support. This packet's later commit is evidence-only relative to the built source; it is not a new build, CI result or main merge.

## Resulting behavior and compatibility

The existing registered-SP authorization now selects an automatic settlement origin. An unsigned claim can still receive its initial signature. A retained validated payment can still reconcile. A retained failed or missing transaction cannot authorize another automatic signature, including through automatic sibling reconciliation; the signer boundary checks the live retained tuple again. The claim, liability and original hash/sequence/destination survive for explicit claimant recovery. Existing manual fee, expiry, sequence, divergence, ownership and custody-lock rules remain.

Receipt polling continues. This bounds automatic replacement signatures for retained attempts; it is not a claim that all future RPC or cycle consumption is eliminated. No production payout is invoked as a test.

The persisted State schema is unchanged; no balance migration is added. The only semantic Candid/event-record extension is optional `UpgradeArg.preserve_sp_liquidation_pause`. `Some(true)` latches the existing `sp_writedown_disabled` flag through `State::upgrade` on both snapshot restoration and event replay, before state publication and `setup_timers`. Omission and false cannot clear a pause. Old Candid arguments and serialized Upgrade events decode; the generated JS/TS contract changes only for this optional field. The new DID documentation comments do not add methods.

## Exact upgrade arguments

[Readable helper output](evidence/xrp-cycle-bound-20261116/upgrade-args.did), [typed hex](evidence/xrp-cycle-bound-20261116/upgrade-args.hex) and [decoded value](evidence/xrp-cycle-bound-20261116/upgrade-args.decoded.did) are frozen. The typed hex-file SHA256 is `37d9ae24baad43051f8fd1aaa556d6a3911bcb0693ac7e75f0882a5a306ed935`.

```candid
(variant {
  Upgrade = record {
    mode = null;
    description = opt "Bound automatic XRP claim signatures; preserve Stability Pool liquidation pause";
    preserve_sp_liquidation_pause = opt true
  }
})
```

Immediately before the approved operation, regenerate these same arguments with the canonical helper and `--preserve-sp-liquidation-pause`, encode against the frozen candidate Candid, and require byte equality with the frozen hex. The helper SHA256 is `b33206cd3f7177badc4baaeeb8a2404346e1ec1aa0d498b768b2c7f0ebf8ba3e`. Its four existing mode outputs remain byte-identical when the new flag is omitted; existing description/mode rejection remains. No resume option was added. `mode = null` requests no protocol-mode change.

The installation command, only after explicit approval and the preservation steps below, is:

```sh
icp canister install tfesu-vyaaa-aaaap-qrd7a-cai \
  --network ic --identity rumi_identity --mode upgrade \
  --wasm docs/operations/evidence/xrp-cycle-bound-20261116/backend.wasm.gz \
  --args-file docs/operations/evidence/xrp-cycle-bound-20261116/upgrade-args.hex \
  --args-format hex
```

Do not use auto/install/reinstall mode, a cached unnamed artifact, default arguments omitting the hold, a different principal, a permissions change or a new build.

## Preservation procedure included in this decision

1. Refresh the module, resolved existing operator/controller authority, settings, live interface, SP flag and retained configuration/claim/journal/vault readbacks. Require baseline `59a64f78…44111` and `get_sp_writedown_disabled() = true`. Require the exact ten-row [snapshot inventory](evidence/xrp-cycle-bound-20261116/snapshot-inventory.json). Halt on unexpected drift; do not overwrite a later release or loosen a hold.
2. Verify the already-downloaded private archive of oldest snapshot **`00000000000000160000000001f088fe0101`**, timestamp `1793927851682795038`, declared total 113,971,296 bytes, against its [corrected complete-coverage manifest](evidence/xrp-cycle-bound-20261116/snapshot16-corrected-coverage.json) and [independent verification](evidence/xrp-cycle-bound-20261116/snapshot16-independent-verification.json). Raw snapshot state remains local/private and is not in this repository. The complete metadata and all three data components were length-checked, fully hashed, read back and fsynced; files are 0600 and directories 0700. The official CLI's empty lock file and empty chunk-store directory are retained. All ten remote entries were unchanged by this read. This is archive verification, not a restore test.
3. Stop only this backend and positively establish Stopped. While it is stopped, create one fresh snapshot using `snapshot create --replace 00000000000000160000000001f088fe0101`. This replaces only the independently archived oldest slot once the new snapshot succeeds. Retain every other existing snapshot (`17` through `1f`); no standalone deletion, restore or other slot replacement is included. Persist the returned new ID and metadata, fully download/verify that fresh stopped backup, and require its module to bind to the current baseline before installation. If any backup/coverage check fails, do not install.
4. Preserve actual stopped-state evidence. Management snapshot creation does not call Rust `pre_upgrade`; its serialized stable-State body can reflect an older hook. Do not treat that body alone, an older heap copy, or a pre-stop accrued projection as the active stopped financial state. Retain the whole stopped image and reconcile expected accrual/timer changes explicitly using the established release readbacks/comparison procedure.
5. Dispatch one upgrade of the exact frozen gzip and hold-bearing arguments. Record durable operation intent/result and chunk/install receipts. Check the installed module and preserved settings while stopped, then start only this backend. The source-applied hold is established before timers, rather than relying on a later setter call.
6. Read back `module_hash = ce014cc2…b3ca5f`, Running, SP flag true, unchanged settings/controllers, the matching candidate contract and retained asset/liability/custody/outbox/recovery/configuration invariants. Record the exact interface source: certified embedded metadata is `0279…ebb6`; the raw Rust-exporter text is `5294…a593`, structurally equal. Verify the existing financial and worker holds, including Vault 202 and the separate Vault 208 work. No live claim retry/payment is part of verification.

No stop, snapshot replacement/creation or installation has been performed by preparation. The existing archive read and artifact are complete; future preservation writes above require the separate approval of this exact scope.

## Checks actually completed

- `f0df1528`: 33 XRP claim/settlement tests, 6 State-snapshot tests and 1 normal Candid compatibility test passed. The counted production signer seam admits an initial attempt, then retains one signature through 144 failed/missing ticks and a State-codec restart; claims/tuples/liability and manual fee treatment are checked.
- `37a1adda`: 4 hold-only tests and 1 canonical Rust Candid exporter passed. Old arguments/events, snapshot and replay paths, mode/liability retention, false/omitted inputs and later upgrades are covered.
- Final built source `2ff2723a`: 1 normal Candid compatibility test passed without regeneration. Existing declaration generation, strict raw/normalized equality, backward Candid check, typed helper argument encode/decode and shell guards passed. Earlier Rust filters were not invalidated or repeated.
- One backend-only offline/locked/jobs1 production release build used Rust/Cargo 1.92, the repository's release profile (`opt-level = z` for this package), canonical public metadata/shrink/O1/deterministic gzip and the mainnet artifact guard. Actual Cargo JSON binds the default-feature binary to the exact source. Embedded Candid, gzip roundtrip/zero mtime, size/reserve, both new code markers and all input/source hashes passed; all owned process groups closed.
- [Independent release verification](evidence/xrp-cycle-bound-20261116/release-independent-verification.json) accepts 336.355590 seconds and the exact artifacts. The [original receipt](evidence/xrp-cycle-bound-20261116/release-2ff2723a-original-receipt.json) retains an invalid negative elapsed field caused by a post-build parser variable shadow; the [separate timing correction](evidence/xrp-cycle-bound-20261116/release-timing-correction.json) is independently verified. No build was repeated or original result rewritten.
- The snapshot download's [original stopped wrapper receipt](evidence/xrp-cycle-bound-20261116/snapshot16-original-stopped-receipt.json) is also retained. All three CLI reads exited zero; its overly narrow four-name assertion rejected normal empty control entries. Local-only corrected coverage and independent official-CLI completion checks passed; no second CLI invocation, resume, second download or snapshot mutation occurred. This does not assert absence of the official CLI's internal idempotent HTTP retry mechanism.
- Focused independent source/hold/helper review found no blocker. [Review summary](evidence/xrp-cycle-bound-20261116/focused-review-summary.json) preserves the limitation: production dispatch helpers are tested, not a new full RPC-backed payout harness. No live transaction or financial canary was used.

There is no hosted CI or main-merge claim. The actual main required-status endpoint reports no branch protection and the repository has no rulesets. Optional broad eight-canister/SP workflows are deferred under the current policy, recorded as [XRP-CYCLE-DEF-01](../coordination/demo-deferred/xrp-cycle-bound-20261116.md). Opening a later PR does not authorize bypassing its normal checks or failures.

## Existing holds and recovery boundaries

Vault 208's partial-liquidation sizing correction is **not included**. The known sizing issue remains contained by the SP pause. If that or any other backend release installs first, this packet's expected baseline fails: compose and validate a new exact candidate rather than installing an old writer. Conversely, any later Vault 208/backend candidate must preserve this retry bound and hold support after this upgrade; it must not overwrite them. No private rejected source handoff was retrieved, relayed or retried; the hold addition was independently written from the already-authorized branch.

No bot upgrade, USDC reserve/cap activation, Vault 202 recovery/payment/worker resumption, compensation, claim recovery/retry, treasury/funding/conversion/topup, controller/permission change, frontend publication or SP liquidation resumption is approved here. Existing independently authorized automatic work, including the exact Vault 51 checks/eventual verified adoption scope, is retained; no operator financial action is added by this packet.

The [fresh baseline summary](evidence/xrp-cycle-bound-20261116/preapproval-summary.json) records Running baseline, SP pause true and unchanged Vault 51 anchors/error-free pending claim. A separate historical approval for obsolete Vault 51 gzip `730f5471…` is not a rollback authorization and is not replayed.

On an unknown installation outcome, reconcile module/status and durable receipts before another write; do not blindly resend. On a post-install invariant failure, retain the backend in, or return it to, Stopped containment and preserve evidence. Do not automatically install an older module, reinstall, restore an old financial snapshot, resume SP liquidations or send funds. If no new code was installed, restarting the unchanged original backend is the bounded pre-install abort path, with its pause rechecked. Any actual state restore or source-changed recovery requires its own concrete decision.
