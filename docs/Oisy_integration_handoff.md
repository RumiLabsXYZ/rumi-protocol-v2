Oisy_integration_handoff

Last updated: February 6, 2026
Repo: /Users/robertripley/coding/rumi-protocol-v2
Primary goal: Make Rumi vault operations work for Oisy wallet users by avoiding ICP-ledger ICRC-2 approve/transfer_from flows that Oisy can’t execute.

--------------------------------------------------------------------------------

Context and problem statement

• Oisy uses ICRC-25/27 signer standards and (in practice) does not support the ICP ledger ICRC-2 calls used by the existing “backend pulls collateral” flow (icrc2_allowance / icrc2_approve / transfer_from).
• Any vault operation that requires the user to approve the protocol to pull funds from their wallet will fail on the ICP ledger with Oisy.
• The implemented workaround is a “push-deposit” flow: the user transfers ICP directly to a backend-controlled deposit account, then the backend credits only the new balance delta and proceeds.

Key idea to keep straight:
• “ICP collateral in” (user -> protocol) needs push-style deposit for Oisy.
• “Funds out” (protocol -> user) works fine with Oisy because the backend can transfer directly to the user without requiring approvals.

--------------------------------------------------------------------------------

Deployed references (mainnet)

• Backend canister: tfesu-vyaaa-aaaap-qrd7a-cai
• Vault frontend canister: tcfua-yaaaa-aaaap-qrd7q-cai
• Vault frontend URL: https://tcfua-yaaaa-aaaap-qrd7q-cai.icp0.io
• Custom domain: https://rumiprotocol.io
• ICP ledger: ryjl3-tyaaa-aaaaa-aaaba-cai
• icUSD ledger: see CONFIG.currentIcusdLedgerId (frontend config)

--------------------------------------------------------------------------------

Current Oisy support status

Works with Oisy today (no ICRC-2 needed because protocol sends assets to the user):
• Create vault via push-deposit: openVaultWithPushDeposit() / backend open_vault_with_deposit
• Borrow icUSD: borrowFromVault()
• Withdraw collateral: withdrawCollateral()
• Close vault: closeVault()
• Withdraw + close: withdrawCollateralAndCloseVault()

Still missing an Oisy-compatible “user -> protocol” path (needs push-style variants):
• Add margin: addMarginToVault() (ICP deposit from user)
• Repay debt: repayToVault() (icUSD payment from user)

--------------------------------------------------------------------------------

Architecture: push-deposit for ICP (vault creation)

High-level sequence (Oisy vault create)
1) Frontend queries backend for the caller’s deposit Account:
   - owner = backend canister principal (ic_cdk::id())
   - subaccount = deterministic hash derived from caller principal
2) Frontend executes icrc1_transfer on the ICP ledger to that Account (push).
3) Frontend calls backend open_vault_with_deposit(borrow_amount).
4) Backend reads the deposit account ICP balance, computes “new deposit” = current_balance - previously_credited, updates credited, then opens/updates the vault using only the newly credited amount.

Backend state to prevent double-crediting
• credited_icp_e8s: BTreeMap<Principal, u64>
  - Tracks how much balance has already been credited for each user’s deposit account.
  - Prevents opening multiple vaults or retrying the flow from re-crediting the same deposit.

Backend implementation anchor points (Rust)
• state.rs
  - credited_icp_e8s map
• management.rs
  - compute_deposit_subaccount(caller: Principal) -> [u8; 32] (SHA-256 over b"rumi-deposit" || caller.as_slice())
  - get_icp_deposit_account() -> Account (query)
  - icp_balance_of(account: Account) -> Result<u64, ProtocolError>
• vault.rs
  - open_vault_with_deposit(borrow_amount: u64) -> Result<..., ProtocolError> (update)

Candid surface updates
• rumi_protocol_backend.did
  - get_icp_deposit_account (query)
  - open_vault_with_deposit (update)

--------------------------------------------------------------------------------

Frontend wallet detection and routing (important)

Do not branch on a variable named “walletType” unless it is sourced from localStorage correctly.

The app stores the last wallet type in:
• localStorage key: rumi_last_wallet
• Expected value when connected with Oisy: "oisy"

Correct detection pattern:
• const walletType = localStorage.getItem('rumi_last_wallet');
• if (walletType === 'oisy') { ... }

