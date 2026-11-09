# Deferred work to revisit before client money

Apply [DEMO_FIRST_DELIVERY_POLICY.md](DEMO_FIRST_DELIVERY_POLICY.md). This ledger records actual shortcuts and omitted investigation for new demo features; it does not pause incident response or weaken an existing money-bearing service.

For each actual omission, record: **ID; feature; owner; concern/skipped work; why not essential to the demo; demo limitation/containment; before-client-money verification; status; evidence/disposition.** Use lane-specific files under `demo-deferred/` and link them below when multiple owners are active. Existing project boards remain authoritative.

No feature-specific omission has been asserted by this initial policy entry. Each supervisor records the actual work it defers while delivering; do not invent a debt inventory or redo completed reviews merely to fill this file.

## Feature ledger index

Append actual lane-log links here. Before client-money use of the affected feature, revisit every applicable open entry and record the resolution, evidence and reviewer. Existing clients and production incidents retain their current safeguards.

- [Multichain icUSD groundwork — MC-DEF-1 to MC-DEF-6](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/codex/base-icusd-bridge/docs/coordination/demo-deferred/multichain-icusd.md), owned by Nora and Miles; destinations remain disabled.
- [Sunset collateral liquidation — SUNSET-DEF-1](demo-deferred/sunset-liquidation.md), owned by the release coordinator; production deployment verification does not include a funded liquidation.

### DEMO-EXP-001 — Explorer revenue source handoff

Owner: Tamsin. Commit `2369985f` on `codex/explorer-revenue-accuracy`. Five focused tests and both Svelte component compile checks passed; full frontend build, rendered 24h/90d unavailable-versus-zero acceptance, and exact live Borrow query/deployed-version evidence were omitted from the bounded source tranche. Hosted CI was not allocated; opening a PR triggers it. Limitation: source correctness evidence only, no demonstrated live repair. Revisit relevant rendered/build/live verification through the normal allocated release path before claiming demo/live accuracy or relying on this display for client-money decisions. No broad audit or new harness is required.

DEMO-EXP-001 update: PR #620 merged at 0f40a0a8; nine focused tests now include rendered chart failure versus empty and Borrow link, and the one allocated Frontend validation run 41951365095 passed its frontend build/auth checks at exact 984813f1. Those omissions are closed. Exact live Borrow query and deployed-version/live accuracy proof remain open for the normal separate release action; no deployed fix is claimed.

- [SEC11 native-XRP deposits — SEC11-DEF-01](demo-deferred/sec11-xrp-repeat-deposits.md), owned by Miles; custody credit is in scope, public Explorer/points projection is deferred.

- [SEC12 saved-request recovery — SEC12-DEF-01/02](demo-deferred/sec12-recovery-release.md), owned by Miles and Ellis; real customer outcome proof and existing global typecheck debt remain distinct from deployed repair.
