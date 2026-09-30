# Bitcoin experience

## Accepted scope

- `/bitcoin`: Rumi's mint/redeem interface for DFINITY ckBTC.
- `/bitcoin/borrow`: dedicated BTC-to-ckBTC-to-vault journey, adapted from `/doge/borrow`.
- Canonical full-word paths: readable and consistent, with no competing `/btc` route.
- Bitcoin audience: charcoal, ivory, orange, restrained typography; Bitcoin collateral only. No token selector or other collateral choices on these pages.
- Preserve supported IC wallet sign-in, exact satoshi amounts, confirmation checks, fee review, current collateral terms, partial-outcome recovery and vault management.
- Luna implementers and independent Luna reviewers. Root owns shared integration.

## Evidence collected

Read-only anonymous mainnet queries on 2026-09-29:

- DFINITY ckBTC minter `mqygn-kiaaa-aaaar-qaadq-cai`: `get_minter_info` returned 4 confirmations, 300 satoshi minimum deposit and 50,000 satoshi current minimum withdrawal.
- `get_deposit_fee` returned 100 satoshis.
- `estimate_withdrawal_fee` returned a bare record with `bitcoin_fee` and `minter_fee` fields; this differs from the DOGE interface.
- ckBTC ledger `mxzaz-hqaaa-aaaar-qaada-cai`: `icrc1_fee` returned 10 satoshis.
- `icrc1_minting_account` returned the minter's default account. The minter withdraws via a transfer to this minting account, which is a fee-free burn under ICRC-1. Redemption reserves the approval fee; ordinary vault collateral transfers reserve two ledger fees.
- Rumi `get_collateral_config` returned ckBTC Active, 8 decimals, ledger fee 10, minimum collateral 1,000 satoshis and minimum debt 10,000,000 raw icUSD units. Pages fetch current values rather than freezing these observations.

Primary interface/caller sources:

- [Official ckBTC Candid](https://github.com/dfinity/ic/blob/master/rs/bitcoin/ckbtc/minter/ckbtc_minter.did).
- [Official minter wrapper](https://github.com/dfinity/ic/blob/master/rs/bitcoin/ckbtc/minter/src/main.rs): `update_balance` rejects an anonymous caller.
- [Official update_balance](https://github.com/dfinity/ic/blob/master/rs/bitcoin/ckbtc/minter/src/updates/update_balance.rs): explicit owner controls credited account; this operation only checks/mints.
- [Official Bitcoin Txid interface](https://github.com/dfinity/bitcoin-canister/blob/master/interface/src/lib.rs): Candid bytes are reversed when formatting the conventional explorer transaction ID.
- [Official retrieval source](https://github.com/dfinity/ic/blob/master/rs/bitcoin/ckbtc/minter/src/updates/retrieve_btc.rs) and [ICRC-1 minting account rules](https://github.com/dfinity/ICRC-1/blob/main/standards/ICRC-1/README.md#minting-account): the redemption transfer burns at the minter's minting account.

## Coordination

Rob explicitly authorized coordination with chat `01a0ebc3-fc5e-7070-874b-279ecec48a67`. That chat supplied reviewed, pushed commit `6a7e44ae985f1fa281f06d781bf1a14366c11bac` on `codex/ckusdc-minter`, confirmed a separately authorized ckERC20 frontend release, and assigned this coordinator the combined draft source PR. Preserve that branch and incorporate its source before the combined build. No Bitcoin deployment or financial action is authorized by this request.

## Independent review rubric

Both independent Luna reviewers receive the same accepted scope, source diff, primary interface sources and deterministic evidence. They do not see each other's findings.

- DFINITY ckBTC only, canonical full-word routes, focused Bitcoin navigation and wallet balances; no other collateral selector on these pages.
- Honest mint, redeem and vault states: a deposit check is not a mint, an accepted withdrawal is not confirmed BTC, and source/build evidence is not deployment evidence.
- Exact satoshis, explicit default-subaccount owner, correct upstream Candid, live minter requirements and fees, current active collateral terms, stale-data refusal and an available refresh action.
- Wallet review matches dispatched destination, amount and fee. Debit operations use the connected wallet; mint-only checks cannot debit it. Oisy approval/retrieval uses one signer without an intervening query that breaks the wallet flow.
- Late address/QR/mint/withdrawal replies cannot paint another wallet or a later session. Durable pending records survive teardown, reload and cross-tab changes. Uncertain outcomes cannot invite blind resubmission.
- Borrowing preserves DOGE's non-atomic open/borrow recovery and explicit existing-balance option. Session-minted receipts are the default; existing ckBTC requires a deliberate choice.
- Combined source preserves the reviewed ckERC20 changes and generic DOGE behavior. No backend state, dependency churn, deployment, wallet signature or funds movement is in scope.
- Desktop/mobile built preview is usable, labels remain legible, and unsupported guarantees are absent.

Every blocking finding needs a concrete file/line or reproducible scenario. Optional redesign preferences do not block the accepted scope.

## Verification

Implementation and final checks in progress. Preliminary browser inspection of the local source showed both routes with the focused Bitcoin navigation. No wallet connected and no funds moved. Global typecheck baseline and final test/build/review evidence will be recorded after the workers finish.
