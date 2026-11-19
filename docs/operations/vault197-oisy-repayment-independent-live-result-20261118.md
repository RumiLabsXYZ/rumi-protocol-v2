# Vault 197 repayment independent live verification

**The approved repayment fix is live and retry-ready.** The customer can refresh `app.rumiprotocol.com`, use the same wallet and open Vault 197 to retry the intended repayment. Real OISY consent and successful customer repayment remain unverified; no agent connected a wallet, signed or submitted a repayment.

Dorian alone published approved packet `8151703f257c1336dddb3ac9aca2cc0c496ef1e4` (document SHA256 `f5f071758d3ebb6e31f174cdec84867895bced9af12ea5b26dc34e567651fa18`). Ellis independently verified the actual staging commit before activation, then the activated stored/HTTP bytes and primary-domain public UI. The explicit human approval was verified in Merritt's chat; no source/build change or additional review campaign occurred during execution.

| Milestone | Observed result |
| --- | --- |
| Stage batch 707 | Commit reply at November 18, 11:05:23 PM Pacific |
| Independent stage proof | PASS at 11:08:21 PM: 1,355 asset keys, 356 stored encodings, 271 HTTP keys; old index/version preserved |
| Activation batch 708 | Commit reply at 11:10:15 PM Pacific |
| Independent final proof | PASS at 11:12:02 PM: 1,355 asset keys, 363 stored encodings, 273 HTTP keys |
| Primary-domain version | `1795064489138` |
| Public vaults UI | Desktop 1440 px and mobile 390 px render without horizontal overflow |

The served API/owned-signer module `Bv_H2GCB.js` has SHA256 `26fddf5c54e2107835e27f0a4bbeea8e6fb36208fa195576f50b74ce352f254b`; the served repayment-card module `CFyzoWdq.js` has SHA256 `b7be781b580f385c738f1e538e5550e8938e85111045a7895f0a93d3a7b84973`. Both were actually loaded/fetched by the primary-domain browser and match the approved manifest. The compiled handler opens and awaits the owned signer before bound repayment; approval and repayment use its shared agent, with cancellation/session guards and unknown-outcome behavior preserved.

Frontend/backend modules, management settings, controllers, permissions, all historical asset keys/properties and untouched encodings passed preservation checks. Backend remains `4409bfb7`, certified Candid stays pinned, and the observed Stability Pool hold remains true. The deployed Stability Pool deposit and recovery-menu/queue code is composed in this artifact. No backend/reserve/bot/worker/Vault 202/claim action or liquidation resumption was performed.

- [Independent live receipt](evidence/vault197-oisy-repayment-live-20261118/independent-live-receipt.json)
- [Actual stage proof](evidence/vault197-oisy-repayment-live-20261118/verified-stage.json), SHA256 `4dc58f3df525c98f413553be8f72da35506097514d81894525dcb0efa27e7369`
- [Actual final proof](evidence/vault197-oisy-repayment-live-20261118/verified-final.json), SHA256 `c81c089be6c52ca0199bdb7f1f683ec481dd1080a465edd76b8ef5255c242da7`
- [Actual desktop browser observation](evidence/vault197-oisy-repayment-live-20261118/browser-desktop.json) and [mobile observation](evidence/vault197-oisy-repayment-live-20261118/browser-mobile.json)
- [Desktop screenshot](evidence/vault197-oisy-repayment-live-20261118/vaults-desktop.png) and [mobile screenshot](evidence/vault197-oisy-repayment-live-20261118/vaults-mobile.png)

The isolated browser stayed disconnected, so authenticated vault-card interaction and real OISY consent are covered by earlier focused source tests, not claimed as live transactions. The known deferred native-XRP alias HTTP400 background lookups remain outside this repair. Merritt received the retry-ready verdict promptly after final verification and remains the single customer-case coordinator. Dorian owns the separate operator execution receipt.
