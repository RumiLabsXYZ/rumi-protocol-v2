# Conflux Public Launch Kanban

Updated: 2026-09-13

## Active

None.

## Blocked

| Card | Owner | Requested / actual model | Scope and file ownership | Blocker | Depends on |
| --- | --- | --- | --- | --- | --- |
| CFX-04 | coordinator | GPT-5 / GPT-5 | Present the generated branded screen concept for explicit design approval. | Concept generated and inspected; awaiting approval before creative implementation. | CFX-02 |
| CFX-05 | Sonnet worker | Sonnet / pending | Implement the approved Conflux frontend redesign in `src/conflux_espace_frontend/**`. | Awaiting explicit approval of CFX-04 design. | CFX-04 |
| CFX-06 | Sonnet worker + coordinator | Sonnet / pending | Browser and wallet-path QA, production build, custom-domain code readiness. | Awaiting CFX-05. User-presence signature tests are explicitly skipped. | CFX-05 |
| CFX-07 | coordinator | GPT-5 / GPT-5 | Merge, exact-head backend deploy, $1,800 config mutation, price/proof refresh, monitor, activation, and post-activation verification. | Source gates are green. Awaiting merge, exact-head rebuild, snapshot, artifact proof, and live readbacks. Frontend redesign is not a backend activation gate. | CFX-01, CFX-03 |
| CFX-08 | coordinator | GPT-5 / GPT-5 | Deploy the exact approved frontend and complete custom-domain registration. | Requires CFX-06; Cloudflare SSL impact must be resolved safely. | CFX-06 |

## Done

| Card | Owner | Requested / actual model | Evidence |
| --- | --- | --- | --- |
| CFX-00 | coordinator | GPT-5 / GPT-5 | Live chain state, endpoint digest, vault/supply zero-state, pool depth, three-source CFX price, controller/module status, and disabled-readiness blockers captured. Price was refreshed to 4,784,000 e8; Conflux remained disabled. |
| CFX-01 | Sonnet worker + coordinator | Sonnet / Sonnet 5 | The compiled public-readiness ceiling, coupled production fixture, and current launch row now use 1,800e8. Old/new config digests were reproduced; `cargo check -p rumi_protocol_backend --lib` and `git diff --check` passed. Coordinator restored consumed recovery scripts and historical evidence unchanged. |
| CFX-02 | Sonnet worker | Sonnet / Sonnet 5 | Read-only control/brand audit completed with file-line evidence, QA matrix, wallet-owner test boundary, and complete Rumi visual brief. |
| CFX-03 | Luna fallback reviewer + coordinator | Sonnet / Sonnet 5 aborted; Luna fallback | Read-only release-delta audit completed after Sonnet's unused Playwright MCP child hung. Luna correctly required a fresh artifact, snapshot, exact module hashes, stopped upgrade, and post-upgrade readbacks. Coordinator rejected rewriting consumed historical recovery artifacts: they truthfully record the earlier $2,000 operation and are not inputs to this launch. Current live source lineage was matched to `28ecfe8b`, whose backend source/tests/dependency inputs match `b155976c` except the intended ceiling change. |
| CFX-09 | Sonnet reviewer | Sonnet / Sonnet 5 | Independent no-MCP review passed. It confirmed the narrow change and fail-closed transition, and requested the explicit old-$2,000 rejection case now covered by the 11/11 focused readiness suite. |

## Coordination rules

- Sonnet workers run with repository shell, git, search, read, and file-write tools when their card permits writes.
- File ownership is exclusive while a card is active. Read-only reviews may overlap.
- A fallback to Luna must be recorded here with the Sonnet failure reason before work starts.
- Only the coordinator may perform merges, canister upgrades, DNS mutations, config mutations, activation, or fund-moving tests.
- Production readiness is an exact-head and live-state claim, not a build-only claim.
