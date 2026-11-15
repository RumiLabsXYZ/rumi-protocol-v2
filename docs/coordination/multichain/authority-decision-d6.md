# D6 authority inventory and decision packet (BB-61 preparation)

**Status: PROPOSAL, UNADOPTED.** This packet prepares the queued BB-61 / D6 decision (Rob with Miles). It does not make or complete D6, it is not a source gate, and it authorizes no live action: no key, wallet, funding, signing, broadcast, canister call, role change or controller change. Every option stays unadopted until Rob's D6 decision. **LIVE HOLD** items are listed in section 7.

**Pins.** Frozen bridge branch `db24716f` (the Move package is unchanged since `23f75ef1`). XRPL: the existing local checkout of branch `codex/xrpl-icusd-followups` at `46cf6b34`, read only. No XRPL source is copied here. XRPL items are plain observations with `path:function` pointers, prefixed `XR@46cf6b34`. The OpenZeppelin files in section 2.1 were read from the working tree (`foundry/lib` is not tracked in git).

**Labels.** FACT (has a citation), INFERENCE, PROPOSAL, UNKNOWN, UNVERIFIED, PARTLY VERIFIED (only some function bodies were read), UNAVAILABLE (the current source has no path for this), LIVE HOLD. Current function bodies override older comments. A mismatch is marked "body vs comment". This document names no live holder, principal or address. Source defaults and runbook placeholders are not live state. Nothing here claims that external platform behavior (Sui, XRPL, IC) was tested.

**Path shorthand.** `BI` foundry/src/BridgedIcUSD.sol. `OZ` foundry/lib/openzeppelin-contracts/contracts/access. `EB` src/rumi_icusd_bridge/src. `SB` src/rumi_icusd_sol_bridge/src. `UB` src/rumi_icusd_sui_bridge/src. `GM` move/rumi_icusd_gate/sources/rumi_icusd_gate.move. `MR` move/rumi_icusd_gate/README.md. `CS`, `CX` docs/coordination/multichain/chain-sui.md and chain-xrpl.md. `RB` docs/coordination/multichain/sui-publish-bind-runbook.md. `DS` docs/superpowers/specs/2026-10-30-multichain-icusd-groundwork-design.md. `DB` foundry/DEPLOY-BASE.md. `PK` docs/coordination/base-icusd-bridge-sepolia-packet.md. `XR` the XRPL observations at `46cf6b34` (section 2.4).

**Official documentation (read by Miles; cited as documentation, not as behavior tested here).** `SUI-UP` https://docs.sui.io/develop/publish-upgrade-packages/upgrade (old packages remain callable after an upgrade, `init` does not re-run, versioned migration is explicit). `SUI-PKG` https://github.com/MystenLabs/sui/blob/main/crates/sui-framework/docs/sui/package.md (`only_dep_upgrades` allows dependency-only upgrades, and `make_immutable` discards the cap). `IC-TRUST` https://docs.internetcomputer.org/guides/canister-management/trust-in-canisters/ (controllers are trusted with a canister's code and assets).

## 1. Authority table (FACT unless marked)

