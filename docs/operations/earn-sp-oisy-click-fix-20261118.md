# Earn Stability Pool OISY deposit click fix

**Source fix and focused verification only. Not deployed; no client signing or financial action.**

The client reports `Signer window should not be opened outside of click handler` with no OISY popup on an ordinary icUSD SP deposit. Reconnecting opened OISY for login but the next deposit failed identically. There is no verified safe screenshare workaround; do not ask for another reconnect or retry and do not infer deposit success from this error.

## Confirmed cause

The actual live app maps `/stability-pool` to node `56.pZ0JwGvw.js`, which imports pool component `D57r2Dem.js`. Its SHA256 is `b1d4f23d369072c2c1268070101fbd5f6fb8db12bb522dfda00c6277044d0ac2`. Both the ordinary deposit and saved-deposit branches call the service without entering OISY's channel. Conversion already opens the channel directly in its click handler. [Served evidence](evidence/earn-sp-oisy-click-20261118/served-evidence.json) retains the exact handler excerpt and hashes.

The unchanged service enters a cross-tab Web Lock and awaits actor acquisition/intent status. By its first signed call, the SDK's strict click-event guard is false. The installed `@icp-sdk/signer/web` transport sets that guard during capture and clears it during bubble; `establishChannel` rejects before `window.open` outside that event. Agent prewarming alone does not open this channel, explaining why reconnect is insufficient. The local OISY integration skill's older batch guidance is superseded here by the actual v5 source: sequential consent screens are retained, with no batch/execute rewrite.

## Narrow fix

Only `DepositInterface.svelte` changes at runtime:

- After local validation and captured wallet/action context, invoke the existing `prepareStabilityPoolOisySignerChannel` synchronously from the fresh Deposit click, then await it before entering the service.
- Do the same for Resume, freezing the existing saved ledger and raw amount before the await and passing the captured context unchanged. The existing service retains/reconciles its exact sequence.

No service, allowance, fee, amount, account, intent, recovery, conversion or withdrawal implementation changes. Existing service guards revalidate wallet/session after the channel await. Duplicate clicks stay blocked while opening. A channel failure is caught without entering the deposit service. Non-OISY wallets use the existing no-op helper path.

## Focused proof

- Initial regression: four failures and one pass before the runtime patch.
- Final: **27 tests pass** in two suites: six rendered component tests and 21 existing wallet-bound deposit/recovery tests.
- Component coverage: synchronous channel entry before service dispatch, exact screenshot amount `388.2514567` icUSD = `38825145670` raw, unchanged saved ledger/amount/context, rejected opening, duplicate click, invalid balance, and the actual installed SDK click guard.
- The SDK check deliberately mocks `window.open` to return null. Reaching that mock proves the guard passed inside the DOM click; it then stops before heartbeat, wallet signing or any transaction.
- Existing lifecycle tests cover wallet/session changes, sequential approval/deposit, retained exact sequences, cross-tab exclusion and lost-response recovery.
- The modified Svelte component compiled in the focused run. Two pre-existing dropdown accessibility warnings remain. No broad build, hosted CI or live client deposit was run.

[Verification and source hashes](evidence/earn-sp-oisy-click-20261118/verification.json), [before log](evidence/earn-sp-oisy-click-20261118/before.log), [final log](evidence/earn-sp-oisy-click-20261118/final.log).

## Release handoff and holds

The source parent is the existing recovery evidence branch. Its pre-fix component blob equals current fetched canonical main (`0026071326637be97ca635cfbce3f847b20a90f5`). Compose only this fix commit's source/test delta into the chosen frontend release; do not merge the entire historical evidence branch by accident.

Dorian remains release owner. This fix needs a source-matched frontend artifact and exact publication scope before a separate production approval; the unchanged frozen `b278d7da` artifact cannot deliver these new source bytes. Keep that packet/artifact/pins intact and explicitly revise the chosen release if combining changes. Do not silently claim the existing baseline-only packet includes the deposit fix. No rebuild or publication has been started by Ellis.

All backend, SP liquidation, reserve, Vault 202, bot/worker and financial holds remain intact. No agents initiate client signing, retry a transaction or move funds. After an authorized release, independently verify served handler bytes and click-time channel entry; actual client deposit completion is a separate unverified outcome.
