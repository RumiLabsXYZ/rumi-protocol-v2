# IcUSD Monad-testnet deploy runbook

PREREQUISITE: derive the backend's settlement address FIRST — it is BOTH the
admin and the minter, so the token is canister-owned from block zero.

1. On the backend (staging), derive the settlement address:
   dfx canister call --network ic <STAGING_ID> get_chain_settlement_address '(record { 0 = 10143 : nat32 })' --identity rumi_identity
   (returns 0x.. — this is CANISTER_SETTLEMENT_ADDR)
2. Fund a testnet deployer key with Monad testnet MON (faucet).
3. cp foundry/.env.example foundry/.env  and fill all three vars.  (.env is gitignored.)
4. cd foundry && forge script script/DeployIcUSD.s.sol:DeployIcUSD --rpc-url monad_testnet --broadcast -vvvv
5. Record the "IcUSD deployed at:" address.
6. Fund CANISTER_SETTLEMENT_ADDR with testnet MON — it is the hot wallet that
   pays gas for mints + withdrawals (Task 11 hot-wallet gate).
7. On the backend: set_chain_contract(10143, "<deployed address>").

## Admin / minter model (Phase 1b — from the IcUSD.sol security review)

For Phase 1b, admin == minter == the canister settlement address. Implications
(acceptable for testnet, revisit before any value-bearing/mainnet deploy):
- The canister is the sole minter AND the DEFAULT_ADMIN_ROLE holder (it can
  grant/revoke MINTER_ROLE). A single tECDSA key thus controls both.
- There is no two-step admin transfer and no admin guardian (OZ
  AccessControlDefaultAdminRules is NOT used). If the admin key were lost,
  minter management would be frozen. The canister's key is tECDSA-managed
  (no single exportable key), which mitigates this for the canister-owned model.
- A hardened later version may split admin (an SNS-controlled address) from
  minter (the canister), and/or adopt AccessControlDefaultAdminRules.

## Dependencies
`foundry/lib/` is gitignored. Before building/deploying: `cd foundry && forge install`
(OpenZeppelin v5.0.2 + forge-std). See foundry/README.md for the exact install
commands and the contract <-> canister contract.
