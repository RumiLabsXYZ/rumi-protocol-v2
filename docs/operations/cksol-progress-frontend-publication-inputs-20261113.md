# Approved SEC18 frontend — frozen actual publication inputs

Original human01a1d57e-43c7-7a02-8cf2-0794a179689c in Merritt explicitly approved
build/publication under6e3fdaa7 and Dorian–Cody coordination/Ellis verification.
Decision SHA2562cd75e9e33be545b842a22218ae375cce7c8ffae588c3245ee17206e5b12d658.
Exact source **ab44413da1a8b394245cd7734da4124a2335621b**; no source repin.
This freezes mechanical outputs covered by that approval, not another request.

One fresh allocation4e737700, once-only offline frontend/auth build: native0,
22.347924seconds, start2026-11-13T15:27:14.759Z/end15:27:37.107Z. PG54334 and
runner closed without kills, independently verified/released by Cody. Root proof
SHA256d2efc18705488885618635bba6254385695a4e03525a4b7ad73f8a86c99213e9.
Peak2,289,369,088B RSS,91,926,528B physical growth,45,977,780,224B minimumfree.
All740current/frozen files verified, mandatory auth/domain postbuild PASS.
Previous740SEC17frozen files, dependency links and receipts remain preserved.

New frontend version **1794583637373**. Fresh stable baseline5124keys/5318encodings;
**96newimmutable keys +only7existing index/version content sets** give expected
5220keys/5414encodings. All prior keys/properties/untouched encodings and auth/domain
bytes retained. No deletion, clear, unset, property edit or backend mutation.
Publication pins SHA256 **19a7321d2c4ac2c8f2b17c0a49d87ef0d1534506e1283684cbd1a0983e9a8435**.
Stage/activation both require unchanged backend6428d103 and frontend04e565b3;
settings/controllers and Prepare/Commit baselines are retained. Fresh served backend
API equals canonicalf0b0fcc2. The repayment fix and existing guards are unchanged.
Independent Ellis artifact verification is pending; no asset write has occurred.

[Manifest](evidence/cksol-progress-frontend-release-20261113/frontend-manifest.json),
[comparison](evidence/cksol-progress-frontend-release-20261113/frontend-comparison.json),
[pins](evidence/cksol-progress-frontend-release-20261113/publication-pins.json),
[plan verification](evidence/cksol-progress-frontend-release-20261113/plan-verification.json),
[build/resource closure](evidence/cksol-progress-frontend-release-20261113/build-session.json),
[backend API comparison](evidence/cksol-progress-frontend-release-20261113/backend-api-comparison.json).
Metadata/scripts only are pushed; no binary or private customer data payload.

## Separate vault213 deposit-pause diagnosis

This release preserves the existing standard AddMargin pull hold introduced in
[3b9e3787/#508](https://github.com/RumiLabsXYZ/rumi-protocol-v2/commit/3b9e37874688ec21f1692e2dbc598a6fbbe0521e), October10.
[VaultCard false flag](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/00cf550d3fcb97870cd4643883d5abd8900fa1a2/src/vault_frontend/src/lib/components/vault/VaultCard.svelte#L31)
and [API pre-approval guard](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/00cf550d3fcb97870cd4643883d5abd8900fa1a2/src/vault_frontend/src/lib/services/protocol/apiClient.ts#L91)
mirror the [unconditionally rejecting backend wrapper](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/00cf550d3fcb97870cd4643883d5abd8900fa1a2/src/rumi_protocol_backend/src/main.rs#L6946).
The [explicit recovery gap](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/00cf550d3fcb97870cd4643883d5abd8900fa1a2/src/rumi_protocol_backend/src/vault.rs#L3420)
is a confirmed delayed pull after liquidation removes its target: there is no
safe terminal recovery/refund destination. The hold is before amount/approval/
ledger dispatch; vault213's selected quantity does not cause it. It does not clear
by waiting for minting. Changing only the UI flag would leave backend rejection.
Fresh readbacks confirm installed6428/backend and1794555990015UI match that known
source lineage. No guarded update endpoint or financial transaction was invoked.
Ellis owns the separate AddMargin issue; it does not change SEC18's approved pin.

No wallet signing/approval, mint submission, deposit transfer, cancellation,
financial retry or customer-state reset is performed for this software release.
Real customer mint completion and restored AddMargin availability remain separate.