This matters because openVault must route to the push-deposit flow when Oisy is connected, otherwise it will try the ICP-ledger ICRC-2 path and fail.

Files to check (SvelteKit)
• src/vault_frontend/src/lib/services/protocol/apiClient.ts
  - openVault routing (Oisy -> push deposit; others -> existing ICRC-2 path)
• src/vault_frontend/src/lib/services/protocol/walletOperations.ts
  - isOisyWallet()
  - supportsIcrc2CanisterCalls()
  - supportsVaultOperations()
  - approveIcpTransfer / checkIcpAllowance (keep blocked for Oisy)
  - approveIcusdTransfer / checkIcusdAllowance (under test, see below)

--------------------------------------------------------------------------------

Known Oisy-related blockers and bugs

1) Query calls triggering signer popup (“Signer window could not be opened”)
• Symptom: When connected with Oisy, read-only operations (example: loading vault list) can trigger a signer popup and fail, preventing data from loading.
• Typical error: “Signer window could not be opened”
• Likely root: something in the “read path” is accidentally going through a signing flow (or a signer window is being requested for a query).
• Action: audit the code path for vault-list loading and any helper that might call into signer / consent methods on read.

2) Vault creation initial borrow / sequencing bug (historical)
• There was a bug report around initial borrow behavior during vault creation.
• Ensure open_vault_with_deposit + borrow_amount sequencing matches what the backend expects and that the UI does not prematurely assume vault state before the backend confirms.

3) Oisy query signer / signer popup bug (historical)
• Separate write-up exists describing Oisy popping signer for queries.
• Treat as a UX + functionality blocker because it can stop vault screens from loading at all.

4) ICRC-28 trusted origins (often overlooked)
• Oisy and other signer flows can depend on trusted origins config (ICRC-28).
• Verify the relevant domains/origins are included where required (especially custom domain vs canister URL).

--------------------------------------------------------------------------------

ICRC-2 support on the icUSD ledger: test branch + decision tree

Why this test exists
• Vault creation failed on the ICP ledger with “Unsupported Canister Call: icrc2_allowance”.
• The frontend then preemptively blocked all ICRC-2 actions for Oisy, including icUSD actions (repay/borrow/add margin).
• Hypothesis: Oisy may support ICRC-2 on the icUSD ledger even if it fails on the ICP ledger.

Test branch details
• Branch: test/oisy-icrc2-repayment
• Commit: 91a2347
• Deployed: yes (frontend on mainnet)
• URLs: https://tcfua-yaaaa-aaaap-qrd7q-cai.icp0.io and https://rumiprotocol.io

What the test changes
• Keeps ICP-ledger ICRC-2 blocks in place (known fail).
• Removes/avoids the Oisy pre-block only for icUSD ICRC-2 calls:
  - walletOperations.checkIcusdAllowance()
  - walletOperations.approveIcusdTransfer()
• Adds detailed console logging prefixed with: [TEST-ICRC2]
• Bypasses the Oisy guard in apiClient.repayToVault() only for the test.

How to run the test
1) Open the live frontend.
2) Open browser console and filter by: [TEST-ICRC2]
3) Connect with Oisy.
4) Go to a vault with debt and attempt a repay.
5) Capture the full [TEST-ICRC2] log sequence.

Expected outcomes and what to do next
Outcome A: Repayment works
• Meaning: Oisy supports ICRC-2 on the icUSD ledger; original failure is ICP-ledger-specific.
• Next step: remove Oisy blocks for icUSD operations permanently; keep ICP blocks (vault creation stays push-deposit).

Outcome B: “Unsupported Canister Call” on icUSD ledger
• Meaning: Oisy can’t do icrc2_allowance / icrc2_approve anywhere (or still blocked by signer rules).
• Next step: implement push-style repayment for Oisy (user transfers icUSD to protocol, backend credits delta, then repays).

Outcome C: ICRC-21 consent message error
• Meaning: icUSD ledger may need different consent message setup for signer compatibility.
• Next step: review ICRC-21 configuration and ledger implementation; redeploy ledger if needed.

Outcome D: Other error
• Next step: document exact error, then decide whether it is:
  - frontend signer-window behavior
  - consent/trusted-origin config
  - ledger implementation mismatch
  - Oisy-specific limitation that requires a push-style design anyway

