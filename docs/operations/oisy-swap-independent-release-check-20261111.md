# OISY stablecoin swap — independent release preparation check

November 11, 2026. Ellis, case/source owner. Dorian owns the sole frontend publisher.

## Result

**PASS for the bounded frontend publication preparation.** This does not authorize or prove publication, a backend installation, a wallet approval, a swap, or financial recovery. The customer's actual initiating wallet state, failed step, prior allowance and swap outcome remain unverified.

Accepted source is `6e6f32fa955fc464ad20aa40e2d832ec6931cfd8`; built source is `d54ac7b7235d27cf4aca52416a854d5cfab6240d`, with only the readable handoff added afterward. The tree preserves accepted ckSOL `94ffd5d5` and Vault51 `097bf775` lineage. Six frontend files differ from the ckSOL base; backend, declarations and dependency files are unchanged. Source evidence records 62 focused tests and two independent PASS reviews. Existing global typecheck remains 30 errors/66 warnings; no whole-project clean claim.

## Independently checked artifact and baseline

- All **740** actual output files match the frozen manifest, including every paired gzip/brotli file decoding to its original. The repaired signer and separate approval/swap rejection messages are present in the new swap bundle; the accepted ckSOL guidance is present in its route bundle. Version: `1794386914589`.
- Stable double-read baseline contains **3,987 keys/4,181 encodings**, including the prior staged assets. The plan adds **411 keys** and sets **418 contents**. Only **seven** existing content sets change: index and version files, including their retained encodings. There are no deletes, clears, unsets or existing property changes. Every new asset's properties match a unique existing template. Planned final keys: **4,398**.
- Both Internet Identity/domain files match their baseline bytes. Fresh primary-domain root, version, both auth files and the old swap service bundle match the pre-release inventory. This independently confirms that the repaired swap implementation is not yet the live entry point.
- Fresh public state-tree module/controller reads match the frozen release baseline: backend `43bbbca98df29296a7dde2f134ce15db519bb745c53c1fcf442ef97b54223f09`; frontend `04e565b3425fe7510ee16b02adcfe3f01abc9a2725c82a21cb08969241debd62`. The release owner's authenticated status reports Running. This check did not independently read private settings.
- Candidate canonical Candid SHA256 is `43da24f0341ca8fe52837071020e910f9ff3ecdef83a04f0dbc8f6e3033334db`; served Candid equals it after removal of its single final newline. Raw served-file SHA256 is `c18553dfedbd1546092def0773857de3d9afc7a286fa80e9028866f384950e92`. Do not describe these raw hashes as equal.

## Exact frozen pins

| Input | SHA256 |
| --- | --- |
| Frontend manifest | `aff95d8a06aa00108a5077cabd2732b039f1bfdca2aac58e4bf3bd21868aa8fb` |
| Publication comparison | `d1302303309d8ab48536728e47fffe13e750f092e249eea507f1fef9147cd709` |
| Baseline inventory | `f70fb087ab8492f2b20d51874db44ca59dead116f789be57ab67e67f2808345f` |
| Publication pins | `0c3d2593620275b5d9a2d1d2b77ac5da4a7ef13b5d544f9e16fe3ce90a6c4ac4` |
| Publisher | `124cf45192957f154094f21d8c59f4ccc74188f7635536d347f86bebfaf9be1d` |

## Publisher review and offline verification

The reviewed publisher pins the source payloads, exact comparison and inventory, installed backend/frontend modules, existing frontend settings and permissions, and the release identity. It requires fresh matching inventory before either phase. It has no backend mutation or prerequisite to install the separately held Vault51 correction. Existing financial holds remain.

Only asset `create_batch`, `create_chunks`, and `commit_batch` mutations are permitted. Durable once-only dispatch records hold uncertain outcomes for reconciliation rather than automatically repeating them. Stage adds new assets first; activation is refused until authoritative and served staging verification is recorded. Existing content changes are confined to the index/version allowlist. A separate publication namespace preserves the old staged batch.

Independent offline Candid encoding/decoding checked stage **822 operations** (411 creates + 411 contents), **574,660 encoded bytes**, and activation **7 operations/652 encoded bytes**, both below the 1.8 MB ingress guard. Nine frozen inputs' hashes match; every payload hash/length is correct and under the per-content bound. No stage or activation dispatch marker existed at this check. These were local read-only checks, not calls to the asset canister.

## Remaining boundary

Dorian must finish and push the readable exact frontend-only decision, preserve the frozen output, and obtain any publication authority required by that packet. After any authorized publication, independently verify the final authoritative inventory, retained properties/encodings, primary-domain root/version/auth files and new swap/ckSOL bundles. A real OISY approval and successful swap still require the owner's deliberate action. No automatic replay or financial canary is included.
