# OISY swap and ckSOL frontend publication — exact held decision

**Preparation complete; publication HELD.** This frontend-only candidate repairs
the direct 3pool OISY sender binding and includes the accepted ckSOL numbered
steps/polling plus the preserved Vault51 frontend lineage. It needs no wallet
retry to finish preparation. It does not establish a successful real swap/mint.
No backend installation, reserve activation or financial operation is included.

## Frozen source and build

- Accepted swap code: `6e6f32fa955fc464ad20aa40e2d832ec6931cfd8`.
- Producer HEAD: `d54ac7b7235d27cf4aca52416a854d5cfab6240d`; only the readable
  source handoff follows that code. Runtime/dependency trees are byte-identical.
- Includes accepted ckSOL `94ffd5d51b7d12c79f7432e0361bc1654ef6bb5c` and Vault51
  lineage `097bf775d3181de5936058ef01e21704d8cfc0ab` without modifying those source
  changes. The current swap delta is six frontend files, only two runtime files.
- Exactly one production frontend build passed on November11, 08:48:32–08:48:50
  UTC (00:48 Pacific), with mandatory Internet Identity/custom-domain postbuild.
  Candidate version: **1794386914589**. All **740** actual files are frozen.
- Swap source: **62 focused tests/four suites, two independent final reviews
  PASS**. Existing **126 ckSOL tests/two reviews/mock browser evidence** are reused
  for unchanged sources. Global typecheck retains30 existing errors/66 warnings;
  it is not claimed clean. No redundant test/build or financial canary ran.

Source acceptance and submission boundaries are in
[Ellis’s exact handoff](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/d54ac7b7235d27cf4aca52416a854d5cfab6240d/docs/operations/oisy-3pool-swap-owner-source-20261111.md).
His independent artifact/readback receipt is
[4758bc21](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/4758bc21/docs/operations/oisy-swap-independent-release-check-20261111.md).
Portable-path confirmation and final refusal-only baseline/input hash binding
are recorded with this packet’s evidence;
no source review or build is repeated for that path-only change.

## Exact publication scope

Use existing frontend **tcfua-yaaaa-aaaap-qrd7q-cai**, network **ic**, identity
**rumi_identity**, principal
`fd7h3-mgmok-dmojz-awmxl-k7eqn-37mcv-jjkxp-parnt-ehngl-l2z3m-kae`.
Preserve frontend module
`04e565b3425fe7510ee16b02adcfe3f01abc9a2725c82a21cb08969241debd62`,
all settings/controllers and Prepare/Commit permission lists. Do not install an
asset-canister module, clear assets, remove keys or change any property/permission.

Bind both stages to the currently Running backend
`43bbbca98df29296a7dde2f134ce15db519bb745c53c1fcf442ef97b54223f09`.
Its API matches the candidate: canonical Candid SHA256
`43da24f0341ca8fe52837071020e910f9ff3ecdef83a04f0dbc8f6e3033334db`.
The raw served Candid is separately
`c18553dfedbd1546092def0773857de3d9afc7a286fa80e9028866f384950e92`;
only its trailing newline was removed for the text comparison. Do not claim raw
hash equality. The separately held Vault51 corrective backend installation is
outside this decision and is not a frontend API prerequisite.

| Operation | Exact scope |
| --- | --- |
| Before inventory | Stable double-read:3,987 keys /4,181 encodings |
| Additive stage |411 new keys and their411 identity content sets |
| Entry-point activation |7 existing content sets, six index/version keys below |
| Total content payload |418 sets /5,188,239 bytes |
| Expected final inventory |4,398 keys /4,592 encodings |
| Deletes, clears, unsets, property changes |0 |

Only existing keys `/index.html`, `/index.html.br`, `/index.html.gz`,
`/_app/version.json`, `/_app/version.json.br` and `/_app/version.json.gz` change.
The extra seventh content set is the existing gzip encoding of `/index.html`.
Literal `.gz`/`.br` asset keys retain their identity encodings. New assets inherit
unique matching existing property templates. All prior keys/properties and
untouched encodings remain, including all300 keys from old staged batch686.
That older frozen bundle and its unexecuted activation are preserved; it is not
silently activated or deleted by this new composed decision.

## Exact input pins