--------------------------------------------------------------------------------

⚠️ KEY INSIGHT: ICRC-21 consent messages may be the root cause (February 6, 2026)

Research turned up that other ICP developers are hitting the same ICRC-2
approve failures with Oisy. The pattern is:

1. Oisy enforces ICRC-21 consent messages before signing any transaction.
2. Before Oisy will let a user approve an icrc2_approve call, it calls
   icrc21_canister_call_consent_message on the TARGET CANISTER (the ledger
   being called, not our backend).
3. If the target canister doesn't implement ICRC-21, or returns an
   unexpected response, Oisy refuses to sign.

This means the failure may not be "Oisy can't do ICRC-2" — it may be
"Oisy can't do ICRC-2 on canisters that don't implement ICRC-21."

Canisters to check:
• ICP ledger (ryjl3-tyaaa-aaaaa-aaaba-cai) — does it implement
  icrc21_canister_call_consent_message? This is a DFINITY system canister,
  so we can't change it. If it doesn't support ICRC-21, Oisy ICP ICRC-2
  will NEVER work and push-deposit is the permanent solution.
• icUSD ledger (t6bor-paaaa-aaaap-qrd5q-cai) — does OUR ledger implement
  ICRC-21? If not, we could add it and potentially unblock Oisy for all
  icUSD operations (repay, add margin via icUSD).

Action items:
1. Query both ledgers for icrc21_canister_call_consent_message support
   (try calling it and see what happens).
2. Check if the ICP ledger's .did file includes ICRC-21 methods.
3. If our icUSD ledger doesn't support ICRC-21, investigate adding it —
   this could be the difference between needing push-deposit for everything
   vs only needing it for ICP.
4. Forum reference: forum.dfinity.org/t/oisy-wallet-icrc2-approve-failing-
   because-of-something-related-to-icrc21-canister-call-consent-message/51621

--------------------------------------------------------------------------------

Recommended implementation path for the remaining missing features (if needed)

If icUSD ICRC-2 works with Oisy (Outcome A)
• Keep repayment and add-margin using existing approve/transfer_from on icUSD (only).
• Maintain push-deposit for ICP collateral operations.
• Make guards operation-specific:
  - “ICP deposit requires push-deposit with Oisy”
  - “icUSD operations supported” (if verified)

If icUSD ICRC-2 does NOT work with Oisy (Outcome B/C/D)
• Implement push-style variants for:
  - repay_to_vault_with_deposit (icUSD push)
  - add_margin_with_deposit (ICP push, similar to vault create)
• Pattern: “read balance at deposit account, credit delta, apply operation”
• Ensure idempotency via credited maps per asset (credited_icp_e8s, credited_icusd_e8s or similar).

--------------------------------------------------------------------------------

Quick file map

Frontend
• src/vault_frontend/src/lib/services/protocol/walletOperations.ts
  - Oisy detection + ICRC-2 allow/approve methods + guards
• src/vault_frontend/src/lib/services/protocol/apiClient.ts
  - openVault routing, repayToVault, addMarginToVault, borrow, close, withdraw
• src/vault_frontend/src/lib/services/auth.ts
  - wallet connection logic (Oisy / Plug / II)
• src/vault_frontend/src/lib/config.ts
  - ledger + canister IDs (CONFIG.currentIcusdLedgerId is important)

Backend
• src/rumi_protocol_backend/src/state.rs
  - credited maps
• src/rumi_protocol_backend/src/management.rs
  - deposit subaccount derivation + balance queries
• src/rumi_protocol_backend/src/vault.rs
  - open_vault_with_deposit and (future) push-style repay/add-margin
• src/rumi_protocol_backend/rumi_protocol_backend.did
  - candid additions for deposit query + push operations

--------------------------------------------------------------------------------

Notes for whoever picks this up next

• The Oisy ICP path is solved via push-deposit for vault creation. The remaining work is making “user -> protocol” flows (repay/add margin) work, either by confirming icUSD ICRC-2 works (best case) or implementing push-style variants.
• Re-check Oisy wallet detection. It must branch on localStorage('rumi_last_wallet') == 'oisy' or the routing will silently fail.
• Treat the “signer popup on query” issue as a priority UX blocker if it prevents vault data from loading. It can make the app feel broken even if deposits/borrows work.
