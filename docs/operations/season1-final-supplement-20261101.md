# Season 1 final-period supplement

Candidate prepared 2026-11-01; live application is separately verified below when executed. Rob requested completion of the final calculation and supplemental credits.

Epoch20 was closed from its two saved snapshots, adding 575,310.45787886 ordinary points in37rows. Earlier45-row compensation remained complete. A new backup was preserved before that close; no older snapshot was replaced.

## Final grant convention

Award **559,710.20305749** additional points to all15season participants for `[2026-10-26 00:00:00 UTC, 2026-10-31 23:59:59 UTC)`, the configured remaining interval. Existing total4,294,097.27203403 becomes **4,853,807.47509152**. Exact per-participant components, prestates and totals are in `season1-final-allocation-20261101.json`.

This is modeled make-whole compensation for missing random snapshots. It cannot reproduce their counterfactual outcomes and may change relative airdrop shares. It preserves all existing points and the5%token pool.

- Debt uses current observed principal debt, reverses recorded borrow/repayment changes to the start, and times those changes at their event timestamps. Current accrued debt interest is allowed over the earlier interval; this is a compensation convention, not a claim of exact historical interest.
- 3pool grants the full recorded composition cap (`icUSD +4*(ckUSDC+ckUSDT)`) for positive continuous qualifying3USD holdings. This intentionally replaces normal proportional verification; historical virtual prices are not proven. Wallet3USD transfer-log length remained2489 since the earlier October18 observation; SP history contains no final-period deposit/withdrawal. Five participants’ raw holdings are below their recorded dollar values, so this allowance is material. Participants without qualifying holdings receive zero for this component.
- Stability Pool uses observed current icUSD balances at1x and3USD at2x throughout the interval. Assigning current interest to the full period is a disclosed allowance. The saved history has127final-period events:122InterestReceived, three opt-ins and two opt-outs.
- No AMM award: its five-event liquidity history is unchanged. No synthetic repayment5x window is added: upstream events still do not identify a qualifying repayment asset. This preserves the existing live earning policy.

## Evidence and source limits

The saved selected vault events cover contiguous bounded scans523370–543825. The final interval has two repayments and one new vault/borrow. A second41-page scan for AdjustVault/Admin/Deficit events found upgrades, collateral registration and an oracle breaker, with no debt corrections, redistribution, dust forgiveness or partial collateral withdrawal. No specific dust omission was found. Query observations are not a certified global historical snapshot; deployed-source/provenance assumptions remain disclosed.

Allocation JSON SHA256: `dbe721f821fd6a309f54fcf9a6d754cd45e38b8afb3436b5fb16ac45d74cf27d`.
Protected raw input inventory SHA256: `eb5f2680b70f9c5150c4a2dce2e90b936dc0cfc49cbc4abd3993410925de34c1`. Raw responses include unrelated private account metadata and remain in the local protected evidence directory; they are not published.

## Application

New memory20stores15receipts. Existing memories0–19and earlier receipts remain intact. The additive admin-only apply call checks493ordinary rows, epoch21with no open epoch, exact15participant pre-credit totals and the completed45-rowprior award. It preflights all rows, then writes totals and receipts in one synchronous message. Repeating a completed call returns the same status without adding points. Existing public outage-history readers include the new separately bound receipts; the prior award’s status still counts only its original log. Final application disables the epoch driver and ingestion polling, clears their timers and leaves no fabricated epoch21snapshot/summary.

Focused checks: locked offline library/binary compile; three supplement tests (auth/prestate, exact grant totals, successful apply/replay, populated receipt decode); native Candid export/drift test and regenerated JS/TS. Two independent reviewers passed numerical and bounded code scopes after fixes. Required release/CI and actual application proof remain distinct.
