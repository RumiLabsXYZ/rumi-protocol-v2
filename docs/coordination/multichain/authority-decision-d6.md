# D6 authority inventory and decision packet (BB-61 preparation)

**Status: PROPOSAL, UNADOPTED.** Prepared for the queued BB-61 / D6 decision (Rob with Miles). It does not make or complete D6,
is not a source gate, and authorizes no live action: no key, wallet, funding, signing, broadcast, canister call, role or
controller change. Every option stays unadopted until Rob's D6 decision. **LIVE HOLD** items are listed in section 7.

**Pins.** Frozen bridge branch `db24716f` (the Move package is unchanged since `23f75ef1`). XRPL: the existing separate checkout
`.worktrees/xrpl-icusd-bb55-20261031` at `46cf6b34` (`codex/xrpl-icusd-followups`), READ only. No XRPL source is copied here.
XRPL items are plain observations with `path:function` pointers, prefixed `XR@46cf6b34`.

**Labels.** FACT (has a citation), INFERENCE, PROPOSAL, UNKNOWN, UNVERIFIED, LIVE HOLD. Current function bodies override older
comments, and a mismatch is called out. This document names no live holder, principal or address. Source defaults and runbook
placeholders are not live state.

**Path shorthand.** `BI` foundry/src/BridgedIcUSD.sol. `EB` src/rumi_icusd_bridge/src. `SB` src/rumi_icusd_sol_bridge/src.
`UB` src/rumi_icusd_sui_bridge/src. `GM` move/rumi_icusd_gate/sources/rumi_icusd_gate.move. `RB` docs/coordination/multichain/
sui-publish-bind-runbook.md. `DB` foundry/DEPLOY-BASE.md. `PK` docs/coordination/base-icusd-bridge-sepolia-packet.md.

## 1. Authority table (FACT unless marked)

| Class | What the source shows | Can do | Cite |
| --- | --- | --- | --- |
| Ordinary test deployer or user wallet | EVM deployer is a throwaway with no token role; the script refuses a deployer that is admin, minter or guardian. Sui publisher receives MintCap, AdminCap, MetadataCap and the UpgradeCap. Solana has no human authority; a wallet only funds the canister-derived fee payer. XRPL holder wallets sign trust lines and payments (older-pin doc, UNVERIFIED at `46cf6b34`). | Pay gas, deploy or publish, make synthetic user transactions. Only the Sui publisher starts with real authority. | BI:48-49,157-170; DeployBridgedIcUSDEvm.s.sol:14-18; GM:218-223; SB bootstrap.rs:35-37; chain-xrpl.md:66 |
| Canister-controlled threshold issuance keys | EVM: minter and admin key per chain. Solana: fee-payer (also nonce authority), mint-authority, return-owner, mint-account (used once). Sui: treasury, gas, upgrade-custody (no signing path). XRPL: one issuer key per network. Derived from canister id, key name and path. Key names: `key_1` mainnets, `test_key_1` testnets. | Sign what the installed code asks for. A controller can replace that code (next rows). | EB signer.rs:3-9,63-109; EB chains.rs:136-139; SB ids.rs:77-109; UB ids.rs:39-42,117-143; UB flows/mint.rs:685; XR xrpl_icusd/types.rs:issuer_derivation_path |
| Ledger escrow and minting authority | The three bridge canisters keep escrow in their own subaccount and are not the ledger's minting account; they refuse to pay it. The fee recipient is an install argument with no runtime setter (none set means sweeps are refused). The Sepolia runbook made the operator the minting account of a synthetic ledger. The XRPL lane lives in the backend and checks that the ledger's minting account is the backend default account. | Move icUSD only through release, refund and sweep paths. XRPL borrow mints ICP icUSD into escrow through that minting account. | EB lib.rs:1-6; design 2026-10-24-base-icusd-bridge-design.md:109; EB flows/release.rs:74-83; SB flows/release.rs:148-153; UB flows/release.rs:154; EB state.rs:280; SB state.rs:300; UB runtime.rs:258; DB:73,82; XR xrpl_icusd/ledger.rs:check_minting_account |
| IC controllers and upgrades | Controllers set the admin list on all three canisters; on Sui also the gate binding (once), role-key derivation and gas coin. Upgrade arguments can replace admins. Admins pause, tune limits and fees, sweep fees, clear halts; module docs say they cannot sign or change ledger, network or keys. Controller sets are not in source. | Controller: choose admins and install new code. Admin: operate inside hard bounds. | EB admin.rs:1-6,31-38,257-265; EB lib.rs:62-75; SB admin.rs:112; SB lib.rs:148-175,508-531; UB admin.rs:1-6,157-165; UB lib.rs:216-233,620-643; XR main.rs:xrpl_require_controller |
| Destination admin, upgrade, freeze, issuer | EVM contract roles DEFAULT_ADMIN, MINTER, PAUSER, no proxy. Solana: mint authority, no freeze authority, no program. Sui: MintCap, AdminCap, UpgradeCap, MetadataCap, wrapped TreasuryCap. XRPL: issuer account, master key kept, optional clawback or NoFreeze flag. | See section 2. | BI:32-49; SB mint.rs:7-21; GM:1-20; move README.md:38-51; XR xrpl_icusd/settlement.rs:account_set_params |

