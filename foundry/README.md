# Rumi Monad contracts (Foundry)

Phase 1b ships a single contract: `src/IcUSD.sol`, the on-chain icUSD ERC-20 on
Monad. The Rumi backend canister's tECDSA-derived settlement address is the sole
minter (`MINTER_ROLE`); any holder may `burn` to repay their vault.

## Contract <-> canister contract (do not break)

`IcUSD.sol` is pinned to the backend's Monad adapter
(`src/rumi_protocol_backend/src/chains/monad/`):

- `mint(address to, uint256 amount, uint64 vault_id)` matches the selector built
  by `tx::encode_mint_calldata` (`keccak256("mint(address,uint256,uint64)")[:4]`).
- `Mint(uint256,address,uint256)` / `Burn(uint256,address,uint256)` topic0 hashes
  match `MINT_EVENT_TOPIC0` / `BURN_EVENT_TOPIC0` in `evm_rpc.rs`. `vault_id` and
  `recipient`/`burner` are `indexed` so the observer reads them from
  `topics[1]`/`topics[2]` and `amount` from `data` (see `MintLog`/`BurnLog`).
- 8 decimals so 1 base unit == 1 e8s (1:1 with the ICP-side e8s accounting).
- `mapping(uint64 => bool) minted` is a per-`vault_id` idempotency guard: a
  canister resubmit-after-transient-RPC-error cannot double-mint.

## Dependencies

`lib/` is gitignored. Re-install before building:

```bash
export PATH="$HOME/.foundry/bin:$PATH"
forge install                                                    # forge-std
forge install OpenZeppelin/openzeppelin-contracts@v5.0.2 --no-git --no-commit
```

`remappings.txt` maps `@openzeppelin/` -> `lib/openzeppelin-contracts/`.

## Build

```bash
export PATH="$HOME/.foundry/bin:$PATH"
forge build
```

Compiles on solc 0.8.24 (pinned in `foundry.toml`).

## Deploy

See [`DEPLOY.md`](./DEPLOY.md) for the Monad-testnet deploy runbook (derive the
canister settlement address first; it is both admin and minter for Phase 1b).
