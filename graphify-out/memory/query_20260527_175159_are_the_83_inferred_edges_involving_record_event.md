---
type: "query"
date: "2026-05-27T17:51:59.291947+00:00"
question: "Are the 83 INFERRED edges involving record_event() actually correct?"
contributor: "graphify"
source_nodes: ["record_event()", "record_liquidate_vault()", "record_redistribute_vault()", "post_upgrade()"]
---

# Q: Are the 83 INFERRED edges involving record_event() actually correct?

## Answer

ALL 83 INFERRED edges are TRUE POSITIVES. Verified by grep: src/rumi_protocol_backend/src/event.rs contains exactly 82 'record_event(' call sites and 82 'pub fn record_*' definitions (a perfect 1:1 wrapper pattern — each record_<EventVariant>() function exists for the sole purpose of calling record_event(&Event::<Variant>{...})). The 83rd edge is post_upgrade() in src/main.rs:L405 which calls record_event(&Event::Upgrade(args.clone())) — also verified by grep. No false positives. Why are these INFERRED instead of EXTRACTED? record_event() is defined in src/storage.rs:L134 while the 82 callers live in src/event.rs. The AST extractor (tree-sitter-rust based) does not resolve cross-module Rust function calls without compile-level type information, so cross-file calls reliably miss the EXTRACTED bucket. The LLM semantic pass caught all 83 and tagged them INFERRED with confidence_score 0.8 — high-confidence inference because the call shape (record_<X>(&Event::<X>{…})) is unambiguous. Recommendation: no remediation. The graph is more honest about its uncertainty by classifying these as INFERRED than it would be by faking EXTRACTED tags. If you want EXTRACTED-tier accuracy, you would need a Rust-aware analyzer (rust-analyzer LSP, syn-based parser, or cargo-graph) integrated into the AST step — out of scope for graphify today.

## Source Nodes

- record_event()
- record_liquidate_vault()
- record_redistribute_vault()
- post_upgrade()