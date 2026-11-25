# EXP-03 cursor reader — source handoff

November 25, 2026. Owner: Tamsin. Status: implemented and focused-tested;
not merged, deployed, or verified against a live Borrow query.

## Scope and outcome

Implements the approved frontend-only plan at `9fcb0a06d0e133c5935be3d7391b71a7e4ca103e`,
`docs/plans/2026-11-25-explorer-revenue-cursor-review.md`, SHA256
`c3f27882490ff451394f615024f8592fe62f262ee620880b14b95a8cc73b34b4`.
Baseline: `8546e6a530eed99428ce4a4639676b602165bf61`.

For visitors checking borrowing and redemption revenue, the reader now uses the
existing bounded cursor endpoint instead of the failing whole-log filter.
One fixed snapshot supplies both 24h and 90d totals. Each run reads at most
25 sequential pages of 2,000 log entries; Continue retains the checkpoint.
Empty pages advance, timestamps need not be ordered, and each page is validated
atomically before its exact bigint amounts or cursor are committed. Retry cannot
recount a previously accepted page. Cancellation discards late replies and stops
further requests; it cannot cancel an already dispatched query.

RevenueLens shows progress, Continue/retry/refresh controls, the snapshot time and
tail, exact eight-decimal icUSD amounts, and partial known-dated subtotals.
Undated fee counts and amounts remain explicit and prevent a complete total for
that category. Redemption delivery events do not add a second fee. Only fully
dated, fully covered snapshots enter the existing ten-second cache. Approximate
combined/swap displays retain their approximation marker; existing charts and
activity navigation remain available.

## Focused verification

- Service tests: **17 passed**, covering shared reads, 25-page continuation,
  sparse pages, inclusive/non-monotonic timestamps, exact amounts above the safe
  JavaScript integer range, duplicate/range/cursor rejection, atomic page failure,
  first/later-page retries, undated records, zero/cache/refresh, and cancellation.
- Rendered RevenueLens tests: **6 passed**, covering chart failure versus empty,
  Borrow navigation, partial-to-complete continuation, retry, missing timestamps
  with swap data still visible, refresh, and cancellation on unmount.
- Strict standalone TypeScript check of `feeScan.ts`: passed.
- `git diff --check`: passed.

Commands from `src/vault_frontend`:

```sh
npm run test:unit -- --run src/lib/services/explorer/explorerService.revenue.spec.ts src/lib/components/explorer/lenses/RevenueLens.spec.ts --no-cache
npm run test:unit -- --run src/lib/components/explorer/lenses/RevenueLens.spec.ts --no-cache
node node_modules/typescript/bin/tsc --noEmit --strict --target es2022 --module esnext --moduleResolution bundler --skipLibCheck src/lib/services/explorer/feeScan.ts
```

The combined run passed all 17 service tests and five of six rendered tests.
The failing test assumed the compact formatter emitted two decimal places;
after correcting that assertion and preserving the existing breakdown bars, the
rendered file passed all six tests. Passing service tests were not repeated.
Initial runner setup lacked installed dependencies and generated SvelteKit config;
reused installed versions matching the lockfile and ran `svelte-kit sync`.
No dependency, lockfile, backend, Candid, or generated declaration change.

## Acceptance limits and next action

This establishes local source behavior, not a usable deployed borrowing total.
No broad build, CI, release, additional live Borrow query, new worker, or funded
action was performed. Review is through the existing owner; no independent
production-readiness verdict is claimed. The separate positions/liquidation
transparency plan remains held.

Long logs can require many Continue steps, and cold-load latency is unmeasured.
Undated historical fees cannot be assigned honestly to a time window by this
reader. The existing revenue activity query is unchanged and can still report
unavailable. Backend endpoint availability, anonymous browser success, and live
totals remain unverified under the current action boundary.

Recommendation: review this bounded source change for the normal integration
path. A future authorized live check should establish cursor compatibility and
usable latency before claiming customer impact. A missing endpoint, bounded-page
trap or unacceptable scan time would change the next investment; this source
result does not presume a backend release.
