# XLM test-only full withdrawal to claim: source complete (UNBUILT)

Recorded 2026-11-11. Status: source accepted by independent review as an **unbuilt** candidate. Not compiled or run. No runtime result is claimed.

## Code

- Implementation commit `0aa0e62338b572dc728889d65930ac71d6be8ff1` on `codex/native-xlm-collateral`.
- Test-build only (`test_endpoints`): `withdraw_all_xlm_collateral_to_claim(vault_id, destination)` shares the existing partial-withdraw helper. It requires exactly zero debt (no dust forgiveness), records one durable XLM claim for all remaining collateral before debiting, and keeps the vault open at zero collateral, following native XRP. The locked custody reserve, custody attribution and deposit receipt are untouched, so the reserve is never claimable. Partial withdrawal behaviour is unchanged.
- The extended `xlm_confirm_pic` funds the full repay with the existing developer-only `admin_mint_icusd` inside PocketIC only (no new API). It checks that full withdrawal is rejected while debt remains, for an outsider, for a bad destination and when repeated on an empty vault, each without state change; and that the owner's full withdrawal creates one claim for 889,990,000 stroops with the correct custody owner, nonce, testnet network id and fee, leaves the vault open with zero collateral and zero debt, and moves no icUSD.
- Production XLM stays Frozen and production route guards are unchanged.

## Prepared focused check (not run)

- Runner `/private/tmp/xlm-fullwithdraw-0aa0e623-docdesc-runner.py`, SHA-256 `cb1c89dacc0114a2f582649856b68b3039f213b65f28970c82237a6ffcd60be5`: a minimal copy of the accepted runner `d78c964a` with only the implementation commit, `main.rs`/`vault.rs`/test pins, output/lock paths and header changed.
- Pins: `main.rs` `4929bcf8d231e03d7ab69a14a6c8758597f9606ed078b92a55aba3372baa5020`, `vault.rs` `76903ee1e2b845dcf182e04c60ad40206067a8b508470fb78cd4c6b4e3f5b193`, test `d6d0a6f679b8aa3833959f1b03e934a8fe4ec62cf356b164c0a528859dc1d9e7`.
- Running it needs a fresh exclusive admission; earlier allocation `19430bb3` is consumed and released.

## Unchanged

The partial withdrawal PASS (receipt `4db74caccd3197acc8306dee4cdbc0617dcb3ce9a54098b53d7160bef9c4f1ff`) stands. No settlement, signing, Stellar transaction, payout or live proof exists.