## 2. Per family: who can do what, and what constrains it

### 2.1 EVM (contract `BI`, canister `EB`)
- **Create supply (FACT).** Only `mint` with MINTER_ROLE (BI:202), bounded by supplyCap, the window limit, one use per mintId and a uint64 amount (BI:204-223).
- **Change authority or limits (FACT).** DEFAULT_ADMIN_ROLE raises supplyCap, window limit and minBridgeOut, unpauses, and can grant or revoke non-admin roles including MINTER_ROLE, so the admin key is a full mint path (BI:35-44,283-323).
- **Pause (FACT, body vs comment).** PAUSER_ROLE (admin, minter, optional guardian) pauses mint and bridgeOut; only the admin unpauses (BI:45-47,278-297). A raised `minBridgeOut` also blocks small returns with no pause flag (BI:249,320-323).
- **Upgrade (FACT).** The contract has no proxy or upgrade code, so its rules are fixed at deploy. Admin transfer is the inherited two-step delayed process (BI:40-43).
- **Escrow (FACT).** The contract holds none. ICP escrow moves only by canister release, refund and fee-sweep paths.
- **Constraints that bind (FACT).**
  - Canister limits must stay inside the contract's (EB admin.rs:77-102).
  - The canister's admin lane encodes only seven actions: pause and unpause both, set cap, rate limit, min bridge-out. No role grant, no admin transfer (EB types.rs:211-219; evm/abi.rs:167-177).
  - Bind refuses unless the minter holds MINTER_ROLE, the admin does not, and `defaultAdmin()` equals the canister admin address (EB observer.rs:303-308).
  - `totalMinted` lets the canister notice a mint it did not make, after the fact (BI:83-87).
- **Bypass paths.** A canister controller upgrades the canister and signs anything with the admin or minter key. A raised minBridgeOut. Bind is a one-time observation, not a continuing guard.
- **UNKNOWN.** Any deployed address, live guardian, live admin delay, live controllers. **UNVERIFIED.** OpenZeppelin v5.0.2 role logic: pinned in foundry.lock, not in the checkout, so read only through the contract header.

### 2.2 Solana (`SB`)
- **Create supply (FACT).** The SPL mint authority is the canister's `mint-authority` key, in a two-signer transaction with the fee payer (SB ids.rs:77-90; tx/builders.rs:192). It is never set to none (ids.rs:83), and no `SetAuthority` encoder exists anywhere in the crate.
- **Change authority or limits (FACT).** Bind requires exactly that mint authority and no freeze authority, which then cannot be added later in either token program (SB mint.rs:7-12,259-264). Metadata and pointer authorities must be none or the canister (mint.rs:23-31). Limits, fees, pauses belong to bridge admins (SB admin.rs:41-112).
- **Pause (FACT).** Canister-side pauses and halts only. No on-chain pause: the extension allow-list excludes pausable and similar extensions (mint.rs:11-16).
- **Upgrade (FACT).** No Solana program, so no program upgrade authority (design D3). Canister upgrade by controllers (SB lib.rs:148-175).
- **Custody (FACT).** Returned tokens sit in the ATA owned by the `return-owner` key, with no delegate or close authority (mint.rs:18-21), until the canister signs a burn or refund (builders.rs:232,275). The accepted user-return shape is fixed (tx/shape.rs:1-22).
- **Bypass paths.** A controller upgrade can use all four role keys. The fee payer is also the nonce authority (nonce.rs:11-14), so replay safety rests on canister code. There is no on-chain consumed record (design D3, the C4 gap).
- **UNKNOWN.** Token program and SOL RPC canister id (install arguments, SB types.rs:28-51), live accounts, live controllers.

