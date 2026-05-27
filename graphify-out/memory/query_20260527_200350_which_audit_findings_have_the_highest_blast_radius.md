---
type: "query"
date: "2026-05-27T20:03:50.299547+00:00"
question: "Which audit findings have the highest blast radius?"
contributor: "graphify"
source_nodes: ["CDP-01", "CDP-08", "CDP-10", "Stability Pool"]
---

# Q: Which audit findings have the highest blast radius?

## Answer

Surface-level answer: CDP-01 (XRC oracle silent failure, no ReadOnly fallback), CDP-08 (notify_liquidatable_vaults missing caller guard, LIVE-EXPLOITABLE on mainnet), and CDP-10 (sp_attempted_vaults set before call resolves) tie at the top — each touches 2 communities. CDP-01 has the most edges (5), connecting to Wave 14 Close-Out, the XRC Exchange Rate Canister concept, Fix, and Part II - CDP Protocol Domain Layer.\n\nDeeper finding (more useful): the graph's audit-to-code linking is WEAK. Of 20 concept-level finding nodes, none touch more than 2 communities. The findings link mostly to OTHER audit concepts (Wave 14 Close-Out, Stability Pool concept, Internal Three-Pass Review) rather than to actual code symbols. CDP-08, for example, should be linked to notify_liquidatable_vaults() in vault.rs — but it isn't.\n\nWhy: audit findings live in markdown files under audits/. The semantic extractor read those docs and connected findings to other doc-level concepts; the leap from 'Finding CDP-08: notify_liquidatable_vaults...' to the actual src/.../vault.rs:notify_liquidatable_vaults code node was not made (cross-document-to-code reference resolution is a known graphify limitation).\n\nFor SNS migration prioritization, this means the audit-finding nodes aren't reliable proxies for code blast radius. Use git log per-finding-PR or branch-diff scanning instead. To improve graph traceability: (a) add explicit function-name backticks in audit docs and run --update so the LLM picks them up as references, or (b) a future extraction pass that explicitly cross-references finding nodes with code-node labels.

## Source Nodes

- CDP-01
- CDP-08
- CDP-10
- Stability Pool