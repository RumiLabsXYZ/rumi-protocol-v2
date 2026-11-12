# Combined UI production release — November 12, 2026

Production version **1794522798107** contains the recovery panel placement/read
coordination fix, collapsed optional Wallet connection help, and repayment Max
refresh/fee explanation. Exact accepted source is
`240db4cd15647a1871dba5e4774ff7d791963b88`.

Rob approved item 2 in Merritt chat `01a1a48c-240e-77f0-8205-91e3120e76c0`,
human message `01a1d1e1-225e-7df2-af76-c4887c4c9e8d`, covering
[decision137e8e71](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/137e8e7124ca3f974696ed870f39f0f1117e9cfa/docs/operations/combined-ux-release-decision-20261111.md)
SHA256 `4864a53faf3d952c0a6b70d988432d2f76e7cfe5f7f0b8bed3e13bd429cfa0d6`.
The later actual output was frozen and pushed in
[publication inputs50dd9267](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/50dd9267f18ce3802e9196c0e097fca94a0a6877/docs/operations/combined-ux-publication-inputs-20261112.md).
Pins SHA256 `b5b4b7b00f29904d5b748d77030f72096377f1917784910b58b1b7fc2398ef4f`.
This approval supersedes held4ea and does not reuse consumed grants or old bundles.

## Execution and preservation

One offline build under fresh allocation8067e5cb exited0 in19.97seconds;
Cody independently verified the empty owned process group and released the slot.
All740files were frozen. Ellis independently verified740files,476compressed
pairs,13pins and the exact additive plan before publication
([artifact receipt3da217ac](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/3da217ac/docs/operations/combined-ux-independent-check-20261112.md)).
No repeated source tests/build/review campaign was needed: accepted source evidence
remains46focused tests/7suites, two source reviews and composed desktop/mobile QA.
The existing global typecheck baseline30errors/66warnings is not claimed clean.

Mainnet frontend `tcfua-yaaaa-aaaap-qrd7q-cai`, identityrumi_identity:
- Stage batch691 completed once at2026-11-12T22:38:17.797Z, adding123immutable keys.
- Stable stage inventory4824keys/5018encodings and125served checks passed before activation.
- Activation batch692 completed once at2026-11-12T22:44:54.562Z, writing only7index/version content sets.
- Activation argument660bytes; no deletion, clear, unset, property edit, module install or controller change.

Automatic review initially rejected activation for missing publication authority.
No command ran from that rejection. Reading the original human approval plus its
preceding item2 production scope resolved the rejection; the same bounded action
was accepted. All dispatched mutations returned known successful IDs. No financial
write or unknown-outcome retry occurred.

## Verification

Own final verification passed at22:46Z: stable4824keys/5018encodings, all4701
prior keys/properties/untouched encodings preserved,123new immutable keys and7
approved changed encodings match the frozen output. Final9served checks and
5primary-domain checks pass, including identity/gzip entry point, version and
auth/domain bytes. Frontend04e565b3 and backend730f5471 remain Running with
unchanged settings/controllers and Prepare/Commit permission lists.

Ellis independently verified the final stable inventory, preserved4701prior keys,
123new keys and exactly7changed content sets. All15critical public responses
match the frozen output, including relevant compiled recovery/help/Max/OISY/ckSOL
code and route fallbacks. His certified frontend/backend module/controller reads
match his independent pre-release baseline. Live desktop/mobile checks confirm
recovery below the portfolio strip and wallet help initially collapsed.
[Independent live evidence](evidence/combined-ux-release-20261112/ellis-live-check.json),
[module/controller comparison](evidence/combined-ux-release-20261112/ellis-modules-after.json).
These checks do not establish authenticated customer financial success.

Evidence: [execution](evidence/combined-ux-release-20261112/execution-result.json),
[stage](evidence/combined-ux-release-20261112/stage-verification.json),
[activation](evidence/combined-ux-release-20261112/activate-verification.json),
[primary domain](evidence/combined-ux-release-20261112/primary-domain-readback.json),
[stable final inventory](evidence/combined-ux-release-20261112/frontend-final-inventory.json),
[unaffected state](evidence/combined-ux-release-20261112/final-unaffected-readbacks.json).

## Outcome limits

This release establishes served UI delivery. Actual customer recovery/repayment
acceptance remains open with Ellis; no customer signing, retry, cancellation,
account switch, saved-data clearing, repayment, swap or funds movement was performed.
SEC14 remains Done from Rob's earlier successful-swap report, without inferring
independent ledger finality. Existing ckSOL/recovery states and financial/reserve
holds remain. Corrective Vault51 backend730f5471 was already installed separately;
this asset release does not establish a subsequent payout, escrow sweep or ICP adoption.
Old740-file builds, grants, journals and unrelated worktree changes remain preserved.
