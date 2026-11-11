# OISY owner mismatch — corrective source and acceptance evidence

November 11, 2026. Ellis owns the active case/source; Dorian owns the sole frontend release.

## Scope and truthful status

Final candidate code `cb1701ee39118d9ac8d76c746dac3cb7dc23cce4` on `codex/oisy-owner-reconnect-20261111`, based on the actually published `d54ac7b7` tree. Twelve frontend files differ: the disconnect lifecycle, its tests, coded swap errors, and an explicit read-only account diagnostic. Backend, Candid, declarations, dependency manifests/locks and accepted ckSOL source are unchanged. The old production build remains frozen; this candidate has not been production-built or published.

**The incident is unresolved in production.** The earlier source passed its tests and its bytes were published/independently verified, but owner acceptance failed after disconnect, hard refresh and reconnect. Do not describe that publication receipt as a working swap. Rob recalls approving multiple things; whether they were account permissions or ledger allowances, the actual rejected stage and any prior swap result remain unverified. No new retry is requested during diagnosis.

## Confirmed defect and actual-evidence boundary

Rumi's `auth.disconnect()` previously cleared its direct signer and local connection markers but omitted `getPnpInstance().disconnect()` for OISY. Installed `@windoge98/plug-n-play` **0.1.0-beta.26** stores `oisy_principal`; its actual OISY adapter tries `connectWithStoredPrincipal()` before `connectWithAccounts()`. A reported Rumi disconnect followed by page reload could therefore leave that cached owner authoritative without another accounts RPC.

The published SDK **`@icp-sdk/signer` 5.3.1** decodes `{result:{accounts:[{owner:string,subaccount?:base64}]}}` and normalizes `owner` through its own `Principal.fromText()`. The account shape was checked against the installed decoder and published OISY account responder. No shape/normalization defect has been confirmed. The failure UI previously merged a local owner-list rejection and remote exact502; its screenshot alone cannot identify which branch ran.

Read-only browser observation confirmed the new deployed failure text and connected Rumi account. The separate main OISY tab showed an expired session; that is **not proof of the closed signing popup's identity**. Historical failed-request RPC payloads were not captured. The confirmed cache defect is reproduced integration evidence, not a captured trace of Rob's actual mismatch.

## Minimal correction and safety

- On explicit OISY disconnect, retire the actual PNP session. Do not use the legacy wrapper that also disconnects Plug. Preserve unrelated wallets and saved recovery journals.
- Verify that `oisy_principal` is actually absent before reporting a clean disconnect. The installed connector silently swallows storage-removal failures; a resolved promise alone is insufficient. An unavailable or retained cache yields a truthful failure, not a claimed retirement.
- Fence asynchronous auth cleanup and the actual `walletStore.disconnect()` entry point. Capture the generation after auth begins; refuse old actor/cache/state/error cleanup after a replacement session, including the lazy cache import and the following await.
- Preserve the expected owner, provider/session checks, exact approval cap/spender/input/minimum output, unknown transport outcomes and no-retry behavior. Never select a different owner during signing or inspection.
- Show distinct `OISY_LOCAL_OWNER_MISMATCH` and `OISY_REMOTE_SENDER_REJECTED` codes. The particular exact502-rejected call is pre-execution; earlier allowance approvals remain possible. Messages do not prescribe repeating the failed refresh.
- Beside these coded errors, **Check signing account (read only)** opens an action-owned popup synchronously from its click and issues only `icrc27_accounts`. It displays connected and reported owners/match without changing the account or making an approval/swap call. Duplicate reads, session/provider changes, unmount and declined access are guarded. A matching read does not verify earlier approvals or swaps.

## Deterministic verification

**91 tests passed across seven focused suites**:

| Suite | Scope |
| --- | --- |
| `auth.session.spec.ts` | Installed real OISY adapter cache/connect/disconnect; actual walletStore → auth → real PNP manager integration; realistic storage persistence and new-manager reload; fresh accounts retrieval; account/provider/actor-provider retirement; failed/cancelled retirement and protected journal preservation |
| `wallet.session.spec.ts` | Actual user-facing disconnect wrapper, including replacement while auth retirement is pending and at cache cleanup |
| `swapOisySigner.spec.ts` | Real SDK with synthetic transport; published account reply shape including subaccount, account mismatch/empty list, synchronous unique popup, cancellation/session protection and accounts-only inspection |
| `OisyAccountCheck.spec.ts` | Mounted actual component, explicit click/no automatic request, no owner adoption, duplicate/stale reads, provider switch and declined permission |
| 3pool binding/deposit policy and router suites | Existing expected-owner/session and exact financial arguments, distinct local/remote errors at both stages, unknown outcomes/no retry, unaffected non-OISY paths |

