---
type: "constraint"
date: "2026-05-27T20:11:55.195978+00:00"
question: "What are the known graphify graph limitations on this corpus?"
contributor: "graphify"
source_nodes: ["graph.json", "main.rs", "vault.rs"]
---

# Q: What are the known graphify graph limitations on this corpus?

## Answer

**Known graphify graph limitations as of 2026-05-27, version 0.8.17, on the Rumi corpus. Apply these caveats whenever interpreting graph traversal results:**

**1. Fuzzy dedup produces ~150-200 false cross-source merges per --update.** Two distinct nodes with identical normalized labels (e.g. .from_bytes() in two different Storable impls, .new() in two different struct constructors, Transaction type in icusd_ledger declarations vs rumi_amm) get fused into one node. The Rust public endpoint repay_and_close_vault was fused with the frontend TS ProtocolManager.repayAndCloseVault method this way. Filed upstream as safishamsi/graphify#1046.

   Practical impact: when querying a common-method-named node, expect spurious neighbors from other modules. Don't trust edge-by-edge traversal from nodes whose labels are dotted methods or generic type names. Trust higher-confidence patterns: dedicated function names (redeem_then_accrue, distribute_interest_revenue), file nodes, and concept nodes.

**2. The detect() carve-out for graphify-out/memory/ doesn't fully work.** detect() walks the dir but the gitignore _is_ignored filter still applies. The Rumi repo works around this with a fenced gitignore rule (graphify-out/* + !graphify-out/memory/ + !graphify-out/memory/**). Filed upstream as safishamsi/graphify#1047. Don't move memory files elsewhere — the docs/ dir is also gitignored for new additions per docs/-gating rule in .gitignore.

**3. Cross-file Rust call resolution is incomplete.** Tree-sitter Rust extraction misses cross-module function calls. Verified: of 24 graph-claimed-untested State methods, 14 (58%) ARE actually tested — graph just missed the edge. For test coverage / call-graph questions: use grep against tests/ as ground truth, not graph adjacency.

**4. TS to Rust call resolution is missing.** Frontend uses @dfinity/agent actors to call backend canisters. The TS AST extractor doesn't resolve actor method calls to the backend Rust function nodes. For "is this backend endpoint called from frontend?" use grep against src/vault_frontend/src/ and src/rumi_homepage/src/ (excluding declarations/ and dist/).

**5. Concept nodes from markdown sometimes get spurious "contains" edges from code files.** E.g. types.rs --contains--> whitepaper_stability_pool is structurally wrong (a Rust file doesn't "contain" a whitepaper concept). The LLM extractor used "contains" where it meant "corresponds_to" or "is_implemented_by". Cosmetic; doesn't affect betweenness math but can mislead on a quick read.

**6. Audit findings have weak code linking.** Concept-level finding nodes (CDP-01, CDP-08, etc.) link mostly to other DOC concepts (Wave 14 Close-Out, Stability Pool concept), not to actual implementing code. For audit-to-code traceability use git log --grep + git blame instead.

**7. The post-commit hook auto-rebuilds AST-only.** Markdown / image changes (including new memory Q&A files) won't fold into the graph until a manual graphify --update. Set expectation: routine code commits → graph stays in sync; doc/audit changes → require --update.

**When the graph IS reliable:**
- Specific named code symbols (e.g., redeem_then_accrue, accrue_redemption_shortfall_at)
- File-level relationships (which file contains which function)
- Community membership and high-betweenness identification
- Concept-to-concept relationships within audit/whitepaper docs
- Hyperedges capturing multi-node flows (the 3 swap-flow / LP-flow / vault-close hyperedges in this graph are accurate)

**Operational tips:**
- After --update: always spot-check a known function's edges to detect fresh dedup damage
- For high-stakes queries (security review, audit prep), always grep-verify graph claims
- Don't query dotted methods (.to_bytes, .new, .default) — they're guaranteed-merged false neighborhoods
- DO query whole-function names (handleSwap, repay_and_close_vault, compute_redemption_shortfall) — those are reliable

These caveats supersede any optimistic interpretation in earlier session notes. Issues #1046/#1047 might land upstream; if so, re-test the dedup pass and update this note.

## Source Nodes

- graph.json
- main.rs
- vault.rs