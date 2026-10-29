# Native XLM demo lane: deferred work

Owner: Mara. Selected demo feature: a developer on an isolated test canister opens a pending XLM deposit address. Opening creates no collateral, vault, debt, or icUSD. This source is not yet demo-ready: compile the test build, run a focused open-flow test, verify the test-build Candid interface, and smoke the real threshold derive before claiming the demo works. The endpoint is compiled out of the production Wasm.

| ID | Actual deferral and reason | Demo limitation | Revisit before client money | Status |
| --- | --- | --- | --- | --- |
| XLM-D-01 | Expiry/cancel transitions for pending addresses are deferred from the single developer open-address demo. | Open rows continue to count toward the 10-per-owner cap; the demo uses only an isolated test canister. | Implement bounded expiry/cancel and retain permanent owner/network attribution for funded addresses. | Open |
| XLM-D-02 | Deposit confirmation, credit, borrow, and payout are outside the selected address-only demo. | An address is not usable XLM collateral and must not be presented as a funded vault. | Complete the verified deposit and claim lifecycles with reserve, fee, oracle, provider, and conservation checks before admitting client funds. | Open |
| XLM-D-03 | Production key admission is deferred while the isolated demo uses `test_key_1`. | The endpoint is test-build-only, so it cannot be called on the production Wasm; the first attribution still locks the selected key in the isolated canister. | Verify `key_1`, public-network binding, deployed artifact, config, and launch guard before mainnet admission. | Open |

The open-address source is in the XLM branch; the wider implementation and evidence plan remains [native-xlm-collateral-2026-10-27.md](../../plans/native-xlm-collateral-2026-10-27.md). Record disposition evidence and reviewer here when each entry closes.
