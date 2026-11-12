# Swap wallet support cleanup — source candidate, not deployed

The user requested removal of the always-visible signing diagnostic and investigation of the teal panel hidden behind the fixed portfolio strip. The existing swap success report remains accepted.

## Scope and evidence

Based on frozen producer `25e1441fd4688f0a8fa3748f9eccf77fb57fb0ff`; its source, build, manifest, dist and publication question remain unchanged. This separate candidate adds a native collapsed Wallet connection help disclosure around the existing OISY diagnostic. Neither mounting nor opening help requests an account; only the existing explicit read-only check does. All signer/session/ownership guards and financial services remain byte-unchanged.

Actual live DOM observation identified the obscured section as `aria-label="Check pending vault action"`, rectangle top11.996/bottom115.278 with parent DIV outside main; main starts127.274. No live tab navigation, reload, wallet click or data mutation was performed. Public served lineage remains1794415826220; the already-reviewed frozen recovery layout moves this same banner into padded main. No second layout patch is needed.

The new connected-OISY integration test failed on baseline because no collapsed disclosure existed, then passed after the markup change. Focused run:31tests/4suites PASS (swap help, inspection session protections, installed signer accounts-only behavior, saved-request recovery). Existing unrelated swap accessibility warnings remain. Full suite and production build were not run; no new dependencies or financial calls.

## Composed browser QA

Synthetic data/transport fixture renders the actual layout, PositionStrip, SeasonBar, swap route, SwapInterface, OisyAccountCheck and recovery banner together; only wallet/network services and unrelated header/unused liquidity widgets are mocked. Desktop1280x800: strip bottom95.5, recovery top127, SeasonBar top242.094; expanded strip bottom133.5, recovery top165. Mobile390x844 expanded: strip bottom125.5, recovery top149, SeasonBar top317.594; no horizontal overflow. Scroll keeps the strip fixed at y56. Help initially closed; opening performs zero inspection calls; explicit Check performs one mocked account read and zero execute/resume/cancel calls. Page identity/content, no framework overlay, screenshots and relevant console health PASS. No real signing, allowance, swap or customer request was tested. Temporary viewport reset and fixture browser closed.

## Release boundary

Dorian must review this separate delta before composing any new frontend build. Existing publication decision4ea7146a covers only the prior frozen recovery bundle; this candidate does not silently replace or repin it. No rebuild, deployment, backend/reserve action or customer financial authority is claimed. Repayment Max investigation is a separate incoming UX issue.