### 2.3 Sui (`GM`, `UB`)
- **Create supply (FACT).** `mint` needs `&MintCap`, is bounded by `supply_cap` (starts 0, so disabled) and a once-per-id table (GM:209,229-262). TreasuryCap is wrapped and no function releases it (GM:4-6; README.md:51). Bind needs the MintCap owned by the treasury role address (UB bind.rs:5-7).
- **Change authority or limits (FACT).** AdminCap `set_limits` raises `supply_cap` to the maximum and sets `min_bridge_out`; `migrate` re-enables calls after an upgrade (GM:342-364). The UpgradeCap holder upgrades under its policy, and a compatible upgrade can change any function body (README.md:40-46). MetadataCap edits metadata only (GM:15).
- **INFERENCE** (README.md:43-46, chain-sui.md:104). An additive upgrade can add a function that reads the private treasury field, so only dependency-only or immutable close that path.
- **Pause (FACT, body vs comment).** The header says returns are never gated (GM:18-19), but `bridge_out` checks `min_bridge_out` (GM:281) and `set_limits` has only a lower bound (GM:350-351), so an AdminCap holder can make returns impossible.
- **Upgrade guard (FACT).** After a package upgrade the version guard blocks every call, returns included, until AdminCap runs `migrate` (GM:278,360-370). A lost AdminCap strands that.
- **Upgrade policy (doc).** Compatible, additive, dependency-only, immutable; tighten only; immutable consumes the cap (chain-sui.md:104). The choice is D6's (RB:30).
- **Constraints that bind (FACT).** Bind records the AdminCap owner and never requires it to be the canister's (UB bind.rs:7-8). A first observation needs a pristine gate (bind.rs:99-102). Escrow moves by canister paths only.
- **Bypass paths.** The publisher holds every cap at publish (GM:218-223). The canister never signs with the upgrade-custody role (UB ids.rs:117-118; flows/mint.rs:685), so caps sent to that derived address are unusable unless a controller upgrades the canister to sign. This conflicts with RB:26-29 (caps to the custody address) and RB:43 (the AdminCap holder calls `set_limits` in a Sui transaction).
- **UNKNOWN.** Publisher, cap holders, upgrade policy, live controllers, the provider contract (open blocker).

### 2.4 XRPL (observations at `XR@46cf6b34`, read only)
- **Create supply.** The issuer account pays IOUs only through lane operations: import from canonical escrow (`main.rs:issue_xrpl_from_canonical`) and vault-backed borrow (`main.rs:borrow_to_xrpl`), which mints ICP icUSD into escrow through the backend's minting account (`xrpl_icusd/ledger.rs` header).
- **Defaults and caps.** The lane defaults to disabled with issuance paused and caps at 0. Caps may not exceed 15 significant digits (`types.rs:XrplIcusdConfig::default`; `mod.rs:apply_config_update`).
- **Change authority or limits.** Controller only: configure, pauses, minting-account verification, setup steps (`main.rs:xrpl_require_controller` and its callers). There is no admin tier below controller in this lane.
- **Flag powers.**
  - The policy (allow_clawback, no_freeze) has no default, cannot change once setup starts, and the two are mutually exclusive (`types.rs:IssuerPolicy`; `mod.rs:apply_config_update`).
  - Setup sets DefaultRipple, RequireDest and par TransferRate, then optional AllowClawback (needs an empty owner directory) or NoFreeze (`types.rs:IssuerSetupStep`; `settlement.rs:account_set_params`).
  - Irreversibility of both flags is XRPL ledger behavior, taken from step comments in `settlement.rs` and chain-xrpl.md:83, not re-proven here.
