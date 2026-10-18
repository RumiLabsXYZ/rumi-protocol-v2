# ckSOL collateral and minter

The `/solana` page uses DFINITY's existing ckSOL minter. Rumi holds collateral on the ckSOL ICRC ledger and does not create a Solana custodian or issue its own wrapped token.

## Verified upstream contract

Verified read-only against mainnet on 2026-10-18:

- Minter: `lh22c-kyaaa-aaaar-qb5nq-cai`.
- Ledger: `ls5lp-lqaaa-aaaar-qb5oa-cai`; 9 decimal lamports.
- Upstream revision: [`dfinity/cksol@2acf44e9e2243f1d10ac0fd75f264f6a0dfb52a2`](https://github.com/dfinity/cksol/tree/2acf44e9e2243f1d10ac0fd75f264f6a0dfb52a2).
- Retained Candid: `src/vault_frontend/src/lib/idls/cksol_minter.did`; SHA-256 `41e5473e0901fa286443a513c663d3ed7b6efe00da7ac3e44f5c2ee2573843e2`. It matches the deployed `candid:service` metadata after trimming its extra trailing blank line; structural equality is also verified with `didc check --strict`. The raw mainnet metadata SHA-256 was `595fdc18c8594a1a19962584a8d399a263fb1f2d84e35ff64173f4b0dd4d0533`.
- Regenerate upstream bindings with `bash scripts/regenerate-cksol-bindings.sh`. This does not fetch a moving upstream interface.
- Current endpoints are `get_deposit_address`, `deposit_sol`, `deposit_sol_status`, `withdraw_sol`, and `withdraw_sol_status`. The general chain-key guide's `process_deposit`/`withdraw` examples differ from the verified deployed API.
- Live terms observed: minimum deposit balance 0.02 SOL; minimum withdrawal 0.002 ckSOL; minter withdrawal fee 0.001 SOL. Read terms live rather than treating this observation as immutable.
- The official logo and Candid come from the pinned Apache-2.0 upstream repository.

## Collateral registration

`register_cksol_collateral()` is developer-only and uses ordinary ICRC registration, metadata queries, SOL/USD XRC pricing, and the existing best-effort Stability Pool registration. It copies ckETH's current risk settings at registration, including its debt ceiling, minimum debt, liquidation and borrowing ratios, fees, APR, redemption settings, recovery settings, rate curve, and XRC source floor. It converts ckETH's minimum token quantity from 18 to 9 decimals using checked integer arithmetic, rounding up. It does not copy ETH prices or raw wei amounts into lamports. Repeated registration preserves existing ckSOL configuration.

The frontend discovers collateral from the backend. Merely merging or deploying the page does not activate ckSOL borrowing. Verify the backend and Stability Pool registrations and a fresh SOL/USD price separately.

## Deposits and the cycle relay

ICP browser ingress cannot attach the cycles required by the official `deposit_sol` call. Rumi's backend relay always credits the authenticated caller's default ICRC account, uses the fixed official minter, and attaches a fixed 1 trillion cycles per notification. Upstream currently charges a 45 billion cycle verification fee and refunds unused cycles.

The relay starts with a zero budget. The developer may set a finite remaining sponsorship budget with `set_cksol_relay_budget`, capped at 100 trillion cycles. This setting allocates existing backend cycles; it does not top up the canister. Every dispatch conservatively consumes a full 1 trillion of sponsorship allocation, even if the minter refunds cycles or rejects the request. Actual canister cycle balance and allocation are distinct. The relay protects a 10 trillion cycle backend reserve and bounds retained owners to 1,000. Unknown outcomes remain locked; status checks do not authorize another deposit.

Deposits progress through Queued, Swept, Finalized, and Minted. A finalized sweep is not a completed ckSOL mint. Quarantined/unknown outcomes require evidence-based operator recovery; no automatic retry is inferred. The UI withholds new deposit addresses when sponsorship is unavailable, but capacity may change after an address is shown. The address remains controlled by the official minter and bound to the user's ICP account.

## Withdrawals and wallets

The user approves the official minter on the ckSOL ledger and calls `withdraw_sol` as that same user. The UI quotes current minimums and fees; the minter fee is deducted from the requested amount. It reserves the ledger approval fee plus a separate ledger fee allowance. All token quantities remain bigint lamports.

The current official minter has no ICRC-21 consent endpoint. OISY withdrawals are blocked before approval; Internet Identity and Plug are the implemented paths. The Rumi deposit relay has a consent message for OISY. Connected-wallet execution has not been established merely by source tests.

Withdrawal requests are durably recorded before approval/burn dispatch, isolated by owner and network, and serialized across tabs with Web Locks. Active authentication, session generation, wallet transition state, actual actor principal, and per-operation identity are checked. A lost burn response or ambiguous minter error blocks retries. A completed receipt may clear only its matching journal; stale tabs cannot acknowledge a newer withdrawal.

Only `TxFinalized.Success` reports successful SOL delivery. `Failure` does not imply reimbursement. Preserve the ledger burn block and contact the official minter's operators for uncertain outcomes. No automatic replacement withdrawal or recovery debit is sent.

## Release boundary

Implementation and test evidence are separate from deployment, collateral activation, sponsorship allocation, wallet signatures, real SOL mint/redeem, and settlement. This change authorizes none of those live actions.
