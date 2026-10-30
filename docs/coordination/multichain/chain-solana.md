# BB-35 Solana groundwork: escrow-backed icUSD on Solana

Card BB-35 (worker W18), 2026-10-30. Design on paper only: no code, build, test, RPC probe, canister call or wallet action was run. Nothing is deployed or enabled; the Solana profile stays `disabled` until an implementation exists.
Settled by the coordinator note (Miles under Rob's authority, planning only): **D3** = memo plus durable nonce for the first plan (a program guard is a stronger later option, not a research gate); **D9** = testnet and devnet demos use synthetic test ledgers, never real canonical icUSD.
Labels: **F** fact with a source key, **R** recommendation, **U** unverified or unmeasured. "Section N.M" refers to `docs/superpowers/specs/2026-10-30-multichain-icusd-groundwork-design.md` (the design). Repo paths are relative.

Sources (all read 2026-10-30; web pages came through a fetch summarizer, so a quoted line is its extraction of the page, to be pinned by a test at implementation):
S1 `raw.githubusercontent.com/solana-program/token/main/program/src/processor.rs` | S2 same repo `solana-program/token-2022/.../processor.rs` | S3 `solana-program/token/main/interface/src/instruction.rs` | S4 `solana-program/memo/main/interface/src/lib.rs` | S5 `docs.anza.xyz/implemented-proposals/durable-tx-nonces` | S6 `solana-foundation/solana-com/main/apps/docs/content/docs/en/rpc/http/{gettransaction,getsignaturesforaddress,getsignaturestatuses}.mdx` | S7 `raw.githubusercontent.com/dfinity/sol-rpc-canister/main/README.md` | S8 `forum.dfinity.org/t/sol-rpc-canister/41896` | S9 `ic-api.internetcomputer.org/api/v3/proposals/136985` and `/canisters/tghme-zyaaa-aaaar-qarca-cai` | S10 `docs.internetcomputer.org/references/protocol-canisters.md` and `/guides/chain-fusion/solana.md` | S11 `docs.anza.xyz/consensus/commitments` | S12 `docs.anza.xyz/implemented-proposals/rpc-transaction-history` | S13 `anza-xyz/solana-sdk/master/sdk-ids/src/lib.rs` | S14 `docs/coordination/multichain/sources-native.md` (BB-32; its section 1 carries the primary URLs for program ids, rent and wallets) | S15 (secondary, not primary) `www.alchemy.com/overviews/solana-archival-data`.

## 1. Representation

| Item | Choice | Basis |
| --- | --- | --- |
| Token program | **R: Token-2022**, a profile field `token_program` with exactly two allowed values (Token-2022, classic SPL). Every check below uses the profile value, including ATA derivation (the program id is a seed of the ATA address). Fallback: classic SPL plus a Metaplex metadata account (adds a second program and update authority; no encoder exists in the repo). | S14 section 1 |
| Why Token-2022 | In-mint metadata (`MetadataPointer` + `TokenMetadata`) needs no second metadata program. Cost: new encoders (extension init order, larger mint rent). The repo code is classic only (`chains/solana/tx.rs` `token_program_id`, `derive_ata`, unchecked `MintTo` tag 7), so both options need new code. | repo |
| Wallet caveat | **U:** Token-2022 and in-mint metadata display in Phantom, Solflare and Backpack is not stated by their doc landing pages (S14 open question 4). Decide at activation with a recorded wallet check; if it fails, switch the profile value to classic SPL. | S14 |
| Decimals | 8, fixed at `InitializeMint2`; `u64` base units; 1 base unit = 1e-8 icUSD = 1 e8s. Max 18,446,744,073,709,551,615 (section 3). | F S14 |
| Mint account | **R:** its keypair is a threshold-derived key (new role `mint-account`, used once) so no operator ever holds a mint secret. The canister creates and initializes the mint (rent and fee in SOL from the `fee-payer` key). | R |
| Mint authority | `mint-authority` threshold key (section 5.1 path). Never set to `None` (that is the irreversible `FixedSupply` state, S1). | F S1 |
| Freeze authority | **None at creation.** F: with a `None` freeze authority, `SetAuthority(FreezeAccount)` returns `MintCannotFreeze` in both programs, so it cannot be added later (S1, S2). Read from source, not run; confirm with a devnet negative test at implementation. | F S1 S2 |
| Metadata authority | Metadata-pointer authority and `TokenMetadata` update authority `None` after the one-time init (**U:** that `None` is settable for both is not confirmed; if not, the `mint-authority` key holds it). | U |

**Extension allow-list (R, stricter than a deny-list).** At bind time the mint's extension list must be a subset of `{MetadataPointer pointing at the mint itself, TokenMetadata}`; anything else fails the bind. The ones that must be named in review because each breaks 1:1 custody or return accounting (S14 section 1 rows 3 and 8; S2 authority list): `TransferFeeConfig` (received amount differs from sent), `ScaledUiAmount` and `InterestBearingConfig` (display amount lies), `ConfidentialTransferMint` and confidential mint/burn (hidden amounts bypass instruction inspection), `PermanentDelegate` (a third party can move or burn custody), `TransferHook` (arbitrary program runs inside transfers and adds accounts), `DefaultAccountState` (accounts born frozen), `NonTransferable`, `MintCloseAuthority`, `Pausable` and `PermissionedBurn` (a third party can halt or gate our burn), group pointers (unneeded).
**Bind checks (R, at `finalized`, by independent providers):** account owner = profile token program; mint authority = our key; freeze authority `None`; `decimals == 8`; supply 0 on first bind (pristine, like Base `bind_contract`, section 5); extension allow-list; the return ATA exists, is owned by the profile program, and its owner is the `return-owner` key. Claims move `none -> deployed_unbound -> bound` (section 8).

## 2. Exact transaction shapes

Program ids (F, S14 section 1 and S4/S13): Token `TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA`; Token-2022 `TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb`; ATA `ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL`; Memo v3 `MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr` (S4 also lists v1 `Memo1Uhk...` as "legacy" and v4 `Memo4c2p...`; **U:** which is deployed on mainnet-beta is not stated, so the profile pins one id confirmed at activation); ComputeBudget `ComputeBudget111111111111111111111111111111`; System = 32 zero bytes.
Instruction tags (F, S3, shared by both token programs): `TransferChecked` = 12 (4 accounts, data = tag, amount u64 LE, decimals u8, 10 bytes), `MintToChecked` = 14, `BurnChecked` = 15 (same data layout). Token-program tags 3 (`Transfer`), 7 (`MintTo`), 8 (`Burn`) take no decimals and are never used or accepted. S3 also lists a `Batch` tag 255 (**U:** deployed or not), so any other token-program tag is rejected, including 255.
Keys (R, section 5.1 path `[rumi-icusd-sol-bridge, v1, role, genesis_hash_32]`): `fee-payer` (pays fees, ATA rent, is the **nonce authority**), `mint-authority`, `return-owner` (owns the return ATA), plus one-time `mint-account`. Roles stay separate (least privilege), so mint and burn are two-signer transactions (two `sign_with_schnorr` calls, section 5). Legacy messages only for our own transactions. Memo kinds: `rumi1|m|<id>` mint, `rumi1|b|<id>` burn, `rumi1|f|<id>` refund (id = 64 lowercase hex), `rumi1|r|...` user return (2.2).

### 2.1 Mint (canister-built, ICP escrow already pulled)
Ordered instructions, one durable-nonce account per operation (section 4):
1. `AdvanceNonceAccount` (System tag 4): accounts [nonce writable, RecentBlockhashes sysvar read-only, nonce authority = `fee-payer` signer]. Must be first; the message `recent_blockhash` = the nonce value (F S5).
2. ATA `CreateIdempotent` (tag 1): [`fee-payer` writable signer funder, recipient ATA writable, recipient wallet read-only, mint, System program, profile token program]. The ATA address must equal the derivation from (wallet, token program, mint).
3. `MintToChecked(amount, 8)`: [mint writable, recipient ATA writable, `mint-authority` signer].
4. Memo `rumi1|m|<mint_id>` with zero account metas.
Fee payer = `account_keys[0]` = `fee-payer`; two required signers, signatures ordered to match the account keys (reuse `tx::order_signatures_by_signer`). No ComputeBudget instruction in v1 (base fee only; adding one is a reviewed profile change). Recipient validation before building: 32-byte base58, not the System Program key, the mint, any of our own role keys, or a token program id; amount in `[1, per_operation_cap]`.
`mint_id = sha256(domain "rumi.icusd.sol.mint.v1", then length-prefixed genesis hash, mint key, instance salt, the ICP escrow-pull block index, recipient key, amount)` (section 5.1). Re-mint of a held claim uses `...remint.v1`.

### 2.2 User return (user-built, observer-verified)
The holder signs ONE transaction with exactly this shape (any order, legacy or v0 **without** address-table lookups in v1):
- exactly one instruction to the profile token program, `TransferChecked(amount, 8)`, exactly 4 account metas: source token account, escrow mint, **destination = the canonical return ATA** (derived from `return-owner`, token program, mint), authority (single signer, no multisig tail);
- exactly one Memo instruction (profile Memo id, zero account metas) whose data matches the return grammar below;
- zero to two ComputeBudget instructions (wallets add them; they move nothing). Every other program is rejected (default deny; **U:** wallet-added guard instructions would be refused, probe wallets at activation).
**Memo grammar (R):** ASCII, at most 160 bytes, case-sensitive, exactly `rumi1|r|<principal>|<subaccount>|<nonce>` with five `|`-separated fields and no other bytes. `<principal>`: ICP textual principal that parses and re-encodes identically, not anonymous, not the management canister, not the bridge itself. `<subaccount>`: `-` for the default subaccount, else 64 lowercase hex that is not all zeros. `<nonce>`: u64 decimal, no leading zeros (the client nonce correlates the user's own intent; uniqueness comes from the signature). A later grammar version needs a new `rumiN` tag and a reviewed profile change.
**Association rule:** credit needs the memo and the transfer in the SAME transaction (both covered by the holder's signature). A memo in another transaction, the `memo` field of `getSignaturesForAddress`, or a balance delta alone is never credit evidence. Return id = `sha256("rumi.icusd.sol.return.v1", genesis hash, signature)`.
**Observer verification (from `getTransaction` base64 at `finalized`, `maxSupportedTransactionVersion: 0`; F S6, S7: the SOL RPC canister's typed `getTransaction` supports only base64/base58, never `jsonParsed`):** decode the message in the canister (bytes, not a provider parser). Accept only when ALL hold: `meta.err == null`; the shape above; mint = bridge mint and `decimals == 8`; destination = return ATA; the authority is a transaction signer AND equals the pre-balance `owner` of the source token account in `preTokenBalances` (delegates are rejected; profile `allowed_delegates` is empty; `owner` "may be omitted" per S6, and then the return is held, not credited); `innerInstructions` is empty; across all `pre/postTokenBalances` entries of this mint exactly two changed, source down by `amount` and destination up by `amount`; amount within `[min, per_operation_cap]`; return id not seen. Verifying instruction bytes AND balance deltas means a delta alone never credits.

| Observed | Observer result | What happens next |
| --- | --- | --- |
| Unchecked `Transfer` (tag 3), any other token-program tag, or tag 255 | Reject (stray) | Refund path 2.4 |
| Two or more transfers, or any transfer in an inner instruction | Reject (stray) | Refund path |
| Wrong mint (other token) | Ignored (cannot enter our ATA) | None |
| Wrong destination (another account) | Not observed | None (outside our account) |
| Missing, duplicate, malformed or non-return memo; memo in another transaction | Reject (stray, tokens landed) | Refund path; never credited |
| Net balance rise of the return ATA without the allowed instruction (hook, CPI, delegate) | Reject (stray) | Refund path or hold if sender not provable |
| Authority is not source owner, multisig, or `owner` missing | Reject | Hold for review; no guess |
| `meta.err != null` | No state change | Ignore |
| Valid | Credit: `O_d -> RC_d` once, at finalized | Section 3 |

### 2.3 Burn of custodied returns (canister-built)
1. `AdvanceNonceAccount` on a burn-lane nonce account (authority `fee-payer`).
2. `BurnChecked(amount, 8)`: [return ATA writable, mint writable, `return-owner` signer].
3. Memo `rumi1|b|<burn_id>`; `burn_id = sha256("rumi.icusd.sol.burn.v1" (new domain, to add to the profile), genesis hash, mint, instance salt, ordered list of covered return ids)`.
Signers: `fee-payer`, `return-owner`. v1 burns one return per transaction (simple 1:1 to `PR_d`); batching several returns under one id is a later optimization. The amount never exceeds the credited `RC_d` and never touches stray balances.

### 2.4 Refund of strays (canister-built, second use of the same machinery)
`[AdvanceNonce, Memo rumi1|f|<refund_id>, TransferChecked(return ATA -> verified sender, authority return-owner, 8)]`, plus a `CreateIdempotent` for the sender's ATA first only if the original source account no longer exists. "Verified sender" = the pre-balance `owner` of the single source account that fell by exactly the stray amount, and that owner signed the transaction. Anything else stays in custody as `stray_d` for manual review. **R:** strays below a profile `min_refund_e8s` are retained (never burned, they stay in `O_d`) to blunt dust spam. The always-present Memo also satisfies a recipient whose Token-2022 account demands a memo on incoming transfers (**U:** not read from a source).

## 3. Custodied return lifecycle (section 2.3, corrected)
```
mint landed:   PM_d --finalized--> O_d
return valid:  O_d  --finalized valid return (2.2)--> RC_d      (supply unchanged, tokens in return ATA)
burn signed:   RC_d --burn message persisted, then signed--> BU_d
burn final:    BU_d --finalized, err null, exact stored BurnChecked message, return ATA -amount--> PR_d
burn failed:   BU_d --proven not landed (nonce consumed by our cancel or by our err-landed burn)--> RC_d
PR_d --ICP escrow release saga (reuse)--> done
```
- **No ICP payout while a unit is in `RC_d` or `BU_d`.** Transition `BU_d -> PR_d` needs the burn transaction itself: finalized, `meta.err == null`, message bytes equal to the stored unsigned `BurnChecked` message (nonce, signer roles, mint, return account, amount, memo `burn_id`), and the return ATA down by exactly that amount in the same transaction. The mint's global supply difference is not used: unrelated owner burns (section 12) can change it. A different transaction consuming the burn nonce halts the lane (6.2 rule 4).
- **Custody read for reporting:** `custody_d` = token balance of the return ATA, `supply_d` = mint supply, both at `finalized`. Two reads are two heads, so a valid pair is `supply, custody, supply` with the two supply reads equal and no own mint or burn in flight; otherwise retry. A single `getMultipleAccounts` response has one context slot but is not in the SOL RPC method table (S7) and a raw `jsonRequest` keeps `context`, which may defeat provider consensus (**U**). Audit check: `custody_measured == RC_d + BU_d + stray_d`; an unexplained excess after the cursor reaches head halts credits and raises an alarm. Strays are reported separately and excluded from circulation while held (section 2.4 term `custody_d`).
- C1 and C2 as in section 2.3. C3 is not an unconditional equality on Solana: holders can burn (section 12). **C4 is unavailable** (6).

## 4. Idempotent mints and same-attempt reconciliation
**Per-operation durable nonce accounts (R).** A pool of N nonce accounts (profile `nonce_pool_size`, start 4; rent about 1,447,680 lamports each, repo `NONCE_ACCOUNT_RENT_LAMPORTS`, consistent with S14's rent constants). An account is reserved by one operation from message persistence until its attempt is proven consumed, then reusable (its stored value changed). Authority = `fee-payer`, so a cancel needs one signature. Creation: **R** use `CreateAccountWithSeed` with base = `fee-payer` so one signature suffices instead of the repo's two (**U:** instruction not read from a source here; hand-encode like `create_account_instruction`). Bootstrap needs one real blockhash: use the README route (`getSlot` then `getBlock`, S7) rather than the repo's operator-supplied override, later creations can themselves be nonce-led.
**Persist before signing (R, section 6.2 rule 1):** `{op id, mint_id, recipient, amount, nonce account, nonce value, exact unsigned message bytes, signer list}`. Then sign. Record EVERY signature received before any send (repo `tx::first_signature_base58`). A lost or ambiguous sign reply (`SYS_UNKNOWN`, `CANISTER_ERROR`; S14) means re-sign the same unsigned bytes. **U:** whether IC Ed25519 signing is deterministic (section 13), so key on message plus nonce, not one signature.
**Facts that shape the loop (F):** a durable-nonce transaction is recognized by `AdvanceNonceAccount` first and its `recent_blockhash` must equal the stored nonce; it stays usable until the stored nonce advances (S5, inferred there, not stated as a time limit). A durable-nonce transaction that FAILS in execution still advances the nonce and charges the fee (S5). So any transaction bound to a nonce value excludes every other transaction on that value: at most one lands.
**Reconcile each tick (read the nonce account at `finalized`; reuse `sol_rpc::parse_nonce_account_blockhash`; also check owner = System program and authority = `fee-payer` each read):**
1. Stored value == recorded: no attempt landed (as of finalized). The signed bytes may still land, so rebroadcast the SAME bytes (all send errors, including preflight "already processed", are maybe-sent). Never a new nonce.
2. **Cancel (R):** after `cancel_after` without landing, persist and sign an advance-only message on the same nonce (recorded as another attempt). When the nonce advances, check all attempts. Only if the cancel landed with `err == null` is the mint proven not delivered; then re-issue the same `mint_id` on a fresh nonce value.
3. Stored value advanced: look up each recorded signature (`getTransaction` base64, finalized). Landed with `err == null` and message bytes equal to the recorded bytes: Delivered. Landed with `err != null`: not delivered (fee lost, nonce consumed); re-sign allowed on the new value. None found: `getSignaturesForAddress(nonce account, before = ...)` returns the few transactions that wrote this account, newest first (**R:** a bounded search key, not global history; confirm in a test); fetch the newest, read its memo. A memo is discovery only, never proof (coordinator correction, section 12): the consuming transaction is adopted only if it is finalized, executed successfully, and its message bytes equal the stored unsigned message (same nonce account and value, signer roles, mint, recipient account and amount), the same checks as the recorded-signature path. Anything different or unverifiable is held and halts the lane, with no replacement mint and no refund. A different transaction consumed our nonce: **halt the lane**, re-sign nothing.
4. Lookups return null or `Inconsistent`: keep waiting. **Absence never proves non-delivery once the nonce has advanced.**
**A lane (or the destination) halts when:** a different transaction consumed a recorded nonce; the nonce advanced and no consumer is found within `history_horizon` (below); nonce owner or authority changed; supply exceeds `O_d + M_inflight + RC_d + BU_d` (C2); an unexplained custody excess persists; C1 fails. Pauses never forget signed attempts (section 8).
**Archival history availability (MANDATORY with D3; how the plan ensures it):**
- P1: correctness while the nonce is unchanged needs no history (the nonce account is current state).
- P2: lookups use recorded signatures and the nonce account's own signature list, never a global scan.
- P3: reconcile every tick; profile `reconcile_alarm` (R 15 minutes) and `history_horizon` (R set after the probe, strictly below the retention measured for the providers that form the quorum). Past the horizon the lane stays halted and is resolved by hand.
- P4: discovery lenient, verification strict. Candidate lists (`getSignaturesForAddress`, always with explicit `before` for idempotent responses, S7) may use a 2-of-3 threshold; each candidate is then verified singly with `getTransaction` under the strictest consensus the providers allow. Presence from agreeing providers counts; absence never does.
- P5 (**blocker** `solana-history-availability-unmeasured`): before activation, show through the SOL RPC canister that `getTransaction`, `getSignaturesForAddress` and `getSignatureStatuses` with `searchTransactionHistory: true` (default false, so only the recent status cache, S6) return Consistent results for a signature older than the horizon. F S12: a validator's local ledger serves history "on the order of days"; longer history needs an external store; no retention figure is stated. A secondary guide (S15) says about two days for standard nodes, 200M-shred default cap, deeper history via Bigtable or archive nodes, retention differing by provider (not primary). **U:** which of the SOL RPC default providers (Alchemy, Ankr, Chainstack, dRPC, Helius, PublicNode; S7) serve archival data through the canister is unmeasured.
- Returns: the observer cursor lag alarms; an observer down longer than the horizon cannot rely on pagination, but the custody-excess check still halts credits instead of silently skipping. Persist compact finalized evidence per operation (signature, slot, err, memo, message hash) so audits never need RPC.

## 5. ICP side
- **Signing (F S10, S14):** `sign_with_schnorr`, algorithm `ed25519`, message = the full serialized message bytes (not a hash), 64-byte signature; `schnorr_public_key` gives the 32-byte key whose base58 is the address. Key names `key_1` (mainnet) and `test_key_1` (test, "should not be used for anything of value"). Cost F: `key_1` 26,153,846,153 cycles, `test_key_1` 10,000,000,000 per signature (repo attaches 30B). A mint or burn is two signatures (derived, about 52B cycles on `key_1`; **U** measured cost). Repo `ted25519::solana_schnorr_key_name` is hard-coded `test_key_1`: the new instance takes it from the profile/network (V6).
- **Derivation:** `[rumi-icusd-sol-bridge, v1, role, genesis_hash_32]` (section 5.1). The backend rail uses `[chain_id_le, "settlement"|"nonce"|principal...]` (`ted25519.rs`); the IC binds paths to the calling canister, so they can never coincide (design statement; not re-verified).
- **RPC path:** SOL RPC canister, hand-mirrored candid on ic-cdk 0.12 (the typed `sol_rpc_client` needs ic-cdk 0.20, the pin conflict recorded in `2026-06-01-solana-integration-design.md` section 4). Attach about 10B cycles per call, unused refunded (F S10: 1-5B per small request). No `getLatestBlockhash` (consensus fails; use durable nonces or `getSlot` plus `getBlock`; F S7). `getSlot` rounds down by 20 by default; `getSignaturesForAddress` needs `before` for idempotence; typed `getTransaction`/`getAccountInfo` strip volatile fields (S7). **U:** the repo wrapper sends raw JSON with `Equality` consensus (`sol_rpc.rs` `json_request`), where `context.slot` or `blockTime` may differ between providers; whether it reaches consensus on a live cluster is unmeasured, so the new instance targets the typed methods.
- **SOL RPC canister id (RESOLVED on paper, blocker `sol-rpc-canister-id-conflict` becomes an activation verification).** Production id is `tghme-zyaaa-aaaar-qarca-cai`: the dfinity README (S7) states it on the fiduciary subnet; the DFINITY forum (S8) names `titvo-eiaaa-aaaar-qaogq-cai` as the April 2025 staging canister and `tghme-...` as production "deployed with proposal 136985" (June 18, 2025); the dashboard API (S9) shows proposal 136985 titled "Upgrade NNS Canister: tghme-..." installing SOL RPC v1.0.0, EXECUTED 2025-06-16, and the canister page named "SOL RPC", controlled by the NNS root, with four listed proposals (install 136985, latest 139986 on 2026-01-19). The id printed in DFINITY's own docs pages (S10), `2xib7-jqaaa-aaaar-qai6q-cai`, is not a valid principal: I recomputed the principal-text CRC32 locally and it fails, while `tghme-...`, `titvo-...` and the EVM RPC id `7hfb6-...` pass, and the dashboard returned an error for it. Remaining action at activation: re-check controller and latest module on the dashboard; report the docs typo to DFINITY. `sol_rpc.rs` already pins `tghme-...`.
- **Finality (F S11, S14):** `finalized`; under Alpenglow `confirmed` and `finalized` are equivalent ("both mean ... permanently committed"). Use `finalized` everywhere; **U:** whether the SOL RPC canister keeps accepting both levels (S14 question 5).

## 6. D3 settled: memo plus durable nonce
What the memo option loses, stated plainly:
- **No on-chain consumed-mint record.** Nothing on Solana stops a second mint with the same `mint_id` under a different nonce; only nonce exclusivity and the canister's own record prevent it.
- **No monotone mint counter, so cumulative-mint check C4 is unavailable** (section 2.3). A foreign mint followed by a burn between two supply reads is invisible; only the point-in-time C2 check (supply at `finalized` against `O_d + M_inflight + RC_d + BU_d`) catches excess supply. A foreign mint implies mint-authority compromise or controller action.
- **No on-chain `bridge_out` event.** Return evidence is an off-chain-verified transaction, trusted to the provider quorum and finality, not to program logic.
- **Reliance on RPC history retention** for late signature and memo lookups: mandatory mitigation P1-P5 in section 4, blocker until measured.
- **A custody step.** Tokens sit in the return ATA until a canister-signed burn is final (two more signatures, a longer path to ICP payout) and the user must attach the memo (strays are refunded, a UX cost, not a safety hole).
- Gains: no program, no upgrade authority, no program-level pause or freeze power.
**Later option (not a gate):** a small program with a `consumed[mint_id]` PDA and a `bridge_out` that burns and emits the ICP account removes custody, adds C4 and the consumed record, and adds an upgrade authority (D6 controller model).

## 7. Reuse map
| Piece (repo) | Verdict |
| --- | --- |
| `chains/solana/ted25519.rs`: schnorr candid structs, `sign_message`, `derive_solana_address`, `solana_address_from_pubkey`, `decode_solana_address` | Copy; key name and path functions replaced (profile key name, role/genesis paths) |
| `chains/solana/tx.rs`: compact-u16, `serialize_legacy_message`, `assemble_wire_tx_multi`, `first_signature_base58`, `order_signatures_by_signer`, `advance_nonce_instruction`, `create_ata_idempotent_ix`, nonce account helpers | Copy; parameterize the token program (hard-coded classic); `derive_ata` must take the token program id |
| `tx.rs` `mint_to_ix` (unchecked `MintTo`, tag 7) | Do NOT reuse: new `MintToChecked`, `BurnChecked`, `TransferChecked`, Memo, advance-only builders |
| `sol_rpc.rs`: nonce parsers, `parse_send_transaction_signature`, `is_valid_tx_signature`, response `Reserved` decode pattern, principal | Copy patterns; add typed methods; replace the `Equality` raw-JSON path (**U** above); cluster fixed to Devnet today, must come from the profile |
| `settlement.rs`: sign-local-signature-then-mark-inflight, never re-sign, one in-flight op | Pattern only: its single nonce and one-op queue become the pool and reconcile loop; it uses backend queue types (not shared) |
| `deposit_watch.rs`: re-entrancy guard, balance polling, `supply_drop_detected` | Guard pattern only; it polls balances, not returns; the return observer is new |
| `hardening.rs`: stale-guard predicate | Copy |
| `chains/solana/config.rs`, backend queue, vault, supply modules | **Do not share.** The escrow mint must never be written into the backend's `chain_contracts` for chain 501 or its Solana config, and the bridge never calls the backend (sections 2.1, 4) |
| `rumi_icusd_bridge`: `ledger.rs`, `flows/ledger_ops.rs`, `accounting.rs`, `admin.rs`, `env.rs`, `icrc21.rs`, stable layout | Reuse by copy (section 1.3); rename `*_base` buckets; add `RC`, `BU`, `stray` |
| `evm/lane.rs` (nonce lane, 3723 lines) | Pattern only: Solana's token is a nonce value, new lane |
| Frontend | None exists; Solana Wallet Standard connector is BB-39 backlog |

## 8. Testing
**Seams, not proof (nothing run):** `src/rumi_protocol_backend/tests/solana_m2_pic.rs` (header lines 17-32 describe the probe; boots `.with_ii_subnet().with_application_subnet()`; falls back to a gated subset if `test_key_1` is absent), `tests/solana_m2_sign_pic.rs` (signs and verifies host-side with `ed25519-dalek`; may skip the assertion), and `src/sol_rpc_mock` (its `getTransaction` returns only `slot` and `meta.err`; it must be extended with instruction bytes and token balances). PocketIC can provision threshold Schnorr; the dfx local replica has no Ed25519 key (design 6.4). Real-network signing is a separate activation step.
**Future focused tests (not groundwork gates):**
1. Host golden vectors for each shape (2.1 to 2.4) against Solana SDK crates, including signer order and a v0 variant.
2. Memo grammar table test (valid, trailing bytes, leading zero nonce, non-canonical principal, 161 bytes).
3. Observer matrix: one fixture per row of the 2.2 table, plus delegate authority, missing `owner`, extra ComputeBudget, and an unknown program.
4. Lifecycle property: no ICP release is reachable from `RC_d` or `BU_d`; burn-not-landed returns to `RC_d`.
5. Reconcile matrix: unchanged, advanced and landed ok, landed with `err`, consumed by cancel, lost signature found by memo, foreign consumer halts, null/`Inconsistent` waits.
6. Negative decoder (design section 4): a backend-rail mint or burn is not accepted by the bridge decoder and the reverse (different mint, different program path).
7. Extension allow-list bind test using a mint fixture with each forbidden extension.

## 9. Profile alignment (`profiles/solana.json`; the profile stays disabled)
Settle in the profile: D3 `memo_plus_durable_nonce` (settled), D9 `test_ledger` (settled). Blockers to carry (ids; V7 requires the first two):
`solana-guard-missing` (memo guard is chosen but unbuilt; C4 unavailable) | `sol-rpc-canister-id-conflict` (RESOLVED to `tghme-...`, keep the id for V7 and reword to an activation re-verification) | `solana-return-custody` | `solana-return-observer-missing` | `solana-nonce-lane-missing` (pool, reconcile, cancel) | `solana-history-availability-unmeasured` (P5) | `solana-token-program-wallet-check` (Token-2022 versus classic) | `solana-rpc-consensus-unmeasured` (typed methods, `context` stripping, 2-of-3 discovery) | `solana-mint-not-created-or-bound` | `solana-fee-payer-funding` (SOL for fees, ATA and nonce rent) | `solana-wallet-connector-missing` (BB-39) | `solana-memo-program-version-unconfirmed`. Add `burn_id` domain `rumi.icusd.sol.burn.v1`, refund id domain, role `mint-account`, and profile fields `token_program`, `memo_program`, `nonce_pool_size`, `history_horizon`, `min_refund_e8s`.

## 10. Ordered future implementation steps
1. Instance skeleton: copy ledger saga, accounting (add `RC`, `BU`, `stray`), admin, env, ICRC-21, stable layout; Solana ids and domains. **Medium.**
2. Roles, derivation paths, ids, memo grammar and parser with unit tests. **Small.**
3. Message builders for 2.1 to 2.4 (token program parameter) with host golden vectors. **Medium.**
4. Typed SOL RPC candid mirror, consensus settings, retention probe tool, mock extension. **Medium** (large if consensus fails).
5. Mint creation, bind checks, extension allow-list, claims. **Medium.**
6. Nonce pool, persist-before-sign, reconcile, cancel, halts. **Large.**
7. Mint saga (`PM -> O`) with C1/C2 monitors. **Medium.**
8. Return observer: discovery, strict verifier, dedup, strays, refund lane. **Large.**
9. Burn lane, `RC -> BU -> PR` gating, custody reading, audit query. **Large.**
10. Test matrix (section 8). **Medium.**
11. Rehearsal on devnet with `test_key_1` and a synthetic ledger (D9). **Small.** Mainnet activation separately, with P5 and wallet checks.
12. Solana connector in the frontend (BB-39). **Large.** Optional program guard later. **Large.**

## 11. Not grounded in source
- Token-2022 and metadata display in named wallets; which Memo program version is deployed; whether metadata authorities can be set to `None`; whether token tag 255 `Batch` is deployed.
- Whether IC Ed25519 signing is deterministic; measured signing and RPC cycle costs; per-provider archival depth behind the SOL RPC canister; raw `jsonRequest` consensus behavior; whether `getSignaturesForAddress` on a nonce account lists only its writers as assumed; `CreateAccountWithSeed` for nonce accounts; `getSlot`+`getBlock` blockhash freshness on mainnet.
- The freeze-authority finding (S1, S2) and the CRC check are source reading and local arithmetic, not executed network tests.

## 12. Coordinator addendum: two corrections (Miles' review, 2026-10-30)

D3 (memo plus durable nonce) stands. Both items are known semantics of the chosen design, recorded as plan corrections.

1. **Recovered signatures need exact-message proof.** In section 4 step 3, finding our `mint_id` memo in a transaction that
   consumed the nonce is discovery, not proof. A nonce-authority or fee-payer signer could submit an advance-only transaction
   carrying our memo and make a missing mint look delivered. Adoption requires finalized successful execution AND message
   bytes equal to the stored unsigned message (nonce account and value, signer roles, mint, recipient account, amount). A
   different or unverifiable consuming message is held and halts the lane; no replacement and no refund is invented.
2. **Ordinary holder burns.** Under classic SPL and under the chosen Token-2022 metadata-only allow-list, a token account's
   owner or delegate can execute `Burn` or `BurnChecked`, which lowers mint supply
   (https://solana.com/docs/tokens/basics/burn-tokens, read 2026-10-30). A holder can burn without our return memo: `O_d`
   stays the same and supply falls. Backing stays safe (escrow simply over-covers), but exact equality fails. So:
   - our own burn is proven by finalized success plus the exact `BurnChecked` message bytes and the return account's balance
     effect, never by the change in global supply;
   - C3 becomes `supply_d + UB_d == O_d`, where `UB_d` is cumulative unattributed holder burn; until a burn observer exists,
     only `supply_d <= O_d` is checked and the gap is reported as unattributed;
   - activation prerequisite and policy, blocker `solana-unattributed-burn-policy`: no ICP credit without a recipient-bound
     claim; no automatic escrow sweep or refund; no global halt solely from a legitimate supply decrease. The `UB_d`
     accounting seam is **implementation missing**. No permissioned-burn extension or program is added silently, and C4
     still does not exist.

Future focused implementation tests (not groundwork gates): (a) an advance-only transaction that consumes the mint nonce
and carries our memo is NOT adopted and holds the lane; (b) a holder `BurnChecked` without a memo lowers supply, leaves
`O_d` unchanged, credits nothing, halts nothing and is reported as unattributed.
