# Conflux frontend design and behavior QA

Date: 2026-09-13. Scope: local source/UI verification, not deployment, activation, or a signed round-trip.

## Accepted design

User accepted `exec-fc519458-1fec-44eb-bc74-09aa33454270.png`. Protocol overview stays left; two CFX/icUSD inputs stay right; projected position sits below the inputs/costs and above the action. Composition is closed by default. ICP, nICP, BOB and EXE group as ICP ecosystem by USD value. Friendly BTC/ETH/XAUT/DOGE labels retain exact original symbols in expanded details. CFX is the sole selectable collateral on this surface.

## Visual comparison

Source and implementation were inspected together in a rendered side-by-side comparison. Both source frames were 1487 × 1058. Evidence directory:

`/Users/robertripley/.codex/visualizations/2026/09/13/01a0996f-9772-7160-a8e2-53af92b85dbe/conflux-implemented-qa/`

- `comparison.html`, `comparison.jpg`: actual two-image comparison, not an assumed match.
- `desktop-1487x1058.jpg`: final Inter-based implementation at the target viewport.
- `desktop-complete.jpg`: taller desktop viewport capturing the full working surface.
- `mobile-top.jpg`, `mobile-wallet.jpg`, `mobile-position.jpg`: mobile behavior and layout evidence from the pre-font-swap pass. Final font change subsequently verified on desktop and narrow/tablet DOM checks.

Intentional state/content differences: mock prices, totals and 0.30% fee are illustrative. Implementation reads public backend/ledger values, explicitly scopes the overview to core pools, and does not pretend these totals include Conflux vaults. Stale CFX price gives no calculated/healthy position. Borrowing pause and technical status details remain visible. The fee configuration field is dormant in the current chain mint path, which queues the full requested debt; this frontend therefore does not invent the mock deduction. The 2.00% APR quote is conditional on exact backend configuration parity.

Fonts: public-hosting rights for the copied Circular TTF were not verified. The new copy was removed from this frontend and all new UI uses Inter v4.1 with OFL included. Rumi's existing homepage font was untouched. Original Rumi/icUSD SVGs and official Conflux vector marks were reused, not redrawn. Five consistent Heroicons utility icons ship with their MIT license. White official Conflux marks are used on dark backgrounds; blue remains the secondary accent. Decorative corner ribbons from the mock are omitted. This is an adapted implementation, not a pixel-identical screenshot clone.

Spacing/layout: two-column desktop proportions, aligned cards, larger heading/input hierarchy, compact risk card and emerald primary action are retained. The live pause notice and overview-scope note add height relative to the illustrative mock. No asset load failures were observed. Inter loaded successfully in the browser. Financial values use clear light text; unavailable values do not masquerade as healthy. Keyboard focus rings, labels, native details controls, native modal focus, and reduced-motion CSS are present.

Responsive checks: desktop 1487 and 1280; mobile 390 and 320; tablet 768. No horizontal document overflow in inspected states. Tablet initially split numerical values across lines; fixed by stacking panels at 980px. Transaction panel comes first below this breakpoint. Mobile header initially wrapped into four rows; fixed to a compact two-row layout with an accessible icon-only wallet control. Mobile risk labels wrap without collision and the action remains full width. Long account and transaction states still need an actual wallet session for visual proof.

## Functional evidence

- Initial composition is closed; expansion displays real public holdings under ICP ecosystem, BTC, ETH, XAUT, XRP and DOGE. Collapse works.
- Borrow/My vaults navigation works. Disconnected My vaults provides a connection action.
- Header, borrow-form and My vaults connect entrypoints open the same wallet picker.
- Mainnet acknowledgement starts unchecked, toggles normally, and remains part of the existing connection gate. No private-key field exists in the public build.
- No-wallet state is explicit. Modal close works and returns focus. No wallet was connected or impersonated.
- Status details link opens its details section. It reports disabled chain state, stale price and missing fresh hot-wallet proof; these are not overridden by the redesign.
- Amount entry is exact decimal text to bigint. Stale data does not calculate a healthy preview; errors and missing reserves remain unavailable rather than zero.
- Found and fixed an accidental repeated overview query caused by an effect tracking its own loading flag. It now loads once via onMount and on manual Refresh only.
- Browser console inspection returned no error or warning entries in the final desktop pass.

## Deterministic checks

- `npm run check`: 0 errors, 0 warnings.
- `npm test`, `npm run test:production-canary`, `npm run test:production-public`: 14 files, 57 tests passed in each mode.
- Public local-verification build: passed the production bundle policy, 17 files scanned after asset license/provenance additions. No dev-key or canary controls leak into the public artifact.
- Canary and testnet builds passed. Canary output was rebuilt after font removal so the local ignored dist no longer contains the stale TTF.
- `git diff --check`: passed.
- Vite's non-blocking large-bundle warning remains. No new runtime dependency or platform-specific Rollup dependency was added.

## Explicit limits

Browser checks used the local production-public origin at `http://127.0.0.1:5174`, not the deployed frontend. No mock wallet/provider was injected. No EIP-712 signature, deposit, mint, repayment, withdrawal or liquidation was performed, per the user's instruction to skip presence-dependent tests. Connected-state controls and successful minting are source/unit-test reviewed, not browser end-to-end proven. Healthy-position math has unit coverage, not a current live healthy-price screenshot.

No deployment, DNS mutation or activation was performed for this redesign. Production Conflux remains disabled in the observed status. Source/build/UI evidence must not be described as launch completion.
