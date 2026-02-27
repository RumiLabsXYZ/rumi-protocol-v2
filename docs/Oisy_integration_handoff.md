Oisy Integration Handoff

Last updated: February 27, 2026
Repo: /Users/robertripley/coding/rumi-protocol-v2
Branch: feature/oisy-integration

Primary goal: Make all Rumi vault operations work for Oisy wallet users
via ICRC-112 batched signing (single signer popup per user action).

Status: ✅ WORKING — Oisy can create vaults, borrow, and add margin.

--------------------------------------------------------------------------------

Solution: ICRC-112 Sequential Batching

Oisy uses the ICRC-25/27 signer standards via `@slide-computer/signer-agent`
(bundled in `@windoge98/plug-n-play`). The key discovery was that Oisy's
SignerAgent supports ICRC-112 batch_call_canister, which lets us batch
multiple canister calls into a single signer popup.

How it works:
1. The SignerAgent exposes batch()/execute()/clear() methods.
2. Calling batch() switches the agent to manual queuing mode.
3. Each subsequent canister call is queued (not sent immediately).
4. Calling batch() again starts a new sequence within the same batch.
5. Calling execute() fires all queued calls as a single ICRC-112 request.
6. The signer presents ONE popup for the entire batch.

The ICRC-112 protocol uses `requests: [][]` where:
- Inner arrays = parallel calls (execute simultaneously)
- Outer arrays = sequential steps (each completes before the next starts)

Our pattern: `batch() → approve → batch() → vault_operation → execute()`
creates `requests: [[approve], [vault_op]]` — approve runs first, then the
vault operation, all in one popup.

Why ICRC-2 approve works despite ICP ledger lacking ICRC-21:
ICRC-112 uses a three-tier consent model per call in the batch:
- Tier 1: Signer natively handles known ICRC methods (icrc2_approve, icrc1_transfer)
- Tier 2: ICRC-21 consent message from the target canister
- Tier 3: Blind request warning
The ICP ledger's icrc2_approve is handled at Tier 1 (the signer knows the
method natively), so no ICRC-21 support is needed on the ledger.

This supersedes the earlier "push-deposit" approach and the ICRC-2/ICRC-21
compatibility investigation. The push-deposit endpoints still exist in the
backend but are no longer called from the frontend.

--------------------------------------------------------------------------------

Deployed references (mainnet)

- Backend canister: tfesu-vyaaa-aaaap-qrd7a-cai
- Vault frontend canister: tcfua-yaaaa-aaaap-qrd7q-cai
- Vault frontend URL: https://tcfua-yaaaa-aaaap-qrd7q-cai.icp0.io
- Custom domain: https://rumiprotocol.io
- ICP ledger: ryjl3-tyaaa-aaaaa-aaaba-cai

--------------------------------------------------------------------------------

Current Oisy support status

✅ Working with Oisy via ICRC-112 batching:
- Create vault + borrow (compound): openVaultAndBorrow() / backend open_vault_and_borrow
- Create vault (without borrow): openVault() / backend open_vault
- Add margin: addMarginToVault() / backend add_margin_to_vault
- Borrow icUSD: borrowFromVault() / backend borrow_from_vault
- Withdraw collateral: withdrawCollateral()
- Close vault: closeVault()
- Withdraw + close: withdrawCollateralAndCloseVault()

Each "user → protocol" operation that needs an approve is batched:
  approve + canister_call → single Oisy popup

--------------------------------------------------------------------------------

Architecture: ICRC-112 batched signing

Vault Creation + Borrow (borrow page — single popup)
1. Frontend detects Oisy via isOisyWallet() and gets SignerAgent via pnp.getSignerAgent()
2. signerAgent.batch()  → queue icrc2_approve on ICP ledger
3. signerAgent.batch()  → queue open_vault_and_borrow on backend
4. signerAgent.execute() → single Oisy popup, user approves once
5. Both calls execute sequentially: approve completes, then compound vault+borrow runs

Add Margin (vault details — single popup)
1. signerAgent.batch()  → queue icrc2_approve on ICP ledger
2. signerAgent.batch()  → queue add_margin_to_vault on backend
3. signerAgent.execute() → single popup

Non-batched operations (borrow, repay, withdraw, close)
These are single canister calls that don't need an approve step,
so they go through PNP's normal actor routing. Oisy handles
these as individual signer popups (one popup per action).

Compound backend method: open_vault_and_borrow
To avoid multiple signer popups when creating a vault AND borrowing
(which are separate canister calls), we added a compound backend endpoint:
  open_vault_and_borrow(collateral_amount, borrow_amount, collateral_type)
