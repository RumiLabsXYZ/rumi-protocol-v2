# Base icUSD bridge: canonical Kanban board

This is the single canonical board for the ICP icUSD <-> Base bridge work.
`base-icusd-bridge-lookout.json` is derived from this file and must stay aligned with it.

- Worktree branch: `codex/base-icusd-bridge`
- Baseline: `origin/main` at `32358d23d98b6430cab0236a3245519c9322fe90` (fetched 2026-10-24)
- Coordinator: Claude (Opus 5.5, main thread). Supervisor: Miles (Codex), for Rob.
- Boundaries: no merge, no deploy, no live signing or broadcast, no funds movement, no controller/role changes.
- Lookout: **FIRST OPERATIONAL UPLOAD HELD** (see `base-icusd-bridge-operating-rules.md`). The projection is maintained locally only; nothing is published.

## Proof levels

| Level | Meaning |
| --- | --- |
| `none` | Not started or no evidence yet. |
| `source` | Source read or written; no test executed. |
| `local-test` | Deterministic local tests (cargo unit, forge, vitest) passed. |
| `local-integration` | Local PocketIC and/or local anvil execution passed. |

No card may claim a live, testnet, or mainnet proof level. Live work is listed under Held.

## Shared build coordination

- One cargo target dir per worktree (this worktree's `target/`). Do not share `CARGO_TARGET_DIR` across sessions.
- Only the coordinator runs wasm release builds and PocketIC suites; workers run focused `cargo test -p <crate> --lib` or `forge test` in their owned area.
- Disk is tight (about 12 GiB free at start). Workers must not create extra worktrees or target dirs.

## Cards

| Card | State | Owner (requested / actual model) | Files owned | Depends on | Acceptance evidence | Proof |
| --- | --- | --- | --- | --- | --- | --- |
| BB-00 Baseline and inventory | done | coordinator (Opus 5.5 / Opus 5.5) | `docs/coordination/*` | none | Worktree inventory run (0 eligible, 65 blocked); removal pass denied by permission classifier and not retried; baseline recorded | source |
| BB-R1 Recon: Foundry contracts | done | worker (Sonnet / Sonnet) | read-only | BB-00 | Cited report delivered: IcUSD has no pause/cap, `burn(amount,vault_id)` is repayment, Base not in backend `evm_chain_config`, no Foundry CI; baseline `forge test` 14/14 | source |
| BB-02 Base bridge contract | in progress | worker (Sonnet / Sonnet) | `foundry/src/BridgedIcUSD.sol`, `foundry/test/BridgedIcUSD.t.sol`, `foundry/script/DeployBridgedIcUSD.s.sol`, `foundry/test/DeployBridgedIcUSD.t.sol` | BB-R1 | `forge test --offline` all pass incl. 14 existing; pinned selector/topic table; size < 24576 bytes; IcUSD.sol untouched | none |
| BB-R2 Recon: backend EVM rails | in progress | worker (Sonnet / Sonnet) | read-only | BB-00 | Cited report on tECDSA, tx, nonce, broadcast, observer, settlement queue, supply | none |
| BB-R3 Recon: backend core, build, Candid, CI | in progress | worker (Sonnet / Sonnet) | read-only | BB-00 | Cited report on ledger calls, ICRC-3 proof, persistence, Wasm size, Candid generation, CI | none |
| BB-R4 Recon: vault frontend | done | worker (Sonnet / Sonnet) | read-only | BB-00 | Cited report delivered: hand-rolled EIP-1193 precedent in `routes/ckusdc`, no viem, CSP blocks direct Base RPC (use wallet provider), Oisy click-bound sequential calls + ICRC-21 required, `getAllDelegationTargets()` must list new canister, `regenerate-declarations.sh` arrays, no node_modules in worktree | source |
| BB-01 Design spec and plan | todo | coordinator (Opus 5.5) | `docs/superpowers/specs/2026-10-24-base-icusd-bridge-design.md`, `docs/superpowers/plans/2026-10-24-base-icusd-bridge.md` | BB-R1..R4 | Both files committed (force-added past `/docs/` ignore) and pushed | none |

## Held (needs separate authorization; not started)

- Public-testnet (Base Sepolia) contract deployment and canister install.
- Any mainnet deploy, controller change, contract role assignment, live signing/broadcast, or funding.
- Worktree removal pass (`scripts/cleanup_worktrees.sh --remove-eligible`): denied by the permission classifier on 2026-10-24; dry run showed 0 eligible anyway.

## Change log

- 2026-10-24: board created; recon cards dispatched.
