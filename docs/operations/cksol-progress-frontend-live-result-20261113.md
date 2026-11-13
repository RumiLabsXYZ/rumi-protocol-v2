# ckSOL progress display — live release result, November 13, 2026

Exact source **ab44413da1a8b394245cd7734da4124a2335621b** is served on mainnet
frontend **tcfua-yaaaa-aaaap-qrd7q-cai**, version **1794583637373**. Dorian owns the
release; Ellis owns the source/case and independent verification. No backend
build, lifecycle change, new wallet action or guard removal was included.

## User-visible change

The ckSOL mint action has persistent progress, last successful check, automatic/
paused checking, read-only Check again and action-needed guidance. Payment
settlement alone does not display mint success. Official Minted evidence must
match the current deposit ID before exact amount/destination/block appear.
Confirmed completion stays visible until Start another deposit, retaining history.
Changed IDs invalidate old evidence/reads; active/unknown holds retain precedence;
terminal payment modes use the authoritative deposit. Queued requests do not ask
for new SOL. These are shipped code/UI behaviors; no real customer mint was tested.

## Authority and source/build evidence

Original human **01a1d57e-43c7-7a02-8cf2-0794a179689c** in Merritt approved
[decision6e3fdaa7](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/6e3fdaa7e7bd405867a029fd7428b57e65a98d30/docs/operations/cksol-progress-frontend-release-decision-20261113.md),
SHA2562cd75e9e33be545b842a22218ae375cce7c8ffae588c3245ee17206e5b12d658,
including both Dorian–Cody directions and Ellis verification. No SEC17 grant or
approval was reused. Derived output pins were frozen/pushed before publication in
[658dbe7c](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/658dbe7c803794204f50df2f7c766e01a11c8de2/docs/operations/cksol-progress-frontend-publication-inputs-20261113.md).
Pins SHA25619a7321d2c4ac2c8f2b17c0a49d87ef0d1534506e1283684cbd1a0983e9a8435.

One fresh allocation4e737700, one offline frontend/auth build, native0 in22.347924s,
740files frozen. NativePG54334/runner closed, independently accepted by Cody and
LOCAL released. Peak2,289,369,088B RSS/growth91,926,528B/minfree45,977,780,224B.
Cody root proof SHAd2efc18705488885618635bba6254385695a4e03525a4b7ad73f8a86c99213e9.
All prior740SEC17frozen files and dependency links remain preserved.

Reuse88focused tests/3suites, both independent correction rechecks and actual
component synthetic desktop/mobile QA with zero financial calls. No repeated
source tests/review campaign, full typecheck, broad suite, backend build or paid
CI was run. Mocked progress states do not establish real wallet/concurrency/mint
completion. [Independent artifact PASS12e6c030](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/12e6c030/docs/operations/cksol-progress-independent-check-20261113.md)
verified all740files/476compressed pairs, source/backend/dependency preservation,
full inventory agreement, compiled features and the complete96new+7set plan.

## Actual production sequence and readback

The first fresh inventory query failed ENOTFOUNDicp-api.io before the publisher
launched. Its absent journal/batch was verified, the failed log retained, DNS
recovered and the read-only preflight repeated. No write was retried or rerouted.

- Stage **695** added only96newimmutable keys; full stable inventory and98served checks passed before entry activation.
- Activation **696** completed once at **2026-11-13T15:42:03.537Z**, only7existing index/version content sets.
- Final stable **5220keys/5414encodings**: all5124prior keys/properties/untouched encodings preserved,96new and7changed sets verified. Own9final+5primary-domain readbacks PASS.
- Backend **6428d1039a9f54e691acfb8cd6c8d5c4cdba8d675b7350f3ad5913815d5575f7**, frontend module04e565b3, both Running/settings/controllers, canonical backend API and Prepare/Commit permissions remain unchanged. Fresh backend settings guards passed before each asset phase.
- No deletes/clear/unset/property edits, backend/snapshot operation, permission/mode/reserve change or customer financial action occurred. Repayment/OISY/recovery/Vault51 behavior and all exact holds remain preserved.

Ellis's [final independent receipt06a85749](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/06a85749/docs/operations/cksol-progress-independent-check-20261113.md)
passes: own stable5220/5414inventory preserves5124prior+96new/7sets;13primary
responses match frozen root/version/auth/progress/repayment/recovery/OISY assets
and applicable HTTP SPA shells. Certified6428/backendAPI/controllers and04e/frontend
are unchanged. Actual anonymous `/solana` at1280desktop and390mobile shows the new
Mint status panel without horizontal overflow or Rumi error overlay. Accepted88
mock checks cover connected/progress/Minted states; no wallet, new mint or stored
customer-state change occurred. Existing unrelated token-logo console noise is
not treated as a clean-console result.

[Independent public/inventory proof](evidence/cksol-progress-frontend-release-20261113/ellis-final-live-check.json),
[certified modules/API/controllers](evidence/cksol-progress-frontend-release-20261113/ellis-certified-after.json),
[anonymous production layout](evidence/cksol-progress-frontend-release-20261113/ellis-live-layout-check.json).
**Software delivery is complete and independently live-verified.**

[Execution summary](evidence/cksol-progress-frontend-release-20261113/live-execution-result.json),
[stage proof](evidence/cksol-progress-frontend-release-20261113/stage-verification.json),
[activation proof](evidence/cksol-progress-frontend-release-20261113/activate-verification.json),
[primary domain](evidence/cksol-progress-frontend-release-20261113/primary-domain-readback.json),
[final inventory](evidence/cksol-progress-frontend-release-20261113/frontend-final-inventory.json),
[module/settings/permissions](evidence/cksol-progress-frontend-release-20261113/final-unaffected-readbacks.json),
[phase preservation](evidence/cksol-progress-frontend-release-20261113/backend-phase-preservation.json).

## Vault213 deposit pause is a separate retained hold

The ordinary AddMargin pull route has been deliberately held since October10's
[3b9e3787/#508](https://github.com/RumiLabsXYZ/rumi-protocol-v2/commit/3b9e37874688ec21f1692e2dbc598a6fbbe0521e).
The UI false flag and API pre-approval guard mirror the backend's unconditional
rejection. Its unfinished recovery/refund path cannot safely handle a confirmed
collateral pull after liquidation removes the target vault. The gate precedes
amount validation/approval/transfer; the selected13ckSOL amount is not its cause.
Waiting for mint completion or refreshing does not clear it. No rejecting update
endpoint was invoked to test this; actual module/source lineage and UI guards were
read back. [Exact source citations and diagnosis](cksol-progress-frontend-publication-inputs-20261113.md#separate-vault213-deposit-pause-diagnosis).

Ellis owns the separate AddMargin repair; it was not silently bundled here and
this progress release does not reopen deposits. No alternate route was used.

Actual customer mint completion, deposit acceptance and any restored AddMargin
operation remain unverified. No signing, approval, transfer, mint, cancellation,
financial retry or customer-state reset was performed to manufacture acceptance.
