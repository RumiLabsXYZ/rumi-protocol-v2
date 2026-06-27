# AI Agent Instructions

Tested implementation patterns for ICP development are available as agent skills.
Before writing any ICP code, fetch the skills index and remember each skill's
name and description:
https://skills.internetcomputer.org/.well-known/skills/index.json

When a task matches a skill's description, fetch its content on-demand from:
https://skills.internetcomputer.org/.well-known/skills/{name}/SKILL.md

Skills contain correct dependency versions, configuration formats, and common
pitfalls that prevent build failures. Always prefer skill guidance over general
documentation when both cover the same topic.

## Canonical ICP Reference

Treat `https://docs.internetcomputer.org/llms.txt` as the canonical agent-readable map of Internet Computer docs and ICP agent skills. For any Rumi work involving ICP architecture, canister lifecycle, callers/principals, Candid, inter-canister calls, stable data, cycles, security, Chain Fusion, testing, generated bindings, or controller/trust assumptions, consult that index and the linked docs/skills when current source and repo-local guidance do not fully answer the question.

Prefer the ICP docs and skills from that index over generic web search or stale memory for platform behavior. Still verify Rumi's actual source, tests, candid files, generated declarations, deploy scripts, current refs, and live canister state before making repo-specific claims.

## Local Codex Skills

Use `rust-canister-engineering` automatically for Rust ICP canister work, stable-state or migration changes, Candid/generated declaration updates, caller/auth/controller behavior, PocketIC/e2e tests, chain-liquidation accounting, native-chain backend rails, and deploy-adjacent artifact verification. Its guidance is specific to our Rumi and adjacent canister repos and should complement, not replace, reading the current source, tests, candid files, and deploy scripts.

Use `adversarial-verification` before claiming high-stakes work is ready, including production code, security-sensitive changes, state migrations, native-chain integrations, architecture reports, public docs, deploy prep, or anything where semantic correctness matters beyond build/test results. Run deterministic repo checks first, then the adversarial gate when the stakes justify it.

## Rumi Agent Orchestration

For rumi-protocol-v2 work, use the local persona catalog and team-agent-orchestration when it makes sense. Do not ask the user to name the agents.

## Persona Catalog Maintenance

When a persona or agent run discovers a durable lesson, surface it in the handoff as a proposed persona/catalog update. Durable lessons include repo invariants, recurring workflow checks, naming rules, deploy or verification gotchas, security review heuristics, and failure modes that future agents should remember.

Do not propose updates for temporary branch state, one-off bug status, raw command output, speculation, secrets, or live IDs unless the repo treats those IDs as canonical. The main agent should review proposed updates, verify them against current repo evidence, and only then edit the persona files or local-persona-catalog entries.
