# XRP automatic settlement cycle-waste correction

Status: source candidate; focused validation and source-matched release artifact pending. Not approved for installation. Stability Pool liquidations must remain paused.

## Confirmed production mechanism

The automatic Stability Pool payout sweep revisits outstanding native-XRP claims every 600 seconds. Previously it used the claimant settlement path, which can replace a failed or safely expired transaction with another paid signature. Claim 7 repeatedly attempted a payment too small to create its unfunded destination account. Complete public account history contains 1,099 validated `tecNO_DST_INSUF_XRP` failures and one successful payment to that destination. First failure: 2026-11-07T03:27:30Z; final success: 2026-11-14T21:21:10Z. Those times match the retained hourly backend cycle-outflow step up and step down. No backend upgrade occurred at either transition.

Public anchors: first failure `15CD4BE95D7B95691064EE792C8D143F871F49B9E94D2AA51B86936C1D094889`; retained claim-7 failure `679C765AB164CFF8DA76CABD19DFB2D04B5FF64BECB8B4AFE4986CB0AD910CD7`; final success `468C3B651C7AEF1D588A000A56AD7C00C15C96C05B61BBB88FEE97C819D704EE`. Two providers independently confirmed the retained failure and final success; complete source-account pagination was obtained from xrplcluster.com (1,108 total transactions, 1,100 matching payments).

At the current documented production Schnorr fee of 26,153,846,153 cycles per signature, 1,099 failed attempts imply approximately 28.74T cycles in signature fees, plus RPC and execution. The 30B cycles attached per signature is not the actual fee; surplus is refunded. Historical per-call cycle refunds are not retained, so this is a fee-rate estimate rather than exact historical accounting. Official reference: https://docs.internetcomputer.org/references/cycle-costs/ .

Retained Sentinel samples show about 4.59-4.71T/day net runtime-balance outflow on November 9-13, compared with approximately 0.25-0.30T/day beforehand. Net balance outflow plus confirmed topups is not exact execution burn. Unknown funding steps remain explicitly unresolved. Production traffic, rather than local development/testing, is proven by the validated XRP history.

## Bounded source correction

Only the registered Stability Pool endpoint selects automatic settlement origin after existing caller/claimant authorization. It may sign an unsigned first attempt and reconcile a validated payment. A retained failed or missing transaction holds its claim, full remaining liability and exact signed tuple for explicit claimant recovery. Automatic sibling reconciliation cannot clear failed/expired tuples and inadvertently rearm signing. A second check at the actual paid-signature boundary rejects any retained automatic attempt. Existing claimant replacement, sequence/divergence quarantine, fee accounting, aggregate solvency and caller/custody locks remain in place.

The retry correction changes no persisted field or public method signature. The separate hold-preservation addition extends UpgradeArg with optional hold-only preserve_sp_liquidation_pause: Some(true) forces the pause on snapshot restoration and event replay; omission or false cannot resume it. State::upgrade applies this before state publication and setup_timers. Old Candid and serialized upgrade records remain decodable; canonical generated interfaces must be refreshed before release. Essential tests exercise the actual signing dispatch boundary with a counted signer, repeated observations, production State serialization/decoding, tuple/liability preservation, successful reconciliation, manual retry and fee treatment. No live claim is retried for testing.

## Release composition and holds

Target under preparation: backend `tfesu-vyaaa-aaaap-qrd7a-cai` only. Last independently observed installed module: `59a64f78e0e652d0e48413aba587d3f826c1dc25258f56ab09575c59de444111`; refresh before the exact approval packet and again before an approved installation.

The pause must survive both snapshot restoration and event replay before timers are scheduled. Automatic review rejected Adrian’s private hold-source handoff as outside the authorized payload/destination scope. That handoff was not retrieved or relayed through another route. This branch independently implements the minimal hold-only addition from its existing source under Rob’s direct test/build direction and explicit requirement to keep liquidations paused. His Vault 208 partial-liquidation sizing candidate is separate and is not authorized for silent inclusion or installation here. The final packet must identify the exact composed source, candidate upload/content hashes, upgrade args, tests, required checks and recovery boundaries. This document is not that final approval request.

No installation, liquidation resume, claim retry/payment, reserve change, funding conversion/topup, controller or permission change is authorized by this preparation. Existing Vault 202 and other action holds remain in force.
