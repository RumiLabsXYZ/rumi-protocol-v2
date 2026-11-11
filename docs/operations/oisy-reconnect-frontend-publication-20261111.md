# Corrective OISY reconnect frontend — November 11, 2026

Dorian owns this single frontend build/publication; Ellis owns source and case
acceptance and independently checks new served artifacts. The earlier bundle and
batches687/688 are preserved. No backend installation, wallet call, allowance
change or financial retry is part of this release.

## Authority and scope

Rob's latest direct instruction is independently verified in Merritt's chat,
user message `01a1cb65-739a-7840-a34c-a4d303fd4fd4`: “Get Ellis to finish the fix
and make sure it actually works this time. Let me know when it's ready for me
to try again.” The bounded repair includes production publication/readback so
that an owner check can use the corrected software. This does not authorize a
swap, signing, account switch, storage manipulation or financial diagnosis on
Rob's behalf. No additional approval is assumed for backend or reserve actions.

Accepted code `cb1701ee39118d9ac8d76c746dac3cb7dc23cce4`; producer HEAD
`801d60aa2b99e16cf555af54ba5f8840ab79d284` adds documentation only. Exact source
packet SHA256 `4905cdf3e91547f823fb4743919c7802d2e38405b67501b78648b46fb5227a91`:
[source/acceptance receipt](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/801d60aa2b99e16cf555af54ba5f8840ab79d284/docs/operations/oisy-owner-reconnect-source-20261111.md).
Twelve frontend files change. Existing backend/Candid/bindings, dependencies,
ckSOL source and recovery journals remain unchanged. Two focused reviews and
91tests across7suites pass; actual installed connector/PNP/wallet integration is
exercised with synthetic transport, and desktop/mobile rendered diagnostic QA
passes. Existing typecheck debt remains30errors/66warnings; no new diagnostics.
The concrete fresh-page placement correction adds a red-to-green parent-wiring regression; final18focused tests across3suites and both placement confirmations pass. Prior91core tests/reviews remain valid; no broad repeat campaign.

## Expected behavior and proof limits

Explicit OISY disconnect retires the actual PNP cache and verifies removal;
failed storage removal is reported truthfully. Delayed cleanup cannot erase a
replacement session or disconnect unrelated Plug. Expected owner/session and
all exact financial arguments stay enforced. Local-owner mismatch and remote
sender rejection have different codes. The explicit “Check signing account
(read only)” button requests only accounts through its own click-owned popup;
it cannot approve/spend or adopt another owner.

The confirmed disconnect/cache defect is reproduced with the installed adapter;
it is not a captured trace proving Rob's actual failed-popup identity. Actual
authenticated account matching, earlier allowance/history and a successful owner
swap remain unverified. Publication/readback and owner acceptance remain separate.

## Preservation and execution

Current paired backend is exact Running `730f54713dca1938c40b661ef6bd6ddc23555d92ca4c8e1feca282e3a33f8cf9`,
with unchanged canonical Candid. No backend readiness gate is added to this
frontend-only repair. Use the existing additive publisher and once-only journals:
new keys first, verify, then only exact mutable index/version content sets.
Preserve every old key/property/untouched encoding, frontend module/settings,
Prepare/Commit permissions and auth/domain files. Unknown writes stop for readback;
no replay of prior batches/chunks or old candidate build.

The single production frontend/auth build completed successfully in18.11seconds,
with native exit0 and owned group closure verified. Source remains exact801d60aa.
Peak owned RSS2.30GB, physical growth52.46MB, minimum free92.41GB; actual shared
allocation0bab7a1b was consumed once. No backend build, test replay, download or CI.
All740artifact hashes match and a complete frozen copy is retained.

| Exact input | SHA256 |
| --- | --- |
| Manifest740files | b011560485aaae7dfe401c3680bfec7deee9d13465fdc643de65835340a56122 |
| Additive comparison | 82916df8b11ca7032b2bb00068618ee8a417413aa2a3e24953bbdd466ab4cf0a |
| Publication pins | 9b3aaa7782dde7d55d66735e122c78190b25f57bcce3078b23a5dc717f8283d8 |
| Once-only publisher | 3fa9d120f5fa6af83f5c01db02179bcba0c230ad2a49e06e16da290c39bb6ff6 |
| Stable full baseline4398keys/4592encodings | 0828c8f7c8db315d57ccb0165d6f051ce79702481773bc840d1a821b6c045cf8 |

New frontend version **1794415826220**. The additive plan creates303keys, stages
303identity content sets, then changes only7index/version content sets. No
property changes, deletes, clears or unsets. Expected final inventory4701keys /
4895encodings. Auth/domain bytes and all300oldstage686keys are explicitly checked;
the general preservation verifier covers all4398prior keys/properties/untouched
encodings including prior batches687/688. All content sources/hashes and actual
Candid commit sizes pass deterministic verification. The same established
publisher differs only in its isolated journal path; returned unique chunk IDs
and exclusive fsynced dispatch markers are never reused. Publication is prepared,
not claimed executed by this packet.

See [pins](evidence/oisy-reconnect-release-20261111/publication-pins.json),
[manifest](evidence/oisy-reconnect-release-20261111/frontend-manifest.json),
[comparison](evidence/oisy-reconnect-release-20261111/frontend-comparison.json),
[plan verification](evidence/oisy-reconnect-release-20261111/plan-verification.json)
and [actual build receipt](evidence/oisy-reconnect-release-20261111/build-session.json).