| Class | What the source shows | Can do | Cite |
| --- | --- | --- | --- |
| Ordinary test deployer or user wallet | The EVM deployer is a throwaway wallet with no token role, and the deploy script refuses a deployer that is admin, minter or guardian. The Sui publisher receives MintCap, AdminCap, MetadataCap and the UpgradeCap. On Solana the funding wallet gets no protocol mint role; it only sends test SOL to the canister-derived fee payer, and ordinary users still control their own tokens and can return them. XRPL holder wallets sign trust lines and payments (older-pin doc, UNVERIFIED at `46cf6b34`). | Pay gas, deploy or publish, and make synthetic user transactions. Only the Sui publisher starts with real authority. | BI:48-49,157-170; DeployBridgedIcUSDEvm.s.sol:14-18; GM:218-223; SB bootstrap.rs:35-37; CX:66 |
| Canister-controlled threshold issuance keys | EVM: a minter key and an admin key per chain. Solana: `fee-payer` (also the nonce authority), `mint-authority`, `return-owner`, and `mint-account` (used once). Sui: `treasury`, `gas`, and `upgrade-custody` (no signing path). XRPL: one issuer key per network. All are derived from the canister id, a key name and a path. Allowed key names are per-network allowlists with defaults, not the selected live config: EVM mainnets `key_1`, testnets `test_key_1`, Base Sepolia both, local `test_key_1` and `dfx_test_key`; Sui fixed per network (`key_1` mainnet, `test_key_1` testnet); Solana and XRPL name the key at install or by state default. The key a live instance uses is UNKNOWN. | Sign whatever the installed code asks for. A controller can replace that code (see the next rows). | EB signer.rs:3-9,63-109; EB chains.rs:136-139; SB ids.rs:77-109; SB types.rs:34-35; UB ids.rs:39-42,64-69,117-143; UB flows/mint.rs:685; XR xrpl_icusd/types.rs:issuer_derivation_path; XR chains/xrp/config.rs |
| Ledger escrow and minting authority | The three bridge canisters keep escrow in their own subaccount. They are not the ledger's minting account. They also refuse to release icUSD to the ledger's minting account, because a transfer there would burn it. The fee recipient is an install argument with no runtime setter; if none is set, sweeps are refused. The Sepolia runbook made the operator the minting account of a synthetic ledger. The XRPL lane lives in the backend and checks that the ledger's minting account is the backend default account. | Move icUSD only through release, refund and sweep paths. XRPL borrow mints ICP icUSD into escrow through that minting account. | EB lib.rs:1-6; design 2026-10-24-base-icusd-bridge-design.md:109; EB flows/release.rs:74-83; SB flows/release.rs:148-153; UB flows/release.rs:154; EB state.rs:280; SB state.rs:300; UB runtime.rs:258; DB:73,82; XR xrpl_icusd/ledger.rs:check_minting_account |
| IC controllers and upgrades | Controllers set the admin list on all three canisters. On the EVM bridge, controllers alone also set the RPC provider set (body vs comment, see 2.1). On Sui, controllers also set the gate binding (once), the role key derivation and the gas coin. Upgrade arguments can replace admins. Admins can pause, tune limits and fees, sweep fees and clear halts. Module docs say admins cannot sign or change the ledger, the network or keys. Controller sets are not in source. | Controller: choose admins and install new code. Admin: operate inside hard bounds. | EB admin.rs:1-6,31-38,257-265; EB lib.rs:62-75; SB admin.rs:112; SB lib.rs:148-175,508-531; UB admin.rs:1-6,157-165; UB lib.rs:216-233,620-643; EB admin.rs:232-243; XR main.rs:xrpl_require_controller |
| Destination admin, upgrade, freeze, issuer | EVM contract roles: DEFAULT_ADMIN_ROLE, MINTER_ROLE and PAUSER_ROLE. No proxy. Solana: mint authority, no freeze authority, no program. Sui: MintCap, AdminCap, UpgradeCap, MetadataCap, wrapped TreasuryCap. XRPL: issuer account, master key kept, optional clawback or NoFreeze flag. | See section 2. | BI:32-49; SB mint.rs:7-21; GM:1-20; MR:38-51; XR xrpl_icusd/settlement.rs:account_set_params |

## 2. Per family: who can do what, and what constrains it

