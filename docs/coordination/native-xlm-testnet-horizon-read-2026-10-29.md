# Native XLM: public Stellar testnet Horizon read

Read-only probe on 2026-10-29 UTC against the [official public testnet Horizon endpoint](https://developers.stellar.org/docs/data/apis/horizon/api-reference/list-all-ledgers). `curl` used normal TLS verification. No account was created, no transaction was submitted, and no funds moved.

| Request | Observed result |
| --- | --- |
| `GET https://horizon-testnet.stellar.org/` | `network_passphrase = "Test SDF Network ; September 2015"`; `core_latest_ledger = history_latest_ledger = 5455379`; Horizon version `30.0.0-b3ff4119cc99ebcc8048334f5ec64e78c16d1666`. |
| `GET /ledgers?order=desc&limit=1` | Ledger `5455381`, closed `2026-10-29T19:02:25Z`, `base_fee_in_stroops = 100`, `base_reserve_in_stroops = 5000000` at that instant. |
| `GET /transactions/{64 zero hex}` | HTTP `404`, content type `application/problem+json`; body `type = https://stellar.org/horizon-errors/not_found`, `title = Resource Missing`, `status = 404`. |

The 404 shape supports the existing parser's expected problem-body form on this one testnet host. The reserve and ledger values are time-specific observations, not configuration constants. This is one public provider's read; it does not prove independent-provider agreement, current mainnet behavior, deposit credit, payout retry safety, signing, broadcast, settlement, or canister runtime behavior.