| File | SHA256 |
| --- | --- |
| [740-file manifest](evidence/oisy-swap-release-20261111/frontend-manifest.json) | `aff95d8a06aa00108a5077cabd2732b039f1bfdca2aac58e4bf3bd21868aa8fb` |
| [Exact comparison/operations](evidence/oisy-swap-release-20261111/frontend-comparison.json) | `d1302303309d8ab48536728e47fffe13e750f092e249eea507f1fef9147cd709` |
| [Before inventory](evidence/oisy-swap-release-20261111/frontend-inventory-before.json) | `f70fb087ab8492f2b20d51874db44ca59dead116f789be57ab67e67f2808345f` |
| [Publication pins](evidence/oisy-swap-release-20261111/publication-pins.json) | `786ea7a8b761fcd5eaeaeada52bcad9517abbd8a203eb8c1ba90bd80dee2ed60` |
| [Once-only publisher](evidence/oisy-swap-release-20261111/publish-frontend.mjs) | `b5d3d599dcb5baabc5666a71f06cfa55eb554d9fec06f343fee43d849d484092` |

The source-matched build remains frozen locally; do not rebuild it at publication.
The reusable reader, comparator, verifier, primary-domain verifier and typed API
are included beside the pins. Run the pinned publisher from the producer project
root, with exact evidence files in its artifact directory and
`OISY_FRONTEND_PINS_SHA256` equal to the publication-pins hash above. Its only
asset mutation methods are `create_batch`, `create_chunks` and `commit_batch`.
Private once-only journals use the separate ignored OISY-release directory;
none exists yet for a publication dispatch. No old batch/chunk IDs are reused.

## Sequence after exact approval

1. Re-read identity, frontend/backend Running modules, frontend settings,
   Prepare/Commit lists and a stable complete asset inventory. Compare all frozen
   inputs and actual file hashes; inventory preflight must be no more than60
   seconds old. Any drift means hold/reconcile, never overwrite or downgrade.
2. Run the pinned publisher `stage` once: create411 keys and set only those new
   assets. Split bounded chunks, require all returned chunk IDs unique and commit
   only the822 typed operations. An outcome-unknown marker stops progression;
   inspect actual results rather than replaying a write.
3. Independently read full inventory, prove every old key/property/untouched
   encoding remains, and verify all411 new contents plus both auth/domain files
   as served bytes. The new stage’s own authoritative/served proof is required;
   old batch686’s proof does not substitute for it.
4. Refresh the complete inventory, settings, permissions and same backend module.
   Run `activate` once for only the seven frozen index/version content sets.
   No backend upgrade/readback/financial action is a step of this publisher.
5. Independently verify full final inventory/preservation, seven changed contents,
   both auth files, primary-domain root identity/gzip, version and auth/domain
   files. Report actual served version and readbacks separately from source/tests.

All mutations have exclusive fsynced dispatch markers, exact encoded arguments
and recorded results. Do not retry uncertain commits, clear assets, change
permissions, perform whole-stack deploy or restore stale state. There has been
**no new stage, activation or other production mutation in this preparation**.

## Independent verification and remaining user proof

Ellis independently hashed740 actual files, decoded every paired gzip/brotli file,
verified accepted-code equality and ckSOL bundle content, all411 unique property
bindings, all418 exact content payloads and the seven-key/encoding-change scope.
Live root/version, both auth/domain assets and the old swap bundle hash-match the
pre-release inventory. Frontend/backend module/controller reads match. Our offline
typed plan and syntax checks pass; post-publication readbacks remain unperformed.

The particular OISY502 sender-rejection handler rejects that call before execution.
The old UI did not identify approval versus pool-swap rejection, so a prior
allowance may exist; neither historical approval nor swap outcome was checked.
Unknown transport failures remain unknown and are never automatically retried.
This repair specifically covers direct stablecoin swaps through Rumi3pool;
other OISY routes retain their existing implementations.

After verified publication, Rob can refresh/reconnect the intended OISY account
and inspect the prior attempt/approval history before deciding on a deliberate
new swap. No wallet retry, signature, approval, transfer, swap, mint, reserve
activation or customer communication is requested or executed by publication.
The existing Vault51/Season2 release and financial holds remain unchanged.