### 2.1 EVM (contract `BI`, canister `EB`)
- **Create supply (FACT).** Only `mint`, which needs MINTER_ROLE (BI:202). It is bounded by supplyCap, the window limit, one use per mintId and a uint64 amount (BI:204-223).
- **Change authority or limits (FACT).** DEFAULT_ADMIN_ROLE can raise supplyCap, the window limit and minBridgeOut, and it can unpause (BI:283-323). It can also grant or revoke roles other than admin, including MINTER_ROLE, so the admin key is a full mint path (BI:35-44).
- **Inherited role logic (FACT, OpenZeppelin v5.0.2 local files; foundry.lock pins v5.0.2).**
  - A role's admin defaults to DEFAULT_ADMIN_ROLE, and `grantRole` and `revokeRole` need the role's admin (OZ/AccessControl.sol:39-44,106-108,122,137). BI never changes a role's admin (no `_setRoleAdmin` call).
  - Any holder can renounce its own role, for example the minter or a pauser (OZ/AccessControl.sol:157-163).
  - For DEFAULT_ADMIN_ROLE itself, `grantRole`, `revokeRole` and `_setRoleAdmin` revert (OZ/extensions/AccessControlDefaultAdminRules.sol:83-98,156-162).
  - Admin transfer is two steps: the admin calls `beginDefaultAdminTransfer`, and the new admin calls `acceptDefaultAdminTransfer` after the delay (same file:211-225,245-266). `cancelDefaultAdminTransfer` is admin only (:229-241).
  - The admin can renounce only after scheduling a transfer to the zero address and waiting out the delay (same file:113-122). Renouncing leaves no admin.
  - The submodule's Git metadata is broken, but the source is present. Per Miles' review both files match official commit `dbb6104c` (lengths 14810 and 7050, which I confirmed locally); I did not re-fetch or re-compare them.
- **Pause (FACT, body vs comment).** PAUSER_ROLE (admin, minter, optional guardian) pauses mint and bridgeOut. Only the admin unpauses (BI:45-47,278-297). A raised `minBridgeOut` also blocks small returns, with no pause flag (BI:249,320-323).
- **RPC provider set (FACT, body vs comment).** `admin_set_rpc_services` is controller-only. The function body calls `require_controller()` (EB admin.rs:238), the function's own doc comment says "Controller-only" (admin.rs:232-234), the endpoint is listed under controller access (EB lib.rs:364), and a unit test expects an admin to be refused (admin.rs:411-416). The module header says admins can "replace RPC providers" (admin.rs:3-4); that is stale, and the body governs. The provider set is a trust root: it decides what the canister believes about Base, so replacing it is as powerful as an upgrade (admin.rs:232-234). The new set must answer `eth_chainId` with the configured chain by quorum before it is stored (admin.rs:240-241), which does not make the providers honest. Whether the Solana and Sui bridge modules carry the same kind of stale header was not checked: UNVERIFIED.
- **Upgrade (FACT).** The contract has no proxy and no upgrade code, so its rules are fixed at deploy.
- **Escrow (FACT).** The contract holds none. ICP escrow moves only through canister release, refund and fee-sweep paths.
- **Constraints that bind (FACT).**
  - Canister limits must stay within the contract's limits (EB admin.rs:77-102).
  - The canister's admin lane encodes exactly seven actions: PauseMint, UnpauseMint, PauseBridgeOut, UnpauseBridgeOut, SetSupplyCap, SetMintRateLimit and SetMinBridgeOut. It has no role grant and no admin transfer (EB types.rs:211-219; evm/abi.rs:167-177).
  - Bind refuses unless the minter holds MINTER_ROLE, the admin does not hold it, and `defaultAdmin()` equals the canister admin address (EB observer.rs:303-308).
  - `totalMinted` lets the canister notice a mint it did not make, after the fact (BI:83-87).
- **Bypass paths.** A canister controller can upgrade the canister and sign anything with the admin or minter key. A raised minBridgeOut blocks small returns. Bind is a one-time observation, not a continuing guard.
- **UNKNOWN.** Any deployed address, live guardian, live admin delay or live controllers.

