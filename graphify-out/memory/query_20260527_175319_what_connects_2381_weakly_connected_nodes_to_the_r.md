---
type: "query"
date: "2026-05-27T17:53:19.184379+00:00"
question: "What connects 2381 weakly-connected nodes to the rest of the system? Doc gaps or genuinely isolated?"
contributor: "graphify"
source_nodes: ["test_treasury_integration.sh", "update_icusd_logo.sh", "test_treasury.sh", "deploy_mainnet.sh"]
---

# Q: What connects 2381 weakly-connected nodes to the rest of the system? Doc gaps or genuinely isolated?

## Answer

The vast majority of weakly-connected nodes are EXPECTED, not gaps. Of 7178 total graph nodes, 3651 have ≤1 edge (50.9%). But only 37 are truly orphan (0 edges); 3614 have exactly one edge — invariably a 'contains' edge from their parent file. Breakdown of the orphan/weak bucket by directory: 875 frontend leaves (Svelte/TS terminal symbols), 706 generated Candid declarations (.did.d.ts/.did.js — mechanical bindings nobody writes by hand), 441 tests (test scenarios that don't call each other by design), 388 backend leaves, 289 analytics, 191 audit POCs, 144 root scripts/config, 122 docs (mostly .md sub-headings), the rest in 3pool/amm/sp/treasury/liqbot.\n\nThe specific files the report named are all bash scripts (test_treasury_integration.sh, update_icusd_logo.sh, test_treasury.sh, deploy_mainnet.sh). Bash scripts don't 'import' each other — each is a self-contained deploy/test ritual that shells out via dfx commands. There is no edge to add. Not a gap.\n\nGenuine gaps (truly orphan, 37 nodes total): (1) Build config files (vite.config.js, tailwind.config.js, postcss.config.js, .ic-assets.json) — expected, they're consumed by tooling not application code. (2) 4 rumi_analytics mod.rs files — these are module-declaration files; the AST extractor missed the implicit 'mod foo;' wire-ups. (3) ~7 Svelte components (Pagination.svelte, StatCard.svelte, VolatilityGauge.svelte, DashboardCard.svelte, FeatureCard.svelte, NaviLink.svelte) — likely consumed via auto-import or dynamic imports that the AST extractor couldn't resolve. (4) 1 vite.config.js.timestamp-* file — build cache leakage; should be filtered. (5) 1 generated icpswap_pool/index.d.ts. (6) 1 LLM-extracted concept ('rumi_amm Constant-Product AMM') from the audit PDF that couldn't link to the live code.\n\nRecommendations: (a) add vite cache and .timestamp-* glob to graphify ignore. (b) Improve Svelte module resolution in AST (auto-imports in SvelteKit). (c) Improve Rust mod-declaration resolution. (d) The 706 generated declarations should arguably be excluded from the corpus entirely — they're mechanical, not architectural. None of this affects answer quality for the questions you ask — the long tail is mostly authentic terminal symbols.

## Source Nodes

- test_treasury_integration.sh
- update_icusd_logo.sh
- test_treasury.sh
- deploy_mainnet.sh