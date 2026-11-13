# Repayment residual fix — live result, November 13, 2026

Exact source **00cf550d3fcb97870cd4643883d5abd8900fa1a2** is installed in the
mainnet backend and served in frontend version **1794555990015**. Dorian owns
the release; Ellis owns the source/case and independent verification. SEC18's
separate ckSOL status candidate is excluded.

## Resulting behavior

Ordinary repayment minimum is0.1icUSD. Actual debit stays within authorization
and current debt, without the old percentage upward snap. After successful
payment and callback-time interest accrual, a positive residual strictly below
0.1is forgiven; exactly0.1remains. Paid interest and forgiven debt are accounted
separately. No automatic collateral withdrawal/closure follows forgiveness.
Coupled Max/API/preview behavior is included. Legacy<=0.0005unpaid withdrawal/
close exceptions and native-XRP zero-debt guard remain unchanged.

Each qualifying settlement can write off up to0.09999999icUSD without a matching
burn or stable deposit. Separate repayment/borrow cycles can repeat the cost;
no aggregate cap was added. This is the explicitly approved economic policy.

## Authority and build

Rob directly approved the [single2d0ad23c release decision](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/2d0ad23c84e4be3728286d5aad048fcce295b4fa/docs/operations/repayment-residual-release-decision-20261112.md)
SHA256ddd4fe41d9f99ab2b9707da0034760ff6802eac2acb907d1ea226b55260459ec in
original human message01a1d3ba-ec02-75d1-a53f-7e0c6df0b64a in Merritt. This covers
verified mechanically derived outputs; the old137UI grant was not extended.

One sequential offline build consumed fresh Cody allocationdadf21c0, all five
named native steps0,187.34seconds,740files frozen, ownedPG69427closed. Cody
independently verified source/files/module/gzip/budget and process closure and
released LOCAL. No repeated build, source review campaign or paid CI ran.
[Actual frozen inputs29b8fc15](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/29b8fc15a7238876fbfbe208afb89e1371a21b4e/docs/operations/repayment-residual-publication-inputs-20261112.md)
retain full hashes/manifests/pins. Ellis's [independent artifact receipt0baae372](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/0baae372/docs/operations/repayment-release-independent-check-20261112.md)
verified206source input hashes,740files/476compressed pairs, direct code-section
parse, exact397-method canonical API, old-service compatibility and the complete
300new+7set additive plan before production mutation.

Source evidence reused:6backend+4rounding+20frontend tests/4suites, canonical
Candid/declarations/source/prior-service compatibility, and two scoped final
source reviews. The optional repayment-event checkpoint is additive and old
omitted-field CBOR behavior is preserved. Full typecheck/broad suite/PocketIC/
real-ledger uncertainty lifecycle were not run; generalized cross-ingress
unknown-retry guarantees and historical withdrawal/replay repair are not claimed.

## Production execution and preservation

Mainnet backend **tfesu-vyaaa-aaaap-qrd7a-cai**, identityrumi_identity:
- Old730f5471 upgraded once to exact upload **6428d1039a9f54e691acfb8cd6c8d5c4cdba8d675b7350f3ad5913815d5575f7**.
- Gzip2,380,549bytes expands to optimizedSHAcd582d70078e0591cd14c5207f587aa6bcb674f633ea3e364c9c5950e2c9fe60.
- Code8,889,183bytes below unchanged10,223,616projectguard; embedded canonicalCandid matchesf0b0fcc2480788b93b4707f707e7dbc8bf9b77c39df0a4ddf3594f54bdf44b6d,397methods exported.
- Controller-confirmed stop/drain, oldest12fully archived, only that slot replaced with fresh stoppedbackup1c; fresh named files/lengths/hashes verified and active50-vault map structurally verified. Other9snapshots remain unchanged. No standalone snapshot deletion, restoration, reinstall or old-writer rollback.
- Installed API/module verified while stopped; start dispatched once. Final Running/settings/controllers preserved, mode unchanged.

