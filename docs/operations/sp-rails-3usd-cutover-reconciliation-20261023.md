# 3USD V1 cutover reconciliation - 2026-10-23

Evidence captured read-only through 2026-10-23T15:24:40.340546+00:00. Root executor independently obtained all 2,489 contiguous 3pool blocks and all 144 backend liquidation events over 266 contiguous bounded windows through global entry 530651. Independent reviewer R-03 verified the files and matches.

| Classification | Raw 3USD | 3USD |
| --- | ---: | ---: |
| 39 SP to backend transfers | 8591654543 | 85.91654543 |
| Confirmed refund block 1444 (offsets 1443) | -120552710 | -1.20552710 |
| Physical default account balance | 8471101833 | 84.71101833 |
| 34 exact transfer/event/SP history reserve matches | 8219310823 | 82.19310823 |
| Retained unallocated legacy excess | 251791010 | 2.51791010 |
| Hashed reserve account | 0 | 0 |

The four early plain ICRC-1 transfers (blocks 10, 11, 12, 13) exactly explain the excess. Their ownership/disposition remains unresolved. They are distinguishable from all later backend-spender ICRC-2 reserve pulls. Preserve them without spending, crediting, refunding or calling them new reserves. No open V1 transfer was found: last bilateral transfer 2432 matches backend event 362053 and terminal SP history; no later bilateral row exists through the pinned tip. New V2 inventory has zero ingress/refund/proof/payout rows. This bounded historical conclusion does not authorize erasing any future unknown outcome.

## Machine binding

Canonical attestation SHA256: `735beb8ca505532c75b1c322188435bd170358aed7dd1672adf286b0981650a2`. The JSON binds full source data hashes, exact balances, matches and preservation disposition. Local source evidence: `/private/tmp/rumi-sp-rails-restoration-20261023/legacy-v1-capture/`. Copy the attestation to the durable task evidence before activation and re-read live balances/inventory and new blocks since the pinned tip. Any unexplained change stops activation for reconciliation.

Activation uses the existing developer-only `reconcile_three_usd_reserve_v1_cutover` with these exact 32 digest bytes, the actual SP caller acknowledgment, then existing ingress enable. V1 must become Retired. Neither book balances nor existing excess change in these calls. New journaled V2 work retains exact tuple and zero-fee requirements.