### 2.2 Solana (`SB`)
- **Create supply (FACT).** The SPL mint authority is the canister's `mint-authority` key. It signs a two-signer transaction together with the fee payer (SB ids.rs:77-90; tx/builders.rs:192). It is never set to none (ids.rs:83), and no `SetAuthority` encoder exists anywhere in the crate.
- **Change authority or limits (FACT).** Bind requires exactly that mint authority and no freeze authority. A freeze authority cannot be added later in either token program (SB mint.rs:7-12,259-264). Metadata and pointer authorities must be none or the canister (mint.rs:23-31). Bridge admins own limits, fees and pauses (SB admin.rs:41-112).
- **Pause (FACT).** Only canister-side pauses and halts exist. There is no on-chain pause: the extension allow-list excludes pausable and similar extensions (mint.rs:11-16).
- **Upgrade (FACT).** There is no Solana program, so there is no program upgrade authority (DS section 12, D3). Controllers upgrade the canister (SB lib.rs:148-175).
- **Custody (FACT).** Returned tokens sit in the associated token account (ATA) owned by the `return-owner` key. That account has no delegate or close authority (mint.rs:18-21). The tokens stay there until the canister signs a burn or refund (builders.rs:232,275). The accepted user-return shape is fixed (tx/shape.rs:1-22).
- **Bypass paths.** A controller upgrade can use all four role keys. The fee payer is also the nonce authority (nonce.rs:11-14), so replay safety depends on canister code. There is no on-chain consumed record. C4 is the design's cumulative-mint check (compare a chain's monotone mint counter with the bridge's own mint total); it is unavailable on Solana without a program, so only the point-in-time supply bound can catch an excess (DS section 2.3; docs/coordination/multichain/chain-solana.md:88,118).
- **UNKNOWN.** The token program and SOL RPC canister id (install arguments, SB types.rs:28-51), live accounts and live controllers.

### 2.3 Sui (`GM`, `UB`)
- **Create supply (FACT).** `mint` needs `&MintCap`. It is bounded by `supply_cap` (starts at 0, so minting is disabled) and by a once-per-id table (GM:209,229-262). TreasuryCap is wrapped, and no function releases it (GM:4-6; MR:51). Bind needs the MintCap owned by the treasury role address (UB bind.rs:5-7).
- **Change authority or limits (FACT).** AdminCap `set_limits` raises `supply_cap` to the maximum and sets `min_bridge_out`. `migrate` raises the Gate's stored version to the package's VERSION (GM:342-364). The UpgradeCap holder upgrades under its policy, and a compatible upgrade can change any function body (MR:40-46). MetadataCap edits metadata only (GM:15).
- **INFERENCE** (MR:43-46, CS:104). An additive upgrade can add a function that reads the private treasury field. Only a dependency-only or immutable policy closes that path.
- **Pause (FACT, body vs comment).** The header says returns are never gated (GM:18-19). But `bridge_out` checks `min_bridge_out` (GM:281), and `set_limits` has only a lower bound (GM:350-351). So an AdminCap holder can make returns impossible.
- **Version guard (FACT; platform behavior INFERENCE).**
  - `assert_version` requires `gate.version == VERSION` (GM:368-370; VERSION is 1 at GM:65). `migrate` requires `gate.version < VERSION` and then sets the stored version (GM:360-364).
  - An upgrade does not change the shared Gate's stored version; only `migrate` does. New code that bumps VERSION refuses a still-old Gate until an AdminCap holder migrates it.
  - Old package code keeps passing its own check until the Gate migrates, and then fails it. A compatible upgrade that keeps VERSION need not block anything. That old packages stay callable after an upgrade, that `init` does not re-run, and that versioned migration is explicit are stated in the official Sui upgrade documentation (SUI-UP, read by Miles); none of it was tested here.
  - So this is not an automatic global freeze, and the guard alone does not enforce safe upgrades (GM:11-14 says the same). A lost AdminCap leaves a Gate that cannot be migrated to bumped code. The comment at GM:357-359 ("every other call is blocked until it runs") is true only for bumped-VERSION code (body vs comment).
- **Upgrade policy (doc, not tested).** The options are compatible, additive, dependency-only and immutable. A policy can only be made stricter later, never looser. Immutable consumes the cap (CS:104). The official framework documentation says `only_dep_upgrades` allows dependency-only upgrades and `make_immutable` discards the cap (SUI-PKG, read by Miles; not tested here). The choice belongs to D6 (RB:30).
- **Constraints that bind (FACT).** Bind records the AdminCap owner. It does not require that owner to be the canister (UB bind.rs:7-8). The first observation needs a pristine gate: zero supply, zero mint and return counters, and empty mint and return tables (bind.rs:99-102). Escrow moves by canister paths only.
- **Bypass paths.** The publisher holds every cap at publish (GM:218-223). The canister never signs with the upgrade-custody role (UB ids.rs:117-118; flows/mint.rs:685). So caps sent to that derived address cannot be used unless a controller upgrades the canister to sign. This conflicts with the runbook at RB:26-29 (caps to the custody address) and RB:43 (the AdminCap holder calls `set_limits` in a Sui transaction).
- **UNKNOWN.** The publisher, cap holders, upgrade policy, live controllers, and the provider contract (an open blocker).

