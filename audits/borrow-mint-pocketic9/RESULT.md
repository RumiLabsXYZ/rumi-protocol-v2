# Focused owner-and-developer recovery rerun

- Command: `POCKET_IC_BIN=/absolute/path/to/pocket-ic-9.0.3 audits/borrow-mint-pocketic9/run.sh`
- PocketIC client/server: 9.0.2 / 9.0.3
- Runtime: **4 passed, 0 failed, 0 ignored** in 13.87 seconds
- Tests: developer exact reconciliation; developer receipt scan; developer complete-history absence recovery; owner exact reconciliation
- Backend Wasm: existing source-matched artifact SHA-256 `6e539c6073b628745b57338fc69128f82cf698a93fa455c108328f5676e60aaf`
- Flaky-ledger Wasm: existing fixture SHA-256 `a10ab2ff6fc3e007f16be1b9a191016fe0ebb324eb785fb723cb6b32d37f5b91`
- The runner's Cargo builds were no-op checks; neither Wasm was rebuilt. The only code change was the audit test copy.
- Loopback access was required for PocketIC. No private data is included here.
