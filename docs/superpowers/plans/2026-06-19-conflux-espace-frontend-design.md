# Conflux eSpace self-serve frontend (Design)

> Standalone MetaMask + viem UI for the M2 EVM-native self-serve CDP rail. The
> backend ([[2026-06-18-conflux-evm-self-serve-auth-design]]) is merged + deployed
> to mainnet-staging (kvg63) and the full open→deposit→4-arg-mint→Open round-trip
> is proven on real eSpace. This is the user-facing surface that was deferred from
> that scope. Testnet/staging-only.

## Goal

Let a user with only MetaMask on Conflux eSpace testnet (chain 71) open / borrow
against / repay / withdraw / close a native-CFX icUSD vault by signing EIP-712
intents — no IC principal, no II login. The vault is owned by the synthetic
principal the backend derives from the EVM signer; the IC caller is anonymous.

## Decisions (settled 2026-06-19)

| Fork | Decision |
| --- | --- |
| Location/stack | **Standalone Vite + Svelte SPA** at `src/conflux_espace_frontend/` (isolated from the production `vault_frontend`, which points at prod `tfesu` and has no EVM deps). |
| Scope | **Full lifecycle**: open / borrow / withdraw / close / repay(burn) + status polling. |
| Target | Staging backend `kvg63-wiaaa-aaaao-bbabq-cai`; IcUSD `0xBD02222D388BC43095A4758C3e977d5dF8f68f7a`; Conflux eSpace testnet chain 71; eSpace RPC `https://evmtestnet.confluxrpc.com`. |
| Signing | MetaMask (primary) via viem `signTypedData`; an **optional dev-key signer** (viem `privateKeyToAccount`) for demo/verification without MetaMask. |

## Architecture

A single-page Svelte app. Two integrations, each a small focused module:

- **`config.ts`** — the staging canister id, IcUSD address, chain id 71, eSpace RPC,
  the EIP-712 domain (`name: "Rumi icUSD CDP"`, `version: "1"`, `chainId: 71`,
  `verifyingContract: <IcUSD>`), and the ICP-mirrored CR params (min CR 1.33).
- **`eip712.ts`** — builds the `VaultIntent` typed-data and signs it (viem
  `signTypedData` for MetaMask, or `account.signTypedData` for the dev key),
  returning the 65-byte `r‖s‖v` signature as a `Uint8Array` for the canister blob.
  MUST be byte-identical to the backend's `chains/evm/eip712.rs` (same domain,
  same `VaultIntent(uint8 action,uint64 chainId,address owner,uint64 vaultId,
  uint256 collateralWei,uint256 debtE8s,address recipient,uint256 nonce,uint256
  deadline)` struct). `recipient` is forced `== owner`.
- **`backend.ts`** — an anonymous `@dfinity/agent` actor (host `https://icp0.io`,
  canister `kvg63`) built from the tracked `src/declarations/rumi_protocol_backend`
  idlFactory, with typed wrappers: `openVaultEvm`, `borrowVaultEvm`,
  `withdrawCollateralEvm`, `closeVaultEvm`, `getChainVault`. The `_evm` calls take
  `(VaultIntent, blob)`.
- **`evm.ts`** — viem `walletClient` (MetaMask) + `publicClient` (eSpace RPC):
  connect + add/switch to chain 71, send the native CFX deposit, call
  `IcUSD.burn(amount, vaultId)` to repay, and read `balanceOf` / CFX balance.

UI: `App.svelte` orchestrates; small components for Connect, the Open form
(enter icUSD debt → required CFX computed from min CR + a CFX price input),
VaultCard (status + the lifecycle action buttons), and a deposit-instructions
panel (custody address + "send X CFX").

## Data flow (open + mint)

1. Connect MetaMask → ensure chain 71.
2. User enters debt (e.g. 0.2 icUSD); UI shows required CFX = debt × minCR / price.
3. Read the per-owner nonce from the vault list (or start at 0); build the Open
   `VaultIntent` (collateralWei, debtE8s, recipient = owner); `signTypedData`.
4. `open_chain_vault_evm(intent, sig)` → `vault_id`; poll `get_chain_vault` →
   show the custody address.
5. User clicks "Send X CFX" → viem `sendTransaction` to custody.
6. Poll status `AwaitingDeposit → MintPending → Open`; icUSD `balanceOf` updates.
7. Borrow / Withdraw / Close = sign the matching intent (nonce += 1 each). Repay =
   `IcUSD.burn(amount, vaultId)` then the burn-watch decrements debt.

## Error handling

- Wrong network → prompt to switch to chain 71.
- Canister `Err(EvmAuth ...)` → surface the message (bad nonce, expired, etc.).
- A failed CR check (`BelowMinCr`) → show required-vs-provided.
- All amounts validated client-side before signing (debt ≥ min, CR ≥ 1.33).

## Testing / verification

- **Vitest unit (the critical gate):** `eip712.ts` builds the typed-data and
  `hashTypedData` MUST equal the backend golden digest
  `0x76fe467010b364bc9ed7caf7153a42bdc924e1cb7bf223d8182d9537717b9adc` for the
  same intent, and signing with the scalar=1 key recovers
  `0x7e5f4552091a69125d5dfcb7b8c2659029395bdf`. This proves frontend↔backend
  EIP-712 parity. Plus the CR/required-CFX math.
- **Build green** (`npm run build`) and **dev server renders** (verified via the
  preview tools — the wallet/signing path is exercised by the dev-key signer,
  the same path the staging round-trip proved).

## Out of scope

Production deployment (asset canister), Monad chain, multi-wallet, i18n. This is
the testnet demo surface; the chains rail stays experimental/parked.
