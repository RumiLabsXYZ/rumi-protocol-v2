# Security Policy

## Reporting a vulnerability

Please **do not** open a public issue for security reports. Email security disclosures to **vector.iso@gmail.com** or open a private GitHub Security Advisory on this repository. Provide enough detail to reproduce the issue and a contact for follow-up. We aim to acknowledge reports within 72 hours.

If a finding affects funds in flight, please prioritise contacting us directly so we can coordinate a response before any public discussion.

## Supported versions

The deployed mainnet canisters are the only supported surface. Module hashes for every canister are recorded in commit messages and on-chain via `dfx canister --network ic info <id>` — anyone can verify what is running.

## Governance posture (controllers and admin authority)

Rumi Protocol is currently operated under a **single-controller model**: each canister has a small set of NNS-recognised controllers held by the founding developer, plus the parent canister where applicable. There is no multi-sig, no two-phase admin rotation, and no hot/cold key split today.

This is **deliberate and time-bounded**. The protocol is migrating to SNS (Service Nervous System) governance, where every privileged action — config changes, parameter updates, code upgrades, controller rotation — moves behind on-chain DAO proposals. The SNS migration is itself the rotation event; bundling key-rotation hygiene into a separate intermediate step would add ceremony without changing the trust model. Until that migration ships, the single-controller posture is the documented and accepted design.

The 2026-04-22 third-party-style internal security review surfaced four governance-hygiene findings (admin allowlist anonymous-rejection, controller/admin separation, 3pool admin rotation endpoint, AMM `set_admin` two-phase delay). These are all tracked and **scheduled to land alongside the SNS migration**, not as standalone deploys. They do not affect end-user funds under the current single-controller model — the controller is the only principal that can invoke the affected paths.

If you want to verify the current controllers for any canister, run:

```
dfx canister --network ic info <canister-id>
```

## Audit history

Rumi Protocol has been through a structured internal security review at commit `28e9896` (audit-anchored 2026-04-22), with three sequential verification passes:

- **First pass — verification sprint:** static walk of 73 findings across 11 specialist analysis passes (async-state races, oracle integrity, ICRC hygiene, stable-memory upgrade safety, stability-pool accounting, redemption peg-defense, caller-auth, debt/interest, liquidation mechanics, inter-canister failure, cycle DoS).
- **Second pass — remediation verification:** every Resolved-Confirmed claim independently re-verified against the post-fix code at the deployed canister hashes.
- **Third pass — drift + follow-up wave verification:** the seven 2026-05-01/02 follow-up waves verified end-to-end and the prior Resolved-Confirmed rows re-checked for refactor drift.

A public-facing summary of the three-pass review is available on request. Findings, fixes, deployed module hashes, and test fences are all traceable through the commit history of this repository.

## Out of scope

The following are intentionally not in scope for current security review:

- Single-controller risk (covered by the SNS migration; see above).
- Pre-existing low-severity housekeeping items explicitly accepted as deferred (event-log eviction, pre-upgrade serialization layout). These are documented and tracked, with watch thresholds rather than fixes, until protocol scale or operational signals justify the change.
- Third-party canisters (NNS, ICP ledger, XRC, Internet Identity) — issues there should be reported to their respective maintainers.

## Responsible disclosure

We treat security reports as collaborations. Confirmed reports will be credited (with permission) once a fix has shipped, and the relevant canister's commit message and module hash will reference the disclosure. We do not currently run a paid bug-bounty programme but are happy to discuss recognition for high-impact findings.
