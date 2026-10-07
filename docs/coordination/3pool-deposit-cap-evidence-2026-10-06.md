# 3pool deposit concentration policy

User-authorized implementation and merge; no deployment, configuration mutation, wallet signing or funds action. Branch `codex/3pool-deposit-cap`.

## Accepted behavior

The limit is exactly 666/1000 (66.6%), calculated from nominal decimal-normalized reserves. It is a deposit admission rule, not an oracle valuation or a pool-wide limit on swaps, donations or withdrawals.

If the current pool is at or below the limit, the resulting pool after a deposit must remain at or below it. If the current pool is above the limit, the incoming deposit itself must contain at most 66.6% icUSD. Thus proportional deposits at the cap are accepted; deposits proportional to an already 70% icUSD pool are rejected. Either or both other stablecoins qualify. Stable-only deposits remain accepted above the cap. Initial liquidity obeys the limit.

The shared calculation enforces query/update parity. The update holds the existing pool lock before reading balances and rejects invalid deposits before the first token pull. Native additions are checked and ratio calculations use normalized U256 values. No stable-state fields or migrations changed.

Frontend policy checks are informational; the backend under the lock is authoritative. Non-OISY performs a fresh preflight before approvals. OISY preserves its click gesture using an exact-amount, principal-bound quote cache with a 30-second freshness limit and no extra network await before the first approval. An expired quote requires an explicit refresh; unknown outcomes are not automatically retried.

## Deterministic evidence

- `cargo test -p rumi_3pool --lib`: 123 passed.
- Fresh `cargo build -p rumi_3pool --target wasm32-unknown-unknown --release`:passed. Added the standard Candid export macro so the compiled interface can be extracted. No canister install performed.
- `cargo test -p rumi_3pool --test deposit_concentration_cap -- --test-threads=1`: 9 passed using official PocketIC server 7.0.0 with pinned crate 6.0.0 and existing real ICRC ledger fixtures. Covers unchanged balances/LP supply on rejection, exact-cap proportional admission, above-cap70/30 rejection, either/both stable pairing, stable-only deposits and swap parity.
- Existing `cargo test -p rumi_3pool --test integration_test -- --test-threads=1`: 9 passed, including upgrade event preservation.
- `cargo test -p stability_pool --lib three_pool_deposit_error_compatibility_tests`: 2 passed; the remote consumer decodes both the new rejection and ordinary successful replies.
- `cargo check -p rumi_protocol_backend --lib`:passed with existing warnings.
- Final frontend full regression suite: 934 passed in 74 files; production build and frontend authentication asset verification passed. Focused policy/service/mounted suites total22cases.
- Full frontend type-check baseline and final: 28 errors and 65 warnings in 24 files. Error-message multisets are identical; no new errors introduced.
- Deposit endpoint Candid comparison:strict structural equality passed for `add_liquidity` and `calc_add_liquidity_query` against the freshly extracted Rust interface, including the new error. Bindings regenerated via `scripts/regenerate-declarations.sh rumi_3pool`.
- Full interface structural equality has pre-existing drift: source exposes `receive_donation` absent from canonical Candid, and source ICRC3 archive callback uses a record while the declared callback is a function. These unrelated repairs are deferred rather than blocking the deposit change. No claim of full-interface equality.
- Local browser `/3usd` could not load pool data (Failed to fetch), so no connected-wallet or browser transaction proof is claimed. Mounted Svelte tests and mocked wallet-flow tests cover the changed form and approval ordering.
- Disk-pressure cleanup: 0 eligible removals, 0 removed; dirty, locked and unmerged worktrees preserved. The managed implementation checkout reuses the existing Rust target cache and locked frontend dependencies.

## Independent review rubric

Both reviewers receive the same accepted behavior and diff. Verify exact 666/1000 boundary and correct nominal normalization; below/above/initial policy; query/update agreement; admission before token pulls under pool lock; overflow handling; either/both/stable-only correction; precise paired amount rounding; no stale quote acceptance or broken OISY gesture; Candid/error mirrors; no stable-state or authority changes; no unintended swap/withdrawal restriction; test strength; accurate user-facing scope and evidence boundaries. Every blocker must have concrete file/function evidence.

Fresh compiled Wasm SHA-256: `f158f3f1bcf13d3bb1983ab240df9ccf952569909325c483f48901e038470078`. This is a build identity, not deployment proof.

Review results and merge proof pending.