### 2.4 XRPL (observations at `XR@46cf6b34`, read only)
- **Create supply (FACT, XR).** The issuer account sends IOUs only through lane operations: import from canonical escrow (`XR main.rs:issue_xrpl_from_canonical`) and vault-backed borrow (`XR main.rs:borrow_to_xrpl`). Borrow mints ICP icUSD into escrow through the backend's minting account (`XR xrpl_icusd/ledger.rs` header).
- **Defaults and caps (FACT, XR).** The lane defaults to disabled, with issuance paused and caps at 0. Caps may not exceed 15 significant digits (`XR xrpl_icusd/types.rs:XrplIcusdConfig::default`; `XR xrpl_icusd/mod.rs:apply_config_update`).
- **Change authority or limits (FACT, XR).** Only controllers can configure the lane, set pauses, verify the minting account and run setup steps (`XR main.rs:xrpl_require_controller` and its callers). There is no admin tier below controller in this lane.
- **Flag powers (FACT, XR).**
  - The policy (allow_clawback, no_freeze) has no default. It cannot change once setup starts, and the two flags are mutually exclusive (`XR xrpl_icusd/types.rs:IssuerPolicy`; `XR xrpl_icusd/mod.rs:apply_config_update`).
  - Setup sets DefaultRipple, RequireDest and par TransferRate, then optional AllowClawback (needs an empty owner directory) or NoFreeze (`XR xrpl_icusd/types.rs:IssuerSetupStep`; `XR xrpl_icusd/settlement.rs:account_set_params`).
  - Irreversibility of both flags is XRPL ledger behavior, taken from step comments in `XR xrpl_icusd/settlement.rs` and CX:83. It is not re-proven here (UNVERIFIED).
- **No other issuer controls (FACT, XR).** The issuer builds only Payment and AccountSet (`XR xrpl_icusd/settlement.rs`, the built-transaction enum). The backend source has no SetRegularKey, SignerListSet, disable-master or Clawback transaction. So the master key stays enabled in the canister, and a chosen clawback flag is a permission that no code exercises.
- **Pause and returns (PARTLY VERIFIED, XR).** `settlement_paused` stops issuer transaction starts and resubmits (`XR xrpl_icusd/settlement.rs` pause checks). `issuance_paused` stops new issuance. The return module says a pause on new issuance never blocks returns (`XR xrpl_icusd/returns.rs` header). Function bodies were only partly read.
- **Upgrade and escrow (FACT, XR).** The backend is upgraded by its controllers. Escrow moves only through ledger jobs that are persisted before their first await (`XR xrpl_icusd/ledger.rs` header).
- **Bypass paths (FACT, XR).** The same backend is the ICP icUSD minting account. It also holds the XRP custody and settlement keys (`XR chains/xrp/ted25519.rs:custody_derivation_path`, `settlement_derivation_path`) and the issuer key. Its controllers can reach all of these, so XRPL production custody is backend custody. The `xrpl-testnet-fixture` build pins XRPL to testnet and `test_key_1`, and a release build rejects it (`XR chains/xrp/network.rs` header; `XR Cargo.toml` feature).
- **UNKNOWN / UNVERIFIED.** Live backend controllers and ledger minting account; wallet behavior; whether a separate fixture instance is planned; the full bodies of the observer, retention and settlement code.

## 3. What threshold signing does and does not remove (FACT / INFERENCE)

