# Fiat-stable top-up report evidence

Date: 2026-10-07 (America/Los_Angeles). This is a read-only report preview
against `bfnu3-6aaaa-aaaab-qhanq-cai`. It does not activate the policy, append
top-up rows, change a timer, deploy a Wasm, or read an admin-only endpoint.

The report reads `get_point_ledger_len` and paginates the global
`get_point_entries` query over the exclusive prefix `[0, cutoff)`. The live
capture used the observed length as the cutoff (`401`) and an explicit
anonymous identity, query mode, `ic` network, JSON wrapper, and checked-in
current Candid file:

```text
python3 scripts/fiat-stable-topup-report.py \
  --canister bfnu3-6aaaa-aaaab-qhanq-cai \
  --network ic \
  --candid src/rumi_points/rumi_points.did \
  --json /private/tmp/fiat-stable-topup-report.json \
  --csv /private/tmp/fiat-stable-topup-report.csv
```

The script invokes `icp canister call` with `--identity anonymous --query
--network ic --candid ... --json --output candid` for every read. The prefix
contained 401 rows and 15 principals (the union of registration marker rows and
positive rows). It contained no `CkStable3PoolFlat4x` or
`CkStable3PoolUnmatchedTopUp` rows, so the result is classified as a
`legacy_only_approximate_historic4x` preview. The uplift is
`sum(floor(original_unmatched_row_points / 3))`, calculated independently for
each original row. It is not an exact replay of historical snapshot selection.

All point and e8s fields below are integers. Decimal fields in the generated
JSON/CSV are display strings. Share basis points are integer floor values; the
percentage strings are display values. Principals are public ledger data.

