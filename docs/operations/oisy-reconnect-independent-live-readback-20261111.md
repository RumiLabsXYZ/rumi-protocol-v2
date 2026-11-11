# Corrective OISY release: independent live readback — November 11, 2026

Ellis independently verified the corrective frontend at 17:03:02 UTC after
Dorian's once-only stage689 and activation690. Version **1794415826220** is
live on the primary domain. This establishes publication and preservation;
actual authenticated account matching, prior allowance and a successful owner
swap remain unverified. The first owner check is explicitly accounts-only.

## Exact source and preparation

[Source and acceptance](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/801d60aa2b99e16cf555af54ba5f8840ab79d284/docs/operations/oisy-owner-reconnect-source-20261111.md)
pins code cb1701ee39118d9ac8d76c746dac3cb7dc23cce4 and producer
801d60aa2b99e16cf555af54ba5f8840ab79d284. The original installed-adapter
regression reproduces stale cached-owner reuse; the correction positively
retires the actual connection and protects replacement sessions. The fresh-page
account diagnostic makes no request on mount and never approves tokens, submits
a swap or adopts another owner. Local and remote rejection branches are distinct.

Deterministic evidence: 91 core tests across seven suites, then 18 focused tests
across three suites for the final parent reachability change. Two fresh source
reviews and both focused placement confirmations passed after reproduced cleanup
and swallowed-storage-error findings were fixed. Actual installed connector/PNP
integration and synthetic desktop/mobile diagnostic states were checked. The
existing 30 type errors and 66 warnings remain; no new diagnostics were introduced.

[Exact publication packet](https://github.com/RumiLabsXYZ/rumi-protocol-v2/blob/4c9496373cf015589f7f25aba6c776da1a9d6e2b/docs/operations/oisy-reconnect-frontend-publication-20261111.md)
binds manifest b011560485aaae7dfe401c3680bfec7deee9d13465fdc643de65835340a56122
and pins 9b3aaa7782dde7d55d66735e122c78190b25f57bcce3078b23a5dc717f8283d8.
Ellis independently checked all740 files,476 compression pairs,13 input pins,
exact new assets/content scope and the namespace-only publisher change. One
production build was used. Placeholder commit size differences were reconciled
to unsigned LEB128 chunk/batch IDs; actual stage encoded423954 bytes.

## Independent live results

- Own stable double-read: **4701 keys /4895 encodings**. Inventory SHA256
  `9d8f527a731e420984e687a0b3721aa592f8060aab7a0d943903d59b5858daa7`.
- Every4398 prior key/property/untouched encoding is preserved. All303 new assets
  and exactly7 changed existing content encodings match the frozen plan. No
  deletion, property change or auth-file change occurred; prior staged assets remain.
- Ten primary-domain reads match frozen files: root identity/gzip, version,
  both auth/domain files, shared connection service, swap diagnostic, ckSOL guide,
  and both swap/Solana fallback routes. Version is1794415826220.
- Certified public status reads before and after show frontend module
  `04e565b3425fe7510ee16b02adcfe3f01abc9a2725c82a21cb08969241debd62`
  and paired backend module
  `730f54713dca1938c40b661ef6bd6ddc23555d92ca4c8e1feca282e3a33f8cf9`,
  with identical controller sets. This release did not install a backend.

| Served file | SHA256 |
| --- | --- |
| Shared connection/swap service hmFKXzdA.js | cae5107e71d4883833a4c9ba9d8f8f40a6e91ef3a3d8ba90792e5f1560b6c1d5 |
| Swap/account check 57.DD9lWGno.js | cfc56914cd122e542f875e47f68e488d4d7eb8920c681059555ec9197dea7434 |
| ckSOL guidance 55.BjiQHkGX.js | 9d415d9aa67fb5c7680cef89ab947acc6b571cf60cc677167d388ba249daabaf |

Retained independent preparation/live/module report SHA256 values respectively:
`a391e54da1f3b74cf7c16eafa6b8fe7ee43985d21db2ff113775075ab8c04e40`,
`8fead57cc6eb77c265c38f96bd9649f5dc3b7bf724fa5a5d78475c4880914361`,
`40bfdc2781f5ec28128cf3285d9b92e5548c560ba73668f214e99fc676f58b7d`.

## Owner acceptance boundary

The earlier published version failed owner acceptance; that failure remains part
of the record. Rob recalls multiple approvals, possibly including an allowance;
we have not verified its actual state or the failed request branch. Reload the
corrective page and choose **Check signing account (read only)** before a further
financial attempt. Report match, mismatch or unavailable result. It reads account
identity and changes no account. Do not interpret this as proof that an earlier
approval or swap succeeded. No wallet signing, financial replay, account switch,
customer storage clear or reserve action was performed by the agents. SEC14 stays
Review pending actual owner acceptance. Existing customer XRP/borrowing acceptance
and disabled ICP reserve remain separate.
