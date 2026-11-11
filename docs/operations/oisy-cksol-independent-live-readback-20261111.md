# OISY and ckSOL frontend — independent live verification

**PASS**, November 11, 2026, 15:57 UTC (07:57 Pacific). Ellis, case/source owner, independently checked the approved frontend-only publication after Dorian's stage **687** and activation **688** completed. Publication authority is Rob's explicit approval of [exact packet57ba7206](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/57ba7206036ec5dc1c463bfeeeb904b378ffea08/docs/operations/oisy-cksol-frontend-publication-20261111.md), SHA256 `06017038c3c422a9de69437029164eb2aa6bac61f53ee6d35b11e28f62ad98f3`.

## Independently observed result

- Live version **1794386914589**, built source `d54ac7b7235d27cf4aca52416a854d5cfab6240d`, runtime code `6e6f32fa` with accepted ckSOL `94ffd5d5` and preserved Vault51 lineage.
- Own complete **stable double-read** of the asset canister: **4,398 keys /4,592 encodings**. Every one of the **3,987 prior keys**, their properties and untouched encodings equals the pinned baseline. All **411 new assets** and **seven approved existing content changes** match their expected hashes and lengths. Old stage686 assets remain present. No inventory gap, deletion or property drift was observed.
- **Nine primary-domain HTTPS checks** match the frozen approved manifest: root identity and gzip, version, both Internet Identity/domain files, repaired swap service bundle, ckSOL route bundle, and `/swap` and `/solana` route fallback entries. TLS validation remained enabled.
- Fresh public state-tree module/controller records for both canisters are byte-equal to the independently read preparation baseline: backend `43bbbca98df29296a7dde2f134ce15db519bb745c53c1fcf442ef97b54223f09`; frontend `04e565b3425fe7510ee16b02adcfe3f01abc9a2725c82a21cb08969241debd62`. This proves public module/controller preservation; private settings and permissions are separately checked by the release owner.

| Served item | SHA256 |
| --- | --- |
| Root identity and route entries | `94a8f4b4f453511dab19c4888cd04cf6fc6e951538c0d03665e95404d87ac649` |
| Root gzip | `f929936343f765cfdce1c8d4a05caa28e6abcadddcda4255c2c6d09fa5db3231` |
| Version | `26fbde01878dd85bf856092600af43faa5f396045dc0cf7f727a55f1d56c79ef` |
| Internet Identity origins | `01d37f58e287d02a74d7767b81d8d214de14d14f062654d660b2b5da3aadd8fd` |
| Custom domains | `d04261cb158bd5c519857f330ae347351cfbfe0ffbf2a2e1c3a7974065c08cf4` |
| Swap service `DrsW40bG.js` | `b2ab59b469e3077e1594405e65fbef1c1d0737063d2d2613a66da6df28c02917` |
| ckSOL route `55.BdQ1zfce.js` | `f9ace34b91123680f645fdb913d7421f1717745435facf63db8c0999566d0d7b` |

The independent final inventory JSON SHA256 is `cf05a3e3932ecdf980cbbf7fafaf5f04fa695dc7472de2d0b822cad64d60cce0`. The approved manifest and comparison remain `aff95d8a` and `d1302303`; publication pins `786ea7a8` and publisher `b5d3d599` are unchanged.

## Proof boundary and owner action

The direct OISY stablecoin swap repair and clearer ckSOL minting page are now deployed and served-byte verified. Source tests/reviews and previous mocked browser checks remain the source evidence; no new production build, full source review or test-suite repetition was performed here. The exact OISY owner-rejection mechanism was reproduced locally; Rob's actual initiating session/failed step and any earlier successful allowance remain unverified.

No wallet signature, approval, swap, mint, transfer, cancellation, storage clear or replay was executed by this verification. A real OISY swap remains an owner-authorized action: refresh the app, reconnect the intended account and review prior attempt/approval history before a deliberate new swap. Customer saved-request recovery/borrowing acceptance remains separately unverified. Backend correction, reserve activation and financial holds remain unchanged.

Routine worktree pressure cleanup found zero eligible removals and removed nothing. All dirty, active, locked and unmerged worktrees were preserved.

## Subsequent owner acceptance failure

After these successful publication-byte/preservation checks, Rob reported the swap still failed after disconnect, hard refresh and reconnect. The exact failed local-versus-remote branch and prior allowance remain unknown. This receipt remains valid evidence of what was published; it is **not evidence that the usable swap was fixed**. SEC14 is reopened. Corrective source and acceptance are tracked in the [November11 handoff](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/606eb896f239bd65aef2cc8f83ce9be20f862352/docs/operations/oisy-owner-reconnect-source-20261111.md); no financial retry was performed.