- **No other issuer controls.** The issuer builds only Payment and AccountSet. The backend source has no SetRegularKey, SignerListSet, disable-master or Clawback transaction. So the master key stays enabled in the canister, and a chosen clawback flag is a permission no code exercises.
- **Pause and returns (PARTLY VERIFIED).** `settlement_paused` stops issuer transaction starts and resubmits (`settlement.rs` pause checks). `issuance_paused` stops new issuance. The return module says a pause of new issuance never blocks returns (`returns.rs` header). Function bodies were only partly read.
- **Upgrade and escrow.** Backend upgrade by its controllers. Escrow moves only through ledger jobs persisted before their first await (`ledger.rs` header).
- **Bypass paths.** The same backend is the ICP icUSD minting account, holds XRP custody and settlement keys (`chains/xrp/ted25519.rs:custody_derivation_path`, `settlement_derivation_path`) and the issuer key. Its controllers reach all of it, so XRPL production custody is backend custody. The `xrpl-testnet-fixture` build pins XRPL to testnet and `test_key_1`, and a release build rejects it (`chains/xrp/network.rs` header; `Cargo.toml` feature).
- **UNKNOWN / UNVERIFIED.** Live backend controllers and ledger minting account; wallet behavior; whether a separate fixture instance is planned; full bodies of observer, retention and settlement.

## 3. What threshold signing does and does not remove (FACT / INFERENCE)

A threshold key hides key bytes, not trust. The management canister signs for the canister id and path that ask (EB signer.rs:63-109),
and a canister controller can install different code under the same id. That code can then request signatures from every role key
above (EVM admin and minter, Solana four roles, Sui treasury and gas, XRPL issuer). So controller trust remains in every family even
with no human key. What narrows it: a small named controller set, upgrades reviewed against a module hash read back after install
(PK section 1 does this for the Base run), and `test_key_1` limited to testnets. A governance-controlled or timelocked controller is a
platform capability that nothing in this branch implements or tests: UNVERIFIED.

## 4. Two separate choices (all PROPOSAL)

### 4.1 Disposable testnet demo custody (recommended)
Principles:
- (a) Synthetic ledger and new dedicated canisters only. No real canonical icUSD and none of the production canisters (D9; PK:17-22 pattern).
- (b) Every protocol-issuing authority is a canister-derived threshold key.
- (c) Dedicated, new, disposable human wallets hold no issuing authority, with the one named Sui exception below.
- (d) One named controller identity per new test canister, as in the Base Sepolia run (PK:13-31; DB:23).
- (e) Backing stays canonical and locked: destination supply never exceeds escrowed synthetic icUSD (EB admin.rs:77-102 caps; Sui `supply_cap` raised only as far as the demo needs).

| Family | Proposed arrangement | Why, and source limit |
| --- | --- | --- |
| EVM | Throwaway deployer with no role. Admin and minter are the canister's derived admin and minter addresses (bind requires it). Guardian none. Admin delay one day, small caps. | EB observer.rs:303-308; DB:105. A named-owner admin is unavailable (4.2). |
| Solana | No human authority. The canister creates the mint and return account with its own keys. The dedicated wallet only funds the fee payer with test SOL. | SB bootstrap.rs:1-4,35-37; mint.rs:7-21. |
| Sui | A dedicated disposable publisher wallet publishes, then moves MintCap to the canister treasury address once derived. It keeps AdminCap, UpgradeCap and MetadataCap for the rehearsal, with small `supply_cap` and a compatible policy. This is the one exception to "no operator key": that wallet can change code or raise the cap, but cannot mint without MintCap unless it upgrades. | The canister cannot sign for the custody role (UB ids.rs:117-118), so sending AdminCap there strands `set_limits` (RB:26-29,43). |
| XRPL | Separate fixture instance on testnet with `test_key_1`. Issuer reserve and fees come from one dedicated test wallet, holders use another. Policy `allow_clawback = false`, `no_freeze = false` (both irreversible, no code path uses them). | XR network.rs header; types.rs:IssuerPolicy. A separate instance is a decision the source does not show. |

### 4.2 Future production custody (options only; none implemented or adopted)
- **Sui caps to a named owner or multisig.** Source supports transferring AdminCap, UpgradeCap, MetadataCap and accepts any AdminCap owner (UB bind.rs:7-8; RB:26-29 "or as D6 decides"). Multisig use is a Sui platform capability not exercised here: UNVERIFIED.
- **Sui upgrade policy.** Dependency-only or immutable close the code path after review. Immutable is irreversible and ends fixes (README.md:40-46).
- **EVM contract admin to a named owner.** UNAVAILABLE now. Bind requires `defaultAdmin()` to equal the canister admin address and the canister has no admin-transfer encoder (EB observer.rs:307-308; evm/abi.rs:167-177). Needs new reviewed canister code.
- **Solana mint authority to a named owner.** UNAVAILABLE now. It is never set to none and no authority-change builder exists (SB ids.rs:83). Needs new code.
- **Controllers.** A multi-principal or governance-controlled controller set is a platform option, not implemented here (section 3).
- **XRPL.** No regular key, signer list or blackhole path in source, and custody equals backend custody (2.4). A production choice waits on the backend controller model and the issuer policy decision.