Key reproduction evidence retained locally:

- Running the **original deployed auth** against the installed OISY adapter's real lifecycle produced `cachedPrincipalSurvived:true`, `accountsRpcCount:0`, `returnedLiveOwner:false` after logout/new-adapter reload/reconnect. It failed the regression before the correction.
- The corrected full UI/auth/real-PNP-manager flow retires cache/account/provider/actor-provider, retains an opaque saved vault request, creates a fresh manager to simulate reload and queries the new actual adapter's account transport once. Only transport replies are synthetic; no wallet or network call is made.
- First focused review found outer UI cleanup and swallowed storage failure. Both were independently reproduced red, fixed narrowly and added to the final passing suite. A generation-only auth test was insufficient; the wrapper and real connector are now exercised.
- Global typecheck: **30 pre-existing errors/66 warnings in25 files**, no new diagnostic. An interim test-only nullability error was corrected. No clean whole-project typecheck claim.
- `git diff --check` passes. No dependency install, Candid generation or backend build occurred. The Svelte formatter plugin failed on its existing visitor API; actual Svelte compilation/typecheck and mounted tests pass, and no formatting dependency was changed.

## Rendered diagnostic QA

An isolated localhost Vite fixture renders the actual account-check component with synthetic account responses and stores. No real wallet requests occur. Desktop and **390×844** interaction checks passed matching owner, different owner, declined account access and clearing a result on session change. No relevant console errors or framework overlay; mobile scroll width equals viewport390, and full principals wrap. The fixture counter remained **zero approvals/swaps**. Preview and temporary browser tab were closed; Rob's swap/OISY tabs were not changed, refreshed or submitted.

## Release handoff

Fresh independent source acceptance must complete before Dorian's single corrective production build. Reuse the established additive publisher and preservation checks; do not replay old batches687/688 or rebuild an unchanged candidate repeatedly. Bind the next exact frontend packet to the currently compatible backend: independent public read confirms module `730f54713dca1938c40b661ef6bd6ddc23555d92ca4c8e1feca282e3a33f8cf9`, installed separately under Rob's Vault51 authority; Dorian reports the canonical API unchanged. The held reserve/financial scope is not part of this frontend repair.

After authorized publication, independently read back the exact new version, diagnostic/swap/auth files and full asset preservation before saying the correction is ready for an owner check. **Actual authenticated OISY account matching and a successful owner swap remain unverified.** The read-only diagnostic can collect that account evidence without requiring another financial submission. No signing, swap, allowance change, account switch, storage clear or financial replay has been performed on Rob's behalf.

## Fresh independent acceptance

Both fresh focused reviewers returned **PASS** for exact code `80ae6ecc71abaf2bc2cb394ef7159c84b7c4de1c`, after inspecting all eleven files, installed PNP lifecycle behavior, the original red observation, 91-test acceptance and baseline typecheck. No confirmed blockers remain for bounded source handoff. They explicitly retain the synthetic-transport/live-account/build/publication proof boundaries above. The next work is Dorian's single source-matched corrective frontend build, exact additive artifact/publisher checks and authorized publication/readback; do not repeat the source review or broaden the test campaign without a concrete new defect.

## Final fresh-page reachability correction

Before any production build, the parent wiring was found to hide account inspection until a coded swap error existed. That would require a fresh-page user to try another financial action before reaching the safe diagnostic. Exact final code `cb1701ee39118d9ac8d76c746dac3cb7dc23cce4` moves the existing child outside the error conditional; the child still renders only for connected OISY and performs no request on mount.

The actual mounted `SwapInterface` test failed red on the previous placement, then passed: fresh connected OISY page exposes the check without any quote or swap failure; no inspection/execution on mount, explicit click performs one inspection and zero route execution, and switching to Internet Identity hides it. The final targeted **18 tests/3 suites pass**, with unchanged **30 errors/66 warnings**; prior91 core-test evidence is reused because other runtime source is unchanged. Both independent focused delta confirmations PASS at the final pin. Dorian held the build, so no production build was wasted or repeated.

This adds only parent placement plus its regression to the accepted80ae6ecc source. All lifecycle, real installed-PNP integration, exact money/owner/session checks and earlier-allowance uncertainty remain unchanged. The first owner check can now be read-only on a fresh published page; no financial retry is required to inspect identities.