```json
{
  "canister": "bfnu3-6aaaa-aaaab-qhanq-cai",
  "network": "ic",
  "identity": "anonymous",
  "query_only": true,
  "ledger_length_observed": 401,
  "cutoff_exclusive": 401,
  "prefix_rows": 401,
  "population_count": 15,
  "classification": "legacy_only_approximate_historic4x",
  "method": "per-recorded-unmatched-row-floor-divide-by-3",
  "exact_historic_replay": false,
  "before_total_points_e8s": 228745060926062,
  "uplift_total_points_e8s": 215174119482,
  "after_total_points_e8s": 228960235045544,
  "source_totals_e8s": {
    "AmmLp": 25737570137930,
    "CkStable3PoolFlat4x": 0,
    "CkStable3PoolMatched": 33241043552203,
    "CkStable3PoolUnmatched": 645522358448,
    "CkStable3PoolUnmatchedTopUp": 0,
    "IcUsd3Pool": 26920995397096,
    "IcUsdDebt": 82398874906768,
    "IcUsdStabilityPool": 19950952488720,
    "Registration": 0,
    "ThreeUsdStabilityPool": 39850102084897,
    "VaultRepayment": 0
  },
  "reconciliation": {
    "before_matches": true,
    "original_rows_sum_e8s": 228745060926062,
    "per_user_before_sum_e8s": 228745060926062,
    "uplift_matches": true,
    "uplift_rows_sum_e8s": 215174119482,
    "per_user_uplift_sum_e8s": 215174119482
  },
  "users": [
    {"principal":"2bcfz-dnfki-nq7q2-v4gmo-4kvkg-3dabg-yn5it-ivcs5-ikpss-kk77p-lae","before_points_e8s":1678116358098,"uplift_points_e8s":0,"after_points_e8s":1678116358098,"before_share_bps":73,"after_share_bps":73},
    {"principal":"4alqm-afk6k-bybok-qvdyo-cnv7y-klel6-xm2pz-7h7jk-utmys-kttf3-vqe","before_points_e8s":140146587,"uplift_points_e8s":0,"after_points_e8s":140146587,"before_share_bps":0,"after_share_bps":0},
    {"principal":"6ixwn-zfdck-dvelg-ng2um-zaoqj-frt7r-ojyox-4hhb2-cexnt-3wsf5-pqe","before_points_e8s":43714635184313,"uplift_points_e8s":0,"after_points_e8s":43714635184313,"before_share_bps":1911,"after_share_bps":1909},
    {"principal":"aja7b-6aown-ucyjz-a6ddu-w67th-fuok4-vxqmi-24d76-74igd-rkgrp-5ae","before_points_e8s":113484794189,"uplift_points_e8s":0,"after_points_e8s":113484794189,"before_share_bps":4,"after_share_bps":4},
    {"principal":"akzqy-74twk-ctls2-jufen-kygii-cev4l-w5w62-foeml-rmgye-xtwme-uae","before_points_e8s":14517557872922,"uplift_points_e8s":1138695600,"after_points_e8s":14518696568522,"before_share_bps":634,"after_share_bps":634},
    {"principal":"cgsme-y5yno-hrlam-ws6he-aryy7-s5m43-6sm4a-4uqds-avudn-tyubn-7qe","before_points_e8s":21853243135392,"uplift_points_e8s":0,"after_points_e8s":21853243135392,"before_share_bps":955,"after_share_bps":954},
    {"principal":"hwnsp-qozsc-pmxto-c3m27-noe32-v45gq-lbo3z-ztkd3-l4e4x-st4bp-wqe","before_points_e8s":74543131721,"uplift_points_e8s":0,"after_points_e8s":74543131721,"before_share_bps":3,"after_share_bps":3},
    {"principal":"ivhvf-a2nxr-t436u-6t3g3-eckj2-gd6ky-q575b-37ciq-7r43a-62nav-wqe","before_points_e8s":320231445170,"uplift_points_e8s":0,"after_points_e8s":320231445170,"before_share_bps":13,"after_share_bps":13},
    {"principal":"khkrd-hwwv7-2m5k4-n6tyi-qqasb-yay5e-dpbtf-2abbf-bzji2-zrvey-dae","before_points_e8s":28750018543953,"uplift_points_e8s":0,"after_points_e8s":28750018543953,"before_share_bps":1256,"after_share_bps":1255},
    {"principal":"nqh27-eplay-bkspf-s4aho-uhvlu-mvwgx-gepgb-6jb3h-tm5qg-egucg-7qe","before_points_e8s":1412322047001,"uplift_points_e8s":0,"after_points_e8s":1412322047001,"before_share_bps":61,"after_share_bps":61},
    {"principal":"sorbk-tm3y3-uzhwj-d45hv-2qquh-tdlln-s3qna-etnov-sgx32-qsvci-kae","before_points_e8s":0,"uplift_points_e8s":0,"after_points_e8s":0,"before_share_bps":0,"after_share_bps":0},
    {"principal":"stzp3-bnvwm-zqzjh-o6mv6-ci53m-wj5k6-xyhe7-fnyp2-c64o3-7vokj-bqe","before_points_e8s":84,"uplift_points_e8s":0,"after_points_e8s":84,"before_share_bps":0,"after_share_bps":0},
    {"principal":"tjsut-lqpan-5xkiw-dq3j4-xpp53-uhhy2-dyam5-vtrcx-aadcg-bvxsx-6ae","before_points_e8s":12254325487089,"uplift_points_e8s":0,"after_points_e8s":12254325487089,"before_share_bps":535,"after_share_bps":535},
    {"principal":"ulapy-ysfkn-fobdy-nif4d-s5y22-s4wlq-bqkxm-4iznv-4r5qo-begk2-hae","before_points_e8s":826472309,"uplift_points_e8s":0,"after_points_e8s":826472309,"before_share_bps":0,"after_share_bps":0},
    {"principal":"zegjz-jpi6k-qkand-c2bgf-qw6za-xk4si-nz3gx-qzzia-fk6fg-snepb-tae","before_points_e8s":104055616307234,"uplift_points_e8s":214035423882,"after_points_e8s":104269651731116,"before_share_bps":4548,"after_share_bps":4554}
  ]
}
```

The live prefix contains no correction rows to reject and no future flat-4x
rows to classify. If a later prefix contains `CkStable3PoolUnmatchedTopUp`, the
script rejects the preview because the original unmatched rows cannot be
paired safely from the append-only ledger. If a later prefix contains
`CkStable3PoolFlat4x` without `--cutover-epoch`, it rejects the preview as
ambiguous; supplying the immutable cutover epoch lets it preserve those
already-recorded future points while applying the uplift only to unmatched
rows before that epoch. Unknown source variants and cursor gaps are hard
errors; they are never treated as zero.

This evidence is a current public read and an accounting preview. It is not
proof that a migration has run, that a policy is active, or that any user's
allocation share is preserved. The denominator increases from the uplift, so
shares for users without uplift dilute.
