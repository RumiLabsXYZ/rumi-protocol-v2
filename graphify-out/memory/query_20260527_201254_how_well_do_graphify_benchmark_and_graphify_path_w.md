---
type: "feature_test"
date: "2026-05-27T20:12:54.421503+00:00"
question: "How well do graphify benchmark and graphify path work on this corpus?"
contributor: "graphify"
source_nodes: ["graph.json", "handleSwap", "swapRouter", "vault.rs"]
---

# Q: How well do graphify benchmark and graphify path work on this corpus?

## Answer

Two graphify features sampled on this corpus, both verified working with caveats:

**graphify benchmark (corpus 880k words / 6696 nodes / 12783 edges):**
- 446,400 tokens of naive corpus context → ~6,535 tokens per graph query
- 68.3x average reduction
- Per-question samples: 'main entry point' 161.7x, 'data layer to api' 79.3x, 'core abstractions' 71.5x, 'authentication' 57.5x, 'error handling' 43.4x
- Reasoning: questions that benefit most are ones whose answer lives in 5-20 specific nodes (entry points, core abstractions, named topology). Questions that benefit least are diffuse pattern questions (error handling) where the relevant code is spread across many small sites.

**graphify path "A" "B" — concrete trace results:**
- ✅ handleSwap → swapRouter — 2 hops, clean trace through executeRoute (correct architecture)
- ✅ executeOperation → vault.rs — 5 hops through reindex_vault_cr → compute_collateral_ratio → liquidate_vault_debt_already_burned. Real call chain.
- ⚠️ redeem_icp → ICP — 8 hops with detours through audit POC tests + @dfinity/principal. The high-betweenness universal import (@dfinity/principal) creates spurious shortcuts.
- ❌ open_vault_and_borrow → Vault System (CDP) — no path found. Code ↔ concept gap (filed as known limitation).
- ❌ distribute_interest_revenue → State — node not found (different name in graph; lives in SP code).

**When path works well:** both endpoints are named code symbols in the same broad subsystem (e.g. frontend service → service). Result: clean ≤5 hop path.

**When path fails or detours:** code ↔ concept paths (whitepaper/audit concepts often disconnected from code), or symbols with common method names that got dedup-merged (introduces spurious shortcuts through @dfinity/principal or other universal-import god nodes).

**Practical usage pattern:** for tracing a user click to its on-chain effect, walk top-down via two-hop probes: handleX → service, service → ApiClient, ApiClient → backend candid endpoint, backend → state mutation. Each two-hop call is clean. End-to-end shortest-path queries are noisier.

Both features earn their keep but require interpreting results through the limitations doc (see saved 'What are the known graphify graph limitations' memory).

## Source Nodes

- graph.json
- handleSwap
- swapRouter
- vault.rs