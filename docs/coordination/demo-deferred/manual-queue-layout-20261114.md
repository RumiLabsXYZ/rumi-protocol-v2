# Deferred manual-queue layout limitation

- ID: SEC21-MOBILE-TABS-1.
- Feature: parent liquidation-page tabs at mobile width.
- Owner: Ellis, within the existing SEC21/customer-resolution lane.
- Observed concern: at390×844, the document is452px and the three-tab navigation437px. The same dimensions were captured before SEC21 on version1794685142389 and after on1794714390164.
- Deferral reason: this pre-existing parent layout is outside the approved queue-availability predicate/control fix. It is not a new regression or a release gate; no additional source/build campaign was run.
- Delivery limitation: do not claim the composed manual page has no horizontal overflow. Actual live queue was empty, so no real reserved-card layout was claimed; conditional state remains backed by mounted tests/mock rendering and exact served code.
- Revisit: make a narrow parent-tab responsive correction and verify390px composition before claiming complete mobile manual-page layout support or using that claim for client-money readiness. No financial canary is required for the layout correction.
- Evidence: [SEC21 live result](../../operations/manual-queue-live-result-20261114.md) and its linked public DOM metrics/independent receipt.