A threshold key hides key bytes, not trust. The management canister signs for the canister id and path that ask (EB signer.rs:63-109). A canister controller can install different code under the same id. That code can then request signatures from every role key above (EVM admin and minter, Solana's four roles, Sui treasury and gas, XRPL issuer). So controller trust remains in every family, even with no human key. What narrows it: a small named controller set; upgrades reviewed against a module hash read back after install (PK section 1 does this for the Base run); and `test_key_1` limited to testnets. The official Internet Computer documentation says controllers are trusted with a canister's code and assets (IC-TRUST, read by Miles; not tested here).

**Several controllers are not a multisig.** Listing multiple controller principals does not create a quorum or an approval step: each listed principal stays independently privileged and can install code on its own. A rule such as a policy, a delay or a required number of approvals binds only if the sole controller is an actual governance or multisig canister that enforces it. Nothing in this branch implements or tests such a controller: UNVERIFIED.

## 4. Two separate choices (all PROPOSAL)

### 4.1 Disposable testnet demo custody (recommended where the source allows)
Principles:
- (a) Synthetic ledger and new dedicated canisters only. No real canonical icUSD and none of the production canisters (D9; PK:17-22 pattern).
- (b) The settled demo's protocol-issuing authority is a canister-derived threshold key. Where an arrangement cannot meet this (Sui, below), the departure is named and needs an explicit, disclosed choice. It is never waived silently.
- (c) Dedicated test wallets hold no protocol-issuing role in the settled arrangement, except where a disclosed Sui choice below says otherwise.
- (d) One named controller identity per new test canister, as in the Base Sepolia run (PK:13-31; DB:23).
- (e) Backing stays canonical and locked. Destination supply never exceeds escrowed synthetic icUSD (EB admin.rs:77-102 caps; Sui `supply_cap` raised only as far as the demo needs).

| Family | Proposed arrangement | Why, and source limit |
| --- | --- | --- |
| EVM | A dedicated deployer with no role, and a role-free deploy call as in the Sepolia pattern. Admin and minter are the canister's derived admin and minter addresses (bind requires it). Guardian none. Admin delay one day, small caps (all pattern values, not selected). | EB observer.rs:303-308; DB:105. A named-owner admin is UNAVAILABLE (4.2). |
| Solana | No protocol-issuing human authority. The canister creates the mint and return account with its own keys. The funding wallet gets no mint role and only sends test SOL to the fee payer. Users keep control of their own tokens; admins and controllers keep the powers listed in sections 1 and 2.2. | SB bootstrap.rs:1-4,35-37; mint.rs:7-21. |
| XRPL | Separate fixture instance on testnet with `test_key_1`. Issuer reserve and fees come from one dedicated test wallet, and holders use another. Proposed policy `allow_clawback = false` and `no_freeze = false` (not adopted). With both false, setup skips the optional AllowClawback and NoFreeze AccountSet steps (XR xrpl_icusd/types.rs:350-361, `IssuerSetupStep::sequence_for`). The current code fixes the policy once setup starts (XR xrpl_icusd/mod.rs:196-200), and no Clawback transaction builder exists (XR xrpl_icusd/settlement.rs:76-77 builds only Payment and AccountSet). Setting either flag to true has the separately disclosed ledger consequences (2.4). | XR chains/xrp/network.rs header; XR xrpl_icusd/types.rs:IssuerPolicy. A separate instance is a decision the source does not show. |
| Sui | See the setup custody and the choices below. The threshold-only recommendation is met only by choice B or C. | GM:218-223; MR:40-46; UB ids.rs:117-118. |

**Sui, temporary setup custody (PROPOSAL, no live transfer proposed now).** The publisher holds MintCap, AdminCap, MetadataCap and the UpgradeCap from publish until D6 names holders (GM:218-223). This is a short window for a real authorization, and moving any cap is a separate, explicit scope (section 5). MintCap must end up at the canister's treasury address for bind (UB bind.rs:5-7).

**Sui, why moving MintCap does not settle it (FACT/INFERENCE).** The Gate's checks bind only the code published today (GM:11-14). A holder of a compatible UpgradeCap can publish code that reads the wrapped TreasuryCap and mints (MR:40-46; additive too, by INFERENCE). So whoever holds that cap has issuing authority even without MintCap.

**Sui, settled demo arrangement: choose one (all UNADOPTED, none decided here).**
- **A. Keep a compatible or additive UpgradeCap with a dedicated test wallet.** This is an extra operator-mint trust choice, disclosed in any demo claim. It does not meet principle (b) and is not the threshold-only recommendation. AdminCap also stays with a signing wallet, because the canister cannot sign for the custody role (UB ids.rs:117-118; RB:26-29,43).
- **B. A policy that stops changing the module's own bodies: dependency-only or immutable (MR:40-46; CS:104).** It is irreversible and ends fixes, so a Gate bug stays. Under B, supply still needs MintCap held by the canister's threshold key, but AdminCap still raises `supply_cap` and `min_bridge_out`, so AdminCap custody remains its own choice. No policy is chosen here.
- **C. A future canister custody signing path, separately authorized.** Today the canister never signs for the upgrade-custody role (UB ids.rs:117-118; flows/mint.rs:685). This needs new reviewed code, is not implemented, and is not a new source gate. It would let the threshold key hold the caps.

### 4.2 Future production custody (options only; none implemented or adopted)
- **Sui caps to a named owner or multisig.** The source supports transferring AdminCap, UpgradeCap and MetadataCap, and it accepts any AdminCap owner (UB bind.rs:7-8; RB:26-29 "or as D6 decides"). Multisig use is a Sui platform capability that is not exercised here: UNVERIFIED.
- **Sui upgrade policy.** Dependency-only or immutable close the code path after review. Immutable is irreversible and ends fixes (MR:40-46). See 4.1 choice B.
- **EVM contract admin to a named owner.** UNAVAILABLE now. The contract itself supports the delayed two-step transfer (section 2.1), but bind requires `defaultAdmin()` to equal the canister admin address, and the canister has no admin-transfer encoder (EB observer.rs:307-308; evm/abi.rs:167-177). This needs new reviewed canister code.
- **Solana mint authority to a named owner.** UNAVAILABLE now. It is never set to none, and no authority-change builder exists (SB ids.rs:83). This needs new code.
- **Controllers.** Multiple controller principals are not a multisig or quorum: each stays independently privileged. A policy is enforced only if the sole controller is an actual governance or multisig canister that enforces it, and that is not implemented here (section 3). The EVM RPC provider set is also controller-only (section 2.1), so it falls under the same choice.
- **XRPL.** No regular key, signer list or blackhole path exists in source. Custody equals backend custody (2.4). A production choice waits on the backend controller model and the issuer policy decision.

## 5. Decision form (placeholders only; Rob or Miles fill in later)

Every row is per network. No row means "all wallets" or "all networks". Do not fill from source defaults. Nothing here asks for wallet access now.

| Item | Value |
| --- | --- |
| Network and genesis allowlist (one line per network) | `<network id>` / `<chain id or genesis>` |
| Test wallets, per role per network: EVM deployer, EVM test user, Solana funding, Solana test user, Sui publisher, Sui test user, XRPL funding, XRPL test user. Each is EITHER a specifically identified EXISTING dedicated test wallet, used only under its own unchanged scope, OR a NEW dedicated wallet. Never a production, shared or quarantined key by default. | `<address>` and `<existing: scope reference / new>` |
| Controller principal(s) per new test canister | `<principal>` |
| Canister admin principal(s) | `<principal>` |
| Sui: AdminCap, UpgradeCap, MetadataCap holders; upgrade policy | `<holder>`; `<compatible / additive / dependency-only / immutable>` |
| EVM: admin delay, guardian, caps | `<seconds>`; `<none / address>`; `<e8s>` |
| XRPL: allow_clawback, no_freeze | `<true/false>`, `<true/false>` |
| Per-network limits: test gas, ICP cycles, synthetic supply cap | `<amount>` each |
| Executor and end date | `<name>`, `<date>` |

The Sui, EVM and XRPL value rows above record the proposed choice only. A value there takes effect only if the matching class C scope below is approved, and recording a value is not an approval.

**A. Ordinary test-wallet operations** (covered by a test-wallet grant for the named wallets, per network): fund a named address within the limits; deploy or publish from the named wallet; synthetic user transactions (deposit, return, round trip); read-only readbacks. Each: `<yes / no>`.

**B. Canister operations** (not included in class A authority; an explicit grant or action packet that names these scopes can cover them, alone or together with class C, and the tool and user approvals actually required still apply): create, install, upgrade, stop or delete a canister; derive role keys; pause or unpause. Each: `<yes / no>`.

**C. Role, cap, controller and policy actions** (proposed additional scopes, OFF by default; a scope is on only if an explicit grant or action packet names it, and class B and class C scopes can be pre-authorized together in one standing grant or one action packet): transfer a cap or role; set limits; change controllers or admins; choose the Sui upgrade policy (4.1 A, B or C); choose XRPL flags; choose the EVM admin delay and guardian. Each: `<approved / not approved>` with `<approval reference>`.

**Boundary.** Class A authority alone covers only the wallets named here, on the networks named here, within the limits here, and does not include class B or class C. An explicit grant that names class B or class C scopes can cover them, together in one standing grant or one action packet. That does not mean each action already named in such a grant needs a fresh prompt. The tool and user approvals actually required still apply, and no new approval layer is added. No grant selects production custody (4.2) or covers an unlisted network. The old Base Sepolia signer grant is not extended to any other network (PK:25-31). This reuses the existing boundaries: scope and out-of-scope lists with one executor (PK:13-31), a separate authorization per network (DeployBridgedIcUSDEvm.s.sol:9-12), and the quarantined Sui bootstrap key staying unused and unfunded (RB:9-10).

## 6. Next execution order after a real authorization (planning only)
1. **Inventory and bind exact identities.** Record the filled form and confirm each address, principal and network id against it. Stop on any mismatch.
2. **Verify existing authority state read-only**, inside the approved network scope: controllers, admins, role holders, cap owners, module hashes. Compare with the form. Do not infer from source defaults.
3. **Prepare a reviewable action packet.** List the exact targets, artifacts and hashes, per-action limits, the order, and what each step must read back. Use the approvals that are actually required (the tool and user approvals, and any approval the packet itself names). This packet adds no new sign-off layer.
4. **Execute only separately authorized test actions**, one at a time, with same-attempt reconciliation. Before repeating anything, read existing targets and durable receipts (PK:10-11). Upgrade only, never reinstall (DB:80).
5. **Read back actual roles, settings and settlement.** Check roles, cap owners, limits, pauses, and supply versus escrow. Record what was seen, not what was intended.

This does not reopen the BB-27 prerequisites and forces no production choice.

## 7. Unknowns, live holds and source limitations
- **UNKNOWN (never inferred here):** every live controller set, canister id, cap holder, contract admin, token program choice, key name in use, module hash, and whether any instance exists.
- **LIVE HOLD:** all destination profiles stay disabled. No activation, funding, signing or broadcast. BB-27 browser acceptance stays held. The XRPL denied push stays held. The quarantined Sui key stays unused. Provider contracts and public RPC probes stay held. No live cap transfer is proposed.
- **Source limitations that make an option unavailable:** EVM admin transfer has no canister path; Solana mint authority has no change path; the Sui upgrade-custody role cannot sign (so Sui choice C needs new code); XRPL has no regular key or signer list.
- **UNVERIFIED:** additive-upgrade access to private Move fields (INFERENCE); Sui package-version callability, upgrade-policy behavior and multisig use (the official Sui documents were read by Miles, not tested here); governance-controlled or multisig canister controllers; whether the Solana and Sui bridge module headers carry stale authority claims like the EVM one; XRPL wallet behavior, flag irreversibility beyond source comments, and pause semantics beyond the named checks. The OpenZeppelin comparison with the official commit is Miles' review, not re-run here.
- **Assumptions:** `db24716f` is the intended source of truth. `46cf6b34` is the accepted XRPL follow-up. The Base Sepolia packet is a pattern, not a grant.