This does transfer_from + create vault + borrow in a single canister call,
so the ICRC-112 batch only needs two items (approve + compound call).

--------------------------------------------------------------------------------

Key implementation files

Frontend:
- src/vault_frontend/src/lib/services/pnp.ts
  - getSignerAgent(): gets SignerAgent from PNP's BaseSignerAdapter
- src/vault_frontend/src/lib/services/protocol/apiClient.ts
  - openVault(): ICRC-112 batch path for approve + open_vault
  - openVaultAndBorrow(): ICRC-112 batch for approve + open_vault_and_borrow
  - addMarginToVault(): ICRC-112 batch for approve + add_margin_to_vault
- src/vault_frontend/src/lib/services/protocol/walletOperations.ts
  - isOisyWallet(): detects Oisy wallet connection
- src/vault_frontend/src/routes/+page.svelte
  - createVault() uses openVaultAndBorrow() for the compound flow

Backend:
- src/rumi_protocol_backend/src/vault.rs
  - open_vault_and_borrow(): compound method (transfer_from + vault + borrow)
  - open_vault(): standard vault creation via ICRC-2 transfer_from
  - add_margin_to_vault(): standard margin via ICRC-2 transfer_from
- src/rumi_protocol_backend/src/main.rs
  - Candid endpoint for open_vault_and_borrow
- src/rumi_protocol_backend/src/icrc21.rs
  - ICRC-21 consent messages for all methods (including open_vault_and_borrow)
  - ICRC-28 trusted origins for Oisy domain verification
- src/rumi_protocol_backend/rumi_protocol_backend.did
  - open_vault_and_borrow : (nat64, nat64, opt principal) -> (Result_5)

Dependencies:
- @windoge98/plug-n-play v0.1.0-beta.26 (bundles signer-agent 3.20.0)
- @slide-computer/signer-agent 3.20.0 (ICRC-112 batch support)

--------------------------------------------------------------------------------

Wallet detection

Oisy detection uses isOisyWallet() in walletOperations.ts which checks:
  localStorage.getItem('rumi_last_wallet') === 'oisy'

When Oisy is detected, apiClient methods get the SignerAgent and use
ICRC-112 batching. When the SignerAgent is null (non-Oisy wallets like
Plug or II), they fall back to the standard sequential ICRC-2 flow.

--------------------------------------------------------------------------------

ICRC-21 / ICRC-28 support (backend)

Our backend canister implements:
- ICRC-21 (consent messages): human-readable descriptions for every
  canister method, shown in the Oisy signer popup
- ICRC-28 (trusted origins): allows Oisy to verify our frontend domains
  (canister URL, raw URL, rumi.finance, rumiprotocol.io)
- ICRC-10 (supported standards): declares ICRC-21 and ICRC-28 support

These are required for Tier 2 consent in the ICRC-112 batch. Without them,
Oisy would show a "blind request" warning for our backend calls.

--------------------------------------------------------------------------------

Legacy: push-deposit endpoints (still in backend, unused)

The following backend endpoints were the previous Oisy workaround and
still exist but are no longer called from the frontend:
- open_vault_with_deposit(borrow_amount, collateral_type)
- add_margin_with_deposit(vault_id)
- get_deposit_account(collateral_type)

These used a "push-deposit" pattern where the user transferred ICP to a
backend-controlled subaccount, then the backend swept those funds.
This is fully superseded by ICRC-112 batching.

The credited_icp_e8s map in state.rs and sweep_deposit in management.rs
are also unused but preserved for now.

These can be removed in a future cleanup.

--------------------------------------------------------------------------------

Known issues (resolved)

1. "Signer window could not be opened" on second popup
   - Cause: browser blocks async popups not tied to user gesture
   - Fix: compound open_vault_and_borrow endpoint reduces the flow to
     one ICRC-112 batch (one popup) instead of separate calls
   - Status: RESOLVED

2. ICRC-2 approve failing on ICP ledger
   - Cause: was originally thought to be ICRC-21 related
   - Fix: ICRC-112 batch handles icrc2_approve at Tier 1 (natively)
   - Status: RESOLVED (approve works fine in ICRC-112 batch)

3. Query calls triggering signer popup
   - Status: not observed with current PNP beta version (0.1.0-beta.26)

--------------------------------------------------------------------------------

Remaining work

- Repay with stable tokens (ckUSDT/ckUSDC): untested with Oisy.
  These use a different ledger and may need ICRC-112 batching too.
- Redeem operations: untested with Oisy. May need batching if they
  involve approve + backend call.
- Remove legacy push-deposit code from backend (optional cleanup).