Post-upgrade private comparisons preserve captured SEC11/SEC12/customer records,
completed Vault158 recovery, debt/interest matched to actual stopped active records,
fixed configuration and escrow/default-ICP/fee response bytes. The raw comparator
reported two differences: the same claim updated_at_ns in the canonical claim and
nested Vault51 projection. All other captured fields matched. The known automatic
check advanced that timestamp to2026-11-13T08:00:37.635465Z; the explicit
reconciliation accepts only those exact forward-clock changes and preserves the
unmodified raw comparison. ClaimConfirmed persists with no receivedICP,
settlement, sweep or adoption. Existing automatic097Vault51 scope remains active;
no manual financial action occurred.

Mainnet frontend **tcfua-yaaaa-aaaap-qrd7q-cai**:
- Stage693 completed07:55:54Z, only300newimmutable keys; full stable inventory and302served checks passed before backend mutation.
- Activation694 completed08:04:39Z, only7existing index/version content sets.
- Stable final5124keys/5318encodings: all4824prior keys/properties/untouched encodings preserved,300new verified, exactly7changed sets.
- Own9final+5primary-domain readbacks passed. Frontend module04e565b3/settings/controllers and Prepare/Commit permission lists remain unchanged. Auth/domain assets, old artifacts, OISY, recovery and ckSOL support are retained.

Ellis's [final independent receipt98cf99aa](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/98cf99aa/docs/operations/repayment-release-independent-check-20261112.md)
passes: his fresh stable final inventory preserves all4824prior keys/properties/
untouched encodings and verifies300new/7changed sets;13primary responses match
frozen files. Certified backend6428/Candidf0b and unchanged frontend04e/controllers
match expectations. Recursive private delta inspection confirms only the two
matching reconciliation timestamps. Actual anonymous `/` and `/solana` render at
1280×900 and390×844 without horizontal overflow or Rumi error overlay. Existing
unrelated token-logo canister-not-found console noise is recorded; no clean-console
claim or connected repayment transaction is made. SEC18 copy is absent.

[Independent public-byte/inventory proof](evidence/repayment-residual-release-20261112/ellis-final-live-check.json),
[certified module/API comparison](evidence/repayment-residual-release-20261112/ellis-certified-after.json),
[retained-delta verification](evidence/repayment-residual-release-20261112/ellis-retained-delta-check.json),
[anonymous layout check](evidence/repayment-residual-release-20261112/ellis-live-layout-check.json).

**Software release complete and independently live-verified.** Actual customer
repayment acceptance remains unverified and separately owned by Ellis.

## Evidence and limits

[Backend execution](evidence/repayment-residual-release-20261112/backend-execution-result.json),
[state reconciliation](evidence/repayment-residual-release-20261112/final-state-reconciliation.json),
[raw comparison verdict](evidence/repayment-residual-release-20261112/retained-state-comparison.json),
[fixed configuration](evidence/repayment-residual-release-20261112/configuration-comparison.json),
[stage proof](evidence/repayment-residual-release-20261112/stage-verification.json),
[activation proof](evidence/repayment-residual-release-20261112/activate-verification.json),
[primary domain](evidence/repayment-residual-release-20261112/primary-domain-readback.json),
[final inventory](evidence/repayment-residual-release-20261112/frontend-final-inventory.json),
[expected modules/settings/permissions](evidence/repayment-residual-release-20261112/final-unaffected-readbacks.json).

Software delivery is separate from customer repayment acceptance. No customer
signing, approval, burn, repayment, withdrawal, close, cancellation, swap, saved-data
clear or financial retry was performed. Actual customer repayment/forgiveness and
SEC12 recovery remain unverified. SEC14's earlier customer-reported swap success
remains separate. Reserve/financial holds and all retained client assets/journals
remain. Future writers must preserve the new optional settlement events; recover
forward instead of losing events through an older writer.

Automatic review rejected outgoing Dorian→Cody messages; those payloads remained
held and were not relayed. Cody directly delivered the authorized fresh allocation
and independently read the native/closure records. An initial combined evidence
preparation/push was rejected as binary export; the file inventory proved a UTF8
metadata/scripts-only payload, and the narrower explicitly bounded metadata push
succeeded. Wasm binaries and private raw claim/vault/snapshot contents stayed local.
All dispatched production operations returned known success; none was retried.