## 5. Decision form (placeholders only; Rob or Miles fill in later)

Every row is per network. No row means "all wallets" or "all networks". Do not fill from source defaults.

| Item | Value |
| --- | --- |
| Network and genesis allowlist (one line per network) | `<network id>` / `<chain id or genesis>` |
| Dedicated wallets (new, throwaway, not any existing or quarantined key): EVM deployer, EVM test user, Solana funding, Solana test user, Sui publisher and cap holder, Sui test user, XRPL funding, XRPL test user | `<address>` per role per network |
| Controller principal(s) per new test canister | `<principal>` |
| Canister admin principal(s) | `<principal>` |
| Sui: AdminCap, UpgradeCap, MetadataCap holders; upgrade policy | `<holder>`; `<compatible / additive / dependency-only / immutable>` |
| EVM: admin delay, guardian, caps | `<seconds>`; `<none / address>`; `<e8s>` |
| XRPL: allow_clawback, no_freeze | `<true/false>`, `<true/false>` |
| Permitted actions, each separately per network: create canisters, install, derive keys, fund, publish, transfer a cap, set limits, test round trip, pause or unpause, upgrade, stop or delete | `<yes / no>` per action |
| Per-network limits: test gas, ICP cycles, synthetic supply cap | `<amount>` each |
| Executor and end date | `<name>`, `<date>` |

**Boundary.** A standing test-wallet grant covers only the wallets named here, on the networks named here, for the actions ticked
here, within the limits here. It does not select production custody (4.2). It does not authorize any role or cap transfer,
controller change, policy flag or upgrade policy, funding beyond the limits, or any unlisted network. This reuses the existing
boundaries: scope and out-of-scope lists with one executor (PK:13-31), a separate authorization per network
(DeployBridgedIcUSDEvm.s.sol:9-12), and the quarantined Sui bootstrap key staying unused and unfunded (RB:9-10).

## 6. Next execution order after a real authorization (planning only)
1. **Inventory and bind exact identities.** Record the filled form and confirm each address, principal and network id against it. Stop on any mismatch.
2. **Verify existing authority state read-only**, inside the approved network scope: controllers, admins, role holders, cap owners, module hashes. Compare with the form. Do not infer from source defaults.
3. **Prepare a reviewable action packet.** Exact targets, artifacts and hashes, per-action limits, order, and what each step must read back. A reviewer signs off before step 4.
4. **Execute only separately authorized test actions**, one at a time, with same-attempt reconciliation: before repeating anything, read existing targets and durable receipts (PK:10-11). Upgrade only, never reinstall (DB:80).
5. **Read back actual roles, settings and settlement.** Roles, cap owners, limits, pauses, supply versus escrow. Record what was seen, not what was intended.

This does not reopen the BB-27 prerequisites and forces no production choice.

## 7. Unknowns, live holds and source limitations
- **UNKNOWN (never inferred here):** every live controller set, canister id, cap holder, contract admin, token program choice, key name in use, module hash, and whether any instance exists.
- **LIVE HOLD:** all destination profiles stay disabled; no activation, funding, signing or broadcast; BB-27 browser acceptance; the XRPL denied push; the quarantined Sui key stays unused; provider contracts and public RPC probes.
- **Source limitations that make an option unavailable:** EVM admin transfer has no canister path; Solana mint authority has no change path; the Sui upgrade-custody role cannot sign; XRPL has no regular key or signer list.
- **UNVERIFIED:** OpenZeppelin v5.0.2 role logic; additive-upgrade access to private Move fields (INFERENCE); Sui multisig and governance-controller use; XRPL wallet behavior, flag irreversibility beyond source comments, and pause semantics beyond the named checks.
- **Assumptions:** `db24716f` is the intended source of truth; `46cf6b34` is the accepted XRPL follow-up; the Base Sepolia packet is a pattern, not a grant.
